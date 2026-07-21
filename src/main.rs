mod cli;
mod catalog_io;
mod runtime;
mod data;
mod gcode;
mod ui;
mod paths;

use cli::CliArgs;
use pcb::KiCad;
use ui::UiLaunchData;
use tracing_subscriber::{fmt, EnvFilter};

fn main() {
    let cli_args: Vec<String> = std::env::args().collect();
    let args = CliArgs::parse_args();
    let vars = collect_env_vars();

    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    dioxus_logger::initialize_default();

    log::info!("Starting k2g with args: {:?}", cli_args);

    let (kicad_status, board_snapshot) = match KiCad::connect() {
        Ok(client) => match client.version() {
            Ok(full_version) => {
                let status = format!("Connected - KiCad {}", full_version);
                // Stitching is done once, in the ctx, when the board is cached
                // (see `AppCtx`); acquisition here only collects the raw snapshot.
                let snapshot = match client.collect_first_snapshot() {
                    Ok(snapshot) => snapshot,
                    Err(err) => {
                        log::warn!("could not collect board snapshot: {err}");
                        None
                    }
                };
                (status, snapshot)
            }
            Err(e) => (format!("Connected but version query failed: {e}"), None),
        },
        Err(e) => (format!("Not connected: {e}"), None),
    };

    ui::launch(UiLaunchData {
        env_vars: vars,
        cli_args,
        kicad_status,
        board_snapshot,
        save_filename_override: args.save_filename_override(),
    });
}

fn collect_env_vars() -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = std::env::vars().collect();
    vars.sort_by(|a, b| a.0.cmp(&b.0));
    vars
}

