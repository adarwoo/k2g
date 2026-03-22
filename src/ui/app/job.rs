use dioxus::prelude::*;

use crate::board::collect_board_snapshot;
use crate::board::HoleKind;
use kicad_ipc_rs::KiCadClientBlocking;
use super::super::model::*;

#[component]
pub fn JobScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let mut board_refresh_status = use_signal(String::new);
    let mut board_zoom = use_signal(|| 1.0_f64);
    let mut board_pan_x = use_signal(|| 0.0_f64);
    let mut board_pan_y = use_signal(|| 0.0_f64);
    let mut board_is_panning = use_signal(|| false);
    let mut board_last_pointer = use_signal(|| (0.0_f64, 0.0_f64));
    let has_atc = snapshot.selected_machine_has_atc();
    let board_thickness_is_probe = snapshot.job_config.board_thickness_mode == BoardThicknessMode::Probe;
    let board_thickness_is_entered = matches!(
        snapshot.job_config.board_thickness_mode,
        BoardThicknessMode::Preset | BoardThicknessMode::UserDefined
    );
    let board_thickness_uses_touch_probe = if board_thickness_is_probe {
        true
    } else {
        snapshot.job_config.z0_determination_mode == Z0DeterminationMode::TouchProbe
    };
    let board_thickness_uses_atc_probe = board_thickness_uses_touch_probe
        && snapshot.job_config.touch_probe_source == TouchProbeSource::AtcSlot;
    let board_thickness_unit = if snapshot.unit_system == UnitSystem::Imperial {
        "in"
    } else {
        "mm"
    };
    let board_thickness_step = if snapshot.unit_system == UnitSystem::Imperial {
        "0.001"
    } else {
        "0.1"
    };
    let atc_slot_count = snapshot.selected_machine().map(|m| m.atc_slot_count).unwrap_or(0);
    let milling_outline_enabled = snapshot
        .job_config
        .selected_operations
        .contains(&ProductionOperation::MillBoard);
    let tab_width_display = if snapshot.unit_system == UnitSystem::Imperial {
        snapshot.job_config.tab_width_mm / 25.4
    } else {
        snapshot.job_config.tab_width_mm
    };
    let tab_width_unit = if snapshot.unit_system == UnitSystem::Imperial {
        "in"
    } else {
        "mm"
    };
    let tab_width_step = if snapshot.unit_system == UnitSystem::Imperial {
        "0.001"
    } else {
        "0.1"
    };
    let tab_width_hint = if snapshot.unit_system == UnitSystem::Imperial {
        "1/16in"
    } else {
        "2.4mm"
    };
    let mouse_bite_pitch_display = if snapshot.unit_system == UnitSystem::Imperial {
        snapshot.job_config.mouse_bite_pitch_mm / 25.4
    } else {
        snapshot.job_config.mouse_bite_pitch_mm
    };
    let mouse_bite_pitch_min = if snapshot.unit_system == UnitSystem::Imperial {
        "0.024"
    } else {
        "0.6"
    };
    let mouse_bite_pitch_max = if snapshot.unit_system == UnitSystem::Imperial {
        "0.059"
    } else {
        "1.5"
    };
    let eligible_router_tools: Vec<&Tool> = snapshot
        .tools
        .iter()
        .filter(|tool| {
            tool.kind.to_ascii_lowercase().contains("router")
                && {
                    let d = tool.diameter.as_mm();
                    (0.8..=2.5).contains(&d)
                }
        })
        .collect();
    let selected_router_diameter_mm = snapshot
        .job_config
        .outline_router_tool_id
        .as_ref()
        .and_then(|id| {
            eligible_router_tools
                .iter()
                .find(|tool| tool.id == *id)
                .map(|tool| tool.diameter.as_mm())
        });
    let eligible_mouse_bite_drills: Vec<&Tool> = snapshot
        .tools
        .iter()
        .filter(|tool| {
            if !tool.kind.to_ascii_lowercase().contains("drill") {
                return false;
            }
            let d = tool.diameter.as_mm();
            if !(0.5..=1.5).contains(&d) {
                return false;
            }
            if let Some(router_d) = selected_router_diameter_mm {
                d <= router_d
            } else {
                false
            }
        })
        .collect();

    let board_view_size = 1000.0_f64;
    let zoom_value = *board_zoom.read();
    let pan_x_value = *board_pan_x.read();
    let pan_y_value = *board_pan_y.read();
    let viewport_size = (board_view_size / zoom_value).clamp(50.0, board_view_size);
    let max_pan = (board_view_size - viewport_size).max(0.0);
    let view_x = pan_x_value.clamp(0.0, max_pan);
    let view_y = pan_y_value.clamp(0.0, max_pan);
    let board_view_box = format!("{view_x} {view_y} {viewport_size} {viewport_size}");
    let zoom_percent = (zoom_value * 100.0).round() as i32;
    let board_hole_markers: Vec<(f64, f64, f64, HoleKind)> = if let Some(board) = snapshot.board.as_ref() {
        if let Some(bbox) = board.bounding_box.as_ref() {
            let min_x = bbox.x.as_mm();
            let max_y = bbox.y.as_mm() + bbox.height.as_mm();
            let width = bbox.width.as_mm();
            let height = bbox.height.as_mm();

            if width > 0.0 && height > 0.0 {
                board
                    .holes
                    .iter()
                    .map(|hole| {
                        let x = ((hole.position.x.as_mm() - min_x) / width).clamp(0.0, 1.0)
                            * board_view_size;
                        // Flip Y so KiCad-style coordinates render with top at SVG y=0.
                        let y = ((max_y - hole.position.y.as_mm()) / height).clamp(0.0, 1.0)
                            * board_view_size;
                        let cross_half = hole
                            .drill_x
                            .as_ref()
                            .map(|d| (d.as_mm() / width) * board_view_size * 1.5)
                            .unwrap_or(6.0)
                            .clamp(4.0, 20.0);
                        (x, y, cross_half, hole.kind.clone())
                    })
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let mut views = vec![JobCenterView::Board, JobCenterView::Machining, JobCenterView::Code];
    if has_atc {
        views.push(JobCenterView::Rack);
    }

    let active_view = if snapshot.selected_job_view == JobCenterView::Rack && !has_atc {
        JobCenterView::Board
    } else {
        snapshot.selected_job_view
    };

    rsx! {
        div { class: "screen single",
            div { class: "job-layout",
                section { class: "panel grow job-main",
                    div { class: "job-view-tabs",
                        for view in views.iter() {
                            button {
                                key: "{view.key()}",
                                class: if *view == active_view { "job-view-tab active" } else { "job-view-tab" },
                                onclick: {
                                    let target = *view;
                                    move |_| state.with_mut(|s| s.selected_job_view = target)
                                },
                                "{view.label()}"
                            }
                        }
                    }

                    match active_view {
                        JobCenterView::Board => rsx! {
                            div { class: "board-preview",
                                button {
                                    class: "btn btn-secondary",
                                    onclick: move |_| {
                                        match KiCadClientBlocking::connect() {
                                            Ok(client) => {
                                                match collect_board_snapshot(&client) {
                                                    Ok(board_snapshot) => {
                                                        let hole_count = board_snapshot.holes.len();
                                                        let has_bbox = board_snapshot.bounding_box.is_some();
                                                        state.with_mut(|s| s.board = Some(board_snapshot));
                                                        let bbox = if has_bbox { "yes" } else { "no" };
                                                        board_refresh_status
                                                            .set(
                                                                format!(
                                                                    "Board snapshot refreshed: {hole_count} holes, bounding box {bbox}.",
                                                                ),
                                                            );
                                                    }
                                                    Err(err) => {
                                                        board_refresh_status
                                                            .set(format!("Board snapshot refresh failed: {err}"));
                                                    }
                                                }
                                            }
                                            Err(err) => {
                                                board_refresh_status.set(format!("KiCad IPC connection failed: {err}"));
                                            }
                                        }
                                    },
                                    "Refresh Board Snapshot"
                                }
                                if !board_refresh_status.read().is_empty() {
                                    p { class: "diag-status", "{board_refresh_status}" }
                                }

                                if let Some(board) = snapshot.board.as_ref() {
                                    if board.bounding_box.is_some() {
                                        div { class: "board-view-controls",
                                            button {
                                                class: "btn btn-secondary",
                                                onclick: move |_| {
                                                    let next_zoom = (*board_zoom.read() * 1.25).clamp(1.0, 20.0);
                                                    board_zoom.set(next_zoom);
                                                },
                                                "+"
                                            }
                                            button {
                                                class: "btn btn-secondary",
                                                onclick: move |_| {
                                                    let next_zoom = (*board_zoom.read() / 1.25).clamp(1.0, 20.0);
                                                    board_zoom.set(next_zoom);
                                                },
                                                "-"
                                            }
                                            button {
                                                class: "btn btn-secondary",
                                                onclick: move |_| {
                                                    board_zoom.set(1.0);
                                                    board_pan_x.set(0.0);
                                                    board_pan_y.set(0.0);
                                                },
                                                "Reset"
                                            }
                                            span { class: "board-view-status", "Zoom {zoom_percent}%" }
                                        }
                                        div {
                                            class: if *board_is_panning.read() { "board-canvas is-panning" } else { "board-canvas" },
                                            onmousedown: move |evt| {
                                                board_is_panning.set(true);
                                                let p = evt.element_coordinates();
                                                board_last_pointer.set((p.x, p.y));
                                            },
                                            onmouseup: move |_| {
                                                board_is_panning.set(false);
                                            },
                                            onmouseleave: move |_| {
                                                board_is_panning.set(false);
                                            },
                                            onmousemove: move |evt| {
                                                if !*board_is_panning.read() {
                                                    return;
                                                }
                                                let p = evt.element_coordinates();
                                                let (last_x, last_y) = *board_last_pointer.read();
                                                board_last_pointer.set((p.x, p.y));

                                                let dx = p.x - last_x;
                                                let dy = p.y - last_y;
                                                let unit_per_px = viewport_size / board_view_size;

                                                let next_x = (*board_pan_x.read() - dx * unit_per_px).clamp(0.0, max_pan);
                                                let next_y = (*board_pan_y.read() - dy * unit_per_px).clamp(0.0, max_pan);
                                                board_pan_x.set(next_x);
                                                board_pan_y.set(next_y);
                                            },
                                            onwheel: move |evt| {
                                                let wheel_y = evt.delta().strip_units().y;
                                                let old_zoom = *board_zoom.read();
                                                let zoom_factor = if wheel_y < 0.0 { 1.12 } else { 1.0 / 1.12 };
                                                let new_zoom = (old_zoom * zoom_factor).clamp(1.0, 20.0);
                                                if (new_zoom - old_zoom).abs() < f64::EPSILON {
                                                    return;
                                                }

                                                let old_viewport = (board_view_size / old_zoom).clamp(50.0, board_view_size);
                                                let new_viewport = (board_view_size / new_zoom).clamp(50.0, board_view_size);
                                                let center_x = view_x + old_viewport * 0.5;
                                                let center_y = view_y + old_viewport * 0.5;
                                                let new_max_pan = (board_view_size - new_viewport).max(0.0);
                                                board_zoom.set(new_zoom);
                                                board_pan_x.set((center_x - new_viewport * 0.5).clamp(0.0, new_max_pan));
                                                board_pan_y.set((center_y - new_viewport * 0.5).clamp(0.0, new_max_pan));
                                            },
                                            svg {
                                                class: "board-svg",
                                                view_box: "{board_view_box}",
                                                preserve_aspect_ratio: "xMidYMid meet",

                                                rect {
                                                    x: "0",
                                                    y: "0",
                                                    width: "{board_view_size}",
                                                    height: "{board_view_size}",
                                                    class: "board-svg-frame",
                                                }

                                                for (x , y , half , kind) in board_hole_markers.iter() {
                                                    line {
                                                        x1: "{x - half}",
                                                        y1: "{y - half}",
                                                        x2: "{x + half}",
                                                        y2: "{y + half}",
                                                        class: match kind {
                                                            HoleKind::Via => "board-hole-cross board-hole-via",
                                                            HoleKind::PadPth => "board-hole-cross board-hole-pth",
                                                            HoleKind::PadNpth => "board-hole-cross board-hole-npth",
                                                        },
                                                    }
                                                    line {
                                                        x1: "{x - half}",
                                                        y1: "{y + half}",
                                                        x2: "{x + half}",
                                                        y2: "{y - half}",
                                                        class: match kind {
                                                            HoleKind::Via => "board-hole-cross board-hole-via",
                                                            HoleKind::PadPth => "board-hole-cross board-hole-pth",
                                                            HoleKind::PadNpth => "board-hole-cross board-hole-npth",
                                                        },
                                                    }

                                                    if matches!(kind, HoleKind::PadPth) {
                                                        rect {
                                                            x: "{x - half * 1.15}",
                                                            y: "{y - half * 1.15}",
                                                            width: "{half * 2.3}",
                                                            height: "{half * 2.3}",
                                                            class: "board-hole-outline board-hole-pth-box",
                                                        }
                                                    }

                                                    if matches!(kind, HoleKind::PadNpth) {
                                                        path {
                                                            d: "M {x - half * 1.2} {y + half * 1.2} L {x - half * 1.2} {y - half * 1.2} L {x + half * 1.2} {y - half * 1.2}",
                                                            class: "board-hole-outline board-hole-npth-halfbox",
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        p { "Board holes: {board.holes.len()} rendered as crosses" }
                                        div { class: "board-legend",
                                            div { class: "board-legend-item",
                                                svg { class: "board-legend-icon", view_box: "0 0 24 24",
                                                    line {
                                                        x1: "5",
                                                        y1: "5",
                                                        x2: "19",
                                                        y2: "19",
                                                        class: "board-hole-cross board-hole-via",
                                                    }
                                                    line {
                                                        x1: "5",
                                                        y1: "19",
                                                        x2: "19",
                                                        y2: "5",
                                                        class: "board-hole-cross board-hole-via",
                                                    }
                                                }
                                                span { "Via" }
                                            }
                                            div { class: "board-legend-item",
                                                svg { class: "board-legend-icon", view_box: "0 0 24 24",
                                                    line {
                                                        x1: "5",
                                                        y1: "5",
                                                        x2: "19",
                                                        y2: "19",
                                                        class: "board-hole-cross board-hole-pth",
                                                    }
                                                    line {
                                                        x1: "5",
                                                        y1: "19",
                                                        x2: "19",
                                                        y2: "5",
                                                        class: "board-hole-cross board-hole-pth",
                                                    }
                                                    rect {
                                                        x: "3.5",
                                                        y: "3.5",
                                                        width: "17",
                                                        height: "17",
                                                        class: "board-hole-outline board-hole-pth-box",
                                                    }
                                                }
                                                span { "PTH" }
                                            }
                                            div { class: "board-legend-item",
                                                svg { class: "board-legend-icon", view_box: "0 0 24 24",
                                                    line {
                                                        x1: "5",
                                                        y1: "5",
                                                        x2: "19",
                                                        y2: "19",
                                                        class: "board-hole-cross board-hole-npth",
                                                    }
                                                    line {
                                                        x1: "5",
                                                        y1: "19",
                                                        x2: "19",
                                                        y2: "5",
                                                        class: "board-hole-cross board-hole-npth",
                                                    }
                                                    path {
                                                        d: "M 4 20 L 4 4 L 20 4",
                                                        class: "board-hole-outline board-hole-npth-halfbox",
                                                    }
                                                }
                                                span { "NPTH" }
                                            }
                                        }
                                    } else {
                                        div { class: "canvas-mock", "Board bounding box unavailable" }
                                        p { "Open a board in KiCad to render the board graph." }
                                    }
                                } else {
                                    div { class: "canvas-mock", "Board snapshot unavailable" }
                                    p { "Click 'Refresh Board Snapshot' while a PCB is open in KiCad." }
                                }
                            }
                        },
                        JobCenterView::Machining => rsx! {
                            div { class: "screen single",
                                div { class: "machining-summary",
                                    div { class: "impact-item",
                                        div { class: "impact-name", "Operations" }
                                        div { class: "impact-state", "{snapshot.job_config.selected_operations.len()} selected" }
                                    }
                                    div { class: "impact-item",
                                        div { class: "impact-name", "Tools in rack" }
                                        div { class: "impact-state",
                                            "{snapshot.tools.iter().filter(|t| t.status == ToolStatus::InRack).count()}"
                                        }
                                    }
                                }
                                div { class: "canvas-mock", "Machining: operation flow + tool paths" }
                            }
                        },
                        JobCenterView::Code => rsx! {
                            div { class: "screen single",
                                textarea {
                                    class: "gcode-editor",
                                    value: snapshot.gcode.clone(),
                                    oninput: move |evt| {
                                        let value = evt.value();
                                        state
                                            .with_mut(|s| {
                                                s.gcode = value;
                                                s.gcode_modified = true;
                                            });
                                    },
                                }
                                div { class: "program-stats",
                                    span { "Save target: {snapshot.save_filename}" }
                                    span { "Lines: {snapshot.gcode.lines().count()}" }
                                    span { "Characters: {snapshot.gcode.len()}" }
                                }
                            }
                        },
                        JobCenterView::Rack => rsx! {
                            if has_atc {
                                div { class: "screen single",
                                    h3 { "Rack Preview" }
                                    div { class: "rack-grid",
                                        for (slot_num , slot) in snapshot.rack_slots.iter() {
                                            div { class: if slot.disabled { "rack-slot disabled" } else if slot.tool_id.is_some() { "rack-slot assigned" } else { "rack-slot" },
                                                div { class: "rack-slot-title", "Slot #{slot_num}" }
                                                p {
                                                    "Tool: "
                                                    {slot.tool_id.as_deref().unwrap_or("Empty")}
                                                }
                                                p {
                                                    "Locked: "
                                                    {if slot.locked { "Yes" } else { "No" }}
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                div { class: "screen single centered",
                                    p { "Rack view is only available when the selected CNC profile has ATC." }
                                }
                            }
                        },
                    }
                }

                section { class: "panel fixed",
                    h3 { "Job Configuration" }

                    div { class: "field",
                        label { "Operations" }
                        for op in ProductionOperation::all().iter() {
                            button {
                                key: "{op.label()}",
                                class: if snapshot.job_config.selected_operations.contains(op) { "btn-op active" } else { "btn-op" },
                                onclick: {
                                    let operation = *op;
                                    move |_| state.with_mut(|s| s.toggle_operation(operation))
                                },
                                "{op.label()}"
                            }
                        }
                    }

                    div { class: "field",
                        label { "Side to machine" }
                        select {
                            value: snapshot.job_config.side.as_str(),
                            onchange: move |evt| {
                                let v = evt.value();
                                state
                                    .with_mut(|s| {
                                        s.job_config.side = if v == "bottom" { Side::Bottom } else { Side::Top };
                                    });
                            },
                            option { value: "top", "Top (Component side)" }
                            option { value: "bottom", "Bottom (Solder side)" }
                        }
                    }

                    div { class: "field",
                        label { "Cut Depth Strategy" }
                        div { class: "radio-group vertical",
                            div { class: "radio-option",
                                label {
                                    input {
                                        r#type: "radio",
                                        name: "cut_depth_strategy",
                                        value: "automatic",
                                        checked: snapshot.job_config.cut_depth_strategy == CutDepthStrategy::Automatic,
                                        onchange: move |_| {
                                            state
                                                .with_mut(|s| s.job_config.cut_depth_strategy = CutDepthStrategy::Automatic);
                                        },
                                    }
                                    span { "Automatic (recommended)" }
                                }
                            }
                            div { class: "radio-option",
                                label {
                                    input {
                                        r#type: "radio",
                                        name: "cut_depth_strategy",
                                        value: "single_pass",
                                        checked: snapshot.job_config.cut_depth_strategy == CutDepthStrategy::SinglePass,
                                        onchange: move |_| {
                                            state
                                                .with_mut(|s| {
                                                    s.job_config.cut_depth_strategy = CutDepthStrategy::SinglePass;
                                                });
                                        },
                                    }
                                    span { "Single Pass" }
                                }
                            }
                            div { class: "radio-option",
                                label {
                                    input {
                                        r#type: "radio",
                                        name: "cut_depth_strategy",
                                        value: "multi_pass",
                                        checked: snapshot.job_config.cut_depth_strategy == CutDepthStrategy::MultiPass,
                                        onchange: move |_| {
                                            state
                                                .with_mut(|s| s.job_config.cut_depth_strategy = CutDepthStrategy::MultiPass);
                                        },
                                    }
                                    span { "Multi-pass" }
                                }
                                if snapshot.job_config.cut_depth_strategy == CutDepthStrategy::MultiPass {
                                    div { class: "sub-field",
                                        span { "Max depth per pass: " }
                                        input {
                                            r#type: "number",
                                            step: "0.1",
                                            value: "{snapshot.job_config.multi_pass_max_depth_mm}",
                                            oninput: move |evt| {
                                                let value = evt.value().parse::<f32>().unwrap_or(1.0);
                                                state.with_mut(|s| s.job_config.multi_pass_max_depth_mm = value);
                                            },
                                        }
                                        span { " mm" }
                                    }
                                }
                            }
                        }
                    }

                    if milling_outline_enabled {
                        details { class: "field collapsible-group",
                            summary { "Outline milling" }

                            div { class: "field section-subfield",
                                label { "Router tool selection" }
                                p { class: "diag-status", "Must be a router, diameter 0.8-2.5mm" }
                                select {
                                    value: snapshot
                                                                                                                                                                                                                                                                                                                                                                                                                .job_config
                                                                                                                                                                                                                                                                                                                                                                                                                .outline_router_tool_id
                                                                                                                                                                                                                                                                                                                                                                                                                .clone()
                                                                                                                                                                                                                                                                                                                                                                                                                .unwrap_or_default(),
                                    onchange: move |evt| {
                                        let value = evt.value();
                                        state
                                            .with_mut(|s| {
                                                s.job_config.outline_router_tool_id = if value.trim().is_empty() {
                                                    None
                                                } else {
                                                    Some(value.clone())
                                                };
                                                let router_d = s
                                                    .job_config
                                                    .outline_router_tool_id
                                                    .as_ref()
                                                    .and_then(|id| s.tools.iter().find(|t| &t.id == id))
                                                    .map(|t| t.diameter.as_mm());
                                                if let Some(drill_id) = s.job_config.mouse_bite_drill_tool_id.clone() {
                                                    let valid = s
                                                        .tools
                                                        .iter()
                                                        .find(|t| t.id == drill_id)
                                                        .map(|t| {
                                                            let kind_ok = t.kind.to_ascii_lowercase().contains("drill");
                                                            let d = t.diameter.as_mm();
                                                            let in_range = (0.5..=1.5).contains(&d);
                                                            let router_ok = router_d.map(|r| d <= r).unwrap_or(false);
                                                            kind_ok && in_range && router_ok
                                                        })
                                                        .unwrap_or(false);
                                                    if !valid {
                                                        s.job_config.mouse_bite_drill_tool_id = None;
                                                    }
                                                }
                                            });
                                    },
                                    option { value: "", "Select router tool" }
                                    for tool in eligible_router_tools.iter() {
                                        option { value: "{tool.id}", "{tool.name} ({tool.diameter})" }
                                    }
                                }
                            }

                            div { class: "field section-subfield",
                                label { "Number of tabs" }
                                input {
                                    r#type: "number",
                                    min: "0",
                                    step: "1",
                                    value: "{snapshot.job_config.tab_count}",
                                    oninput: move |evt| {
                                        let value = evt.value().parse::<u8>().unwrap_or(0);
                                        state.with_mut(|s| s.job_config.tab_count = value);
                                    },
                                }
                            }

                            if snapshot.job_config.tab_count > 0 {
                                div { class: "field section-subfield",
                                    label { "Width of tabs" }
                                    p { class: "diag-status", "Recommended default: {tab_width_hint}" }
                                    div { class: "sub-field",
                                        input {
                                            r#type: "number",
                                            min: "0",
                                            step: "{tab_width_step}",
                                            value: "{tab_width_display}",
                                            oninput: move |evt| {
                                                let value = evt.value().parse::<f32>().unwrap_or(0.0).max(0.0);
                                                state
                                                    .with_mut(|s| {
                                                        s.job_config.tab_width_mm = if s.unit_system == UnitSystem::Imperial {
                                                            value * 25.4
                                                        } else {
                                                            value
                                                        };
                                                    });
                                            },
                                        }
                                        span { " {tab_width_unit}" }
                                    }
                                }

                                div { class: "field section-subfield",
                                    label { "Mouse bites" }
                                    label { class: "checkbox-line",
                                        input {
                                            r#type: "checkbox",
                                            checked: snapshot.job_config.mouse_bites_enabled,
                                            oninput: move |evt| {
                                                let enabled = evt.checked();
                                                state.with_mut(|s| s.job_config.mouse_bites_enabled = enabled);
                                            },
                                        }
                                        span { "Enable mouse bites" }
                                    }
                                }

                                if snapshot.job_config.mouse_bites_enabled {
                                    div { class: "field section-subfield",
                                        label { "Center to center" }
                                        div { class: "sub-field",
                                            input {
                                                r#type: "number",
                                                min: "{mouse_bite_pitch_min}",
                                                max: "{mouse_bite_pitch_max}",
                                                step: "{tab_width_step}",
                                                value: "{mouse_bite_pitch_display}",
                                                oninput: move |evt| {
                                                    let value = evt.value().parse::<f32>().unwrap_or(0.8).max(0.0);
                                                    state
                                                        .with_mut(|s| {
                                                            s.job_config.mouse_bite_pitch_mm = if s.unit_system
                                                                == UnitSystem::Imperial
                                                            {
                                                                value * 25.4
                                                            } else {
                                                                value
                                                            };
                                                        });
                                                },
                                            }
                                            span {
                                                if snapshot.unit_system == UnitSystem::Imperial {
                                                    " in"
                                                } else {
                                                    " mm"
                                                }
                                            }
                                        }
                                    }

                                    div { class: "field section-subfield",
                                        label { "Mouse-bite drill tool" }
                                        p { class: "diag-status",
                                            "Only drill bits 0.5-1.5mm, and not larger than selected router diameter"
                                        }
                                        select {
                                            disabled: snapshot.job_config.outline_router_tool_id.is_none(),
                                            value: snapshot
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        .job_config
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        .mouse_bite_drill_tool_id
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        .clone()
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        .unwrap_or_default(),
                                            onchange: move |evt| {
                                                let value = evt.value();
                                                state
                                                    .with_mut(|s| {
                                                        s.job_config.mouse_bite_drill_tool_id = if value.trim().is_empty() {
                                                            None
                                                        } else {
                                                            Some(value)
                                                        };
                                                    });
                                            },
                                            option { value: "", "Select drill tool" }
                                            for tool in eligible_mouse_bite_drills.iter() {
                                                option { value: "{tool.id}",
                                                    "{tool.name} ({tool.diameter})"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "field",
                        label { "Board Thickness" }
                        div { class: "radio-group vertical",
                            div { class: "radio-option",
                                label {
                                    input {
                                        r#type: "radio",
                                        name: "board_thickness_mode",
                                        value: "preset",
                                        checked: snapshot.job_config.board_thickness_mode == BoardThicknessMode::Preset,
                                        onchange: move |_| {
                                            state
                                                .with_mut(|s| {
                                                    s.job_config.board_thickness_mode = BoardThicknessMode::Preset;
                                                });
                                        },
                                    }
                                    span { "Preset values" }
                                }
                                select {
                                    disabled: snapshot.job_config.board_thickness_mode != BoardThicknessMode::Preset,
                                    value: "{snapshot.job_config.board_thickness_preset_mm}",
                                    onchange: move |evt| {
                                        let value = evt.value().parse::<f32>().unwrap_or(1.6);
                                        state.with_mut(|s| s.job_config.board_thickness_preset_mm = value);
                                    },
                                    option { value: "0.8", "0.8 mm" }
                                    option { value: "1.0", "1.0 mm" }
                                    option { value: "1.2", "1.2 mm" }
                                    option { value: "1.6", "1.6 mm" }
                                    option { value: "2.0", "2.0 mm" }
                                    option { value: "2.4", "2.4 mm" }
                                }
                            }
                            div { class: "radio-option",
                                label {
                                    input {
                                        r#type: "radio",
                                        name: "board_thickness_mode",
                                        value: "user_defined",
                                        checked: snapshot.job_config.board_thickness_mode == BoardThicknessMode::UserDefined,
                                        onchange: move |_| {
                                            state
                                                .with_mut(|s| {
                                                    s.job_config.board_thickness_mode = BoardThicknessMode::UserDefined;
                                                });
                                        },
                                    }
                                    span { "User defined value" }
                                }
                                if snapshot.job_config.board_thickness_mode == BoardThicknessMode::UserDefined {
                                    div { class: "sub-field",
                                        input {
                                            r#type: "number",
                                            step: "{board_thickness_step}",
                                            value: "{snapshot.job_config.board_thickness_user_value}",
                                            oninput: move |evt| {
                                                let value = evt.value().parse::<f32>().unwrap_or(1.6).max(0.0);
                                                state.with_mut(|s| s.job_config.board_thickness_user_value = value);
                                            },
                                        }
                                        span { " {board_thickness_unit}" }
                                    }
                                }
                            }
                            div { class: "radio-option",
                                label {
                                    input {
                                        r#type: "radio",
                                        name: "board_thickness_mode",
                                        value: "probe",
                                        checked: snapshot.job_config.board_thickness_mode == BoardThicknessMode::Probe,
                                        onchange: move |_| {
                                            state
                                                .with_mut(|s| {
                                                    s.job_config.board_thickness_mode = BoardThicknessMode::Probe;
                                                });
                                        },
                                    }
                                    span { "Probe" }
                                }
                            }
                        }

                        if board_thickness_is_entered {
                            div { class: "field section-subfield",
                                label { "Z0 determination" }
                                div { class: "nested-radio-group",
                                    label {
                                        input {
                                            r#type: "radio",
                                            name: "z0_determination_mode",
                                            value: "manual_adjust_z0",
                                            checked: snapshot.job_config.z0_determination_mode == Z0DeterminationMode::ManualAdjustZ0,
                                            onchange: move |_| {
                                                state
                                                    .with_mut(|s| {
                                                        s.job_config.z0_determination_mode = Z0DeterminationMode::ManualAdjustZ0;
                                                    });
                                            },
                                        }
                                        span { "Manually adjust Z0" }
                                    }
                                    label {
                                        input {
                                            r#type: "radio",
                                            name: "z0_determination_mode",
                                            value: "touch_probe",
                                            checked: snapshot.job_config.z0_determination_mode == Z0DeterminationMode::TouchProbe,
                                            onchange: move |_| {
                                                state
                                                    .with_mut(|s| {
                                                        s.job_config.z0_determination_mode = Z0DeterminationMode::TouchProbe;
                                                    });
                                            },
                                        }
                                        span { "Use touch probe" }
                                    }
                                }
                            }
                        }

                        if board_thickness_is_probe || board_thickness_uses_touch_probe {
                            div { class: "field section-subfield",
                                label { "Touch probe setup" }
                                div { class: "nested-radio-group",
                                    label {
                                        input {
                                            r#type: "radio",
                                            name: "touch_probe_source",
                                            value: "manual_installation",
                                            checked: snapshot.job_config.touch_probe_source == TouchProbeSource::ManualInstallation,
                                            onchange: move |_| {
                                                state
                                                    .with_mut(|s| {
                                                        s.job_config.touch_probe_source = TouchProbeSource::ManualInstallation;
                                                    });
                                            },
                                        }
                                        span { "Manual installation of the touch probe" }
                                    }
                                    if has_atc {
                                        label {
                                            input {
                                                r#type: "radio",
                                                name: "touch_probe_source",
                                                value: "atc_slot",
                                                checked: snapshot.job_config.touch_probe_source == TouchProbeSource::AtcSlot,
                                                onchange: move |_| {
                                                    state
                                                        .with_mut(|s| {
                                                            s.job_config.touch_probe_source = TouchProbeSource::AtcSlot;
                                                        });
                                                },
                                            }
                                            span { "Load touch probe from slot" }
                                        }
                                    }
                                    if has_atc && board_thickness_uses_atc_probe {
                                        div { class: "sub-field",
                                            span { "Slot" }
                                            input {
                                                r#type: "number",
                                                min: "0",
                                                max: "{atc_slot_count}",
                                                step: "1",
                                                value: "{snapshot.job_config.touch_probe_atc_slot}",
                                                oninput: move |evt| {
                                                    let value = evt.value().parse::<u8>().unwrap_or(0).min(atc_slot_count);
                                                    state.with_mut(|s| s.job_config.touch_probe_atc_slot = value);
                                                },
                                            }
                                            span { " / 0-{atc_slot_count}" }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "field",
                        label { "Board Orientation" }
                        div { class: "radio-group vertical",
                            div { class: "radio-option",
                                label {
                                    input {
                                        r#type: "radio",
                                        name: "board_orientation",
                                        value: "automatic",
                                        checked: snapshot.job_config.board_orientation == BoardOrientation::Automatic,
                                        onchange: move |_| {
                                            state
                                                .with_mut(|s| {
                                                    s.job_config.board_orientation = BoardOrientation::Automatic;
                                                });
                                        },
                                    }
                                    span { "Automatic" }
                                }
                            }
                            div { class: "radio-option",
                                label {
                                    input {
                                        r#type: "radio",
                                        name: "board_orientation",
                                        value: "no_rotation",
                                        checked: snapshot.job_config.board_orientation == BoardOrientation::NoRotation,
                                        onchange: move |_| {
                                            state
                                                .with_mut(|s| {
                                                    s.job_config.board_orientation = BoardOrientation::NoRotation;
                                                });
                                        },
                                    }
                                    span { "No rotation" }
                                }
                            }
                            div { class: "radio-option",
                                label {
                                    input {
                                        r#type: "radio",
                                        name: "board_orientation",
                                        value: "rotate_group",
                                        checked: matches!(
                                            snapshot.job_config.board_orientation,
                                            BoardOrientation::Rotate90
                                            | BoardOrientation::Rotate180
                                            | BoardOrientation::Rotate270
                                            | BoardOrientation::RotateCustom
                                        ),
                                        onchange: move |_| {
                                            state
                                                .with_mut(|s| {
                                                    s.job_config.board_orientation = BoardOrientation::Rotate90;
                                                });
                                        },
                                    }
                                    span { "Rotate" }
                                }
                                if matches!(
                                    snapshot.job_config.board_orientation,
                                    BoardOrientation::Rotate90
                                    | BoardOrientation::Rotate180
                                    | BoardOrientation::Rotate270
                                    | BoardOrientation::RotateCustom
                                )
                                {
                                    div { class: "nested-radio-group",
                                        label {
                                            input {
                                                r#type: "radio",
                                                name: "board_rotation_angle",
                                                value: "90",
                                                checked: snapshot.job_config.board_orientation == BoardOrientation::Rotate90,
                                                onchange: move |_| {
                                                    state
                                                        .with_mut(|s| {
                                                            s.job_config.board_orientation = BoardOrientation::Rotate90;
                                                        });
                                                },
                                            }
                                            span { "90°" }
                                        }
                                        label {
                                            input {
                                                r#type: "radio",
                                                name: "board_rotation_angle",
                                                value: "180",
                                                checked: snapshot.job_config.board_orientation == BoardOrientation::Rotate180,
                                                onchange: move |_| {
                                                    state
                                                        .with_mut(|s| {
                                                            s.job_config.board_orientation = BoardOrientation::Rotate180;
                                                        });
                                                },
                                            }
                                            span { "180°" }
                                        }
                                        label {
                                            input {
                                                r#type: "radio",
                                                name: "board_rotation_angle",
                                                value: "270",
                                                checked: snapshot.job_config.board_orientation == BoardOrientation::Rotate270,
                                                onchange: move |_| {
                                                    state
                                                        .with_mut(|s| {
                                                            s.job_config.board_orientation = BoardOrientation::Rotate270;
                                                        });
                                                },
                                            }
                                            span { "270°" }
                                        }
                                        label {
                                            input {
                                                r#type: "radio",
                                                name: "board_rotation_angle",
                                                value: "custom",
                                                checked: snapshot.job_config.board_orientation == BoardOrientation::RotateCustom,
                                                onchange: move |_| {
                                                    state
                                                        .with_mut(|s| {
                                                            s.job_config.board_orientation = BoardOrientation::RotateCustom;
                                                        });
                                                },
                                            }
                                            span { "Custom" }
                                        }
                                        if snapshot.job_config.board_orientation == BoardOrientation::RotateCustom {
                                            div { class: "custom-angle-input",
                                                input {
                                                    r#type: "number",
                                                    min: "0",
                                                    max: "360",
                                                    step: "0.1",
                                                    value: "{snapshot.job_config.board_orientation_custom_degrees}",
                                                    oninput: move |evt| {
                                                        let value = evt.value().parse::<f32>().unwrap_or(0.0).clamp(0.0, 360.0);
                                                        state.with_mut(|s| s.job_config.board_orientation_custom_degrees = value);
                                                    },
                                                }
                                                span { " degrees" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if has_atc {
                        div { class: "field",
                            label { "Automatic Tool Change" }
                            select {
                                value: snapshot.job_config.atc_strategy.as_str(),
                                onchange: move |evt| {
                                    let v = evt.value();
                                    state
                                        .with_mut(|s| {
                                            s.job_config.atc_strategy = if v == "overwrite" {
                                                AtcRackStrategy::Overwrite
                                            } else if v == "reuse" {
                                                AtcRackStrategy::Reuse
                                            } else {
                                                AtcRackStrategy::Off
                                            };
                                        });
                                },
                                option { value: "off", "Manual tool change" }
                                option { value: "reuse", "Reuse rack" }
                                option { value: "overwrite", "Overwrite rack" }
                            }
                        }
                    }
                }
            }
        }
    }
}
