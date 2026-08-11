//! The executable KiCad runs when the user clicks k2g's toolbar button.
//!
//! # Why this exists at all
//!
//! KiCad will not launch an arbitrary program. `api_plugin.cpp` rejects any action
//! whose `entrypoint` is an absolute path outright ("action contains abs path %s;
//! skipping"), resolves the relative one against the directory holding `plugin.json`,
//! and then requires the result to be executable. The program KiCad starts therefore
//! *must* be a file sitting inside the plugin directory.
//!
//! That leaves two options: copy the whole k2g executable into the plugin directory,
//! or put something small there that starts the installed one. Copying loses badly.
//! It duplicates a multi-megabyte binary per KiCad version, and — the reason that
//! settles it — the copy keeps working after k2g is updated. A superseded build with
//! a known vulnerability, still sitting in the user's Documents folder and still
//! wired to a toolbar button, is precisely the outcome the vulnerability-handling
//! duties in EU CRA Annex I Part II exist to prevent. Patching the installed copy
//! must patch what KiCad launches.
//!
//! So the plugin directory gets this shim plus a `k2g-target.txt` naming the real
//! executable. Updating k2g replaces the target; the shim never changes, and the
//! registration keeps working without KiCad or the user being told anything.
//!
//! # What it does
//!
//! Reads the target path, spawns it, exits. The environment is inherited untouched,
//! which is the whole trick: KiCad puts `KICAD_API_SOCKET` and `KICAD_API_TOKEN` into
//! the child environment before launching, and `kicad-ipc-rs` already prefers both
//! over its temp-directory guess. k2g consequently connects straight back to the
//! exact instance that launched it, with no discovery and no ambiguity about which
//! KiCad it is talking to.
//!
//! Anything written to stderr here is surfaced by KiCad in its plugin-error reporter,
//! so failures explain themselves in the UI rather than vanishing.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Name of the sidecar file holding the installed executable's absolute path.
/// Written by k2g at registration time; see `runtime::kicad_integration`.
const TARGET_FILE: &str = "k2g-target.txt";

fn main() -> std::process::ExitCode {
    match launch() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            // KiCad captures stderr and shows it against the failed action, so this
            // is a real user-facing channel rather than a void.
            let _ = writeln!(std::io::stderr(), "k2g: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn launch() -> Result<(), String> {
    let here = std::env::current_exe()
        .map_err(|e| format!("cannot locate the plugin directory: {e}"))?
        .parent()
        .ok_or_else(|| "the plugin launcher has no parent directory".to_string())?
        .to_path_buf();

    let pointer = here.join(TARGET_FILE);
    let raw = std::fs::read_to_string(&pointer).map_err(|e| {
        format!(
            "cannot read {} ({e}). Re-register the plugin from k2g's Settings screen.",
            pointer.display()
        )
    })?;

    // Strip a byte-order mark before trimming. k2g writes this file without one, but
    // it is plain text in the user's Documents folder and a hand-edit in Notepad or
    // PowerShell's `Set-Content -Encoding utf8` adds one. U+FEFF is not whitespace,
    // so `trim` leaves it on the front of the path and the open fails with a message
    // naming a path that looks perfectly correct.
    let target = PathBuf::from(raw.trim_start_matches('\u{feff}').trim());
    if !target.is_file() {
        return Err(format!(
            "k2g is not at {} any more. Re-register the plugin from k2g's Settings screen.",
            target.display()
        ));
    }

    // Detached stdio on purpose. KiCad launches this shim with its pipes redirected
    // and waits for them to close to decide the action has finished; handing those
    // same handles to a GUI application that runs for hours would leave KiCad
    // believing the action never completed.
    Command::new(&target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", target.display()))?;

    Ok(())
}
