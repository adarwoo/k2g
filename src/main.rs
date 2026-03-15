mod catalog;
mod cli;
mod config;
mod ui;
mod units;
mod user_path;

use cli::CliArgs;
use kicad_ipc_rs::KiCadClientBlocking;
use ui::UiLaunchData;

fn main() {
    // First-run: write built-in catalogs and schema references to the user
    // data directory.  Failure is non-fatal — the application continues with
    // degraded catalog functionality and logs a warning.
    if let Err(e) = catalog::init::first_run_init() {
        eprintln!("warning: could not initialise user data directory: {e}");
    }

    let cli_args: Vec<String> = std::env::args().collect();

    let args = CliArgs::parse_args();

    let vars = collect_env_vars();

    let kicad_status = match KiCadClientBlocking::connect() {
        Ok(client) => match client.get_version() {
            Ok(v) => format!("Connected - KiCad {}", v.full_version),
            Err(e) => format!("Connected but version query failed: {e}"),
        },
        Err(e) => format!("Not connected: {e}"),
    };

    let summary = format_summary(&args, vars.len());
    ui::launch(UiLaunchData {
        env_vars: vars,
        env_summary: summary,
        cli_args,
        kicad_status,
        save_filename_override: args.save_filename_override(),
    });
}

fn collect_env_vars() -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = std::env::vars().collect();
    vars.sort_by(|a, b| a.0.cmp(&b.0));
    vars
}

fn format_summary(args: &CliArgs, count: usize) -> String {
    let input_label = args
        .filename
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(none)".to_string());

    format!(
        "ops={}, input={}, output={}, env={count}",
        args.operations_label(),
        input_label,
        args.output_label(),
    )
}
