//! Removable media (USB keys, SD cards): detection, save targeting, and eject.
//!
//! A generated program's usual destination is a stick that goes to the machine, so the
//! Save control offers "write it there and eject it" as one action — the affordance
//! PrusaSlicer popularised. That needs three things this module provides: a list of what
//! is currently plugged in, kept live because a stick can appear at any moment; a rule
//! for *where on it* the dialog should open; and a safe eject.
//!
//! ## Shape
//!
//! The platform half is behind [`platform`], which is `win32` on Windows and a stub
//! everywhere else, so **no `cfg` appears anywhere in `src/ui`** — the other platforms
//! simply always report an empty list, which is a state the UI must handle regardless
//! (a Windows machine with nothing plugged in looks identical). Every `unsafe` in this
//! crate lives in `removable/win32.rs`.
//!
//! ## Where the list lives, and why it is not in `AppCtx`
//!
//! In a `static` here, read through [`removable_media`]. Putting it in the app context
//! would mean a [`with_ctx_mut`](crate::runtime::with_ctx_mut) on every poll tick, and
//! that call clones the whole `AppState` and runs the post-mutation reconciliation —
//! including the regeneration trigger — which is not something a 2-second timer should
//! be driving. `AppCtx` also has no `PartialEq`, so publishing into it repaints the
//! entire tree on a timer.
//!
//! The consequence is an invariant, not an optimisation: **the watcher publishes and
//! wakes the UI only when the scan materially changed** (see [`is_material_change`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

#[cfg(windows)]
#[path = "removable/win32.rs"]
mod platform;

#[cfg(not(windows))]
#[path = "removable/unsupported.rs"]
mod platform;

/// Shown in place of a missing volume label. Never an empty string: a blank entry in a
/// drive picker reads as a bug rather than as an unnamed stick.
// Only the Windows scan has a label to substitute for; the stub never names a medium.
#[cfg_attr(not(windows), allow(dead_code))]
pub const UNLABELLED_MEDIUM: &str = "Removable disk";

/// How often the drive table is re-read.
///
/// `WM_DEVICECHANGE` would be event-driven and free, but it needs a window handle and a
/// message pump, and tao/wry own the event loop with no hook to hang a window procedure
/// on. At two seconds the poll is imperceptible to someone plugging a stick in, and it
/// costs one `GetLogicalDrives` plus three cheap calls per *removable* drive — usually
/// zero or one.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Free-space movement below this does not count as a change.
///
/// Windows moves reported free space on its own (delayed metadata, shadow copies), and
/// every wake repaints the whole UI — so this threshold is what stops an idle app
/// redrawing twice a minute for the life of the session.
const FREE_SPACE_NOTICE_BYTES: u64 = 1 << 20; // 1 MiB

/// One mounted removable volume, as the UI needs to describe it: where to write, what to
/// call it, and whether the program will fit.
///
/// `root` is the volume's *root directory* (`E:\`, trailing separator included) — the
/// form a save dialog opens at. It is deliberately not the device path (`\\.\E:`), which
/// the eject builds separately and which differs by exactly that separator; see
/// `win32::device_path_wide` for why conflating the two fails in a way that looks like an
/// unsupported device.
#[derive(Clone, Debug, PartialEq, Eq)]
// Never constructed off Windows (the stub scan returns nothing), which `dead_code` reads
// as an unused struct. It is still the cross-platform API the UI compiles against.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct RemovableMedium {
    /// The volume root, e.g. `E:\`.
    pub root: PathBuf,
    /// The volume label, or [`UNLABELLED_MEDIUM`].
    pub label: String,
    /// Uppercase ASCII, e.g. `'E'` — what the device path is built on, and what the
    /// operator reads on screen.
    pub drive_letter: char,
    /// The volume serial number, when the filesystem has one.
    ///
    /// This is what a remembered export folder is filed under ([`volume_key`]), because
    /// the drive letter is not an identity: it is handed to whatever is plugged in next,
    /// so a folder remembered against `E:` would be offered for a stick that never had
    /// it. The serial survives replugging and reassignment.
    ///
    /// Its honest limits, since they decide what this can be trusted for: it is a
    /// *filesystem* property written at format time, so reformatting a stick changes it
    /// (the remembered folder is forgotten, which is the right answer for a wiped disk),
    /// and a bit-for-bit clone of one carries the same serial. `None` where the
    /// filesystem reports none.
    pub serial: Option<u32>,
    /// Free bytes available *to this process* (the quota-aware figure), because the
    /// question being asked is "will my program fit", not "how empty is this disk".
    pub free_bytes: u64,
}

impl RemovableMedium {
    /// How the medium is named to the user: `KINGSTON (E:)`.
    pub fn display_name(&self) -> String {
        format!("{} ({}:)", self.label, self.drive_letter)
    }
}

/// How far an eject got. Both values mean "safe to pull"; they differ only in whether the
/// device physically went away, which changes what the user should expect to see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum EjectOutcome {
    /// The device acknowledged the eject — it has vanished from the file manager.
    Ejected,
    /// Flushed, locked and dismounted, but the device has no eject mechanism and said so.
    /// This is the *normal* answer for a large fraction of plain USB sticks, and the
    /// drive letter may linger; it is not a failure.
    SafeToRemove,
}

/// Why an eject did not complete.
///
/// Every variant is a warning rather than an error: [`eject`] is only ever reached after
/// the file is already on the medium, so none of these means lost work. See
/// [`eject_advice`] for the wording rule that follows from that.
#[derive(Debug, thiserror::Error)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum EjectError {
    /// The inverse of the attribute above: this is the *only* variant the stub
    /// constructs, so on Windows it is the one that reads as dead. It stays because
    /// [`eject_advice`] must answer for every platform's failures.
    #[cfg_attr(windows, allow(dead_code))]
    #[error("ejecting removable media is only supported on Windows")]
    Unsupported,

    /// The volume could not be locked within the retry window — something still holds a
    /// handle on it (a file manager showing the drive, a shell preview, an antivirus
    /// scan of the file just written). The data is already flushed; only the eject failed.
    #[error("the drive stayed busy after {attempts} attempts over about {seconds}s")]
    Busy { attempts: u32, seconds: u64 },

    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// The last published scan.
///
/// A plain `RwLock`: writes are one `Vec` swap every couple of seconds and reads are one
/// clone per render, so contention is not a consideration.
static MEDIA: RwLock<Vec<RemovableMedium>> = RwLock::new(Vec::new());

/// Guards against a second watcher thread. `initialize_ctx` is `OnceLock`-guarded today,
/// but this module should not depend on that staying true.
static WATCHER_STARTED: AtomicBool = AtomicBool::new(false);

/// Eject requests, serviced by the watcher thread.
static CMD_TX: OnceLock<mpsc::Sender<Command>> = OnceLock::new();

/// Work the watcher thread does besides polling.
enum Command {
    Eject(RemovableMedium),
}

/// The removable media currently mounted — newest scan, cheapest read.
///
/// **Not a Dioxus signal.** Callers re-render because the watcher bumps the shared UI
/// wake channel, not because this was read, so only read it from a subtree that
/// `AppRoot` re-renders. Returns a clone so the lock is held for the length of a `Vec`
/// copy rather than for a render.
pub fn removable_media() -> Vec<RemovableMedium> {
    MEDIA.read().map(|media| media.clone()).unwrap_or_default()
}

/// Starts the poller. Idempotent, and safe to call before the UI exists.
pub fn start_removable_media_watcher() {
    if WATCHER_STARTED.swap(true, Ordering::SeqCst) {
        return; // already running
    }
    let (tx, rx) = mpsc::channel::<Command>();
    let _ = CMD_TX.set(tx);

    // Deliberately not scanned synchronously first: probing a card reader with no card
    // in it can block on a device timeout, and this is called on the thread that is about
    // to become the WebView thread. The loop's first iteration scans immediately, so the
    // cost is a few milliseconds of an empty list rather than a stalled launch.
    std::thread::Builder::new()
        .name("k2g-removable".to_string())
        .spawn(|| watcher_loop(rx))
        .expect("failed to spawn removable-media watcher thread");

    // Logged because the failure mode of this whole facility is *absence* — a button that
    // never appears — and with nothing plugged in the loop is otherwise completely silent.
    // This line is what distinguishes "the watcher is not running" from "it is running and
    // there is genuinely nothing there".
    log::info!("removable-media watcher started, polling every {POLL_INTERVAL:?}");
}

/// Queues an eject and returns at once.
///
/// Fire-and-forget because an eject blocks for up to a few seconds waiting for the volume
/// lock, and this is called from a click handler on the WebView thread. The outcome
/// arrives as a toast.
pub fn request_eject(medium: RemovableMedium) {
    let Some(tx) = CMD_TX.get() else {
        // The watcher never started (a test binary, or a platform stub). Nothing to do:
        // the file is written either way, and the user unmounts as they normally would.
        log::debug!("eject requested with no watcher running; ignoring");
        return;
    };
    if tx.send(Command::Eject(medium)).is_err() {
        log::warn!("removable-media watcher is gone; eject request dropped");
    }
}

/// The medium an export to removable media should go to, or `None` when there is nothing
/// plugged in.
///
/// *Which* medium, not where on it: the folder is [`crate::runtime::export`]'s business,
/// because it is remembered per volume and the hard disk is remembered the same way. This
/// only answers "is there a stick, and which one".
pub fn export_medium(media: &[RemovableMedium]) -> Option<RemovableMedium> {
    // The lowest-lettered one. With two sticks in, the operator picks in the dialog; this
    // is only the opening bid.
    media.first().cloned()
}

/// What a volume's remembered export folder is filed under.
///
/// The serial where the filesystem has one, because a drive letter is not an identity —
/// it is handed to whatever is plugged in next, and a folder remembered against `E:`
/// would then be offered for a stick that never had it. The letter is the fallback for a
/// filesystem with no serial, which is no worse than what this replaced.
///
/// Prefixed so the two kinds cannot collide, and so an entry in the settings file says
/// what it is to anyone reading it.
pub fn volume_key(medium: &RemovableMedium) -> String {
    match medium.serial {
        Some(serial) => format!("usb:{serial:08X}"),
        None => format!("drive:{}", medium.drive_letter),
    }
}

/// What a written-to path is filed under.
///
/// A path on a stick that is plugged in takes that stick's serial key, so the folder is
/// remembered against the medium rather than against the letter it happens to hold today.
/// Anything else takes its drive letter — a fixed drive keeps its letter, which is the
/// whole difference from a stick — and a path with no letter to read (every non-Windows
/// path, and the UNC and drive-relative forms [`drive_letter_of`] declines) takes
/// [`HOST_VOLUME_KEY`].
pub fn volume_key_for_path(path: &Path, media: &[RemovableMedium]) -> String {
    match drive_letter_of(&path.to_string_lossy()) {
        Some(letter) => match media.iter().find(|medium| medium.drive_letter == letter) {
            Some(medium) => volume_key(medium),
            None => format!("drive:{letter}"),
        },
        None => HOST_VOLUME_KEY.to_string(),
    }
}

/// The bucket a path with no drive letter is filed under — every path on Linux and macOS,
/// where there is one destination to remember and nothing to distinguish.
pub const HOST_VOLUME_KEY: &str = "host";

/// One remembered export destination: which volume, and where on it.
///
/// Stored in `global.setting.yaml` as an array of these, most recently used first — see
/// `export_directories` there for why an array rather than a map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportDirectory {
    pub volume: String,
    pub path: String,
}

/// Which volume an export is aimed at: the machine, or a stick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportDestination {
    /// The **Export** button — wherever the machine last exported to.
    Host,
    /// The USB button — this medium.
    Medium(RemovableMedium),
}

/// Where an export dialog opens, and what the result is filed under afterwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportTarget {
    pub directory: PathBuf,
    /// What [`ExportDirectory::volume`] to write when the export lands *here*. The folder
    /// the operator finally browses to can be somewhere else entirely, so the key is
    /// recomputed from the real path after the write — this is the opening bid.
    pub volume_key: String,
    /// The eject candidate, for a `Medium` destination. Whether anything is *actually*
    /// ejected is decided after the write, from where the files landed.
    pub medium: Option<RemovableMedium>,
}

/// Where an export should open. See [`choose_export_target`].
pub fn export_target(
    destination: &ExportDestination,
    remembered: &[ExportDirectory],
    host_default: &Path,
) -> ExportTarget {
    choose_export_target(destination, remembered, host_default, |path| path.is_dir())
}

/// The destination decision, with the filesystem injected.
///
/// Pure and hardware-free for the reason the medium selection it replaced was: half the
/// input is a synthesised medium list, and a test mixing synthetic media with real
/// directories is a test that passes on its author's machine.
///
/// For a **stick**: the folder remembered against its serial, *re-based onto the letter it
/// holds now* — `F:\jobs` remembered, plugged back in as `E:`, opens `E:\jobs`. Re-basing
/// is the entire point of keying by serial; without it the remembered path names a drive
/// that may belong to something else. A migrated `drive:` entry is accepted as a second
/// chance, and the first export re-files it under the serial. Failing both, the root of
/// the device.
///
/// For the **host**: the most recent remembered folder that is not on a stick and still
/// exists, else the host's own default. Most-recent-first is what gives each fixed drive
/// its own memory — exporting to `D:` puts `drive:D` at the front, so the next export
/// offers `D:`, and going back to `C:` restores it.
fn choose_export_target(
    destination: &ExportDestination,
    remembered: &[ExportDirectory],
    host_default: &Path,
    dir_exists: impl Fn(&Path) -> bool,
) -> ExportTarget {
    match destination {
        ExportDestination::Medium(medium) => {
            let key = volume_key(medium);
            // The serial's own entry first, then a `drive:` entry left by a migration —
            // which cannot have carried a serial, because nothing was plugged in when the
            // settings were read.
            let letter_key = format!("drive:{}", medium.drive_letter);
            let candidate = remembered
                .iter()
                .find(|entry| entry.volume == key)
                .or_else(|| remembered.iter().find(|entry| entry.volume == letter_key))
                .and_then(|entry| rebase_onto(&medium.root, &entry.path))
                .filter(|path| dir_exists(path));

            ExportTarget {
                directory: candidate.unwrap_or_else(|| medium.root.clone()),
                volume_key: key,
                medium: Some(medium.clone()),
            }
        }
        ExportDestination::Host => {
            let directory = remembered
                .iter()
                .find(|entry| !entry.volume.starts_with("usb:") && dir_exists(Path::new(&entry.path)))
                .map(|entry| PathBuf::from(&entry.path))
                .unwrap_or_else(|| host_default.to_path_buf());
            ExportTarget {
                volume_key: volume_key_for_path(&directory, &[]),
                directory,
                medium: None,
            }
        }
    }
}

/// A remembered path moved onto `root`: `E:\` + `F:\jobs\out` → `E:\jobs\out`.
///
/// A serial names a *filesystem*, so a folder on it only means anything relative to that
/// filesystem's root — and the root is whichever letter the stick was given this time.
/// `None` for a remembered path that is not rooted at a drive letter, which is not a
/// location on a stick and cannot be moved onto one.
fn rebase_onto(root: &Path, remembered: &str) -> Option<PathBuf> {
    let within = path_within_volume(remembered)?;
    Some(if within.is_empty() { root.to_path_buf() } else { root.join(within) })
}

/// What follows the drive prefix in a rooted Windows path: `E:\jobs\out` → `jobs\out`,
/// `E:\` and `E:` → the empty string. `None` for anything not rooted at a drive letter.
///
/// String-parsed for the same reason as [`drive_letter_of`]: on a non-Windows test host
/// `Path` has no notion of a drive prefix and hands back `E:\jobs` as one component.
fn path_within_volume(path: &str) -> Option<&str> {
    let trimmed = path.trim();
    drive_letter_of(trimmed)?;
    Some(trimmed[2..].trim_start_matches(['\\', '/']))
}

/// The medium a written file landed on, if any.
///
/// Matched against where the file *actually* went rather than against the target the
/// dialog opened at, because the dialog lets the user navigate anywhere — and ejecting a
/// drive the program was not written to would be both useless and alarming.
pub fn medium_for_path(media: &[RemovableMedium], path: &Path) -> Option<RemovableMedium> {
    let letter = drive_letter_of(&path.to_string_lossy())?;
    media.iter().find(|medium| medium.drive_letter == letter).cloned()
}

/// The toast for a successful eject.
pub fn eject_report(medium: &RemovableMedium, outcome: EjectOutcome) -> String {
    match outcome {
        EjectOutcome::Ejected => {
            format!("Ejected {} — safe to remove.", medium.display_name())
        }
        EjectOutcome::SafeToRemove => format!(
            "Exported to {} — safe to remove (this device has no eject mechanism).",
            medium.display_name()
        ),
    }
}

/// The toast for a failed eject.
///
/// Every message leads with the export. By the time this runs the program is already on
/// the stick, and a user who reads "Could not eject" first will assume they lost the file
/// and export it again.
pub fn eject_advice(medium: &RemovableMedium, error: &EjectError) -> String {
    let name = medium.display_name();
    match error {
        EjectError::Busy { .. } => format!(
            "Exported to {name}. Windows would not release the drive — close anything \
             showing it, then use Safely Remove Hardware before unplugging."
        ),
        EjectError::Unsupported => format!(
            "Exported to {name}. Ejecting is only supported on Windows — unmount the device \
             the usual way."
        ),
        EjectError::Io(err) => format!(
            "Exported to {name}. Could not eject the drive ({err}); use Safely Remove \
             Hardware before unplugging."
        ),
    }
}

/// Flushes, dismounts and ejects `medium`.
///
/// Blocking, for up to a few seconds. Not to be called on the UI thread — use
/// [`request_eject`].
pub fn eject(medium: &RemovableMedium) -> Result<EjectOutcome, EjectError> {
    platform::eject(medium)
}

/// The drive letter a Windows path is rooted at, uppercased.
///
/// Parsed from the string rather than through `Path::components`, because this also runs
/// in unit tests on non-Windows hosts, where `Path` has no notion of a drive prefix and
/// would hand back `E:\out` as one opaque component. `None` for UNC paths, relative
/// paths, and `E:relative` — that last one is a real Win32 form meaning "the current
/// directory *on* E:", which is not a rooted path and must not be treated as one.
pub(crate) fn drive_letter_of(path: &str) -> Option<char> {
    let bytes = path.trim().as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    match bytes.get(2) {
        None | Some(b'\\') | Some(b'/') => Some(bytes[0].to_ascii_uppercase() as char),
        _ => None,
    }
}

/// Whether a new scan is worth waking the UI for.
///
/// "Changed" means identity plus *material* free space: a stick appearing, vanishing or
/// being relabelled always counts; a few kilobytes of free-space drift never does. Pure
/// and unit-tested, because this predicate is the only thing standing between a
/// two-second poll and a two-second full-app repaint.
///
/// Both lists are in drive-letter order (the scan walks the drive bitmask A→Z), so a
/// positional comparison is a comparison of the same drives.
fn is_material_change(previous: &[RemovableMedium], current: &[RemovableMedium]) -> bool {
    if previous.len() != current.len() {
        return true;
    }
    previous.iter().zip(current).any(|(before, after)| {
        before.drive_letter != after.drive_letter
            || before.label != after.label
            // Without this, one stick swapped for another that happens to share its
            // letter, its label and its free space to the megabyte is a change nothing
            // reports — and the export folder offered would be the *previous* stick's.
            || before.serial != after.serial
            || before.free_bytes.abs_diff(after.free_bytes) >= FREE_SPACE_NOTICE_BYTES
    })
}

/// Polls, publishes when something changed, and services eject requests — forever.
///
/// `recv_timeout` doubles as the poll sleep, so an eject runs the instant it is requested
/// rather than up to a poll interval later, and the rescan at the top of the next
/// iteration drops the ejected drive out of the picker immediately. There is no shutdown
/// path: like `k2g-generation`, this is a daemon thread the process exit reaps, and a
/// join would make quitting wait out a poll interval for nothing.
fn watcher_loop(rx: mpsc::Receiver<Command>) {
    platform::prepare_thread();

    let mut published: Vec<RemovableMedium> = Vec::new();
    loop {
        let scanned = platform::scan();
        if is_material_change(&published, &scanned) {
            log::debug!(
                "removable media: {}",
                if scanned.is_empty() {
                    "none".to_string()
                } else {
                    scanned.iter().map(RemovableMedium::display_name).collect::<Vec<_>>().join(", ")
                }
            );
            published = scanned.clone();
            if let Ok(mut media) = MEDIA.write() {
                *media = scanned;
            }
            // The generation service's channel, not a second one: `AppRoot` subscribes
            // exactly once, so a private channel here would never be read.
            super::wake_ui();
        }

        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(Command::Eject(medium)) => handle_eject(&medium),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Ejects and reports the outcome as a toast.
///
/// `log_event` rather than `push_runtime_error`: that helper marks everything an error and
/// would raise the red diagnostics banner, and a failed eject *after a successful save* is
/// a warning at most. The detail goes to the log for anyone who wants it.
fn handle_eject(medium: &RemovableMedium) {
    match eject(medium) {
        Ok(outcome) => {
            let message = eject_report(medium, outcome);
            log::info!("{message}");
            super::with_ctx_mut(|ctx| ctx.app.log_event(message));
        }
        Err(error) => {
            log::warn!("could not eject {}: {error}", medium.display_name());
            let message = eject_advice(medium, &error);
            super::with_ctx_mut(|ctx| ctx.app.log_event(message));
        }
    }
    super::wake_ui();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A medium with everything but the drive letter and label left at a plausible
    /// default. Serial-free, which is the pre-serial world and the fallback path.
    fn medium(letter: char, label: &str) -> RemovableMedium {
        RemovableMedium {
            root: PathBuf::from(format!("{letter}:\\")),
            label: label.to_string(),
            drive_letter: letter,
            serial: None,
            free_bytes: 4 * 1024 * 1024 * 1024,
        }
    }

    /// The same, with an identity — which is what the export memory is filed under.
    fn stick(letter: char, label: &str, serial: u32) -> RemovableMedium {
        RemovableMedium { serial: Some(serial), ..medium(letter, label) }
    }

    fn remembered(volume: &str, path: &str) -> ExportDirectory {
        ExportDirectory { volume: volume.to_string(), path: path.to_string() }
    }

    #[test]
    fn nothing_plugged_in_means_no_medium_to_export_to() {
        assert_eq!(export_medium(&[]), None);
    }

    #[test]
    fn with_two_sticks_in_the_lowest_letter_is_the_opening_bid() {
        let media = [medium('E', "KINGSTON"), medium('F', "SDCARD")];
        assert_eq!(export_medium(&media).unwrap().drive_letter, 'E');
    }

    #[test]
    fn a_volume_is_filed_under_its_serial_and_falls_back_to_its_letter() {
        assert_eq!(volume_key(&stick('E', "KINGSTON", 0x1A2B_3C4D)), "usb:1A2B3C4D");
        assert_eq!(volume_key(&medium('E', "KINGSTON")), "drive:E");
        let media = [stick('E', "KINGSTON", 0x1A2B_3C4D)];
        assert_eq!(volume_key_for_path(Path::new("E:\\jobs"), &media), "usb:1A2B3C4D");
        assert_eq!(volume_key_for_path(Path::new("C:\\out"), &media), "drive:C");
        assert_eq!(volume_key_for_path(Path::new("/home/me/out"), &media), HOST_VOLUME_KEY);
    }

    /// The headline, and the only reason the serial is read at all: a stick that comes
    /// back on a different letter still opens in the folder it was last exported to.
    #[test]
    fn a_sticks_folder_survives_it_returning_on_another_letter() {
        let table = [remembered("usb:1A2B3C4D", "F:\\jobs")];
        let now_at_e = stick('E', "KINGSTON", 0x1A2B_3C4D);
        let target = choose_export_target(
            &ExportDestination::Medium(now_at_e),
            &table,
            Path::new("C:\\Downloads"),
            |_| true,
        );
        assert_eq!(target.directory, PathBuf::from("E:\\jobs"));
        assert_eq!(target.volume_key, "usb:1A2B3C4D");
    }

    #[test]
    fn two_sticks_that_have_both_been_e_keep_their_own_folders() {
        let table = [
            remembered("usb:11111111", "E:\\alpha"),
            remembered("usb:22222222", "E:\\beta"),
        ];
        let host = Path::new("C:\\Downloads");
        let first = choose_export_target(
            &ExportDestination::Medium(stick('E', "ONE", 0x1111_1111)), &table, host, |_| true);
        let second = choose_export_target(
            &ExportDestination::Medium(stick('E', "TWO", 0x2222_2222)), &table, host, |_| true);
        assert_eq!(first.directory, PathBuf::from("E:\\alpha"));
        assert_eq!(second.directory, PathBuf::from("E:\\beta"));
    }

    /// A folder deleted since — or a stick reformatted, which is the same thing from
    /// here — opens at the root of the device rather than at nothing.
    #[test]
    fn a_vanished_folder_on_a_stick_falls_back_to_its_root() {
        let table = [remembered("usb:1A2B3C4D", "E:\\jobs")];
        let target = choose_export_target(
            &ExportDestination::Medium(stick('E', "KINGSTON", 0x1A2B_3C4D)),
            &table,
            Path::new("C:\\Downloads"),
            |_| false,
        );
        assert_eq!(target.directory, PathBuf::from("E:\\"));
    }

    #[test]
    fn a_stick_with_nothing_remembered_opens_at_its_root() {
        let target = choose_export_target(
            &ExportDestination::Medium(stick('E', "KINGSTON", 0x1A2B_3C4D)),
            &[],
            Path::new("C:\\Downloads"),
            |_| true,
        );
        assert_eq!(target.directory, PathBuf::from("E:\\"));
    }

    /// What a migration leaves behind: a folder filed under a letter, because nothing was
    /// plugged in when the old settings were read. It is honoured once, and the target it
    /// produces is keyed by serial — so the first export re-files it.
    #[test]
    fn a_migrated_letter_entry_is_honoured_then_refiled_under_the_serial() {
        let table = [remembered("drive:E", "E:\\jobs")];
        let target = choose_export_target(
            &ExportDestination::Medium(stick('E', "KINGSTON", 0x1A2B_3C4D)),
            &table,
            Path::new("C:\\Downloads"),
            |_| true,
        );
        assert_eq!(target.directory, PathBuf::from("E:\\jobs"));
        assert_eq!(target.volume_key, "usb:1A2B3C4D", "the write re-files it by serial");
    }

    /// Each fixed drive keeps its own, by being the most recent one that still exists.
    #[test]
    fn the_host_takes_the_most_recent_folder_that_is_not_on_a_stick() {
        let table = [
            remembered("usb:1A2B3C4D", "E:\\jobs"),
            remembered("drive:D", "D:\\work"),
            remembered("drive:C", "C:\\out"),
        ];
        let target = choose_export_target(
            &ExportDestination::Host, &table, Path::new("C:\\Downloads"), |_| true);
        assert_eq!(target.directory, PathBuf::from("D:\\work"), "not the stick's folder");
        assert_eq!(target.volume_key, "drive:D");
        assert_eq!(target.medium, None);
    }

    /// Covers both "never exported" and "the folder has since gone" — a remembered path
    /// that is a file rather than a directory is just `dir_exists` saying no.
    #[test]
    fn the_host_falls_back_to_the_platform_default() {
        let table = [remembered("drive:D", "D:\\work")];
        let target = choose_export_target(
            &ExportDestination::Host, &table, Path::new("C:\\Downloads"), |_| false);
        assert_eq!(target.directory, PathBuf::from("C:\\Downloads"));

        let empty = choose_export_target(
            &ExportDestination::Host, &[], Path::new("C:\\Downloads"), |_| true);
        assert_eq!(empty.directory, PathBuf::from("C:\\Downloads"));
    }

    #[test]
    fn paths_within_a_volume_are_parsed_from_rooted_paths_only() {
        assert_eq!(path_within_volume("E:\\jobs\\out"), Some("jobs\\out"));
        assert_eq!(path_within_volume("e:/jobs"), Some("jobs"));
        assert_eq!(path_within_volume("E:\\"), Some(""));
        assert_eq!(path_within_volume("E:"), Some(""));
        assert_eq!(path_within_volume("E:relative"), None);
        assert_eq!(path_within_volume("/home/user"), None);
        assert_eq!(path_within_volume(""), None);
    }

    #[test]
    fn drive_letters_are_parsed_from_rooted_paths_only() {
        assert_eq!(drive_letter_of("E:\\out"), Some('E'));
        assert_eq!(drive_letter_of("e:/out"), Some('E'));
        assert_eq!(drive_letter_of("E:"), Some('E'));
        assert_eq!(drive_letter_of("  E:\\out  "), Some('E'));
        // Drive-*relative*: means "the current directory on E:", which is not a location.
        assert_eq!(drive_letter_of("E:relative"), None);
        assert_eq!(drive_letter_of("\\\\server\\share"), None);
        assert_eq!(drive_letter_of("jobs\\out"), None);
        assert_eq!(drive_letter_of("/home/user"), None);
        assert_eq!(drive_letter_of(""), None);
    }

    #[test]
    fn the_written_file_decides_which_medium_to_eject() {
        let media = [medium('E', "KINGSTON"), medium('F', "SDCARD")];
        let found = medium_for_path(&media, Path::new("F:\\jobs\\panel.nc")).unwrap();
        assert_eq!(found.drive_letter, 'F');
        // Saved somewhere else entirely: nothing to eject, which is the point.
        assert_eq!(medium_for_path(&media, Path::new("C:\\Users\\me\\panel.nc")), None);
    }

    #[test]
    fn insertion_removal_and_relabelling_are_material() {
        let one = [medium('E', "KINGSTON")];
        let two = [medium('E', "KINGSTON"), medium('F', "SDCARD")];
        assert!(is_material_change(&[], &one), "a stick appearing");
        assert!(is_material_change(&one, &[]), "a stick vanishing");
        assert!(is_material_change(&one, &two), "a second stick appearing");
        assert!(
            is_material_change(&one, &[medium('E', "PANELS")]),
            "the same drive relabelled"
        );
        assert!(
            is_material_change(&one, &[medium('F', "KINGSTON")]),
            "the same label on a different letter"
        );
        assert!(!is_material_change(&one, &one), "nothing changed");

        // Two sticks of the same make, formatted the same way, swapped between polls:
        // identical on letter, label and free space, and a different volume. Missing this
        // would leave the export dialog offering the folder belonging to the one that has
        // just been pulled out.
        let first = [stick('E', "KINGSTON", 0x1111_1111)];
        let second = [stick('E', "KINGSTON", 0x2222_2222)];
        assert!(is_material_change(&first, &second), "one stick swapped for another");
    }

    /// The predicate that stops an idle app repainting on a timer.
    #[test]
    fn free_space_drift_is_only_material_above_the_threshold() {
        let before = [medium('E', "KINGSTON")];
        let mut drifted = before.clone();
        drifted[0].free_bytes -= FREE_SPACE_NOTICE_BYTES - 1;
        assert!(!is_material_change(&before, &drifted), "sub-threshold drift");

        let mut written = before.clone();
        written[0].free_bytes -= FREE_SPACE_NOTICE_BYTES;
        assert!(is_material_change(&before, &written), "a real write");
    }
}
