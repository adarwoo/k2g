use eframe::egui;
use kicad::{DocumentType, KiCad, KiCadConnectionConfig};
use std::time::{Duration, Instant};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();

    eframe::run_native(
        "GCoder (Rust)",
        options,
        Box::new(|_cc| Ok(Box::<GCoderApp>::default())),
    )
}

#[derive(Default)]
struct KiCadUiState {
    connected: bool,
    version: Option<String>,
    open_pcb_documents: usize,
    board_name: Option<String>,
    last_error: Option<String>,
}

struct GCoderApp {
    kicad: Option<KiCad>,
    state: KiCadUiState,
    last_refresh: Option<Instant>,
}

impl Default for GCoderApp {
    fn default() -> Self {
        Self {
            kicad: None,
            state: KiCadUiState::default(),
            last_refresh: None,
        }
    }
}

impl GCoderApp {
    fn ensure_connected(&mut self) {
        if self.kicad.is_some() {
            return;
        }

        match KiCad::new(KiCadConnectionConfig {
            client_name: String::from("gcoder"),
            ..Default::default()
        }) {
            Ok(client) => {
                self.kicad = Some(client);
                self.state.connected = true;
                self.state.last_error = None;
            }
            Err(err) => {
                self.kicad = None;
                self.state.connected = false;
                self.state.last_error = Some(format!("Connection failed: {}", err));
            }
        }
    }

    fn refresh_kicad_state(&mut self) {
        self.ensure_connected();

        let Some(kicad) = self.kicad.as_ref() else {
            self.last_refresh = Some(Instant::now());
            return;
        };

        match kicad.get_version() {
            Ok(version) => {
                self.state.connected = true;
                self.state.version = Some(version.to_string());
                self.state.last_error = None;
            }
            Err(err) => {
                self.kicad = None;
                self.state.connected = false;
                self.state.last_error = Some(format!("Version query failed: {}", err));
                self.last_refresh = Some(Instant::now());
                return;
            }
        }

        match kicad.get_open_documents(DocumentType::DOCTYPE_PCB) {
            Ok(docs) => {
                self.state.open_pcb_documents = docs.len();
            }
            Err(err) => {
                self.state.last_error = Some(format!("Open document query failed: {}", err));
            }
        }

        match kicad.get_open_board() {
            Ok(board) => {
                self.state.board_name = Some(board.name().to_string());
            }
            Err(err) => {
                self.state.board_name = None;
                self.state.last_error = Some(format!("Open board query: {}", err));
            }
        }

        self.last_refresh = Some(Instant::now());
    }
}

impl eframe::App for GCoderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let should_refresh = self
            .last_refresh
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(2));
        if should_refresh {
            self.refresh_kicad_state();
        }

        ctx.request_repaint_after(Duration::from_secs(1));

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("GCoder Rust Plugin");
            ui.label("Live KiCad state");

            if ui.button("Refresh now").clicked() {
                self.refresh_kicad_state();
            }

            ui.separator();
            ui.label(format!("Connected: {}", self.state.connected));
            ui.label(format!(
                "Version: {}",
                self.state
                    .version
                    .as_deref()
                    .unwrap_or("(unavailable)")
            ));
            ui.label(format!("Open PCB docs: {}", self.state.open_pcb_documents));
            ui.label(format!(
                "Board: {}",
                self.state.board_name.as_deref().unwrap_or("(none)")
            ));

            if let Some(err) = &self.state.last_error {
                ui.separator();
                ui.label(format!("Last error: {}", err));
            }
        });
    }
}
