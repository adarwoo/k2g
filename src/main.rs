mod cli;
mod catalog_io;
mod runtime;
mod data;
mod gcode;
mod ui;
mod paths;

use cli::CliArgs;
use pcb::{stitch_edge_shapes, KiCad};
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
                let snapshot = match client.collect_first_snapshot() {
                    Ok(Some(s)) => {
                        let stitch_result = stitch_edge_shapes(&s.edge_shapes);
                        if stitch_result.errors.is_empty() {
                            println!(
                                "[stitch] startup: {} edge shape(s), {} contour(s) — OK",
                                s.edge_shapes.len(),
                                stitch_result.contours.len(),
                            );
                        } else {
                            eprintln!(
                                "[stitch] startup: {} error(s) — board cannot be processed:",
                                stitch_result.errors.len(),
                            );
                            for e in &stitch_result.errors {
                                eprintln!("[stitch]   {e}");
                            }
                        }
                        Some(s)
                    }
                    Ok(None) => None,
                    Err(err) => {
                        eprintln!("warning: could not collect board snapshot: {err}");
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

