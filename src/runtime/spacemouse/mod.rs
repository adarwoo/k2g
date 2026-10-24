//! The 3D mouse — a 3Dconnexion SpaceMouse — as a way to steer the Machining 3D view.
//!
//! **Read as raw HID, not through 3Dconnexion's SDK.** This is what PrusaSlicer and
//! OrcaSlicer do, and why the operator's mouse already works there: the device's input
//! reports are a small fixed format, so reading them takes a parser rather than a
//! proprietary library. The SDK (the "Navigation Library") would bring the driver's own
//! settings panel and button mapping, at the price of a C++/COM interface, a revocable
//! licence that wants attribution, and a careful read against GPL-3.0-only. The browser
//! routes are worse: WebHID sees nothing while 3DxWare holds the device, and the Onshape
//! route is an undocumented WebSocket server on a loopback address — a network connection
//! in an application whose only one is the update check (`PRIVACY.md`).
//!
//! **Windows only, for now** ([`windows`]). Raw input coexists with 3DxWare, which is
//! installed wherever the mouse is. Linux (hidraw, or `spacenavd`) and macOS (where the
//! driver owns the device outright) are left for later, and on them [`subscribe`] simply
//! never yields anything.
//!
//! The device reports how far its cap is pushed, not how far it has moved: a held cap
//! sends the same deflection again and again. So what crosses to the view is a
//! [`Deflection`] — a *rate* — and the view integrates it over its own frame time. That
//! keeps the motion smooth at whatever rate the reports happen to arrive, and stops the
//! instant the cap is let go.

use std::sync::{Mutex, Once};

use serde::Serialize;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

#[cfg(windows)]
mod windows;

/// The vendor ids 3Dconnexion devices report: Logitech's, from when 3Dconnexion was a
/// Logitech company (the SpaceNavigator and every older model), and 3Dconnexion's own
/// (the current SpaceMouse range and the universal receiver). The report format below is
/// theirs; another vendor's multi-axis controller may lay its reports out differently, so
/// it is not read.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const VENDORS: [u32; 2] = [0x046d, 0x256f];

/// Raw counts at full deflection. The axes are signed 16-bit, but a cap pushed as far as
/// it goes reads about ±350 on the devices PrusaSlicer tuned against; dividing by this puts
/// a full push near 1, and anything past it is clamped.
const FULL_SCALE: f64 = 350.0;

/// Deflection below this fraction of full scale reads as none. The cap never rests at
/// exactly zero — it settles a few counts off — and without a dead zone the view would
/// creep whenever a hand was near it. The rest of the range is stretched back to 0..1, so
/// a light touch still starts gently rather than jumping to the threshold.
const DEAD_ZONE: f64 = 0.04;

/// How far the cap is pushed on each axis, each in −1..1: translation along X, Y, Z and
/// rotation about them, in the device's own frame. What each axis *does* to the view is the
/// view's business (`machining_3d.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Deflection {
    pub t: [f64; 3],
    pub r: [f64; 3],
}

/// One thing the device said.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Input {
    /// The cap's deflection changed (or was restated).
    Motion { deflection: Deflection },
    /// Which buttons are down, as a bitmask; sent when it changes.
    Buttons { mask: u32 },
}

/// What one device has reported so far: the last deflection and button state.
///
/// Kept because the reports are partial. Older devices send translation and rotation as two
/// separate reports, so each one updates half of the deflection and the other half has to
/// be remembered; and a button report restates every button, so a change has to be found by
/// comparing with the last.
#[derive(Default)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct Device {
    deflection: Deflection,
    buttons: u32,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Device {
    /// Reads one input report (report id first), or `None` for one that says nothing new or
    /// is not understood.
    ///
    /// The layout, as the devices send it:
    ///
    /// | id | length | carries |
    /// |---|---|---|
    /// | 1 | 13 | translation (bytes 1–6) and rotation (7–12) — current devices |
    /// | 1 | 7 | translation only — older devices, which follow it with id 2 |
    /// | 2 | 7 | rotation only |
    /// | 3 | 2+ | button bitmask, little-endian |
    ///
    /// Each axis is a little-endian `i16`. Anything else (battery level on the wireless
    /// models, vendor reports) is ignored.
    pub(crate) fn read(&mut self, report: &[u8]) -> Option<Input> {
        match *report.first()? {
            1 if report.len() >= 13 => {
                self.deflection.t = axes(&report[1..7]);
                self.deflection.r = axes(&report[7..13]);
            }
            1 if report.len() >= 7 => self.deflection.t = axes(&report[1..7]),
            2 if report.len() >= 7 => self.deflection.r = axes(&report[1..7]),
            3 if report.len() >= 2 => {
                let mut bytes = [0u8; 4];
                let n = (report.len() - 1).min(4);
                bytes[..n].copy_from_slice(&report[1..1 + n]);
                let mask = u32::from_le_bytes(bytes);
                if mask == self.buttons {
                    return None;
                }
                self.buttons = mask;
                return Some(Input::Buttons { mask });
            }
            _ => return None,
        }
        Some(Input::Motion { deflection: self.deflection })
    }
}

/// Three little-endian `i16` axes, normalised.
fn axes(bytes: &[u8]) -> [f64; 3] {
    [0, 1, 2].map(|i| normalise(i16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]])))
}

/// Raw counts to −1..1, with the dead zone taken out and the rest stretched back to full.
fn normalise(raw: i16) -> f64 {
    let value = (f64::from(raw) / FULL_SCALE).clamp(-1.0, 1.0);
    if value.abs() < DEAD_ZONE {
        0.0
    } else {
        (value - DEAD_ZONE * value.signum()) / (1.0 - DEAD_ZONE)
    }
}

/// Where input goes: the one view currently listening, if any.
///
/// One subscriber, replaced by the next, because there is one 3D view: it subscribes when
/// it mounts, and a remount (the dock reopened, the tab switched back) takes over from the
/// last. Input arriving with nobody listening is dropped — it describes a cap being pushed
/// *now*, and there is nothing to replay it to later.
static SUBSCRIBER: Mutex<Option<UnboundedSender<Input>>> = Mutex::new(None);

/// The device thread starts once, on the first subscription, so a session that never opens
/// the 3D view never registers for raw input at all.
static START: Once = Once::new();

/// Starts listening to the 3D mouse, if this platform can, and returns the input from it.
/// Replaces any earlier subscriber.
pub fn subscribe() -> UnboundedReceiver<Input> {
    START.call_once(|| {
        #[cfg(windows)]
        windows::start();
    });
    let (tx, rx) = unbounded_channel();
    *SUBSCRIBER.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(tx);
    rx
}

/// Hands `input` to the subscriber, forgetting it once its receiver is gone.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn publish(input: Input) {
    let mut slot = SUBSCRIBER.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(tx) = slot.as_ref() {
        if tx.send(input).is_err() {
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report: id, then each value as a little-endian `i16`.
    fn report(id: u8, values: &[i16]) -> Vec<u8> {
        std::iter::once(id).chain(values.iter().flat_map(|v| v.to_le_bytes())).collect()
    }

    fn motion(input: Option<Input>) -> Deflection {
        match input {
            Some(Input::Motion { deflection }) => deflection,
            other => panic!("expected motion, got {other:?}"),
        }
    }

    /// A current device sends both halves in one report.
    #[test]
    fn a_full_report_carries_translation_and_rotation() {
        let mut device = Device::default();
        let d = motion(device.read(&report(1, &[350, 0, -350, 0, 175, 0])));
        assert_eq!(d.t, [1.0, 0.0, -1.0]);
        assert_eq!(d.r[0], 0.0);
        assert!((d.r[1] - (0.5 - DEAD_ZONE) / (1.0 - DEAD_ZONE)).abs() < 1e-12);
    }

    /// An older device splits them, and each report updates only its own half.
    #[test]
    fn split_reports_each_update_their_own_half() {
        let mut device = Device::default();
        motion(device.read(&report(2, &[0, 0, 350])));
        let d = motion(device.read(&report(1, &[350, 0, 0])));
        assert_eq!(d.t, [1.0, 0.0, 0.0]);
        assert_eq!(d.r, [0.0, 0.0, 1.0], "the rotation from the earlier report is kept");
    }

    /// A cap at rest a few counts off zero is at rest, and a push past the stop is a full
    /// push, not more.
    #[test]
    fn small_deflection_is_none_and_large_is_clamped() {
        let mut device = Device::default();
        let d = motion(device.read(&report(1, &[10, -10, 0, 1000, -1000, 0])));
        assert_eq!(d.t, [0.0, 0.0, 0.0]);
        assert_eq!(d.r, [1.0, -1.0, 0.0]);
    }

    /// Buttons are reported when they change, and only then.
    #[test]
    fn buttons_are_reported_on_change() {
        let mut device = Device::default();
        assert_eq!(device.read(&[3, 0b01]), Some(Input::Buttons { mask: 1 }));
        assert_eq!(device.read(&[3, 0b01]), None, "the same state again says nothing");
        assert_eq!(device.read(&[3, 0, 0]), Some(Input::Buttons { mask: 0 }));
    }

    /// Battery and vendor reports, and truncated ones, are not motion.
    #[test]
    fn other_reports_are_ignored() {
        let mut device = Device::default();
        assert_eq!(device.read(&[23, 80]), None);
        assert_eq!(device.read(&[1, 0, 0]), None, "too short to hold three axes");
        assert_eq!(device.read(&[]), None);
    }

    /// The view parses this by field name, so the shape is a contract.
    #[test]
    fn input_serialises_with_a_kind_tag() {
        let json = serde_json::to_value(Input::Buttons { mask: 2 }).unwrap();
        assert_eq!(json, serde_json::json!({ "kind": "buttons", "mask": 2 }));
        let json = serde_json::to_value(Input::Motion { deflection: Deflection::default() }).unwrap();
        assert_eq!(json["kind"], "motion");
        assert_eq!(json["deflection"]["t"], serde_json::json!([0.0, 0.0, 0.0]));
    }
}
