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
    println!("cargo:rerun-if-changed=src/version.rs");
    println!("cargo:rerun-if-changed=assets/icons/icon.png");

    warn_on_version_drift();

    #[cfg(windows)]
    windows_icon::embed();
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
    use image::ColorType;

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
    fn render_ico() -> Result<PathBuf, String> {
        let source = image::open(ICON_PNG)
            .map_err(|e| format!("cannot read {ICON_PNG}: {e}"))?
            .into_rgba8();

        let frames = ICON_SIZES
            .iter()
            .map(|&size| {
                // Lanczos3 holds the artwork's edges together at 16px, where a cheaper
                // filter turns fine detail to mush.
                let scaled = image::imageops::resize(&source, size, size, FilterType::Lanczos3);
                IcoFrame::as_png(scaled.as_raw(), size, size, ColorType::Rgba8)
                    .map_err(|e| format!("cannot encode the {size}px frame: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let out_dir = std::env::var("OUT_DIR").map_err(|e| format!("no OUT_DIR: {e}"))?;
        let path = PathBuf::from(out_dir).join("k2g.ico");
        let file = std::fs::File::create(&path)
            .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
        IcoEncoder::new(BufWriter::new(file))
            .encode_images(&frames)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(path)
    }

    fn warn(message: &str) {
        println!("cargo:warning=k2g icon not embedded — {message}");
    }
}
