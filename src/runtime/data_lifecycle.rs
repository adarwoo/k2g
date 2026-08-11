//! Resetting k2g to its shipped state, and removing every trace of it.
//!
//! Two operations that sound similar and differ in exactly one way: what survives.
//!
//! - [`factory_reset`] clears the *configuration* — settings, profiles, stock, the
//!   job — and lets the next start re-seed the bundled defaults. Catalogs and the
//!   security record are kept, because they are reference data and history, not
//!   configuration. This is EU CRA Annex I (2)(b), "the possibility to reset the
//!   product to its original state".
//! - [`delete_all_data`] removes the whole k2g data directory: configuration,
//!   catalogs, logs, everything. This is Annex I (2)(m), the user's right to "securely
//!   and easily remove on a permanent basis all data and settings".
//!
//! Neither is offered without a confirmation naming the directory, and neither runs
//! anywhere except behind an explicit button.
//!
//! # On "securely"
//!
//! These delete files; they do not overwrite the underlying blocks. On the SSDs this
//! software runs on, overwriting a file's bytes does not reliably erase the physical
//! copy anyway — wear levelling relocates writes, so the old blocks survive under a
//! different mapping and the shredding is theatre. Saying so plainly is more useful
//! than pretending otherwise: a user who needs guaranteed erasure needs full-disk
//! encryption or the drive's own secure-erase, and the documentation says that.
//!
//! What these *do* guarantee is that k2g holds nothing afterwards: no settings, no
//! profiles, no record, and nothing outside the directory named here — the only
//! exception being any KiCad plugin registration, which lives in KiCad's directories
//! and is removed from the KiCad integration card.

use std::path::PathBuf;

use log::info;
use serde_json::json;

use super::security_log::{self, Event, Outcome};

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("Cannot locate the k2g data directory")]
    NoDataDir,

    #[error("Could not remove '{0}': {1}")]
    Remove(String, std::io::Error),
}

/// The directory the destructive actions operate on, for display in a confirmation.
pub fn data_dir() -> Option<PathBuf> {
    crate::paths::k2g_data_dir()
}

/// Delete the configuration tree so the next start re-seeds the shipped defaults.
///
/// Catalogs survive deliberately. They are a reference library — often edited over
/// months, and often the reason a machine is set up the way it is — and "reset my
/// settings" is not a request to throw them away. Deleting them is what
/// [`delete_all_data`] is for.
///
/// Requires a restart: `AppData` holds the parsed store in memory and is the sole
/// writer of these files, so a live re-seed would race its background flush and could
/// write the old state straight back over the new one.
pub fn factory_reset() -> Result<PathBuf, LifecycleError> {
    let root = data_dir().ok_or(LifecycleError::NoDataDir)?;
    let configs = factory_reset_in(&root)?;
    info!("Factory reset: removed {}", configs.display());
    Ok(configs)
}

/// [`factory_reset`] against an arbitrary root, so the "what survives" contract can
/// be tested without touching the real data directory.
fn factory_reset_in(root: &std::path::Path) -> Result<PathBuf, LifecycleError> {
    let configs = root.join("configs");
    let outcome = remove_tree(&configs);

    security_log::record(
        Event::FactoryReset,
        if outcome.is_ok() { Outcome::Ok } else { Outcome::Failed },
        json!({
            "removed": security_log::redact(&configs),
            "kept": ["catalogs", "logs"],
            "error": outcome.as_ref().err().map(|e| e.to_string()),
        }),
    );
    outcome?;
    Ok(configs)
}

/// Remove a directory tree, treating "already gone" as success.
///
/// Both operations are idempotent for the same reason: the user asked for something
/// not to be there, and it is not there. Erroring on a fresh install that never
/// created the directory would be a failure report for a satisfied request.
fn remove_tree(path: &std::path::Path) -> Result<(), LifecycleError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(LifecycleError::Remove(path.display().to_string(), err)),
    }
}

/// Remove the entire k2g data directory.
///
/// The security record goes too. It is k2g's data, the user asked for k2g's data to
/// be gone, and keeping an audit trail the user has explicitly asked to delete would
/// be the wrong side of the trade — so the *last* thing recorded is the deletion
/// itself, written before the directory goes, on the chance the removal fails
/// part-way and leaves the file behind.
pub fn delete_all_data() -> Result<PathBuf, LifecycleError> {
    let root = data_dir().ok_or(LifecycleError::NoDataDir)?;

    security_log::record_ok(
        Event::DataDeleted,
        json!({ "root": security_log::redact(&root) }),
    );

    remove_tree(&root)?;

    info!("Deleted all k2g data at {}", root.display());
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the directory shape `ensure_app_dirs` creates, with a file in each part.
    fn populated_root(root: &std::path::Path) {
        for sub in ["configs", "configs/cnc_profiles", "catalogs", "logs"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(root.join("configs/global.setting.yaml"), "units: mm").unwrap();
        std::fs::write(root.join("configs/cnc_profiles/a.yaml"), "id: a").unwrap();
        std::fs::write(root.join("catalogs/mine.yaml"), "tools: []").unwrap();
        std::fs::write(root.join("logs/security.jsonl"), "{}\n").unwrap();
    }

    /// The distinction between the two operations, pinned: a reset must leave the
    /// catalog library and the record standing.
    #[test]
    fn a_factory_reset_clears_configuration_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        populated_root(root);

        factory_reset_in(root).expect("reset should succeed");

        assert!(!root.join("configs").exists(), "configuration is cleared");
        assert!(
            root.join("catalogs/mine.yaml").exists(),
            "a hand-edited catalog is reference data and must survive a settings reset"
        );
        assert!(
            root.join("logs/security.jsonl").exists(),
            "the security record must survive a settings reset"
        );
    }

    #[test]
    fn deleting_all_data_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("k2g");
        populated_root(&root);

        remove_tree(&root).expect("deletion should succeed");
        assert!(!root.exists(), "nothing of k2g's may remain");
    }

    /// Both operations are idempotent — running one twice, or on a fresh install
    /// where the directory was never created, must succeed rather than error.
    #[test]
    fn removing_something_already_absent_is_success() {
        let dir = tempfile::tempdir().unwrap();
        assert!(remove_tree(&dir.path().join("never-existed")).is_ok());

        // And a second reset over an already-reset tree.
        let root = dir.path().join("twice");
        populated_root(&root);
        factory_reset_in(&root).unwrap();
        assert!(
            factory_reset_in(&root).is_ok(),
            "resetting an already-reset install must not report a failure"
        );
    }
}
