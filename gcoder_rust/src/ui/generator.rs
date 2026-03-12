use dioxus::prelude::*;
use std::collections::HashSet;

use super::gcode::generate_gcode;
use super::model::{AppConfig, PCBData, Page};

pub fn render_generator(
    board: Option<PCBData>,
    current_config: AppConfig,
    gcode_text: String,
    mut page: Signal<Page>,
    hidden_tools: Signal<HashSet<String>>,
    config: Signal<AppConfig>,
    pcb_data: Signal<Option<PCBData>>,
    gcode: Signal<String>,
) -> Element {
    if let Some(pcb) = board.as_ref() {
        rsx! {
            h2 { class: "text-2xl font-semibold mb-2", "G-Code Generator" }
            p { class: "mb-4 text-sm", "Visualize and generate tool paths for {pcb.file_name}" }

            div { class: "flex gap-3 mb-4",
                button {
                    onclick: {
                        let config = config;
                        let pcb_data = pcb_data;
                        let mut gcode = gcode;
                        move |_| {
                            if let Some(board) = pcb_data.read().as_ref() {
                                let text = generate_gcode(&config.read(), board);
                                gcode.set(text);
                            } else {
                                gcode.set(String::new());
                            }
                        }
                    },
                    "Regenerate"
                }
                button {
                    onclick: move |_| page.set(Page::Output),
                    "Continue to Output"
                }
            }

            div { class: "grid grid-cols-4 gap-3 mb-4",
                div { class: "border rounded p-3", "Total Operations: {pcb.drill_holes.len() + pcb.routes.len()}" }
                div { class: "border rounded p-3", "Drill Holes: {pcb.drill_holes.len()}" }
                div { class: "border rounded p-3", "Routes: {pcb.routes.len()}" }
                div { class: "border rounded p-3", "G-Code Lines: {gcode_text.lines().count()}" }
            }

            div { class: "grid grid-cols-3 gap-4",
                div { class: "col-span-2 space-y-4",
                    div { class: "border rounded p-3 space-y-1",
                        h3 { class: "font-semibold", "PCB Visualization" }
                        p { "Board size: 100mm x 80mm (mock)" }
                        p { "PTH holes: {pcb.drill_holes.iter().filter(|h| h.hole_type == \"PTH\").count()}" }
                        p { "NPTH holes: {pcb.drill_holes.iter().filter(|h| h.hole_type == \"NPTH\").count()}" }
                    }
                    div { class: "border rounded p-3",
                        h3 { class: "font-semibold", "G-Code Preview" }
                        textarea {
                            class: "w-full h-96 font-mono",
                            value: "{gcode_text}",
                            readonly: true,
                        }
                    }
                }

                div { class: "space-y-4",
                    div { class: "border rounded p-3",
                        h3 { class: "font-semibold mb-2", "Tool Filter" }
                        for tool in current_config.tool_stock.clone() {
                            {
                                let mut hidden_tools = hidden_tools;
                                let tool_id = tool.id.clone();
                                let is_hidden = hidden_tools.read().contains(&tool.id);
                                rsx!(
                                    label { class: "flex items-center gap-2",
                                        input {
                                            r#type: "checkbox",
                                            checked: !is_hidden,
                                            onchange: move |_| {
                                                let mut next = hidden_tools.read().clone();
                                                if next.contains(&tool_id) {
                                                    next.remove(&tool_id);
                                                } else {
                                                    next.insert(tool_id.clone());
                                                }
                                                hidden_tools.set(next);
                                            }
                                        }
                                        "{tool.name}"
                                    }
                                )
                            }
                        }
                    }
                    div { class: "border rounded p-3 space-y-1",
                        h3 { class: "font-semibold", "Job Configuration" }
                        p { "Job Type: {current_config.job_type}" }
                        p { "Machine: {current_config.machine.name}" }
                        p { "Tool Change: {current_config.machine.tool_change}" }
                        button {
                            onclick: move |_| page.set(Page::Setup),
                            "Edit Setup"
                        }
                    }
                }
            }
        }
    } else {
        rsx!(p { "No PCB data loaded. Please check the Setup page." })
    }
}
