//! Platform-aware user data directory helper for k2g.
//!
//! Resolves the operating-system–appropriate location where k2g stores its
//! user data, then creates the required subdirectory tree on first access.
//!
//! | Platform | Root path                               |
//! |----------|-----------------------------------------|
//! | Windows  | `%APPDATA%\k2g`                         |
//! | macOS    | `~/Library/Application Support/k2g`     |
//! | Linux    | `$XDG_CONFIG_HOME/k2g` or `~/.config/k2g` |

use std::path::PathBuf;

/// Handles to all k2g user data subdirectories.
pub struct AppDirs {
    /// Root data directory (e.g. `%APPDATA%\k2g` on Windows).
    pub root: PathBuf,
    /// Tool catalogs — user-editable YAML files, validated on load.
    pub catalogs: PathBuf,
    /// Session state (last-used filenames, recent job settings, etc.).
    pub last_session: PathBuf,
    /// JSON Schema reference copies — written once, not intended for editing.
    pub schemas: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum UserPathError {
    #[error("Cannot determine user data directory (missing HOME or APPDATA environment variable)")]
    NoPlatformDir,

    #[error("Failed to create directory '{0}': {1}")]
    CreateDir(String, std::io::Error),

    #[error("Directory '{0}' exists but is not writable — check permissions")]
    NoWriteAccess(String),
}

/// Return the platform-appropriate root data directory for k2g without
/// creating it.
pub fn k2g_data_dir() -> Option<PathBuf> {
    platform_data_dir()
}

/// Ensure the full application directory tree exists and is writable.
///
/// Creates `catalogs/`, `last_session/`, and `schemas/` under the k2g data
/// root if they are absent. Each directory is probed for write access by
/// creating and immediately removing a sentinel file.
pub fn ensure_app_dirs() -> Result<AppDirs, UserPathError> {
    let root = k2g_data_dir().ok_or(UserPathError::NoPlatformDir)?;

    let dirs = AppDirs {
        catalogs: root.join("catalogs"),
        last_session: root.join("last_session"),
        schemas: root.join("schemas"),
        root,
    };

    for dir in [&dirs.root, &dirs.catalogs, &dirs.last_session, &dirs.schemas] {
        create_if_missing(dir)?;
        check_writable(dir)?;
    }

    Ok(dirs)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn create_if_missing(dir: &std::path::Path) -> Result<(), UserPathError> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .map_err(|e| UserPathError::CreateDir(dir.display().to_string(), e))?;
    }
    Ok(())
}

fn check_writable(dir: &std::path::Path) -> Result<(), UserPathError> {
    let probe = dir.join(".k2g_write_probe");
    match std::fs::write(&probe, b"") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(_) => Err(UserPathError::NoWriteAccess(dir.display().to_string())),
    }
}

// ---------------------------------------------------------------------------
// Platform-specific path resolution (one function per target, no dead code)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn platform_data_dir() -> Option<PathBuf> {
    // %APPDATA% resolves to e.g. C:\Users\<name>\AppData\Roaming
    std::env::var_os("APPDATA").map(|v| PathBuf::from(v).join("k2g"))
}

#[cfg(target_os = "macos")]
fn platform_data_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|v| PathBuf::from(v).join("Library").join("Application Support").join("k2g"))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_data_dir() -> Option<PathBuf> {
    // Prefer XDG_CONFIG_HOME, fall back to ~/.config
    std::env::var_os("XDG_CONFIG_HOME")
        .map(|v| PathBuf::from(v).join("k2g"))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|v| PathBuf::from(v).join(".config").join("k2g"))
        })
}
