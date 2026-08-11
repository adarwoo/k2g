//! Registering k2g as a KiCad IPC plugin, and enabling KiCad's API server.
//!
//! Both actions reach outside k2g's own data directory and change how *another*
//! application behaves, so both are user-initiated, reversible, and reported before
//! they run. Nothing here happens automatically at startup except
//! [`refresh_registrations`], which only repairs a registration the user already
//! asked for.
//!
//! That restraint is deliberate rather than timid. An installer that silently flips
//! another product's API server on would be doing the opposite of EU CRA Annex I
//! (2)(b) "secure by default", and KiCad rewrites `kicad_common.json` wholesale when
//! it exits — so a write performed behind a running KiCad is not merely rude, it is
//! silently discarded. The user closes KiCad, presses a button, and is told what
//! changed.
//!
//! # Layout written into a KiCad plugin directory
//!
//! ```text
//! <documents>/KiCad/<version>/plugins/k2g/
//!     plugin.json              generated here, never copied from the repo
//!     k2g-kicad-launcher.exe   the shim KiCad executes (see src/bin/)
//!     k2g-target.txt           absolute path of the installed k2g executable
//!     icon.png                 toolbar icon
//! ```

use std::path::{Path, PathBuf};

use log::{info, warn};
use serde_json::{json, Value};

/// Directory name k2g claims inside KiCad's `plugins/` folder, and the plugin
/// identifier KiCad knows it by. The identifier is also what KiCad namespaces
/// per-plugin settings under, so it must not drift.
const PLUGIN_DIR_NAME: &str = "k2g";
const PLUGIN_IDENTIFIER: &str = "com.github.adarwoo.k2g";

/// Sidecar naming the installed executable. Read by the launcher shim; see
/// [`crate::runtime::kicad_integration`] module docs for why the indirection exists.
const TARGET_FILE: &str = "k2g-target.txt";

/// Base name of the shim, without the platform's executable suffix.
const LAUNCHER_STEM: &str = "k2g-kicad-launcher";

/// The toolbar icon. `icon_small.png` rather than the 1.2 MB `icon.png`: KiCad loads
/// the PNG at its native size and does not resize it, so the full-resolution artwork
/// would be rendered as a giant button.
const PLUGIN_ICON: &[u8] = include_bytes!("../../assets/icons/icon_small.png");

#[derive(Debug, thiserror::Error)]
pub enum IntegrationError {
    #[error("KiCad is running. Close it first — it rewrites its settings file on exit, which would discard this change.")]
    KicadRunning,

    #[error("Cannot find the k2g plugin launcher next to the application ({0}). This build may be incomplete; reinstall k2g.")]
    LauncherMissing(String),

    #[error("Cannot read '{0}': {1}")]
    Read(String, std::io::Error),

    #[error("Cannot write '{0}': {1}")]
    Write(String, std::io::Error),

    #[error("KiCad's settings file '{0}' is not valid JSON ({1}). k2g will not touch it — repair or delete it and let KiCad rebuild it.")]
    MalformedConfig(String, String),
}

/// One KiCad version installed for this user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KicadInstall {
    /// The version directory's name, e.g. `"10.0"` — KiCad's own scheme, not semver.
    pub version: String,
    /// `kicad_common.json` for this version.
    pub common_file: PathBuf,
    /// The `plugins/` directory API plugins are discovered from.
    pub plugins_dir: PathBuf,
}

impl KicadInstall {
    /// Where k2g's plugin directory would live for this version.
    fn plugin_dir(&self) -> PathBuf {
        self.plugins_dir.join(PLUGIN_DIR_NAME)
    }
}

/// Whether KiCad's API server is on, and whether k2g is wired into this version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationStatus {
    pub install: KicadInstall,
    /// `api.enable_server` in `kicad_common.json`. KiCad ships this `false`.
    pub api_enabled: bool,
    /// A k2g plugin directory with a manifest is present.
    pub registered: bool,
    /// Registered, but `k2g-target.txt` names an executable that is not the one
    /// running now — typically after an update, a move, or a second install.
    pub stale: bool,
}

/// Whether a KiCad process is up. `Unknown` is a real answer on platforms where
/// k2g cannot check cheaply, and is reported as such rather than guessed at: telling
/// a user "KiCad is closed" when it is open would make the change silently vanish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KicadRunning {
    Yes,
    No,
    Unknown,
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// KiCad's per-user configuration root (the parent of the version directories).
///
/// `KICAD_CONFIG_HOME` wins when set — KiCad honours it, so a user who has moved
/// their configuration must not have k2g write to the default location instead.
fn kicad_config_home() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("KICAD_CONFIG_HOME") {
        return Some(PathBuf::from(explicit));
    }

    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|v| PathBuf::from(v).join("kicad"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|v| PathBuf::from(v).join("Library").join("Preferences").join("kicad"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".config")))
            .map(|base| base.join("kicad"))
    }
}

/// KiCad's per-user documents root (the parent of the version directories that hold
/// `plugins/`).
///
/// Resolved through `dirs::document_dir` on Windows rather than by joining
/// `%USERPROFILE%\Documents`: the Documents folder is relocatable, and a user who has
/// moved it to another drive would otherwise get a plugin installed where KiCad never
/// looks.
fn kicad_documents_home() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("KICAD_DOCUMENTS_HOME") {
        return Some(PathBuf::from(explicit));
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        dirs::document_dir().map(|d| d.join("KiCad"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        dirs::data_dir().map(|d| d.join("kicad"))
    }
}

/// Whether a directory name is one of KiCad's version directories (`9.0`, `10.0`).
fn is_version_dir(name: &str) -> bool {
    let mut parts = name.split('.');
    let (Some(major), Some(minor), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !major.is_empty()
        && !minor.is_empty()
        && major.chars().all(|c| c.is_ascii_digit())
        && minor.chars().all(|c| c.is_ascii_digit())
}

/// Every KiCad version with a configuration directory for this user, newest first.
///
/// Configuration rather than installation is the right thing to enumerate: KiCad
/// creates it on first run, it is where the API setting lives, and it survives the
/// application being installed somewhere unusual.
pub fn detect_installs() -> Vec<KicadInstall> {
    let (Some(config_home), Some(documents_home)) = (kicad_config_home(), kicad_documents_home())
    else {
        return Vec::new();
    };

    let Ok(entries) = std::fs::read_dir(&config_home) else {
        return Vec::new();
    };

    let mut installs: Vec<KicadInstall> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let version = entry.file_name().to_string_lossy().into_owned();
            is_version_dir(&version).then(|| KicadInstall {
                common_file: entry.path().join("kicad_common.json"),
                plugins_dir: documents_home.join(&version).join("plugins"),
                version,
            })
        })
        .collect();

    // Newest first, numerically — a plain string sort puts "10.0" before "9.0".
    installs.sort_by_key(|install| std::cmp::Reverse(version_key(&install.version)));
    installs
}

fn version_key(version: &str) -> (u32, u32) {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// The current integration state of one install. Never fails: an unreadable KiCad
/// config is reported as "API off" rather than as an error, because the user's next
/// action (enable it) is the same either way.
pub fn status(install: &KicadInstall) -> IntegrationStatus {
    let api_enabled = read_common(install)
        .ok()
        .and_then(|value| value.pointer("/api/enable_server").and_then(Value::as_bool))
        .unwrap_or(false);

    let plugin_dir = install.plugin_dir();
    let registered = plugin_dir.join("plugin.json").is_file();
    let stale = registered && !registered_target_is_current(&plugin_dir);

    IntegrationStatus {
        install: install.clone(),
        api_enabled,
        registered,
        stale,
    }
}

/// Whether the recorded target still names the executable running right now.
fn registered_target_is_current(plugin_dir: &Path) -> bool {
    let Ok(current) = std::env::current_exe() else {
        // Cannot tell — claim current rather than provoke a pointless rewrite loop.
        return true;
    };
    let Ok(recorded) = std::fs::read_to_string(plugin_dir.join(TARGET_FILE)) else {
        return false;
    };
    // Compare canonicalised where possible: the recorded path and `current_exe` can
    // differ by symlink or 8.3 shortening while naming the same file.
    let recorded = PathBuf::from(recorded.trim());
    match (recorded.canonicalize(), current.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => recorded == current,
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// The shim's filename, with the platform's executable suffix.
fn launcher_file_name() -> String {
    format!("{LAUNCHER_STEM}{}", std::env::consts::EXE_SUFFIX)
}

/// The shim as built and installed beside the main executable.
fn installed_launcher() -> Result<PathBuf, IntegrationError> {
    let exe = std::env::current_exe()
        .map_err(|e| IntegrationError::LauncherMissing(e.to_string()))?;
    let candidate = exe
        .parent()
        .ok_or_else(|| IntegrationError::LauncherMissing("no parent directory".into()))?
        .join(launcher_file_name());

    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(IntegrationError::LauncherMissing(candidate.display().to_string()))
    }
}

/// The `plugin.json` KiCad will parse.
///
/// Generated rather than templated from a file in the repository. The entrypoint is
/// platform-dependent and must be *relative* (KiCad discards actions carrying an
/// absolute path), so a checked-in manifest could never be shipped as-is — it would
/// only be a second place for the identifier and scopes to drift out of step with
/// this function.
fn manifest(entrypoint: &str) -> Value {
    json!({
        "$schema": "https://go.kicad.org/api/schemas/v1",
        "identifier": PLUGIN_IDENTIFIER,
        "name": "k2g",
        "description": "Generate CNC machining G-code from the open KiCad board.",
        // "exec", not "python": the schema's enum is ["python", "exec"], and this is
        // a native binary. KiCad launches it with KICAD_API_SOCKET and
        // KICAD_API_TOKEN in the environment, which is how k2g reconnects to the
        // exact instance that started it.
        "runtime": { "type": "exec" },
        "actions": [{
            "identifier": "k2g-create-gcode",
            "name": "Create GCode",
            "description": "Open k2g with this board and generate machining G-code for it.",
            "show-button": true,
            // PCB only. k2g machines a board; a button in the schematic editor
            // would open an application with nothing to act on.
            "scopes": ["pcb"],
            "entrypoint": entrypoint,
            "icons-light": ["icon.png"],
            "icons-dark": ["icon.png"],
        }],
    })
}

/// Install k2g's plugin directory for one KiCad version.
///
/// Idempotent: re-registering over an existing directory refreshes every file, which
/// is also how [`refresh_registrations`] repairs a stale target.
pub fn register(install: &KicadInstall) -> Result<PathBuf, IntegrationError> {
    let launcher = installed_launcher()?;
    let current_exe = std::env::current_exe()
        .map_err(|e| IntegrationError::LauncherMissing(e.to_string()))?;

    let plugin_dir = install.plugin_dir();
    let outcome = write_plugin_dir(&plugin_dir, &launcher, &current_exe);

    // Writing into another application's installation is exactly the "modification of
    // data, services or functions" Annex I (2)(l) asks to see recorded — and unlike
    // most events here, both outcomes matter: a half-written plugin directory is
    // worth being able to find later.
    super::security_log::record(
        super::security_log::Event::KicadPluginRegistered,
        match &outcome {
            Ok(()) => super::security_log::Outcome::Ok,
            Err(_) => super::security_log::Outcome::Failed,
        },
        serde_json::json!({
            "kicad_version": install.version,
            "plugin_dir": super::security_log::redact(&plugin_dir),
            "target": super::security_log::redact(&current_exe),
            "error": outcome.as_ref().err().map(|e| e.to_string()),
        }),
    );
    outcome?;

    info!(
        "Registered the k2g plugin for KiCad {} at {}",
        install.version,
        plugin_dir.display()
    );
    Ok(plugin_dir)
}

/// Lay out a plugin directory. Split from [`register`] so the layout is testable
/// without a KiCad installation, an installed launcher, or the real executable —
/// none of which exist under `cargo test`, whose `current_exe` is the test harness.
fn write_plugin_dir(
    plugin_dir: &Path,
    launcher_src: &Path,
    target_exe: &Path,
) -> Result<(), IntegrationError> {
    std::fs::create_dir_all(plugin_dir)
        .map_err(|e| IntegrationError::Write(plugin_dir.display().to_string(), e))?;

    let entrypoint = launcher_file_name();
    let launcher_dst = plugin_dir.join(&entrypoint);
    let bytes = std::fs::read(launcher_src)
        .map_err(|e| IntegrationError::Read(launcher_src.display().to_string(), e))?;
    write_file(&launcher_dst, &bytes)?;
    ensure_executable(&launcher_dst)?;

    write_file(&plugin_dir.join("icon.png"), PLUGIN_ICON)?;
    write_file(
        &plugin_dir.join(TARGET_FILE),
        target_exe.display().to_string().as_bytes(),
    )?;

    let rendered = serde_json::to_vec_pretty(&manifest(&entrypoint))
        .expect("the manifest is built from literals and always serialises");
    write_file(&plugin_dir.join("plugin.json"), &rendered)
}

/// Remove k2g's plugin directory for one KiCad version. Removing a directory that is
/// not there succeeds — the user asked for it to be gone, and it is.
pub fn unregister(install: &KicadInstall) -> Result<(), IntegrationError> {
    let plugin_dir = install.plugin_dir();
    if !plugin_dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&plugin_dir)
        .map_err(|e| IntegrationError::Write(plugin_dir.display().to_string(), e))?;
    super::security_log::record_ok(
        super::security_log::Event::KicadPluginUnregistered,
        serde_json::json!({
            "kicad_version": install.version,
            "plugin_dir": super::security_log::redact(&plugin_dir),
        }),
    );
    info!(
        "Removed the k2g plugin for KiCad {} from {}",
        install.version,
        plugin_dir.display()
    );
    Ok(())
}

/// Re-point every stale registration at the executable running now.
///
/// Called once at startup. This is the mechanism that makes an update invisible: the
/// installer replaces k2g, the new build notices the recorded target no longer names
/// it, and rewrites the sidecar. It only ever touches a directory the user explicitly
/// asked to have created, and never creates one.
///
/// Returns the versions it repaired, so the caller can record them.
pub fn refresh_registrations() -> Vec<String> {
    let mut repaired = Vec::new();
    for install in detect_installs() {
        let state = status(&install);
        if !state.registered || !state.stale {
            continue;
        }
        match register(&install) {
            Ok(_) => {
                info!(
                    "Re-pointed the KiCad {} plugin registration at this build",
                    install.version
                );
                repaired.push(install.version.clone());
            }
            // A failure here must not be fatal: the user can still run k2g directly,
            // and the Settings screen will show the registration as stale.
            Err(err) => warn!(
                "Could not refresh the KiCad {} plugin registration: {err}",
                install.version
            ),
        }
    }
    repaired
}

// ---------------------------------------------------------------------------
// KiCad's API server setting
// ---------------------------------------------------------------------------

fn read_common(install: &KicadInstall) -> Result<Value, IntegrationError> {
    let raw = std::fs::read_to_string(&install.common_file)
        .map_err(|e| IntegrationError::Read(install.common_file.display().to_string(), e))?;
    serde_json::from_str(&raw).map_err(|e| {
        IntegrationError::MalformedConfig(install.common_file.display().to_string(), e.to_string())
    })
}

/// Turn KiCad's IPC API server on (or off) by editing `api.enable_server`.
///
/// Refuses while KiCad is running: it writes `kicad_common.json` out in full when it
/// exits, so an edit made underneath it is overwritten and the user is left with a
/// setting that appeared to change and did not.
///
/// The file is round-tripped through `serde_json::Value` and only that one pointer is
/// touched, so every other preference — including keys this build has never heard of
/// — survives. A backup is written first.
pub fn set_api_enabled(install: &KicadInstall, enabled: bool) -> Result<(), IntegrationError> {
    if kicad_is_running() == KicadRunning::Yes {
        return Err(IntegrationError::KicadRunning);
    }

    let mut config = read_common(install)?;

    let backup = install.common_file.with_extension("json.k2g-backup");
    std::fs::copy(&install.common_file, &backup)
        .map_err(|e| IntegrationError::Write(backup.display().to_string(), e))?;

    // `pointer_mut` cannot create the `api` object, and a KiCad that has never had the
    // setting written may not have one.
    if !config.get("api").is_some_and(Value::is_object) {
        config
            .as_object_mut()
            .ok_or_else(|| {
                IntegrationError::MalformedConfig(
                    install.common_file.display().to_string(),
                    "the top level is not a JSON object".to_string(),
                )
            })?
            .insert("api".to_string(), json!({}));
    }
    config["api"]["enable_server"] = Value::Bool(enabled);

    let rendered = serde_json::to_vec_pretty(&config).map_err(|e| {
        IntegrationError::MalformedConfig(install.common_file.display().to_string(), e.to_string())
    })?;
    write_file(&install.common_file, &rendered)?;

    // Changing another product's security-relevant setting is the single most
    // consequential thing k2g does outside its own data directory. The backup path is
    // recorded too, so the change can be undone from the record alone.
    super::security_log::record_ok(
        super::security_log::Event::KicadApiSettingChanged,
        serde_json::json!({
            "kicad_version": install.version,
            "setting": "api.enable_server",
            "value": enabled,
            "file": super::security_log::redact(&install.common_file),
            "backup": super::security_log::redact(&backup),
        }),
    );

    info!(
        "Set KiCad {} api.enable_server = {enabled} (backup at {})",
        install.version,
        backup.display()
    );
    Ok(())
}

/// Whether a KiCad process is currently running.
#[cfg(target_os = "windows")]
pub fn kicad_is_running() -> KicadRunning {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    // Every KiCad frame is its own executable; any of them holds the settings file.
    const KICAD_EXECUTABLES: [&str; 7] = [
        "kicad.exe",
        "pcbnew.exe",
        "eeschema.exe",
        "gerbview.exe",
        "pcb_calculator.exe",
        "pl_editor.exe",
        "bitmap2component.exe",
    ];

    // SAFETY: a ToolHelp snapshot walked with the documented First/Next pair. The
    // entry is zeroed with its `dwSize` set as the API requires, every field read is
    // a plain POD copy out of it, and the handle is closed on every exit path.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return KicadRunning::Unknown;
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        let mut found = false;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
                if KICAD_EXECUTABLES.contains(&name.as_str()) {
                    found = true;
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
        if found {
            KicadRunning::Yes
        } else {
            KicadRunning::No
        }
    }
}

/// Whether a KiCad process is currently running, by walking `/proc`.
#[cfg(target_os = "linux")]
pub fn kicad_is_running() -> KicadRunning {
    const KICAD_EXECUTABLES: [&str; 7] = [
        "kicad",
        "pcbnew",
        "eeschema",
        "gerbview",
        "pcb_calculator",
        "pl_editor",
        "bitmap2component",
    ];

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return KicadRunning::Unknown;
    };

    for entry in entries.flatten() {
        // Only numeric entries are processes.
        if !entry
            .file_name()
            .to_string_lossy()
            .chars()
            .all(|c| c.is_ascii_digit())
        {
            continue;
        }
        // `comm` is the executable name, truncated to 15 bytes — short enough for
        // every name above to survive intact.
        if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
            if KICAD_EXECUTABLES.contains(&comm.trim()) {
                return KicadRunning::Yes;
            }
        }
    }
    KicadRunning::No
}

/// Whether a KiCad process is currently running.
///
/// Not implemented here: macOS has no `/proc`, and shelling out to `ps` from a GUI
/// application to answer a question the user can answer by looking at their dock is
/// a poor trade. The caller warns instead of blocking.
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn kicad_is_running() -> KicadRunning {
    KicadRunning::Unknown
}

// ---------------------------------------------------------------------------
// Small filesystem helpers
// ---------------------------------------------------------------------------

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), IntegrationError> {
    std::fs::write(path, bytes)
        .map_err(|e| IntegrationError::Write(path.display().to_string(), e))
}

/// Give the shim the executable bit. A no-op on Windows, where executability is
/// decided by the extension — but KiCad checks `IsFileExecutable()` before launching,
/// so on Unix a plain byte copy would be rejected.
#[cfg(unix)]
fn ensure_executable(path: &Path) -> Result<(), IntegrationError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| IntegrationError::Read(path.display().to_string(), e))?
        .permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms)
        .map_err(|e| IntegrationError::Write(path.display().to_string(), e))
}

#[cfg(not(unix))]
fn ensure_executable(_path: &Path) -> Result<(), IntegrationError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_kicad_style_version_directories_are_recognised() {
        for good in ["9.0", "10.0", "8.0", "10.10"] {
            assert!(is_version_dir(good), "{good} should be a version directory");
        }
        // `3d_models`, `colors`, `scripting` and friends share the directory.
        for bad in ["", "9", "9.0.1", "colors", "3d", "9.x", ".", "v9.0"] {
            assert!(!is_version_dir(bad), "{bad} should not be a version directory");
        }
    }

    #[test]
    fn installs_sort_newest_first_numerically() {
        // The bug this pins: a lexicographic sort puts "10.0" before "9.0", so the
        // newest KiCad would be offered last (or defaulted away from).
        let mut versions = ["9.0", "10.0", "8.0"];
        versions.sort_by_key(|v| std::cmp::Reverse(version_key(v)));
        assert_eq!(versions, ["10.0", "9.0", "8.0"]);
    }

    /// The manifest must satisfy the constraints KiCad enforces in `api_plugin.cpp`,
    /// none of which produce a visible error — a violation just makes the button
    /// silently not appear.
    #[test]
    fn the_manifest_meets_kicads_parsing_rules() {
        let entrypoint = launcher_file_name();
        let value = manifest(&entrypoint);

        for key in ["identifier", "name", "description", "runtime", "actions"] {
            assert!(value.get(key).is_some(), "plugin key '{key}' is required");
        }
        assert_eq!(value["runtime"]["type"], "exec", "the enum is [python, exec]");

        let action = &value["actions"][0];
        for key in ["identifier", "name", "description", "entrypoint"] {
            assert!(action.get(key).is_some(), "action key '{key}' is required");
        }

        // The one that bites hardest: an absolute entrypoint makes KiCad discard the
        // action outright ("action contains abs path %s; skipping").
        let entry = action["entrypoint"].as_str().unwrap();
        assert!(
            Path::new(entry).is_relative(),
            "the entrypoint must be relative, got {entry}"
        );
        assert!(
            !entry.contains('/') && !entry.contains('\\'),
            "the entrypoint must sit directly in the plugin directory, got {entry}"
        );

        // Icons are resolved the same way, and the schema requires a .png suffix.
        for icon in action["icons-light"].as_array().unwrap() {
            let icon = icon.as_str().unwrap();
            assert!(icon.ends_with(".png"), "icons must be PNG, got {icon}");
            assert!(Path::new(icon).is_relative(), "icon paths must be relative");
        }

        // The identifier pattern KiCad applies is
        // ^[a-zA-Z][-_a-zA-Z0-9.]{0,98}[a-zA-Z0-9]$.
        let id = value["identifier"].as_str().unwrap();
        assert!(id.starts_with(|c: char| c.is_ascii_alphabetic()));
        assert!(id.ends_with(|c: char| c.is_ascii_alphanumeric()));
        assert!(id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)));
    }

    /// The generated manifest must satisfy KiCad's *own* schema, not just this
    /// module's reading of it. Catches what the hand-written checks above cannot:
    /// the identifier and action-identifier regexes, the runtime and scope enums,
    /// the `maxLength` caps, and the `.png` pattern on icon paths.
    ///
    /// The schema is vendored at `schemas/vendor/` — see the README there for its
    /// provenance and for the two rules KiCad enforces in C++ that the schema does
    /// not express.
    #[test]
    fn the_manifest_validates_against_kicads_published_schema() {
        let schema: Value =
            serde_json::from_str(include_str!("../../schemas/vendor/kicad-api.v1.schema.json"))
                .expect("the vendored KiCad schema should be valid JSON");
        let validator =
            jsonschema::validator_for(&schema).expect("the vendored KiCad schema should compile");

        let instance = manifest(&launcher_file_name());
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|error| format!("{}: {error}", error.instance_path))
            .collect();

        assert!(
            errors.is_empty(),
            "the generated plugin.json violates KiCad's schema:\n{}",
            errors.join("\n")
        );
    }

    /// Registration must produce exactly the four files KiCad and the shim expect,
    /// with the manifest's entrypoint naming the launcher that was actually written.
    #[test]
    fn registering_lays_out_every_file_the_shim_and_kicad_need() {
        let dir = tempfile::tempdir().unwrap();
        let launcher_src = dir.path().join("built-launcher.exe");
        std::fs::write(&launcher_src, b"MZ fake launcher").unwrap();
        let target_exe = dir.path().join("install").join("k2g.exe");

        let plugin_dir = dir.path().join("plugins").join("k2g");
        write_plugin_dir(&plugin_dir, &launcher_src, &target_exe).unwrap();

        let entrypoint = launcher_file_name();
        assert!(plugin_dir.join("plugin.json").is_file(), "manifest");
        assert!(plugin_dir.join("icon.png").is_file(), "toolbar icon");
        assert!(plugin_dir.join(&entrypoint).is_file(), "the shim itself");

        // The sidecar is the whole point of the indirection: it, not the manifest,
        // is what an update rewrites.
        let recorded = std::fs::read_to_string(plugin_dir.join(TARGET_FILE)).unwrap();
        assert_eq!(recorded.trim(), target_exe.display().to_string());

        // The launcher is copied byte-for-byte, and the manifest points at the name
        // it was written under — a mismatch here is a button that does nothing.
        assert_eq!(
            std::fs::read(plugin_dir.join(&entrypoint)).unwrap(),
            b"MZ fake launcher"
        );
        let written: Value =
            serde_json::from_slice(&std::fs::read(plugin_dir.join("plugin.json")).unwrap())
                .unwrap();
        assert_eq!(written["actions"][0]["entrypoint"], entrypoint.as_str());
    }

    /// Re-registering over an existing directory must refresh the target, which is
    /// exactly what `refresh_registrations` relies on after an update.
    #[test]
    fn re_registering_repoints_a_stale_target() {
        let dir = tempfile::tempdir().unwrap();
        let launcher_src = dir.path().join("built-launcher.exe");
        std::fs::write(&launcher_src, b"MZ").unwrap();
        let plugin_dir = dir.path().join("k2g");

        let old = dir.path().join("v0.9.0").join("k2g.exe");
        write_plugin_dir(&plugin_dir, &launcher_src, &old).unwrap();

        let new = dir.path().join("v0.9.1").join("k2g.exe");
        write_plugin_dir(&plugin_dir, &launcher_src, &new).unwrap();

        let recorded = std::fs::read_to_string(plugin_dir.join(TARGET_FILE)).unwrap();
        assert_eq!(
            recorded.trim(),
            new.display().to_string(),
            "the sidecar must follow the new build, or the toolbar button keeps \
             launching the superseded one"
        );
    }

    #[test]
    fn the_icon_is_the_small_one() {
        // KiCad renders the PNG at its native size, so shipping the 1.2 MB
        // full-resolution icon would put an enormous button on the toolbar.
        assert!(
            PLUGIN_ICON.len() < 50_000,
            "the toolbar icon should be the small asset, got {} bytes",
            PLUGIN_ICON.len()
        );
        assert_eq!(&PLUGIN_ICON[1..4], b"PNG", "the icon must be a PNG");
    }

    /// Only the API pointer changes; every other key the user has must survive.
    #[test]
    fn enabling_the_api_preserves_unrelated_settings() {
        let mut config = json!({
            "api": { "enable_server": false, "interpreter_path": "C:\\python.exe" },
            "system": { "editor_name": "vim" },
            "a_key_this_build_has_never_heard_of": [1, 2, 3],
        });

        // The transformation `set_api_enabled` applies, without the filesystem.
        config["api"]["enable_server"] = Value::Bool(true);

        assert_eq!(config["api"]["enable_server"], true);
        assert_eq!(config["api"]["interpreter_path"], "C:\\python.exe");
        assert_eq!(config["system"]["editor_name"], "vim");
        assert_eq!(config["a_key_this_build_has_never_heard_of"], json!([1, 2, 3]));
    }

    #[test]
    fn an_api_block_is_created_when_kicad_has_never_written_one() {
        let mut config = json!({ "system": { "editor_name": "vim" } });
        if !config.get("api").is_some_and(Value::is_object) {
            config.as_object_mut().unwrap().insert("api".to_string(), json!({}));
        }
        config["api"]["enable_server"] = Value::Bool(true);

        assert_eq!(config["api"]["enable_server"], true);
        assert_eq!(config["system"]["editor_name"], "vim", "untouched");
    }
}
