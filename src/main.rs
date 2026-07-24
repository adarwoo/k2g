mod catalog_io;
mod runtime;
mod data;
mod gcode;
mod ui;
mod paths;

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

    log::info!("Starting k2g");

    // Collect the reachable KiCad's open board (at most one). Stitching happens
    // once when the board is cached in the ctx (see `AppCtx`).
    let (kicad_status, board_snapshot) = runtime::acquire_board();

    ui::launch(UiLaunchData {
        kicad_status,
        board_snapshot,
    });
}
