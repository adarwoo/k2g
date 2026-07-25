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

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/icon.png");

    #[cfg(windows)]
    windows_icon::embed();
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
