mod defaults;
mod gcode;
mod generator;
mod model;
mod output;
mod setup;

use dioxus::prelude::*;
use std::collections::HashSet;

use defaults::{default_config, mock_pcb_data};
use model::{Page, SetupTab};

const APP_CSS: &str = include_str!("app.css");
const MOCK_DRIVES: [&str; 3] = ["E: (USB Drive)", "F: (SD Card)", "G: (External HDD)"];

#[component]
pub fn App() -> Element {
    let page = use_signal(|| Page::Setup);
    let setup_tab = use_signal(|| SetupTab::Job);

    let config = use_signal(default_config);
    let pcb_data = use_signal(|| Some(mock_pcb_data()));
    let gcode = use_signal(|| {
        let cfg = default_config();
        let pcb = mock_pcb_data();
        gcode::generate_gcode(&cfg, &pcb)
    });

    let hidden_tools = use_signal(HashSet::<String>::new);

    let save_dialog_open = use_signal(|| false);
    let removable_dialog_open = use_signal(|| false);
    let send_dialog_open = use_signal(|| false);
    let file_name = use_signal(|| "output.nc".to_string());
    let selected_drive = use_signal(String::new);
    let ip_address = use_signal(|| "192.168.1.100".to_string());
    let port = use_signal(|| "8080".to_string());
    let uploading = use_signal(|| false);
    let upload_progress = use_signal(|| 0u32);
    let is_saved = use_signal(|| false);

    let mut nav_setup = page;
    let mut nav_generator = page;
    let mut nav_output = page;

    let current_page = *page.read();
    let current_tab = *setup_tab.read();
    let current_config = config.read().clone();
    let board = pcb_data.read().clone();
    let gcode_text = gcode.read().clone();

    rsx! {
        style { "{APP_CSS}" }

        div { class: "flex h-screen",
            div { class: "w-64 border-r p-4 flex flex-col",
                div { class: "mb-6",
                    h1 { class: "font-semibold text-lg", "PCB G-Code Generator" }
                    p { class: "text-sm", "Fabrication Tool" }
                }

                button {
                    class: if current_page == Page::Setup { "text-left p-2 rounded nav-active" } else { "text-left p-2 rounded" },
                    onclick: move |_| nav_setup.set(Page::Setup),
                    "Setup"
                }
                button {
                    class: if current_page == Page::Generator { "text-left p-2 rounded nav-active" } else { "text-left p-2 rounded" },
                    onclick: move |_| nav_generator.set(Page::Generator),
                    "Generator"
                }
                button {
                    class: if current_page == Page::Output { "text-left p-2 rounded nav-active" } else { "text-left p-2 rounded" },
                    onclick: move |_| nav_output.set(Page::Output),
                    "Output"
                }

                div { class: "mt-auto text-xs pt-4",
                    p { "Version 1.0.0" }
                    p { "© 2026 Arex" }
                }
            }

            div { class: "flex-1 overflow-auto p-6",
                if current_page == Page::Setup {
                    {
                        setup::render_setup(
                            current_tab,
                            setup_tab,
                            current_config.clone(),
                            config,
                            pcb_data,
                            gcode,
                        )
                    }
                }

                if current_page == Page::Generator {
                    {
                        generator::render_generator(
                            board.clone(),
                            current_config.clone(),
                            gcode_text.clone(),
                            page,
                            hidden_tools,
                            config,
                            pcb_data,
                            gcode,
                        )
                    }
                }

                if current_page == Page::Output {
                    {
                        output::render_output(
                            board.clone(),
                            gcode_text.clone(),
                            gcode,
                            save_dialog_open,
                            removable_dialog_open,
                            send_dialog_open,
                            file_name,
                            selected_drive,
                            ip_address,
                            port,
                            uploading,
                            upload_progress,
                            is_saved,
                            &MOCK_DRIVES,
                        )
                    }
                }
            }
        }
    }
}
