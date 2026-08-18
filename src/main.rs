mod catalog_io;
mod runtime;
mod data;
mod gcode;
mod ui;
mod paths;
mod version;

use ui::UiLaunchData;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};

fn main() {
    // Two parallel sinks under one shared filter: the usual stdout formatter, plus
    // an in-memory capture that backs the in-app Logs viewer (see
    // `runtime::log_capture`). The `EnvFilter` on the registry gates both, so the
    // viewer honours `RUST_LOG` exactly like the console does.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .with(runtime::CaptureLayer)
        .init();

    dioxus_logger::initialize_default();

    log::info!("Starting k2g {}", env!("CARGO_PKG_VERSION"));

    // WebKitGTK renders through a DMABUF path that assumes the GPU stack can share
    // buffers between the web process and the compositor. Where it can't, the web
    // process dies mid-frame and the only thing k2g sees is
    // `Error sending edits to webview: Broken pipe` — the window is simply blank,
    // with the actual fault (a rejected GPU command submission) buried in pages of
    // driver noise on stderr, under no heading that mentions the webview.
    //
    // Set unconditionally rather than probing the driver: it was found on nouveau,
    // but the same path breaks on the proprietary NVIDIA driver and inside VMs, and
    // a blank window is a far worse failure than losing hardware compositing on a
    // UI this static. WebKit reads `0` as "keep the renderer", so a stack that
    // works can have it back with `WEBKIT_DISABLE_DMABUF_RENDERER=0`; only an
    // absent variable is overridden, never a value the user chose.
    //
    // Safe here, and only here: `set_var` requires that no other thread is reading
    // the environment, and main() is still single-threaded at this point — the
    // KiCad connection below and the webview itself both come later.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        log::info!(
            "Disabled the WebKit DMABUF renderer; \
             set WEBKIT_DISABLE_DMABUF_RENDERER=0 to keep hardware compositing"
        );
    }

    // One k2g per data directory. Taken before anything else touches that directory,
    // and held — in `_instance` — for the whole run: dropping the file releases the
    // lock, so binding it to a name that lives until `main` returns is what keeps the
    // claim. See `runtime::single_instance` for why two instances are not merely
    // redundant but destructive.
    let _instance = match claim_single_instance() {
        Some(claim) => claim,
        None => return,
    };

    // Repair any KiCad plugin registration that still points at a previous build,
    // before connecting — an update replaces the executable, and the registration
    // has to follow it or the toolbar button starts launching nothing. Only ever
    // touches a registration the user explicitly created; it never makes one.
    runtime::kicad_integration::refresh_registrations();

    // Collect the reachable KiCad's open board (at most one). Stitching happens
    // once when the board is cached in the ctx (see `AppCtx`).
    //
    // When KiCad launched us as a plugin it put `KICAD_API_SOCKET` and
    // `KICAD_API_TOKEN` in our environment, and `kicad-ipc-rs` prefers both over its
    // temp-directory guess — so this connects straight back to the instance that
    // started us, with no discovery involved.
    let acquired = runtime::acquire_board();

    ui::launch(UiLaunchData {
        kicad_status: acquired.status,
        board_snapshot: acquired.board,
        copper: acquired.copper,
    });
}

/// Takes the single-instance lock, or hands this launch over to the k2g already running.
///
/// `None` means stop — quietly, because the operator's request has been answered by the
/// window coming forward. Where it could not be raised (no portable way off Windows, or
/// the window was not found) the refusal is said out loud instead: a launch that appears
/// to do nothing at all is the one outcome worth avoiding, since the natural response to
/// it is to press the button again.
fn claim_single_instance() -> Option<runtime::single_instance::Claim> {
    use runtime::single_instance::{claim, raise_running_window, Claim};

    let Some(data_dir) = paths::k2g_data_dir() else {
        // No data directory resolved: the launch has larger problems than coordination,
        // and `ensure_app_dirs` reports them properly a moment from now.
        return Some(Claim::Unknown("no platform data directory".to_string()));
    };
    // The lock lives in the directory it protects, so the directory has to exist first.
    // Ignored on failure for the same reason the claim itself is: an unusable path is
    // reported by the real check, not by this one.
    let _ = std::fs::create_dir_all(&data_dir);

    match claim(&data_dir) {
        held @ Claim::Held(_) => Some(held),
        Claim::Unknown(reason) => {
            log::warn!("could not take the single-instance lock ({reason}); starting anyway");
            Some(Claim::Unknown(reason))
        }
        Claim::Taken => {
            log::info!("k2g is already running for this user; handing over to it");
            if !raise_running_window() {
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Info)
                    .set_title("k2g is already running")
                    .set_description(
                        "k2g is already open for this user. Switch to its window — a second \
                         copy would share the same settings and profiles, and each would \
                         overwrite the other's changes.",
                    )
                    .show();
            }
            None
        }
    }
}
