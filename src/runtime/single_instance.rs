//! One k2g at a time, per user.
//!
//! Two instances share one data directory (`%APPDATA%\k2g` and its equivalents) with no
//! coordination whatsoever. Individual writes are atomic — the datastore writes a temp
//! file and renames it — so nothing is *corrupted*; what happens instead is quieter and
//! worse. Each instance loaded the configuration at its own launch and holds it in
//! memory, and every settings write replaces the document whole. So the second instance's
//! next write reverts every change the first has made since, with no error, no conflict
//! and nothing on screen to suggest anything happened.
//!
//! There is also no use for two. KiCad serves a single API socket, so both windows would
//! be showing the same board.
//!
//! The lock is an advisory lock on a file in the data directory, taken through
//! [`std::fs::File::try_lock`] — which is `LockFileEx` on Windows and `flock` on Unix.
//! The important property is that the operating system releases it when the process ends,
//! **including when it crashes**: a lock file holding a process id has to decide whether
//! that process is still alive, and every way of asking that question is either
//! unportable or wrong. Here there is no stale lock to reason about.
//!
//! The second instance then tries to bring the running window forward rather than merely
//! refusing, because the launch was a request to *use* k2g — usually KiCad's toolbar
//! button pressed while it is already open. Raising another process's window is a Win32
//! call with no portable equivalent, so elsewhere the second instance says what happened
//! and exits.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

/// Name of the lock file inside the data directory. Dotted so it sorts away from the
/// configuration a user might browse, and named for what it is rather than for the pid it
/// happens to contain.
const LOCK_FILE: &str = ".instance.lock";

/// The window title `ui::launch` gives the application, and the only handle another
/// process has on it. Kept here beside the code that searches for it — the two have to
/// agree, and a test asserts they do.
#[cfg(windows)]
pub(crate) const WINDOW_TITLE: &str = "k2g - KiCAD to GCode";

/// Whether this process may proceed, and — while it may — what keeps the claim.
pub enum Claim {
    /// This process holds the lock. The file must be kept alive for the run: dropping it
    /// closes the handle, which releases the lock.
    ///
    /// Never read, and that is not an oversight — the value *is* the claim, and its
    /// `Drop` is what gives it up.
    Held(#[allow(dead_code)] File),
    /// Another k2g already holds it.
    Taken,
    /// The lock could not be evaluated (no data directory, unreadable path). The
    /// application runs: refusing to start because a lock file could not be opened would
    /// trade a rare coordination fault for a total one.
    Unknown(String),
}

/// Takes the single-instance lock for `data_dir`.
pub fn claim(data_dir: &Path) -> Claim {
    let path = data_dir.join(LOCK_FILE);
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(file) => file,
        Err(err) => return Claim::Unknown(format!("{}: {err}", path.display())),
    };

    match file.try_lock() {
        Ok(()) => {
            // Diagnostic only — nothing reads it back. The lock is held by the open
            // handle, not by anything written here, so a pid left behind by a crash is
            // inert rather than misleading.
            let mut file = file;
            let _ = file.set_len(0);
            let _ = writeln!(file, "{}", std::process::id());
            let _ = file.flush();
            Claim::Held(file)
        }
        Err(std::fs::TryLockError::WouldBlock) => Claim::Taken,
        Err(std::fs::TryLockError::Error(err)) => {
            Claim::Unknown(format!("{}: {err}", path.display()))
        }
    }
}

/// Brings the running instance's window to the front, if it can be found.
///
/// Returns whether the window was raised, which is the difference between the second
/// launch looking like it worked and looking like nothing happened.
#[cfg(windows)]
pub fn raise_running_window() -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let title: Vec<u16> = std::ffi::OsStr::new(WINDOW_TITLE)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: `title` is a NUL-terminated UTF-16 buffer that outlives the call, and a
    // null class name asks for any class — the documented way to search by title alone.
    // A window that has closed between the lock test and here returns null, which is the
    // "not found" answer rather than an error.
    let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    if window.is_null() {
        return false;
    }

    // SAFETY: `window` is a handle Windows just returned. `ShowWindow` on a window that
    // has since closed fails rather than misbehaving.
    unsafe {
        if IsIconic(window) != 0 {
            ShowWindow(window, SW_RESTORE);
        }
        SetForegroundWindow(window) != 0
    }
}

/// No portable way to raise another process's window, so nothing is raised.
#[cfg(not(windows))]
pub fn raise_running_window() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lock is exclusive: a second claim on the same directory is refused while the
    /// first is held, and granted again once it is dropped.
    ///
    /// Two claims from *one* process, which is what a test can do — and on both platforms
    /// the lock is per open file description, so this exercises the same exclusion a
    /// second process meets.
    #[test]
    fn a_second_claim_is_refused_while_the_first_is_held() {
        let dir = tempfile::tempdir().unwrap();

        let first = claim(dir.path());
        assert!(
            matches!(first, Claim::Held(_)),
            "the first claim on a free directory must succeed"
        );

        assert!(
            matches!(claim(dir.path()), Claim::Taken),
            "a second k2g must be told the directory is in use"
        );

        drop(first);
        assert!(
            matches!(claim(dir.path()), Claim::Held(_)),
            "and the lock must be free again once the holder exits — which is the whole \
             reason it is an OS lock rather than a pid written to a file"
        );
    }

    /// A directory that cannot hold a lock file does not stop the application.
    #[test]
    fn an_unusable_path_is_not_a_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no").join("such").join("directory");

        assert!(
            matches!(claim(&missing), Claim::Unknown(_)),
            "an unopenable lock path is inconclusive, and inconclusive means carry on"
        );
    }

    /// The title searched for is the title the window is given.
    ///
    /// They are two string literals in two files, and the hand-off is silent when they
    /// disagree: the second instance finds nothing, raises nothing, and the operator sees
    /// a launch that did nothing at all.
    #[cfg(windows)]
    #[test]
    fn the_window_title_matches_the_one_the_window_is_created_with() {
        const LAUNCH: &str = include_str!("../ui/mod.rs");
        assert!(
            LAUNCH.contains(&format!("with_title(\"{WINDOW_TITLE}\")")),
            "ui::launch must create the window with the title this module searches for"
        );
    }
}
