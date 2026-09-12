//! Build script: compiles the application icon into the Windows `.exe`.
//!
//! `Dioxus.toml`'s `[bundle.windows] icon_path` only dresses a *bundled* app, so a
//! plain `cargo build`/`cargo run` produced an executable with the default blank
//! icon in Explorer, the taskbar and Alt-Tab. Embedding a Windows resource here
//! fixes every build, bundled or not.
//!
//! The `.ico` is rendered from the same PNG the bundler uses rather than committed
//! alongside it, so there is one piece of artwork to maintain and the two can never
//! drift apart.
//!
//! A missing or unreadable icon is cosmetic, so every failure below degrades to a
//! `cargo:warning` and the default icon — it never breaks the build.

/// Version parsing, shared verbatim with the application.
///
/// `include!` rather than a `use`: a build script is compiled as its own crate and
/// cannot import from the crate it is building. The alternative — restating the rules
/// here — is what lets a build script's idea of "newer" drift from the update
/// checker's, which is the one place the two must agree.
// The build script uses only `parse_core`; `is_newer` exists for the update checker.
#[allow(dead_code)]
#[path = "src/version.rs"]
mod version;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/icon.png");
    // The whole source tree, not just `version.rs`. Cargo walks a directory, and the
    // build stamp below has to be recomputed whenever the binary is — a stamp that
    // says `clean` because a `.rs` edit did not re-run this script is worse than no
    // stamp at all. `render_ico` skips its work when the icon is already current, so
    // the wider watch costs a `git` call rather than six Lanczos resizes.
    println!("cargo:rerun-if-changed=src");

    warn_on_version_drift();
    emit_build_stamp();

    #[cfg(windows)]
    windows_icon::embed();
}

/// Compiles the git commit, its dirty flag and the CI run into the binary, for
/// `build_info::current()` to report.
///
/// # Why this is not in the version number
///
/// The obvious shape — patch digit as a build counter, `0.13.124` — breaks
/// [`version::is_newer`], which orders releases on `(major, minor, patch)`. A build
/// stamped `0.13.124` computes `is_newer("v0.13.1", "0.13.124") == false`, so the
/// updater would refuse a genuine hotfix. The release version stays semantic and the
/// build identifies itself alongside it.
///
/// # Every value is always emitted, empty when unknown
///
/// `env!` is a compile error on an unset variable, and a build from a source tarball
/// has no git to ask. Emitting an empty string keeps the crate compiling anywhere and
/// leaves "unknown" a value the reporting code can handle — which it does, by omitting
/// the line rather than printing a blank one. Nothing in this file ever fails a build
/// over metadata (see the module docs).
///
/// # What can still go stale
///
/// The commit and its dirty flag are a *provenance* answer and are only as fresh as the
/// last time this script ran. The freshness question — is this the build I just made —
/// is answered at runtime from the executable's own mtime, which cannot go stale. See
/// `build_info` for that half.
fn emit_build_stamp() {
    // A commit lands without `.git/HEAD` moving (it names a branch, and the branch's
    // ref file is what advances), so watch the resolved ref as well as HEAD itself.
    //
    // `.git/index` is deliberately **not** watched. It looks like the right file and it
    // is a trap: `git status` refreshes the index's stat cache, so a build script that
    // both watches the index and runs `git status` invalidates itself — every build
    // triggers one more, each costing a full relink. Nothing is lost by leaving it out.
    // Staging a file does not change whether the tree is dirty; a commit moves the ref
    // below; and any edit to a tracked source file is caught by the `src` watch above.
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Ok(head) = std::fs::read_to_string(".git/HEAD") {
        if let Some(reference) = head.trim().strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed=.git/{reference}");
        }
    }

    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    // Non-empty porcelain output is any uncommitted change, staged or not. An error
    // (no git, no repository) reads as clean rather than dirty: claiming a tarball
    // build carries uncommitted work would be inventing a fact.
    let dirty = match git(&["status", "--porcelain"]) {
        Some(status) if !status.is_empty() => "1",
        _ => "",
    };

    println!("cargo:rustc-env=K2G_COMMIT={commit}");
    println!("cargo:rustc-env=K2G_COMMIT_DIRTY={dirty}");
    println!("cargo:rustc-env=K2G_CI={}", ci_run());
}

/// `"Rust run 124"` for a GitHub Actions build, empty for a local one.
///
/// The attempt is appended only when it is not the first: `GITHUB_RUN_NUMBER` is stable
/// across a re-run of the same workflow run, so `Rust run 124.2` is the only way to tell
/// a re-run's artifacts from the original's — and printing `.1` on every ordinary build
/// would be noise.
fn ci_run() -> String {
    // Not `rerun-if-changed`: an environment variable needs its own declaration, or a
    // cached build script result would carry a stale run number into a later run.
    for name in ["GITHUB_WORKFLOW", "GITHUB_RUN_NUMBER", "GITHUB_RUN_ATTEMPT"] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let (Ok(workflow), Ok(number)) = (
        std::env::var("GITHUB_WORKFLOW"),
        std::env::var("GITHUB_RUN_NUMBER"),
    ) else {
        return String::new();
    };

    let attempt = std::env::var("GITHUB_RUN_ATTEMPT").unwrap_or_default();
    match attempt.as_str() {
        "" | "1" => format!("{workflow} run {number}"),
        other => format!("{workflow} run {number}.{other}"),
    }
}

/// Trimmed stdout of a successful `git`, or `None` for any failure at all — no git on
/// PATH, not a repository, a non-zero exit, non-UTF-8 output.
///
/// Always `--no-optional-locks`, so reading the repository never writes to it. Without
/// it `git status` rewrites `.git/index` to refresh its stat cache, which is a build
/// script mutating the source tree it is inspecting — and it is what makes watching the
/// index self-invalidating. A build must be able to run on a read-only checkout, and on
/// two checkouts at once.
fn git(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}

/// Warns when `Cargo.toml`'s version has fallen behind the newest release tag.
///
/// The version in `Cargo.toml` is the single source of truth — it is what the About
/// screen shows, what a G-code header prints as `k2g_version`, and what Windows reads
/// out of the executable's file properties. Git tags cannot supply any of those:
/// `winresource` seeds the version block from `CARGO_PKG_*`, and a source tarball or a
/// shallow CI clone has no tags to read at all.
///
/// Which leaves one failure mode, and it is the one that actually happened: the tags
/// advanced to `v0.9.0-typed-values` while `Cargo.toml` sat at `0.1.0` through nine
/// releases, so every build reported a version nine releases stale and nothing said so.
/// This is that missing signal.
///
/// Compared against the **nearest tag reachable from HEAD**, not the newest tag in the
/// repository: that is the release this build descends from, which is the thing the
/// version should agree with. Only the numeric part is compared, so the descriptive
/// suffix this project's tags carry (`v0.9.0-typed-values`) is free to say whatever it
/// likes.
///
/// Silent when there is no git, no tags, or an unparsable one — a build from a tarball
/// is not a mistake, and this file never fails a build over metadata (see the module
/// docs).
fn warn_on_version_drift() {
    // Re-run when HEAD moves or a tag is written, or the answer goes stale the moment
    // the next tag lands.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");

    let Ok(output) = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .output()
    else {
        return; // no git on PATH
    };
    if !output.status.success() {
        return; // not a repository, or no tags yet
    }
    let Ok(tag) = String::from_utf8(output.stdout) else { return };
    let tag = tag.trim();

    // `v0.9.0-typed-values` -> `0.9.0`. Anything that does not look like a three-part
    // version is someone else's tagging scheme, and not this check's business.
    let Some(core) = version::parse_core(tag) else {
        return;
    };
    let numeric = format!("{}.{}.{}", core.0, core.1, core.2);

    let declared = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    if numeric != declared {
        println!(
            "cargo:warning=k2g version drift — Cargo.toml says {declared}, but the \
             nearest release tag is {tag}. The About screen, the `k2g_version` in every \
             G-code header and the executable's file properties all report {declared}. \
             Set `version = \"{numeric}\"` in Cargo.toml, or tag this release."
        );
    }
}

#[cfg(windows)]
mod windows_icon {
    use std::io::BufWriter;
    use std::path::PathBuf;

    use image::codecs::ico::{IcoEncoder, IcoFrame};
    use image::imageops::FilterType;
    // `ExtendedColorType`, not `ColorType`: since image 0.25 the encoders take the
    // wider enum — it can name layouts (packed / sub-byte formats) that `ColorType`,
    // which only describes buffers image itself can hold in memory, cannot. `Rgba8`
    // exists in both, so this is a rename at the call site and nothing more.
    use image::ExtendedColorType;

    /// Source artwork — the same file `Dioxus.toml` points the bundler at.
    const ICON_PNG: &str = "assets/icons/icon.png";

    /// Sizes packed into the `.ico`. Windows picks one per context: 16/24/32 for
    /// Explorer lists and the title bar, 48 for medium icons, 128/256 for large tiles
    /// and the Alt-Tab switcher. Shipping a single size leaves Windows to rescale,
    /// which smears at the small end. 256 is the format's maximum.
    const ICON_SIZES: [u32; 6] = [16, 24, 32, 48, 128, 256];

    /// Renders the icon and attaches it (plus the package metadata Windows shows under
    /// file properties) to the executable.
    pub fn embed() {
        // Build scripts run on the **host**, so a Windows host cross-compiling to a
        // non-Windows target must not emit a Windows resource.
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }

        let ico = match render_ico() {
            Ok(path) => path,
            Err(err) => return warn(&err),
        };

        // `WindowsResource` seeds the version block from the CARGO_PKG_* environment,
        // so the name, version, description and authors in Cargo.toml become the
        // executable's file properties for free.
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(&ico.to_string_lossy());
        if let Err(err) = resource.compile() {
            warn(&format!("resource compiler failed: {err}"));
        }
    }

    /// Renders the PNG into a multi-size `.ico` under `OUT_DIR`, returning its path.
    ///
    /// Skipped entirely when the `.ico` under `OUT_DIR` is already newer than the PNG.
    /// This build script now re-runs on any source change — it has to, or the git stamp
    /// it emits would be a build behind — and six Lanczos resizes on every incremental
    /// build is exactly the cost the old narrow watch list was avoiding. The artwork
    /// changes about once a year; the source changes every minute.
    fn render_ico() -> Result<PathBuf, String> {
        let out_dir = std::env::var("OUT_DIR").map_err(|e| format!("no OUT_DIR: {e}"))?;
        let cached = PathBuf::from(&out_dir).join("k2g.ico");
        if is_current(&cached) {
            return Ok(cached);
        }

        let source = image::open(ICON_PNG)
            .map_err(|e| format!("cannot read {ICON_PNG}: {e}"))?
            .into_rgba8();

        let frames = ICON_SIZES
            .iter()
            .map(|&size| {
                // Lanczos3 holds the artwork's edges together at 16px, where a cheaper
                // filter turns fine detail to mush.
                let scaled = image::imageops::resize(&source, size, size, FilterType::Lanczos3);
                IcoFrame::as_png(scaled.as_raw(), size, size, ExtendedColorType::Rgba8)
                    .map_err(|e| format!("cannot encode the {size}px frame: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let file = std::fs::File::create(&cached)
            .map_err(|e| format!("cannot create {}: {e}", cached.display()))?;
        IcoEncoder::new(BufWriter::new(file))
            .encode_images(&frames)
            .map_err(|e| format!("cannot write {}: {e}", cached.display()))?;
        Ok(cached)
    }

    /// Whether `ico` exists and is at least as new as the artwork it is rendered from.
    ///
    /// `false` for every uncertainty — no file, no timestamp on this filesystem, an
    /// unreadable source — so the only way to skip the render is a positive answer.
    /// Re-rendering unnecessarily costs a fraction of a second; skipping when the
    /// artwork has moved ships the old icon until someone runs `cargo clean`.
    fn is_current(ico: &PathBuf) -> bool {
        let modified = |path: &dyn AsRef<std::path::Path>| {
            std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
        };
        match (modified(&ico), modified(&ICON_PNG)) {
            (Some(built), Some(source)) => built >= source,
            _ => false,
        }
    }

    fn warn(message: &str) {
        println!("cargo:warning=k2g icon not embedded — {message}");
    }
}
