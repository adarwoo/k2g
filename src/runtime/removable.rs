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
    /// Uppercase ASCII, e.g. `'E'` — the identity both the remembered-path match and the
    /// device path are built on.
    pub drive_letter: char,
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

/// Where a "save to removable media" dialog should open, and which medium that is.
///
/// The two travel together because the button needs both: the directory to open at, and
/// the medium's name for its tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct SaveTarget {
    pub medium: RemovableMedium,
    pub directory: PathBuf,
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

/// Where a "save to removable media" dialog should open, or `None` when there is nothing
/// to save to.
pub fn save_target(media: &[RemovableMedium], remembered: Option<&str>) -> Option<SaveTarget> {
    choose_save_target(media, remembered, |path| path.is_dir())
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
            "Saved to {} — safe to remove (this device has no eject mechanism).",
            medium.display_name()
        ),
    }
}

/// The toast for a failed eject.
///
/// Every message leads with the save. By the time this runs the program is already on the
/// stick, and a user who reads "Could not eject" first will assume they lost the file and
/// save it again.
pub fn eject_advice(medium: &RemovableMedium, error: &EjectError) -> String {
    let name = medium.display_name();
    match error {
        EjectError::Busy { .. } => format!(
            "Saved to {name}. Windows would not release the drive — close anything \
             showing it, then use Safely Remove Hardware before unplugging."
        ),
        EjectError::Unsupported => format!(
            "Saved to {name}. Ejecting is only supported on Windows — unmount the device \
             the usual way."
        ),
        EjectError::Io(err) => format!(
            "Saved to {name}. Could not eject the drive ({err}); use Safely Remove \
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

/// Chooses which removable medium a save dialog should target, and where on it.
///
/// Ordered by what the user most likely means:
///  1. Nothing plugged in ⇒ `None`; the caller falls back to the ordinary save directory
///     rather than inventing a destination.
///  2. The remembered path names a drive letter that is *still present* ⇒ reuse it. If
///     that exact sub-directory has since gone — the classic case being the same letter
///     now belonging to a different stick — fall back to **that medium's root**, not to
///     the first medium: the user asked for `E:`, so give them `E:`.
///  3. Otherwise the lowest-lettered medium's root.
///
/// `dir_exists` is injected rather than probed directly so the whole decision is testable
/// with no hardware and no filesystem. (Compare `resolve_save_directory` in `state.rs`,
/// which probes for real — it can, because its only input is a path. Here half the input
/// is a synthesised medium list, and mixing synthetic and real inputs in one test is how
/// a test ends up passing only on the author's machine.)
fn choose_save_target(
    media: &[RemovableMedium],
    remembered: Option<&str>,
    dir_exists: impl Fn(&Path) -> bool,
) -> Option<SaveTarget> {
    if media.is_empty() {
        return None;
    }

    if let Some(remembered) = remembered {
        if let Some(letter) = drive_letter_of(remembered) {
            if let Some(medium) = media.iter().find(|medium| medium.drive_letter == letter) {
                let directory = Path::new(remembered);
                let directory =
                    if dir_exists(directory) { directory.to_path_buf() } else { medium.root.clone() };
                return Some(SaveTarget { medium: medium.clone(), directory });
            }
        }
    }

    media.first().map(|medium| SaveTarget {
        medium: medium.clone(),
        directory: medium.root.clone(),
    })
}

/// The drive letter a Windows path is rooted at, uppercased.
///
/// Parsed from the string rather than through `Path::components`, because this also runs
/// in unit tests on non-Windows hosts, where `Path` has no notion of a drive prefix and
/// would hand back `E:\out` as one opaque component. `None` for UNC paths, relative
/// paths, and `E:relative` — that last one is a real Win32 form meaning "the current
/// directory *on* E:", which is not a rooted path and must not be treated as one.
fn drive_letter_of(path: &str) -> Option<char> {
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
    /// default — the selection rules only ever look at those two.
    fn medium(letter: char, label: &str) -> RemovableMedium {
        RemovableMedium {
            root: PathBuf::from(format!("{letter}:\\")),
            label: label.to_string(),
            drive_letter: letter,
            free_bytes: 4 * 1024 * 1024 * 1024,
        }
    }

    #[test]
    fn no_media_means_no_target() {
        assert_eq!(choose_save_target(&[], Some("E:\\jobs"), |_| true), None);
    }

    #[test]
    fn the_remembered_directory_wins_while_its_drive_is_present() {
        let media = [medium('E', "KINGSTON"), medium('F', "SDCARD")];
        let target = choose_save_target(&media, Some("F:\\jobs"), |_| true).unwrap();
        assert_eq!(target.directory, PathBuf::from("F:\\jobs"));
        assert_eq!(target.medium.drive_letter, 'F');
    }

    /// The same letter can come back as a *different* stick, so a remembered
    /// sub-directory that is gone falls back to that drive's root — not to another drive.
    #[test]
    fn a_vanished_subdirectory_falls_back_to_its_own_drives_root() {
        let media = [medium('E', "KINGSTON"), medium('F', "SDCARD")];
        let target = choose_save_target(&media, Some("F:\\jobs"), |_| false).unwrap();
        assert_eq!(target.directory, PathBuf::from("F:\\"));
        assert_eq!(target.medium.drive_letter, 'F');
    }

    #[test]
    fn a_remembered_drive_that_is_gone_falls_back_to_the_first_medium() {
        let media = [medium('E', "KINGSTON")];
        let target = choose_save_target(&media, Some("G:\\jobs"), |_| true).unwrap();
        assert_eq!(target.directory, PathBuf::from("E:\\"));
    }

    #[test]
    fn with_nothing_remembered_the_lowest_letter_wins() {
        let media = [medium('E', "KINGSTON"), medium('F', "SDCARD")];
        let target = choose_save_target(&media, None, |_| true).unwrap();
        assert_eq!(target.medium.drive_letter, 'E');
        assert_eq!(target.directory, PathBuf::from("E:\\"));
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
