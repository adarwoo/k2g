use dioxus::prelude::*;

use super::gcode::generate_gcode;
use super::model::{AppConfig, PCBData, SetupTab, Tool};

pub fn render_setup(
    current_tab: SetupTab,
    setup_tab: Signal<SetupTab>,
    current_config: AppConfig,
    config: Signal<AppConfig>,
    pcb_data: Signal<Option<PCBData>>,
    gcode: Signal<String>,
) -> Element {
    rsx! {
        h2 { class: "text-2xl font-semibold mb-2", "Setup" }
        p { class: "mb-4 text-sm", "Configure job parameters and machine settings" }

        div { class: "flex gap-2 mb-4",
            {
                let mut tab_btn = setup_tab;
                rsx!(button { class: if current_tab == SetupTab::Job { "nav-active" } else { "" }, onclick: move |_| tab_btn.set(SetupTab::Job), "Job" })
            }
            {
                let mut tab_btn = setup_tab;
                rsx!(button { class: if current_tab == SetupTab::Machine { "nav-active" } else { "" }, onclick: move |_| tab_btn.set(SetupTab::Machine), "Machine" })
            }
            {
                let mut tab_btn = setup_tab;
                rsx!(button { class: if current_tab == SetupTab::Tools { "nav-active" } else { "" }, onclick: move |_| tab_btn.set(SetupTab::Tools), "Tool Stock" })
            }
            {
                let mut tab_btn = setup_tab;
                rsx!(button { class: if current_tab == SetupTab::Rack { "nav-active" } else { "" }, onclick: move |_| tab_btn.set(SetupTab::Rack), "Rack Config" })
            }
            {
                let mut tab_btn = setup_tab;
                rsx!(button { class: if current_tab == SetupTab::Output { "nav-active" } else { "" }, onclick: move |_| tab_btn.set(SetupTab::Output), "Output" })
            }
        }

        if current_tab == SetupTab::Job {
            div { class: "space-y-4",
                div { class: "border rounded p-3 space-y-2",
                    h3 { "Job Type" }
                    select {
                        value: "{current_config.job_type}",
                        onchange: {
                            let mut config = config;
                            let pcb_data = pcb_data;
                            let mut gcode = gcode;
                            move |e: Event<FormData>| {
                                let mut next = config.read().clone();
                                next.job_type = e.value();
                                config.set(next);
                                if let Some(board) = pcb_data.read().as_ref() {
                                    let text = generate_gcode(&config.read(), board);
                                    gcode.set(text);
                                } else {
                                    gcode.set(String::new());
                                }
                            }
                        },
                        option { value: "drill-locating", "Drill Locating Pins" }
                        option { value: "drill-pth", "Drill PTH Holes" }
                        option { value: "drill-npth-route", "Drill NPTH and Route" }
                        option { value: "route-board", "Route Board" }
                    }
                }

                div { class: "border rounded p-3 space-y-2",
                    h3 { "Work Origin" }
                    div { class: "grid grid-cols-3 gap-3",
                        input {
                            value: "{current_config.work_origin.coordinate_system}",
                            oninput: {
                                let mut config = config;
                                move |e: Event<FormData>| {
                                    let mut next = config.read().clone();
                                    next.work_origin.coordinate_system = e.value();
                                    config.set(next);
                                }
                            }
                        }
                        input {
                            value: "{current_config.work_origin.x}",
                            oninput: {
                                let mut config = config;
                                move |e: Event<FormData>| {
                                    if let Ok(v) = e.value().parse::<f64>() {
                                        let mut next = config.read().clone();
                                        next.work_origin.x = v;
                                        config.set(next);
                                    }
                                }
                            }
                        }
                        input {
                            value: "{current_config.work_origin.y}",
                            oninput: {
                                let mut config = config;
                                move |e: Event<FormData>| {
                                    if let Ok(v) = e.value().parse::<f64>() {
                                        let mut next = config.read().clone();
                                        next.work_origin.y = v;
                                        config.set(next);
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "border rounded p-3 space-y-2",
                    h3 { "Rack Management (ATC)" }
                    label {
                        input {
                            r#type: "checkbox",
                            checked: current_config.rack_options.optimize_for_fewer_changes,
                            onchange: {
                                let mut config = config;
                                move |_| {
                                    let mut next = config.read().clone();
                                    next.rack_options.optimize_for_fewer_changes =
                                        !next.rack_options.optimize_for_fewer_changes;
                                    config.set(next);
                                }
                            }
                        }
                        " Optimize the rack for fewer changes"
                    }
                    label {
                        input {
                            r#type: "checkbox",
                            checked: current_config.rack_options.allow_mid_job_reload,
                            onchange: {
                                let mut config = config;
                                move |_| {
                                    let mut next = config.read().clone();
                                    next.rack_options.allow_mid_job_reload =
                                        !next.rack_options.allow_mid_job_reload;
                                    config.set(next);
                                }
                            }
                        }
                        " Allow reloading the rack mid-job"
                    }
                }
            }
        }

        if current_tab == SetupTab::Machine {
            div { class: "grid grid-cols-2 gap-3",
                input {
                    value: "{current_config.machine.name}",
                    oninput: {
                        let mut config = config;
                        move |e: Event<FormData>| {
                            let mut next = config.read().clone();
                            next.machine.name = e.value();
                            config.set(next);
                        }
                    }
                }
                select {
                    value: "{current_config.machine.tool_change}",
                    onchange: {
                        let mut config = config;
                        move |e: Event<FormData>| {
                            let mut next = config.read().clone();
                            next.machine.tool_change = e.value();
                            config.set(next);
                        }
                    },
                    option { value: "automatic", "Automatic Tool Change" }
                    option { value: "manual", "Manual Tool Change" }
                }
                input {
                    value: "{current_config.machine.max_size_x}",
                    oninput: {
                        let mut config = config;
                        move |e: Event<FormData>| {
                            if let Ok(v) = e.value().parse::<f64>() {
                                let mut next = config.read().clone();
                                next.machine.max_size_x = v;
                                config.set(next);
                            }
                        }
                    }
                }
                input {
                    value: "{current_config.machine.max_size_y}",
                    oninput: {
                        let mut config = config;
                        move |e: Event<FormData>| {
                            if let Ok(v) = e.value().parse::<f64>() {
                                let mut next = config.read().clone();
                                next.machine.max_size_y = v;
                                config.set(next);
                            }
                        }
                    }
                }
                input {
                    value: "{current_config.machine.max_speed_xy}",
                    oninput: {
                        let mut config = config;
                        move |e: Event<FormData>| {
                            if let Ok(v) = e.value().parse::<f64>() {
                                let mut next = config.read().clone();
                                next.machine.max_speed_xy = v;
                                config.set(next);
                            }
                        }
                    }
                }
                input {
                    value: "{current_config.machine.max_speed_z}",
                    oninput: {
                        let mut config = config;
                        move |e: Event<FormData>| {
                            if let Ok(v) = e.value().parse::<f64>() {
                                let mut next = config.read().clone();
                                next.machine.max_speed_z = v;
                                config.set(next);
                            }
                        }
                    }
                }
            }
        }

        if current_tab == SetupTab::Tools {
            div { class: "space-y-3",
                for tool in current_config.tool_stock.clone() {
                    div { class: "border rounded p-3",
                        p { "{tool.name} ({tool.tool_type}) Ø{tool.diameter:.3}" }
                    }
                }

                button {
                    onclick: {
                        let mut config = config;
                        move |_| {
                            let mut next = config.read().clone();
                            let id = format!("t{}", next.tool_stock.len() + 1);
                            next.tool_stock.push(Tool {
                                id,
                                tool_type: "drill".to_string(),
                                diameter: 1.0,
                                name: format!("New Tool {}", next.tool_stock.len() + 1),
                                flutes: None,
                                status: "in-stock-preferred".to_string(),
                            });
                            config.set(next);
                        }
                    },
                    "Add Tool"
                }
            }
        }

        if current_tab == SetupTab::Rack {
            div { class: "grid grid-cols-2 gap-3",
                for slot in current_config.rack_config.clone() {
                    div { class: "border rounded p-3",
                        p { "Slot {slot.slot_number}" }
                        p { "Disabled: {slot.disabled}" }
                        p {
                            "Tool: "
                            if let Some(tool) = slot.tool_id {
                                "{tool}"
                            } else {
                                "None"
                            }
                        }
                    }
                }
            }
        }

        if current_tab == SetupTab::Output {
            div { class: "space-y-4",
                for operation in current_config.output_config.operations.clone() {
                    div { class: "border rounded p-3",
                        p { class: "font-semibold", "{operation.id}. {operation.name}" }
                        textarea {
                            class: "w-full h-24 font-mono",
                            value: "{operation.template}",
                            oninput: {
                                let mut config = config;
                                let operation_id = operation.id;
                                move |e: Event<FormData>| {
                                    let mut next = config.read().clone();
                                    if let Some(op) = next
                                        .output_config
                                        .operations
                                        .iter_mut()
                                        .find(|item| item.id == operation_id)
                                    {
                                        op.template = e.value();
                                    }
                                    config.set(next);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
