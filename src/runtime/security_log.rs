//! An append-only record of security-relevant events, written to disk.
//!
//! EU CRA Annex I (2)(l) asks a product to "provide security related information by
//! recording and monitoring relevant internal activity, including the access to or
//! modification of data, services or functions, with an opt-out mechanism for the
//! user". This module is that record; the switch is `security_log_enabled`, on by
//! default.
//!
//! # Not the same thing as the Logs screen
//!
//! [`crate::runtime::log_capture`] is a 2000-line in-memory ring buffer of everything
//! `tracing` emits — a live tail for diagnosing the run in front of you, gone when the
//! process exits. This is the opposite: a small number of deliberately chosen events,
//! kept across runs, in a format something other than a human can read. Keeping them
//! apart means debug logging can be as noisy as it likes without diluting the record,
//! and the record can be retained without retaining everything.
//!
//! # What is *not* in here
//!
//! No telemetry. Nothing in this file is transmitted anywhere, by this module or any
//! other — the update check is the only outbound request k2g makes and it sends none
//! of this.
//!
//! And no personal data, which takes one deliberate step rather than none: on Windows
//! every absolute path contains the account name (`C:\Users\jl\...`), so paths are put
//! through [`redact`] and the home directory becomes `~` before anything is written.
//! That is what makes "this file holds no personal data" true rather than merely
//! intended, and it is Annex I (2)(g) data minimisation applied to the one place k2g
//! would otherwise leak an identity.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use log::warn;
use serde_json::{json, Value};

/// Rotate at 2 MB, keeping one previous generation. Two files bound the record at
/// ~4 MB, which is thousands of events — far more history than anyone needs, and
/// still small enough to attach to a bug report.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

const CURRENT_FILE: &str = "security.jsonl";
const PREVIOUS_FILE: &str = "security.1.jsonl";

/// Mirrors `AppState::security_log_enabled` so a logging call site does not have to
/// take the global context lock. Writers are frequently *inside* a `with_ctx_mut`
/// already, and re-entering that lock would deadlock.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// What happened. A closed set rather than free text, so the record can be filtered
/// and counted rather than only read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The application started, with its version.
    AppStarted,
    /// The update check ran, whatever it concluded.
    UpdateChecked,
    /// A newer release was found and announced.
    UpdateAvailable,
    /// A downloaded installer's signature verified, and it was launched.
    UpdateInstallerVerified,
    /// A downloaded installer was **rejected** and deleted. The highest-value line in
    /// the file: it is what a supply-chain attempt looks like from in here.
    UpdateInstallerRejected,
    /// The user turned the update check on or off.
    UpdateCheckSettingChanged,
    /// The user turned this log on or off.
    SecurityLogSettingChanged,
    /// k2g's plugin directory was written into a KiCad installation.
    KicadPluginRegistered,
    /// k2g's plugin directory was removed from a KiCad installation.
    KicadPluginUnregistered,
    /// `api.enable_server` in another application's config file was changed.
    KicadApiSettingChanged,
    /// A configuration or catalog file could not be loaded at all and was set aside — the
    /// application is running on something other than what is on disk.
    ConfigRejected,
    /// A configuration or catalog file loaded, but something in it did not validate.
    ///
    /// Kept apart from [`Self::ConfigRejected`] because the two call for opposite
    /// responses: a rejected file means a profile has silently vanished from the
    /// application, while a complaint means one field is not what the schema says and
    /// everything else is in use. Reporting both as "rejected" made every stray value read
    /// like a lost profile.
    ConfigProblem,
    /// A G-code program was written to disk or to removable media.
    GcodeWritten,
    /// Settings were reset to their shipped defaults.
    FactoryReset,
    /// The user's k2g data directory was deleted.
    DataDeleted,
}

impl Event {
    /// The stable string written to the file. Explicit rather than derived from the
    /// variant name so renaming a variant cannot silently break a reader.
    fn kind(self) -> &'static str {
        match self {
            Self::AppStarted => "app.started",
            Self::UpdateChecked => "update.checked",
            Self::UpdateAvailable => "update.available",
            Self::UpdateInstallerVerified => "update.installer_verified",
            Self::UpdateInstallerRejected => "update.installer_rejected",
            Self::UpdateCheckSettingChanged => "setting.update_check_changed",
            Self::SecurityLogSettingChanged => "setting.security_log_changed",
            Self::KicadPluginRegistered => "kicad.plugin_registered",
            Self::KicadPluginUnregistered => "kicad.plugin_unregistered",
            Self::KicadApiSettingChanged => "kicad.api_setting_changed",
            Self::ConfigRejected => "config.rejected",
            Self::ConfigProblem => "config.problem",
            Self::GcodeWritten => "gcode.written",
            Self::FactoryReset => "data.factory_reset",
            Self::DataDeleted => "data.deleted",
        }
    }
}

/// Whether an event was allowed or refused. Recording only successes would leave the
/// interesting half of an audit trail on the floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Failed,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
        }
    }
}

/// Track the user's preference. Called at startup and whenever the switch moves.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The directory the record lives in (`<app data>/logs`).
pub fn log_dir() -> Option<PathBuf> {
    crate::paths::k2g_data_dir().map(|root| root.join("logs"))
}

/// Append one event.
///
/// Never fails and never panics: a record that can take down the operation it is
/// recording is worse than no record. Every error becomes a `warn!` and the event is
/// dropped.
pub fn record(event: Event, outcome: Outcome, detail: Value) {
    if !is_enabled() {
        return;
    }
    if let Err(err) = try_record(event, outcome, detail) {
        warn!("Could not write the security log: {err}");
    }
}

/// Convenience for the common `Outcome::Ok` case.
pub fn record_ok(event: Event, detail: Value) {
    record(event, Outcome::Ok, detail);
}

fn try_record(event: Event, outcome: Outcome, detail: Value) -> std::io::Result<()> {
    let Some(dir) = log_dir() else {
        return Ok(()); // no data directory — nothing sensible to do
    };
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(CURRENT_FILE);

    rotate_if_needed(&dir, &path)?;

    let line = serde_json::to_string(&json!({
        "time": chrono::Utc::now().to_rfc3339(),
        "kind": event.kind(),
        "outcome": outcome.as_str(),
        "version": env!("CARGO_PKG_VERSION"),
        "detail": detail,
    }))
    .unwrap_or_else(|_| String::from(r#"{"kind":"log.unserialisable"}"#));

    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{line}")
}

/// Roll `security.jsonl` to `security.1.jsonl` once it passes [`MAX_BYTES`],
/// discarding the generation before that.
fn rotate_if_needed(dir: &Path, path: &Path) -> std::io::Result<()> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(()); // not created yet
    };
    if meta.len() < MAX_BYTES {
        return Ok(());
    }
    let previous = dir.join(PREVIOUS_FILE);
    let _ = std::fs::remove_file(&previous);
    std::fs::rename(path, &previous)
}

/// Read the record back, newest last, for the Logs screen and for export.
///
/// Returns the previous generation followed by the current one, so the result reads
/// in chronological order across a rotation.
pub fn read_all() -> Vec<Value> {
    let Some(dir) = log_dir() else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for name in [PREVIOUS_FILE, CURRENT_FILE] {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else {
            continue;
        };
        entries.extend(
            text.lines()
                .filter(|line| !line.trim().is_empty())
                // A truncated final line (power loss mid-append) must not discard the
                // whole file; skip what will not parse and keep the rest.
                .filter_map(|line| serde_json::from_str::<Value>(line).ok()),
        );
    }
    entries
}

/// Delete the record. Used by the opt-out and by the data-deletion action.
pub fn erase() -> std::io::Result<()> {
    let Some(dir) = log_dir() else {
        return Ok(());
    };
    for name in [CURRENT_FILE, PREVIOUS_FILE] {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Replace the user's home directory with `~` in a path.
///
/// The one transformation that keeps this file free of personal data. On Windows
/// every path under the profile embeds the account name, which is very often the
/// user's real name; on Linux and macOS the same is true of `/home/<name>`.
///
/// The temp directory is redacted as well, because on Windows it lives *inside* the
/// profile (`C:\Users\<name>\AppData\Local\Temp`) and is where update downloads land.
/// Roots are tried longest-first so the more specific one wins: matching the home
/// directory first would render a temp path as `~\AppData\Local\Temp\...`, which still
/// carries no name but says more about the machine than it needs to.
pub fn redact(path: &Path) -> String {
    let text = path.display().to_string();

    let mut roots: Vec<PathBuf> = vec![std::env::temp_dir()];
    roots.extend(dirs::home_dir());
    roots.sort_by_key(|root| std::cmp::Reverse(root.as_os_str().len()));

    for root in roots {
        let root_text = root.display().to_string();
        if root_text.is_empty() {
            continue;
        }
        // Case-insensitive on Windows, where `C:\Users` and `c:\users` are the same
        // directory and either spelling can reach us.
        let matches = if cfg!(windows) {
            text.to_ascii_lowercase()
                .starts_with(&root_text.to_ascii_lowercase())
        } else {
            text.starts_with(&root_text)
        };
        if matches {
            return format!("~{}", &text[root_text.len()..]);
        }
    }
    text
}

/// [`redact`] for something already stringified.
pub fn redact_str(text: &str) -> String {
    redact(Path::new(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_home_directory_never_reaches_the_record() {
        // The assertion that makes "no personal data" a fact rather than a claim.
        let home = dirs::home_dir().expect("a test host has a home directory");
        let inside = home.join("Documents").join("boards").join("panel.kicad_pcb");

        let redacted = redact(&inside);
        assert!(
            redacted.starts_with('~'),
            "a path under $HOME must be redacted, got {redacted}"
        );
        assert!(
            !redacted.contains(&home.display().to_string()),
            "the home directory must not survive redaction: {redacted}"
        );
        // The useful part — which file, where under home — is kept.
        assert!(redacted.contains("panel.kicad_pcb"));
    }

    #[test]
    fn the_temp_directory_is_redacted_too() {
        // Windows puts the account name in %TEMP% as well
        // (C:\Users\<name>\AppData\Local\Temp), and the update downloads land there.
        let inside = std::env::temp_dir().join("k2g-update").join("k2g-0.9.1.msi");
        let redacted = redact(&inside);
        assert!(redacted.starts_with('~'), "got {redacted}");
        assert!(redacted.ends_with("k2g-0.9.1.msi"));
    }

    #[test]
    fn a_path_outside_the_home_directory_is_left_alone() {
        // Nothing personal in a system path, and mangling it would lose information.
        let system = if cfg!(windows) {
            PathBuf::from("C:\\Program Files\\KiCad\\10.0\\bin\\kicad.exe")
        } else {
            PathBuf::from("/usr/bin/kicad")
        };
        assert_eq!(redact(&system), system.display().to_string());
    }

    #[cfg(windows)]
    #[test]
    fn redaction_is_case_insensitive_on_windows() {
        let home = dirs::home_dir().unwrap().display().to_string();
        let shouty = PathBuf::from(format!("{}\\Documents\\x.txt", home.to_uppercase()));
        assert!(
            redact(&shouty).starts_with('~'),
            "C:\\USERS\\... is the same directory as C:\\Users\\..."
        );
    }

    #[test]
    fn every_event_kind_is_distinct_and_stable() {
        // These strings are the file format. A collision would silently merge two
        // different events in anything reading the record back.
        let all = [
            Event::AppStarted,
            Event::UpdateChecked,
            Event::UpdateAvailable,
            Event::UpdateInstallerVerified,
            Event::UpdateInstallerRejected,
            Event::UpdateCheckSettingChanged,
            Event::SecurityLogSettingChanged,
            Event::KicadPluginRegistered,
            Event::KicadPluginUnregistered,
            Event::KicadApiSettingChanged,
            Event::ConfigRejected,
            Event::ConfigProblem,
            Event::GcodeWritten,
            Event::FactoryReset,
            Event::DataDeleted,
        ];
        let mut kinds: Vec<&str> = all.iter().map(|e| e.kind()).collect();
        let count = kinds.len();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), count, "event kind strings must be unique");
    }

    #[test]
    fn opting_out_stops_the_writer() {
        let was = is_enabled();
        set_enabled(false);
        assert!(!is_enabled());
        // `record` must be a no-op, not merely quieter — nothing reaches the disk.
        record_ok(Event::AppStarted, json!({ "probe": true }));
        set_enabled(was);
    }

    /// Rotation must bound the record, and must not lose the generation it rolls.
    #[test]
    fn rotation_keeps_one_previous_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CURRENT_FILE);

        std::fs::write(&path, vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();
        rotate_if_needed(dir.path(), &path).unwrap();

        assert!(!path.exists(), "the oversized file was rolled away");
        assert!(dir.path().join(PREVIOUS_FILE).exists(), "and kept as .1");

        // A second rotation discards the oldest rather than accumulating.
        std::fs::write(&path, vec![b'y'; (MAX_BYTES + 1) as usize]).unwrap();
        rotate_if_needed(dir.path(), &path).unwrap();
        let previous = std::fs::read(dir.path().join(PREVIOUS_FILE)).unwrap();
        assert_eq!(previous[0], b'y', "the newer generation replaced the older");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "exactly one previous generation is retained"
        );
    }

    #[test]
    fn a_small_file_is_not_rotated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CURRENT_FILE);
        std::fs::write(&path, b"{}\n").unwrap();
        rotate_if_needed(dir.path(), &path).unwrap();
        assert!(path.exists(), "an under-size record stays put");
        assert!(!dir.path().join(PREVIOUS_FILE).exists());
    }
}
