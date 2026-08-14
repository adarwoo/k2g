//! Checking GitHub for a newer k2g release, and installing one on request.
//!
//! This is the whole of k2g's network activity. Nothing else in the application
//! opens a socket to anything but the local KiCad IPC pipe, and with
//! `update_check_enabled` off, nothing here runs at all.
//!
//! # The shape of the thing, and why
//!
//! EU CRA Annex I (2)(c) asks that security updates be "installed within an
//! appropriate timeframe enabled as a default setting, with a clear and easy-to-use
//! opt-out mechanism", together with notification and the option to postpone. k2g
//! implements the notify-and-confirm reading of that: the check is on by default and
//! automatic, the *install* is one click away and always announced.
//!
//! Silently swapping the binary of a program that drives a CNC spindle, potentially
//! while a job is loaded, is not a safety property — it is a hazard. The user is told
//! a release exists and chooses when to take it.
//!
//! # Trust
//!
//! Every artifact is verified against [`PUBLIC_KEY`] before k2g will execute it. The
//! release workflow signs each installer with minisign and publishes the detached
//! `.minisig` beside it. A download whose signature does not verify is deleted, not
//! quarantined and not offered — an updater that runs unverified binaries is a
//! remote-code-execution channel wearing a helpful face.
//!
//! Trust therefore rests on the key below, compiled into this executable, and not on
//! TLS, GitHub's account security, or the release page's contents.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use log::{info, warn};
use serde::Deserialize;

use crate::version;

/// Release feed. `/releases/latest` deliberately excludes drafts and pre-releases, so
/// tagging a pre-release cannot push an unfinished build at every user.
const RELEASES_URL: &str = "https://api.github.com/repos/adarwoo/k2g/releases/latest";

/// GitHub rejects API requests without a User-Agent. Naming the version makes the
/// traffic legible in the project's own logs, and is honest about who is calling.
const USER_AGENT: &str = concat!("k2g/", env!("CARGO_PKG_VERSION"), " (+https://github.com/adarwoo/k2g)");

/// Once a day. The check is cheap, but it is also unsolicited traffic to a third
/// party, so it happens as rarely as it can while still being useful.
const CHECK_INTERVAL_HOURS: i64 = 24;

/// How long "remind me later" silences the banner for.
pub const POSTPONE_DAYS: i64 = 7;

/// Network timeouts. A slow or hijacked endpoint must never make the application feel
/// broken — the check is strictly optional, so it gives up quickly and tries tomorrow.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const METADATA_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// Refuse absurd downloads outright rather than filling the user's disk while a
/// progress bar spins. Comfortably above a real installer (~60 MB).
const MAX_DOWNLOAD_BYTES: u64 = 400 * 1024 * 1024;

/// The minisign public key every release artifact is verified against.
///
/// Its private half lives only in the release workflow's secrets. Replacing this key
/// is a breaking change for anyone running an older build: they will reject the new
/// signatures and have to update by hand, so it is rotated only if it is compromised.
///
/// The placeholder is inert — [`verify_signature`] fails closed on an unparsable key,
/// so a build that has not had a real key set cannot install anything.
pub const PUBLIC_KEY: &str = include_str!("../../assets/release-signing.pub");

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("Could not reach GitHub: {0}")]
    Network(String),

    #[error("GitHub answered with an unexpected response: {0}")]
    Protocol(String),

    #[error("Release {0} has no installer for this platform")]
    NoArtifact(String),

    #[error("Release {0} has no signature file for its installer — k2g will not install an unverified download")]
    NoSignature(String),

    #[error("The downloaded installer's signature is not valid ({0}). The download has been deleted. Update manually from the releases page if this persists.")]
    BadSignature(String),

    #[error("The download exceeded {0} bytes and was abandoned")]
    TooLarge(u64),

    #[error("Could not write '{0}': {1}")]
    Write(String, std::io::Error),

    #[error("Could not start the installer: {0}")]
    Launch(String),
}

/// A newer release than the one running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    /// Bare `X.Y.Z`, with the tag's `v` prefix and codename suffix stripped.
    pub version: String,
    /// The tag as GitHub reports it, e.g. `v0.9.1-edge-routing`.
    pub tag: String,
    /// Release notes, as markdown. May be empty.
    pub notes: String,
    /// Where a human can read about it.
    pub page_url: String,
    /// The installer for this platform.
    installer: ReleaseAsset,
    /// Its detached minisign signature.
    signature: ReleaseAsset,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseFeed {
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Whether a check is due: enabled, not postponed, and not run in the last day.
///
/// Takes the three persisted values rather than reading global state so the policy is
/// testable in isolation — it is the part most likely to be quietly wrong.
pub fn check_is_due(
    enabled: bool,
    last_check: Option<&str>,
    postponed_until: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if !enabled {
        return false;
    }

    // A live postponement suppresses the check itself, not merely the banner: there
    // is no point asking GitHub about a release we have already agreed not to mention.
    if let Some(until) = postponed_until.and_then(parse_stamp) {
        if now < until {
            return false;
        }
    }

    match last_check.and_then(parse_stamp) {
        // An unparsable or absent stamp checks now and rewrites it.
        None => true,
        // A stamp in the future means the clock moved backwards. Check now rather
        // than wait for real time to catch up, which could be months.
        Some(last) => now < last || now - last >= chrono::Duration::hours(CHECK_INTERVAL_HOURS),
    }
}

fn parse_stamp(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|when| when.with_timezone(&chrono::Utc))
}

fn client(timeout: Duration) -> Result<reqwest::blocking::Client, UpdateError> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        // Refuse to be redirected off HTTPS. Belt and braces beside the https_only
        // setting, since the artifact URLs come from a remote response.
        .https_only(true)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
        .build()
        .map_err(|e| UpdateError::Network(e.to_string()))
}

/// Ask GitHub for the latest release, and report it only if it is genuinely newer
/// than this build and carries a signed installer for this platform.
pub fn fetch_latest(current_version: &str) -> Result<Option<AvailableUpdate>, UpdateError> {
    let response = client(METADATA_TIMEOUT)?
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| UpdateError::Network(e.to_string()))?;

    // GitHub answers `/releases/latest` with 404 when a repository has no published
    // release at all. That is a normal state, not a failure — it is what every build
    // sees before the first release ships — so it reports "nothing newer" rather than
    // writing an error into the log once a day forever.
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        info!("The k2g repository has no published releases yet");
        return Ok(None);
    }
    if !response.status().is_success() {
        // 403 here is almost always the unauthenticated rate limit, which is a
        // perfectly ordinary thing to hit and not worth alarming anyone about.
        return Err(UpdateError::Protocol(format!("HTTP {}", response.status())));
    }

    let feed: ReleaseFeed = response
        .json()
        .map_err(|e| UpdateError::Protocol(e.to_string()))?;

    if feed.draft || feed.prerelease {
        return Ok(None);
    }
    if !version::is_newer(&feed.tag_name, current_version) {
        return Ok(None);
    }

    let Some(installer) = pick_installer(&feed.assets) else {
        return Err(UpdateError::NoArtifact(feed.tag_name));
    };
    // The signature is required, not optional. A release that lost its .minisig is a
    // broken release, and treating it as "install unverified" would defeat the point.
    let signature_name = format!("{}.minisig", installer.name);
    let Some(signature) = feed.assets.iter().find(|a| a.name == signature_name).cloned() else {
        return Err(UpdateError::NoSignature(feed.tag_name));
    };

    let core = version::parse_core(&feed.tag_name).expect("is_newer accepted this tag");
    Ok(Some(AvailableUpdate {
        version: format!("{}.{}.{}", core.0, core.1, core.2),
        tag: feed.tag_name,
        notes: feed.body,
        page_url: feed.html_url,
        installer,
        signature,
    }))
}

/// The installer asset for the platform this build targets.
///
/// Matched on extension rather than on a name template so a change to the release
/// file-naming does not silently stop finding anything. `.minisig` files are excluded
/// explicitly — `k2g-0.9.1-setup.exe.minisig` ends in neither `.exe` nor `.msi`, but
/// being wrong about that would be an unbounded download of a signature file.
fn pick_installer(assets: &[ReleaseAsset]) -> Option<ReleaseAsset> {
    let extensions: &[&str] = if cfg!(target_os = "windows") {
        // MSI first: it is the one that upgrades an existing install in place.
        &[".msi", ".exe"]
    } else if cfg!(target_os = "macos") {
        &[".dmg"]
    } else {
        &[".AppImage", ".deb"]
    };

    extensions.iter().find_map(|ext| {
        assets
            .iter()
            .find(|asset| {
                !asset.name.ends_with(".minisig")
                    && asset.name.to_ascii_lowercase().ends_with(&ext.to_ascii_lowercase())
            })
            .cloned()
    })
}

/// Download the installer and its signature, verify, and hand the verified path back.
///
/// The caller launches it. Splitting download-and-verify from execute keeps the
/// dangerous step a single, obvious line at the call site rather than something
/// buried at the end of a long function.
pub fn download_verified(update: &AvailableUpdate, into: &Path) -> Result<PathBuf, UpdateError> {
    std::fs::create_dir_all(into)
        .map_err(|e| UpdateError::Write(into.display().to_string(), e))?;

    // Reject a name that is not a plain file name before it is joined to a directory:
    // the asset name comes from a remote response, and `..\..\Startup\evil.exe` would
    // otherwise escape the download directory.
    let file_name = safe_file_name(&update.installer.name)
        .ok_or_else(|| UpdateError::Protocol(format!("unsafe asset name {:?}", update.installer.name)))?;

    let installer_path = into.join(file_name);
    let http = client(DOWNLOAD_TIMEOUT)?;

    let signature = download_to_memory(&http, &update.signature.browser_download_url)?;
    download_to_file(&http, &update.installer.browser_download_url, &installer_path)?;

    let bytes = std::fs::read(&installer_path)
        .map_err(|e| UpdateError::Write(installer_path.display().to_string(), e))?;

    if let Err(err) = verify_signature(&bytes, &signature) {
        // Delete first, report second. A rejected installer must not be left on disk
        // where a puzzled user might double-click it.
        let _ = std::fs::remove_file(&installer_path);
        warn!("Rejected the downloaded k2g {} installer: {err}", update.version);
        // The single most important line this application can write: whatever else
        // it means, it means the bytes that arrived are not the bytes that were
        // signed.
        super::security_log::record(
            super::security_log::Event::UpdateInstallerRejected,
            super::security_log::Outcome::Failed,
            serde_json::json!({
                "version": update.version,
                "asset": update.installer.name,
                "source": update.installer.browser_download_url,
                "reason": err.to_string(),
            }),
        );
        return Err(err);
    }

    info!(
        "Verified the k2g {} installer ({} bytes) at {}",
        update.version,
        bytes.len(),
        installer_path.display()
    );
    super::security_log::record_ok(
        super::security_log::Event::UpdateInstallerVerified,
        serde_json::json!({
            "version": update.version,
            "asset": update.installer.name,
            "bytes": bytes.len(),
            "path": super::security_log::redact(&installer_path),
        }),
    );
    Ok(installer_path)
}

/// Whether `name` is a plain file name, safe to join onto a directory.
fn safe_file_name(name: &str) -> Option<&str> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return None;
    }
    // Any separator, drive letter or NUL means this is not a bare file name.
    if trimmed.contains(['/', '\\', '\0']) || trimmed.contains(':') {
        return None;
    }
    Some(trimmed)
}

fn download_to_memory(
    http: &reqwest::blocking::Client,
    url: &str,
) -> Result<Vec<u8>, UpdateError> {
    let response = http
        .get(url)
        .send()
        .map_err(|e| UpdateError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| UpdateError::Protocol(e.to_string()))?;
    // Signature files are a couple of hundred bytes; anything else is not one.
    let bytes = response
        .bytes()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    if bytes.len() as u64 > 64 * 1024 {
        return Err(UpdateError::TooLarge(64 * 1024));
    }
    Ok(bytes.to_vec())
}

fn download_to_file(
    http: &reqwest::blocking::Client,
    url: &str,
    path: &Path,
) -> Result<(), UpdateError> {
    let mut response = http
        .get(url)
        .send()
        .map_err(|e| UpdateError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| UpdateError::Protocol(e.to_string()))?;

    if let Some(len) = response.content_length() {
        if len > MAX_DOWNLOAD_BYTES {
            return Err(UpdateError::TooLarge(MAX_DOWNLOAD_BYTES));
        }
    }

    let mut file = std::fs::File::create(path)
        .map_err(|e| UpdateError::Write(path.display().to_string(), e))?;
    // `copy_to` rather than buffering the whole installer in memory, and the
    // Content-Length check above is advisory only — a server may lie about it, so the
    // written size is checked afterwards as well.
    let written = response
        .copy_to(&mut file)
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    file.flush()
        .map_err(|e| UpdateError::Write(path.display().to_string(), e))?;

    if written > MAX_DOWNLOAD_BYTES {
        let _ = std::fs::remove_file(path);
        return Err(UpdateError::TooLarge(MAX_DOWNLOAD_BYTES));
    }
    Ok(())
}

/// Check `bytes` against the detached minisign signature in `signature`.
///
/// Fails closed on every error path, including an unparsable [`PUBLIC_KEY`] — a build
/// shipped with the placeholder key installs nothing.
fn verify_signature(bytes: &[u8], signature: &[u8]) -> Result<(), UpdateError> {
    verify_against(PUBLIC_KEY, bytes, signature)
}

/// [`verify_signature`] with the key as a parameter, so the tests can exercise the
/// accept path against a keypair they generate. Production has exactly one caller and
/// it passes [`PUBLIC_KEY`].
fn verify_against(public_key: &str, bytes: &[u8], signature: &[u8]) -> Result<(), UpdateError> {
    use minisign_verify::{PublicKey, Signature};

    let key = PublicKey::decode(public_key.trim())
        .map_err(|e| UpdateError::BadSignature(format!("this build has no usable signing key: {e}")))?;
    let text = std::str::from_utf8(signature)
        .map_err(|e| UpdateError::BadSignature(format!("the signature file is not text: {e}")))?;
    let signature = Signature::decode(text)
        .map_err(|e| UpdateError::BadSignature(format!("malformed signature: {e}")))?;

    key.verify(bytes, &signature, false)
        .map_err(|e| UpdateError::BadSignature(e.to_string()))
}

/// Run a verified installer and leave. The caller is expected to close k2g
/// immediately afterwards — an installer cannot replace a running executable.
pub fn launch_installer(path: &Path) -> Result<(), UpdateError> {
    std::process::Command::new(path)
        .spawn()
        .map_err(|e| UpdateError::Launch(format!("{}: {e}", path.display())))?;
    info!("Launched the k2g installer at {}", path.display());
    Ok(())
}

/// Where downloads land: a k2g-owned subdirectory of the system temp directory.
pub fn download_dir() -> PathBuf {
    std::env::temp_dir().join("k2g-update")
}

// ---------------------------------------------------------------------------
// Background service
// ---------------------------------------------------------------------------

/// Run the daily check on a background thread, if it is due.
///
/// Returns immediately. Called once at startup, after the global context exists.
/// Every exit path is silent from the user's point of view except the one that finds
/// a release: a failed check is a log line, never a dialog, because there is nothing
/// the user can usefully do about GitHub being unreachable.
pub fn start_update_check() {
    let (enabled, last_check, postponed, skipped) = super::with_ctx(|ctx| {
        (
            ctx.update_check_enabled,
            ctx.update_last_check.clone(),
            ctx.update_postponed_until.clone(),
            ctx.update_skipped_version.clone(),
        )
    });

    if !check_is_due(
        enabled,
        last_check.as_deref(),
        postponed.as_deref(),
        chrono::Utc::now(),
    ) {
        if !enabled {
            info!("Update check is switched off; k2g will make no network requests");
        }
        return;
    }

    std::thread::Builder::new()
        .name("k2g-update-check".to_string())
        .spawn(move || {
            let current = env!("CARGO_PKG_VERSION");
            let found = fetch_latest(current);

            // Stamp the attempt whatever happened. Stamping only on success would make
            // an offline machine re-check on every single launch.
            super::with_ctx_mut(|ctx| ctx.record_update_check_now());

            super::security_log::record(
                super::security_log::Event::UpdateChecked,
                match &found {
                    Ok(_) => super::security_log::Outcome::Ok,
                    Err(_) => super::security_log::Outcome::Failed,
                },
                match &found {
                    Ok(Some(update)) => serde_json::json!({ "found": update.version }),
                    Ok(None) => serde_json::json!({ "found": serde_json::Value::Null }),
                    Err(err) => serde_json::json!({ "error": err.to_string() }),
                },
            );

            match found {
                Ok(Some(update)) if Some(&update.version) == skipped.as_ref() => {
                    info!("Release {} is available but was skipped by the user", update.version);
                }
                Ok(Some(update)) => {
                    info!("k2g {} is available (running {current})", update.version);
                    super::security_log::record_ok(
                        super::security_log::Event::UpdateAvailable,
                        serde_json::json!({ "version": update.version, "tag": update.tag }),
                    );
                    super::with_ctx_mut(|ctx| {
                        ctx.app.available_update = Some(update);
                    });
                    super::wake_ui();
                }
                Ok(None) => info!("k2g {current} is up to date"),
                Err(err) => warn!("Update check failed: {err}"),
            }
        })
        .expect("failed to spawn the update-check thread");
}

/// Download, verify and start the installer on a background thread.
///
/// `on_done` receives `Ok(())` once the installer has been launched — the caller is
/// expected to close the application at that point, since an installer cannot replace
/// a running executable. On failure it receives a message already fit to show.
pub fn start_install(update: AvailableUpdate, on_done: impl FnOnce(Result<(), String>) + Send + 'static) {
    std::thread::Builder::new()
        .name("k2g-update-install".to_string())
        .spawn(move || {
            let outcome = download_verified(&update, &download_dir())
                .and_then(|path| launch_installer(&path))
                .map_err(|err| err.to_string());
            on_done(outcome);
        })
        .expect("failed to spawn the update-install thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(text: &str) -> chrono::DateTime<chrono::Utc> {
        parse_stamp(text).unwrap()
    }

    #[test]
    fn an_opted_out_user_is_never_checked_for() {
        // The single most important assertion in this module.
        assert!(!check_is_due(false, None, None, utc("2026-08-11T12:00:00Z")));
        assert!(!check_is_due(
            false,
            Some("2020-01-01T00:00:00Z"),
            None,
            utc("2026-08-11T12:00:00Z")
        ));
    }

    #[test]
    fn the_check_runs_at_most_once_a_day() {
        let now = utc("2026-08-11T12:00:00Z");
        assert!(
            check_is_due(true, None, None, now),
            "a machine that has never checked should check"
        );
        assert!(
            !check_is_due(true, Some("2026-08-11T02:00:00Z"), None, now),
            "ten hours ago is too recent"
        );
        assert!(
            check_is_due(true, Some("2026-08-10T11:00:00Z"), None, now),
            "twenty-five hours ago is due"
        );
    }

    #[test]
    fn a_postponement_suppresses_the_check_until_it_lapses() {
        let now = utc("2026-08-11T12:00:00Z");
        assert!(!check_is_due(true, None, Some("2026-08-18T12:00:00Z"), now));
        assert!(
            check_is_due(true, None, Some("2026-08-10T12:00:00Z"), now),
            "a lapsed postponement stops suppressing"
        );
    }

    #[test]
    fn a_clock_that_moved_backwards_does_not_wedge_the_check_shut() {
        // A stamp in the future would otherwise block every check until real time
        // caught up with it — potentially months after a bad clock or a timezone slip.
        let now = utc("2026-08-11T12:00:00Z");
        assert!(check_is_due(true, Some("2027-01-01T00:00:00Z"), None, now));
    }

    #[test]
    fn an_unreadable_timestamp_checks_rather_than_gives_up() {
        let now = utc("2026-08-11T12:00:00Z");
        assert!(check_is_due(true, Some("not a date"), None, now));
        assert!(check_is_due(true, Some(""), None, now));
    }

    /// A real release carries every platform's installer, so the fixture does too.
    ///
    /// It used to carry only the Windows ones, which made the test unrunnable anywhere
    /// else: `pick_installer` correctly found no `.AppImage` and the `expect` below blew
    /// up on the Linux CI job and on any developer's Linux machine. Listing one asset per
    /// platform fixes that and buys the thing that was actually missing — the Linux and
    /// macOS choices were never asserted at all, on a program that ships on Linux.
    #[test]
    fn the_installer_is_picked_by_extension_and_never_the_signature() {
        let asset = |name: &str| ReleaseAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.invalid/{name}"),
        };
        let assets = vec![
            asset("k2g-0.9.1.cdx.json"),
            asset("k2g-0.9.1-setup.exe"),
            asset("k2g-0.9.1-setup.exe.minisig"),
            asset("k2g-0.9.1.msi"),
            asset("k2g-0.9.1.msi.minisig"),
            asset("k2g-0.9.1-portable.zip"),
            asset("k2g-0.9.1.AppImage"),
            asset("k2g-0.9.1.AppImage.minisig"),
            asset("k2g-0.9.1.deb"),
            asset("k2g-0.9.1.deb.minisig"),
            asset("k2g-0.9.1.dmg"),
            asset("k2g-0.9.1.dmg.minisig"),
        ];

        let picked = pick_installer(&assets).expect("an installer should be found");
        assert!(
            !picked.name.ends_with(".minisig"),
            "a signature file must never be chosen as the installer"
        );

        // `cfg!` rather than `#[cfg]`, so every branch is compiled on every platform and
        // a rename that breaks one of them cannot hide behind not being built.
        let expected = if cfg!(target_os = "windows") {
            "k2g-0.9.1.msi" // the MSI upgrades an existing install in place
        } else if cfg!(target_os = "macos") {
            "k2g-0.9.1.dmg"
        } else {
            "k2g-0.9.1.AppImage" // preferred over the .deb: it runs on any distribution
        };
        assert_eq!(picked.name, expected);
    }

    #[test]
    fn a_release_with_no_installer_for_this_platform_yields_nothing() {
        let assets = vec![ReleaseAsset {
            name: "k2g-0.9.1.cdx.json".to_string(),
            browser_download_url: "https://example.invalid/sbom".to_string(),
        }];
        assert!(pick_installer(&assets).is_none());
    }

    /// The asset name arrives in a remote response and is joined onto a local
    /// directory, so it must be a bare file name and nothing else.
    #[test]
    fn a_traversing_asset_name_is_refused() {
        for hostile in [
            "../k2g.exe",
            "..\\..\\Startup\\evil.exe",
            "/etc/cron.d/evil",
            "C:\\Windows\\System32\\evil.exe",
            "sub/dir/k2g.exe",
            "",
            "   ",
            "..",
        ] {
            assert!(
                safe_file_name(hostile).is_none(),
                "{hostile:?} must be refused"
            );
        }
        assert_eq!(safe_file_name("k2g-0.9.1.msi"), Some("k2g-0.9.1.msi"));
    }

    /// Verification must fail closed on every malformed input, and — most
    /// importantly — on tampered content that carries an otherwise well-formed
    /// signature.
    #[test]
    fn verification_rejects_everything_it_cannot_prove() {
        assert!(verify_signature(b"payload", b"").is_err(), "empty signature");
        assert!(
            verify_signature(b"payload", b"not a minisign signature").is_err(),
            "garbage signature"
        );
        assert!(
            verify_signature(b"payload", &[0xff, 0xfe, 0x00]).is_err(),
            "non-UTF-8 signature"
        );
    }

    /// A build carrying the placeholder key must be unable to install anything at
    /// all. This is what makes "forgot to set the signing key" a safe failure rather
    /// than an updater that accepts whatever it is given.
    #[test]
    fn the_placeholder_signing_key_installs_nothing() {
        if minisign_verify::PublicKey::decode(PUBLIC_KEY.trim()).is_err() {
            assert!(
                verify_signature(b"anything", b"anything").is_err(),
                "an unusable key must reject every artifact"
            );
        }
    }

    /// A generated keypair, the payload it signed, and its detached signature —
    /// exactly the shape the release workflow produces.
    fn signed_fixture(payload: &[u8]) -> (String, Vec<u8>) {
        use std::io::Cursor;

        let keypair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let public_key = keypair.pk.to_box().unwrap().to_string();
        let signature = minisign::sign(None, &keypair.sk, Cursor::new(payload), None, None)
            .unwrap()
            .into_string()
            .into_bytes();
        (public_key, signature)
    }

    /// The accept path. Without this, every other verification test could pass while
    /// `verify_against` rejected everything unconditionally — an updater that can
    /// never install is a bug too, just a quieter one.
    #[test]
    fn a_correctly_signed_installer_is_accepted() {
        let payload = b"pretend this is k2g-0.9.1.msi";
        let (public_key, signature) = signed_fixture(payload);

        assert!(
            verify_against(&public_key, payload, &signature).is_ok(),
            "a genuine signature over unmodified bytes must verify"
        );
    }

    /// The assertion the whole update channel rests on: a single flipped byte in the
    /// installer must be caught. This is what stops a compromised mirror, a corrupted
    /// download or a man-in-the-middle from getting code executed on the machine.
    #[test]
    fn one_tampered_byte_is_enough_to_reject_an_installer() {
        let payload = b"pretend this is k2g-0.9.1.msi";
        let (public_key, signature) = signed_fixture(payload);

        let mut tampered = payload.to_vec();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;

        assert!(
            verify_against(&public_key, &tampered, &signature).is_err(),
            "a modified installer must be rejected"
        );

        // Appending is the other obvious attack — a self-extracting installer with a
        // payload stapled to the end.
        let mut appended = payload.to_vec();
        appended.extend_from_slice(b"and now some malware");
        assert!(
            verify_against(&public_key, &appended, &signature).is_err(),
            "an installer with content appended must be rejected"
        );
    }

    /// A valid signature made by the wrong key must not verify. This is what makes
    /// the compiled-in key the actual root of trust, rather than TLS or GitHub.
    #[test]
    fn a_signature_from_another_key_is_rejected() {
        let payload = b"pretend this is k2g-0.9.1.msi";
        let (_ours, _) = signed_fixture(payload);
        let (theirs, their_signature) = signed_fixture(payload);
        let (ours_again, _) = signed_fixture(payload);

        assert!(
            verify_against(&ours_again, payload, &their_signature).is_err(),
            "a well-formed signature from an unrelated key must be rejected"
        );
        // ...and sanity-check that the same signature does verify under its own key,
        // so the assertion above is failing for the right reason.
        assert!(verify_against(&theirs, payload, &their_signature).is_ok());
    }
}
