use dioxus::prelude::*;

use crate::board::{collect_board_snapshot, BoardEdgeShape, HoleKind};
use crate::stitching::stitch_edge_shapes;
use kicad_ipc_rs::KiCadClientBlocking;
use super::super::model::*;

/// Pre-computed SVG primitive for one edge-shape segment.
#[derive(Clone)]
enum SvgShape {
    Line { x1: f64, y1: f64, x2: f64, y2: f64 },
    Path(String),
    Rect { x: f64, y: f64, w: f64, h: f64, rx: f64 },
    Circle { cx: f64, cy: f64, r: f64 },
}

/// Given three points (start, mid, end) that lie on a circular arc, return
/// an SVG path string `M ... A ... ` for that arc.  Falls back to a straight
/// line if the points are collinear.
fn arc_svg_path(sx: f64, sy: f64, mx: f64, my: f64, ex: f64, ey: f64) -> String {
    let d = 2.0 * (sx * (my - ey) + mx * (ey - sy) + ex * (sy - my));
    if d.abs() < 1e-9 {
        // Collinear – draw a straight line.
        return format!("M {sx} {sy} L {ex} {ey}");
    }
    let sq = |v: f64| v * v;
    let mag1 = sq(sx) + sq(sy);
    let mag2 = sq(mx) + sq(my);
    let mag3 = sq(ex) + sq(ey);
    let cx = (mag1 * (my - ey) + mag2 * (ey - sy) + mag3 * (sy - my)) / d;
    let cy = (mag1 * (ex - mx) + mag2 * (sx - ex) + mag3 * (mx - sx)) / d;
    let r = ((sx - cx).powi(2) + (sy - cy).powi(2)).sqrt();

    let angle = |px: f64, py: f64| (py - cy).atan2(px - cx);
    let t1 = angle(sx, sy);
    let t2 = angle(mx, my);
    let t3 = angle(ex, ey);

    // Determine if the arc from t1 to t3 going clockwise (increasing atan2 in
    // SVG y-down space) passes through t2.
    let cw_span = (t3 - t1).rem_euclid(std::f64::consts::TAU);
    let cw_to_mid = (t2 - t1).rem_euclid(std::f64::consts::TAU);
    let mid_on_cw = cw_to_mid <= cw_span;

    let (sweep, large_arc) = if mid_on_cw {
        // CW arc through mid.
        let large = if cw_span > std::f64::consts::PI { 1 } else { 0 };
        (1, large)
    } else {
        // CCW arc through mid.
        let ccw_span = std::f64::consts::TAU - cw_span;
        let large = if ccw_span > std::f64::consts::PI { 1 } else { 0 };
        (0, large)
    };

    format!("M {sx} {sy} A {r} {r} 0 {large_arc} {sweep} {ex} {ey}")
}

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

    // Board coordinate space: width is always 1000 SVG units; height is scaled
    // proportionally so the aspect ratio matches the real board dimensions.
    // KiCad uses screen coordinates (Y increases downward), same as SVG, so no
    // Y-flip is needed.
    let board_view_width = 1000.0_f64;
    let board_view_height = {
        let aspect = snapshot.board.as_ref()
            .and_then(|b| b.bounding_box.as_ref())
            .filter(|bbox| bbox.width.as_mm() > 0.0 && bbox.height.as_mm() > 0.0)
            .map(|bbox| bbox.height.as_mm() / bbox.width.as_mm())
            .unwrap_or(1.0);
        board_view_width * aspect
    };
    let zoom_value = *board_zoom.read();
    let pan_x_value = *board_pan_x.read();
    let pan_y_value = *board_pan_y.read();
    let viewport_w = (board_view_width / zoom_value).clamp(10.0, board_view_width);
    let viewport_h = (board_view_height / zoom_value).clamp(10.0, board_view_height);
    let max_pan_x = (board_view_width - viewport_w).max(0.0);
    let max_pan_y = (board_view_height - viewport_h).max(0.0);
    let view_x = pan_x_value.clamp(0.0, max_pan_x);
    let view_y = pan_y_value.clamp(0.0, max_pan_y);
    let board_view_box = format!("{view_x} {view_y} {viewport_w} {viewport_h}");
    let zoom_percent = (zoom_value * 100.0).round() as i32;
    let board_hole_markers: Vec<(f64, f64, f64, HoleKind)> = if let Some(board) = snapshot.board.as_ref() {
        if let Some(bbox) = board.bounding_box.as_ref() {
            let min_x = bbox.x.as_mm();
            let min_y = bbox.y.as_mm();
            let width = bbox.width.as_mm();
            let height = bbox.height.as_mm();

            if width > 0.0 && height > 0.0 {
                board
                    .holes
                    .iter()
                    .map(|hole| {
                        let x = ((hole.position.x.as_mm() - min_x) / width).clamp(0.0, 1.0)
                            * board_view_width;
                        let y = ((hole.position.y.as_mm() - min_y) / height).clamp(0.0, 1.0)
                            * board_view_height;
                        let cross_half = hole
                            .drill_x
                            .as_ref()
                            .map(|d| (d.as_mm() / width) * board_view_width * 1.5)
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

    let board_edge_shapes_svg: Vec<SvgShape> = if let Some(board) = snapshot.board.as_ref() {
        if let Some(bbox) = board.bounding_box.as_ref() {
            let min_x = bbox.x.as_mm();
            let min_y = bbox.y.as_mm();
            let width = bbox.width.as_mm();
            let height = bbox.height.as_mm();

            if width > 0.0 && height > 0.0 {
                let tx = |px: f64| ((px - min_x) / width).clamp(0.0, 1.0) * board_view_width;
                let ty = |py: f64| ((py - min_y) / height).clamp(0.0, 1.0) * board_view_height;

                board.edge_shapes.iter().filter_map(|shape| {
                    match shape {
                        BoardEdgeShape::Track { start, end, .. }
                        | BoardEdgeShape::GraphicSegment { start, end, .. } => {
                            Some(SvgShape::Line {
                                x1: tx(start.x.as_mm()),
                                y1: ty(start.y.as_mm()),
                                x2: tx(end.x.as_mm()),
                                y2: ty(end.y.as_mm()),
                            })
                        }
                        BoardEdgeShape::Arc { start, mid, end, .. }
                        | BoardEdgeShape::GraphicArc { start, mid, end, .. } => {
                            Some(SvgShape::Path(arc_svg_path(
                                tx(start.x.as_mm()), ty(start.y.as_mm()),
                                tx(mid.x.as_mm()),   ty(mid.y.as_mm()),
                                tx(end.x.as_mm()),   ty(end.y.as_mm()),
                            )))
                        }
                        BoardEdgeShape::GraphicRectangle { top_left, bottom_right, corner_radius, .. } => {
                            let x = tx(top_left.x.as_mm());
                            let y = ty(top_left.y.as_mm());
                            let x2 = tx(bottom_right.x.as_mm());
                            let y2 = ty(bottom_right.y.as_mm());
                            let rx_val = corner_radius
                                .as_ref()
                                .map(|r| (r.as_mm() / width) * board_view_width)
                                .unwrap_or(0.0);
                            Some(SvgShape::Rect {
                                x: x.min(x2),
                                y: y.min(y2),
                                w: (x2 - x).abs(),
                                h: (y2 - y).abs(),
                                rx: rx_val,
                            })
                        }
                        BoardEdgeShape::GraphicCircle { center, radius_point, .. } => {
                            let cx = tx(center.x.as_mm());
                            let cy = ty(center.y.as_mm());
                            let rx_pt = tx(radius_point.x.as_mm());
                            let ry_pt = ty(radius_point.y.as_mm());
                            let r = ((rx_pt - cx).powi(2) + (ry_pt - cy).powi(2)).sqrt();
                            Some(SvgShape::Circle { cx, cy, r })
                        }
                        BoardEdgeShape::GraphicBezier { start, control1, control2, end, .. } => {
                            let (sx, sy) = (tx(start.x.as_mm()), ty(start.y.as_mm()));
                            let (c1x, c1y) = (tx(control1.x.as_mm()), ty(control1.y.as_mm()));
                            let (c2x, c2y) = (tx(control2.x.as_mm()), ty(control2.y.as_mm()));
                            let (ex, ey) = (tx(end.x.as_mm()), ty(end.y.as_mm()));
                            Some(SvgShape::Path(format!(
                                "M {sx} {sy} C {c1x} {c1y} {c2x} {c2y} {ex} {ey}"
                            )))
                        }
                        // GraphicPolygon only carries a count; skip it.
                        BoardEdgeShape::GraphicPolygon { .. } => None,
                    }
                }).collect()
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
                                                        let stitch_result = stitch_edge_shapes(&board_snapshot.edge_shapes);
                                                        let contour_count = stitch_result.contours.len();
                                                        state.with_mut(|s| s.board = Some(board_snapshot));
                                                        let bbox = if has_bbox { "yes" } else { "no" };
                                                        if stitch_result.errors.is_empty() {
                                                            board_refresh_status.set(format!(
                                                                "Board snapshot refreshed: {hole_count} holes, bounding box {bbox}, {contour_count} contour(s) — OK.",
                                                            ));
                                                        } else {
                                                            let error_list = stitch_result.errors.join("; ");
                                                            board_refresh_status.set(format!(
                                                                "Board geometry invalid — cannot process. {error_list}",
                                                            ));
                                                        }
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
                                                let unit_per_px_x = viewport_w / board_view_width;
                                                let unit_per_px_y = viewport_h / board_view_height;

                                                let next_x = (*board_pan_x.read() - dx * unit_per_px_x).clamp(0.0, max_pan_x);
                                                let next_y = (*board_pan_y.read() - dy * unit_per_px_y).clamp(0.0, max_pan_y);
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

                                                let old_vw = (board_view_width / old_zoom).clamp(10.0, board_view_width);
                                                let old_vh = (board_view_height / old_zoom).clamp(10.0, board_view_height);
                                                let new_vw = (board_view_width / new_zoom).clamp(10.0, board_view_width);
                                                let new_vh = (board_view_height / new_zoom).clamp(10.0, board_view_height);
                                                let center_x = view_x + old_vw * 0.5;
                                                let center_y = view_y + old_vh * 0.5;
                                                let new_max_pan_x = (board_view_width - new_vw).max(0.0);
                                                let new_max_pan_y = (board_view_height - new_vh).max(0.0);
                                                board_zoom.set(new_zoom);
                                                board_pan_x.set((center_x - new_vw * 0.5).clamp(0.0, new_max_pan_x));
                                                board_pan_y.set((center_y - new_vh * 0.5).clamp(0.0, new_max_pan_y));
                                            },
                                            svg {
                                                class: "board-svg",
                                                view_box: "{board_view_box}",
                                                preserve_aspect_ratio: "xMidYMid meet",

                                                rect {
                                                    x: "0",
                                                    y: "0",
                                                    width: "{board_view_width}",
                                                    height: "{board_view_height}",
                                                    class: "board-svg-frame",
                                                }

                                                for shape in board_edge_shapes_svg.iter() {
                                                    match shape {
                                                        SvgShape::Line { x1, y1, x2, y2 } => rsx! {
                                                            line {
                                                                x1: "{x1}",
                                                                y1: "{y1}",
                                                                x2: "{x2}",
                                                                y2: "{y2}",
                                                                class: "board-edge-shape",
                                                            }
                                                        },
                                                        SvgShape::Path(d) => rsx! {
                                                            path {
                                                                d: "{d}",
                                                                class: "board-edge-shape",
                                                            }
                                                        },
                                                        SvgShape::Rect { x, y, w, h, rx } => rsx! {
                                                            rect {
                                                                x: "{x}",
                                                                y: "{y}",
                                                                width: "{w}",
                                                                height: "{h}",
                                                                rx: "{rx}",
                                                                class: "board-edge-shape",
                                                            }
                                                        },
                                                        SvgShape::Circle { cx, cy, r } => rsx! {
                                                            circle {
                                                                cx: "{cx}",
                                                                cy: "{cy}",
                                                                r: "{r}",
                                                                class: "board-edge-shape",
                                                            }
                                                        },
                                                    }
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
                                        p { "Board edge shapes: {board.edge_shapes.len()} · Holes: {board.holes.len()}" }
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
                                            div { class: "board-legend-item",
                                                svg { class: "board-legend-icon", view_box: "0 0 24 24",
                                                    path {
                                                        d: "M 3 12 L 9 4 L 21 4 L 21 20 L 3 20 Z",
                                                        class: "board-edge-shape",
                                                    }
                                                }
                                                span { "Edge cuts" }
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
