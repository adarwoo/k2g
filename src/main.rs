mod catalog_io;
mod runtime;
mod data;
mod gcode;
mod ui;
mod paths;

use ui::UiLaunchData;
use tracing_subscriber::{fmt, EnvFilter};

fn main() {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
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
