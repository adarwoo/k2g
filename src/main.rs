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
    let (kicad_status, board_snapshot) = runtime::acquire_board();

    ui::launch(UiLaunchData {
        kicad_status,
        board_snapshot,
    });
}
