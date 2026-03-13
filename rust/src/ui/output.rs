use dioxus::prelude::*;
use std::fs;

use super::model::PCBData;

pub fn render_output(
    board: Option<PCBData>,
    gcode_text: String,
    gcode: Signal<String>,
    mut save_dialog_open: Signal<bool>,
    mut removable_dialog_open: Signal<bool>,
    mut send_dialog_open: Signal<bool>,
    mut file_name: Signal<String>,
    mut selected_drive: Signal<String>,
    mut ip_address: Signal<String>,
    mut port: Signal<String>,
    mut uploading: Signal<bool>,
    mut upload_progress: Signal<u32>,
    mut is_saved: Signal<bool>,
    mock_drives: &'static [&'static str],
) -> Element {
    if board.is_none() {
        return rsx!(p { "No PCB data loaded. Please check the Setup page." });
    }

    if gcode_text.is_empty() {
        return rsx!(p { "No G-Code generated yet. Visit Generator page first." });
    }

    rsx! {
        h2 { class: "text-2xl font-semibold mb-2", "Output & Export" }
        p { class: "mb-4 text-sm", "Review, edit, and export your G-Code" }

        div { class: "grid grid-cols-3 gap-4",
            div { class: "col-span-2 border rounded p-3",
                h3 { class: "font-semibold mb-2", "G-Code Editor" }
                textarea {
                    class: "w-full h-[600px] font-mono",
                    value: "{gcode_text}",
                    oninput: {
                        let mut gcode = gcode;
                        move |e: Event<FormData>| gcode.set(e.value())
                    }
                }
            }

            div { class: "space-y-4",
                div { class: "border rounded p-3 space-y-2",
                    h3 { class: "font-semibold", "Export Options" }
                    button {
                        class: "w-full",
                        onclick: move |_| save_dialog_open.set(true),
                        "Save to File"
                    }
                    button {
                        class: "w-full",
                        onclick: move |_| removable_dialog_open.set(true),
                        "Save to Removable Media"
                    }
                    button {
                        class: "w-full",
                        onclick: move |_| send_dialog_open.set(true),
                        "Send Over the Air"
                    }
                    if *is_saved.read() && !selected_drive.read().is_empty() {
                        button {
                            class: "w-full",
                            onclick: move |_| {
                                println!("{} can now be safely removed", selected_drive.read());
                            },
                            "Eject"
                        }
                    }
                }

                div { class: "border rounded p-3 space-y-1",
                    h3 { class: "font-semibold", "File Information" }
                    if let Some(pcb) = board.as_ref() {
                        p { "Source File: {pcb.file_name}" }
                    }
                    p { "Lines of Code: {gcode_text.lines().count()}" }
                    p { "Status: " if *is_saved.read() { "✓ Saved" } else { "Not saved" } }
                }
            }
        }

        if *save_dialog_open.read() {
            div { class: "border rounded p-3 mt-4",
                h3 { class: "font-semibold", "Save G-Code to File" }
                input {
                    value: "{file_name}",
                    oninput: move |e: Event<FormData>| file_name.set(e.value())
                }
                div { class: "mt-2 flex gap-2",
                    button {
                        onclick: move |_| save_dialog_open.set(false),
                        "Cancel"
                    }
                    button {
                        onclick: {
                            let gcode = gcode;
                            let file_name = file_name;
                            let mut is_saved = is_saved;
                            let mut save_dialog_open = save_dialog_open;
                            move |_| {
                                if fs::write(&*file_name.read(), gcode.read().clone()).is_ok() {
                                    is_saved.set(true);
                                    save_dialog_open.set(false);
                                }
                            }
                        },
                        "Save"
                    }
                }
            }
        }

        if *removable_dialog_open.read() {
            div { class: "border rounded p-3 mt-4",
                h3 { class: "font-semibold", "Save to Removable Media" }
                input {
                    value: "{file_name}",
                    oninput: move |e: Event<FormData>| file_name.set(e.value())
                }
                select {
                    onchange: move |e: Event<FormData>| selected_drive.set(e.value()),
                    option { value: "", "Choose a drive..." }
                    for drive in mock_drives {
                        option { value: "{drive}", "{drive}" }
                    }
                }
                if *uploading.read() {
                    progress { value: "{upload_progress.read()}" }
                    p { "{upload_progress.read()}% complete" }
                }
                div { class: "mt-2 flex gap-2",
                    button {
                        onclick: move |_| removable_dialog_open.set(false),
                        "Cancel"
                    }
                    button {
                        onclick: move |_| {
                            uploading.set(true);
                            upload_progress.set(100);
                            uploading.set(false);
                            is_saved.set(true);
                            removable_dialog_open.set(false);
                        },
                        "Save to Drive"
                    }
                }
            }
        }

        if *send_dialog_open.read() {
            div { class: "border rounded p-3 mt-4",
                h3 { class: "font-semibold", "Send Over the Air" }
                input {
                    value: "{ip_address}",
                    oninput: move |e: Event<FormData>| ip_address.set(e.value()),
                }
                input {
                    value: "{port}",
                    oninput: move |e: Event<FormData>| port.set(e.value()),
                }
                if *uploading.read() {
                    progress { value: "{upload_progress.read()}" }
                    p { "{upload_progress.read()}% complete" }
                }
                div { class: "mt-2 flex gap-2",
                    button {
                        onclick: move |_| send_dialog_open.set(false),
                        "Cancel"
                    }
                    button {
                        onclick: move |_| {
                            uploading.set(true);
                            upload_progress.set(100);
                            uploading.set(false);
                            send_dialog_open.set(false);
                        },
                        "Send"
                    }
                }
            }
        }
    }
}
