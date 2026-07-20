//! Job "Board" view — the PCB visualization: renders the active board's edge
//! graph and drill holes as pan/zoom-able SVG, with a drill-size legend. Reads
//! the active board snapshot; all view state (zoom, pan, selected document) is
//! local to this component.

use dioxus::prelude::*;
use std::path::Path;

use pcb::{BoardEdgeShape, HoleKind};
use units::Length;

use crate::runtime::AppCtx;
use crate::ui::unit_format;

fn board_display_label(board_filename: &str) -> String {
    Path::new(board_filename)
        .file_name()
        .and_then(|v| v.to_str())
        .filter(|name| !name.is_empty())
        .map(|name| format!("{name} ({board_filename})"))
        .unwrap_or_else(|| board_filename.to_string())
}

/// Pre-computed SVG primitive for one edge-shape segment.
#[derive(Clone)]
enum SvgShape {
    Line { x1: f64, y1: f64, x2: f64, y2: f64 },
    Path(String),
    Rect { x: f64, y: f64, w: f64, h: f64, rx: f64 },
    Circle { cx: f64, cy: f64, r: f64 },
}

#[derive(Clone, Copy)]
enum DrillBaseShape {
    Circle,
    Square,
    Diamond,
    Triangle,
    Hexagon,
}

#[derive(Clone, Copy)]
enum DrillModifier {
    None,
    Filled,
    Dot,
    Plus,
    X,
    Bullseye,
    HalfFill,
    QuarterFill,
}

#[derive(Clone)]
struct BoardHoleMarker {
    x: f64,
    y: f64,
    marker_radius: f64,
    rotation_deg: f64,
    kind: HoleKind,
    base: DrillBaseShape,
    modifier: DrillModifier,
}

#[derive(Clone)]
struct DrillLegendEntry {
    diameter_mm: f64,
    base: DrillBaseShape,
    modifier: DrillModifier,
    rotation_deg: f64,
}

fn drill_symbol_from_index(index: usize) -> (DrillBaseShape, DrillModifier, f64) {
    const BASE_SHAPES: [DrillBaseShape; 5] = [
        DrillBaseShape::Circle,
        DrillBaseShape::Square,
        DrillBaseShape::Diamond,
        DrillBaseShape::Triangle,
        DrillBaseShape::Hexagon,
    ];
    const MODIFIERS: [DrillModifier; 8] = [
        DrillModifier::None,
        DrillModifier::Filled,
        DrillModifier::Dot,
        DrillModifier::Plus,
        DrillModifier::X,
        DrillModifier::Bullseye,
        DrillModifier::HalfFill,
        DrillModifier::QuarterFill,
    ];
    const ROTATIONS: [f64; 3] = [0.0, 45.0, 90.0];

    let base = BASE_SHAPES[index % BASE_SHAPES.len()];
    let modifier = MODIFIERS[(index / BASE_SHAPES.len()) % MODIFIERS.len()];
    let rotation = ROTATIONS[(index / (BASE_SHAPES.len() * MODIFIERS.len())) % ROTATIONS.len()];

    (base, modifier, rotation)
}

fn hole_marker_class(kind: &HoleKind) -> &'static str {
    match kind {
        HoleKind::Via => "board-hole-cross board-hole-via",
        HoleKind::PadPth => "board-hole-cross board-hole-pth",
        HoleKind::PadNpth => "board-hole-cross board-hole-npth",
    }
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

/// The PCB board-preview view: document selector, zoom/pan controls, the SVG
/// board render (edge shapes + hole markers), and the drill-size legend.
#[component]
pub fn BoardView(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let board_refresh_status = use_signal(String::new);
    let open_board_filenames = use_signal(Vec::<String>::new);
    let mut selected_board_filename = use_signal(String::new);
    let open_board_filenames_value = open_board_filenames.read().clone();
    let selected_board_filename_value = selected_board_filename.read().clone();
    let mut board_zoom = use_signal(|| 1.0_f64);
    let mut board_pan_x = use_signal(|| 0.0_f64);
    let mut board_pan_y = use_signal(|| 0.0_f64);
    let mut board_is_panning = use_signal(|| false);
    let mut board_last_pointer = use_signal(|| (0.0_f64, 0.0_f64));
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
    let (board_hole_markers, drill_size_legend): (Vec<BoardHoleMarker>, Vec<DrillLegendEntry>) = if let Some(board) = snapshot.board.as_ref() {
        if let Some(bbox) = board.bounding_box.as_ref() {
            let min_x = bbox.x.as_mm();
            let min_y = bbox.y.as_mm();
            let width = bbox.width.as_mm();
            let height = bbox.height.as_mm();

            if width > 0.0 && height > 0.0 {
                let mut drill_size_classes = board
                    .holes
                    .iter()
                    .filter_map(|hole| hole.drill_x.as_ref().or(hole.drill_y.as_ref()))
                    .map(|d| d.as_mm())
                    .collect::<Vec<_>>();
                drill_size_classes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                drill_size_classes.dedup_by(|a, b| (*a - *b).abs() < 1e-6);

                let legend_entries = drill_size_classes
                    .iter()
                    .enumerate()
                    .map(|(class_idx, diameter_mm)| {
                        let (base, modifier, rotation_deg) = drill_symbol_from_index(class_idx);
                        DrillLegendEntry {
                            diameter_mm: *diameter_mm,
                            base,
                            modifier,
                            rotation_deg,
                        }
                    })
                    .collect::<Vec<_>>();

                (board
                    .holes
                    .iter()
                    .map(|hole| {
                        let x = ((hole.position.x.as_mm() - min_x) / width).clamp(0.0, 1.0)
                            * board_view_width;
                        let y = ((hole.position.y.as_mm() - min_y) / height).clamp(0.0, 1.0)
                            * board_view_height;
                        let hole_diameter = hole
                            .drill_x
                            .as_ref()
                            .or(hole.drill_y.as_ref())
                            .map(|d| d.as_mm())
                            .unwrap_or(0.1)
                            .max(0.05);

                        let min_marker_radius = ((2.0 / width) * board_view_width * 0.5)
                            / zoom_value.max(1.0);
                        let marker_radius = ((hole_diameter / width) * board_view_width * 0.5)
                            .max(min_marker_radius)
                            .clamp((1.5 / zoom_value).max(0.5), 28.0);

                        let class_idx = drill_size_classes
                            .iter()
                            .position(|d| (*d - hole_diameter).abs() < 1e-6)
                            .unwrap_or(0);
                        let (base, modifier, rotation_deg) = drill_symbol_from_index(class_idx);
                        BoardHoleMarker {
                            x,
                            y,
                            marker_radius,
                            rotation_deg,
                            kind: hole.kind.clone(),
                            base,
                            modifier,
                        }
                    })
                    .collect::<Vec<_>>(),
                legend_entries)
            } else {
                (Vec::new(), Vec::new())
            }
        } else {
            (Vec::new(), Vec::new())
        }
    } else {
        (Vec::new(), Vec::new())
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

    rsx! {
                            div { class: "board-preview",
                                if !open_board_filenames_value.is_empty() {
                                    div { class: "field section-subfield",
                                        label { "Open PCB documents" }
                                        select {
                                            disabled: open_board_filenames_value.len() <= 1,
                                            value: selected_board_filename_value.clone(),
                                            onchange: move |evt| {
                                                selected_board_filename.set(evt.value());
                                            },
                                            for board_filename in open_board_filenames_value.iter() {
                                                option { value: board_filename.clone(), "{board_display_label(board_filename)}" }
                                            }
                                        }
                                        if open_board_filenames_value.len() > 1 {
                                            p { class: "diag-status",
                                                "Multiple PCBs detected. Selected board will be used for snapshot refresh."
                                            }
                                        }
                                    }
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
                                        div { class: "board-preview-layout",
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
                                                                path { d: "{d}", class: "board-edge-shape" }
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

                                                    for (idx , marker) in board_hole_markers.iter().enumerate() {
                                                        {
                                                            let r = marker.marker_radius;
                                                            let stroke_width = 1.8_f64;
                                                            let symbol_class = hole_marker_class(&marker.kind);
                                                            let half_fill_w = r;
                                                            let quarter_fill_w = r;
                                                            let quarter_fill_h = r;
                                                            rsx! {
                                                                g {
                                                                    key: "hole-marker-{idx}",
                                                                    transform: "translate({marker.x} {marker.y}) rotate({marker.rotation_deg})",

                                                                    // Base outline.
                                                                    if matches!(marker.base, DrillBaseShape::Circle) {
                                                                        circle {
                                                                            cx: "0",
                                                                            cy: "0",
                                                                            r: "{r}",
                                                                            fill: if matches!(marker.modifier, DrillModifier::Filled) { "currentColor" } else { "none" },
                                                                            class: "{symbol_class}",
                                                                            stroke_width: "{stroke_width}",
                                                                        }
                                                                    }
                                                                    if matches!(marker.base, DrillBaseShape::Square) {
                                                                        rect {
                                                                            x: "{-r * 0.95}",
                                                                            y: "{-r * 0.95}",
                                                                            width: "{r * 1.9}",
                                                                            height: "{r * 1.9}",
                                                                            fill: if matches!(marker.modifier, DrillModifier::Filled) { "currentColor" } else { "none" },
                                                                            class: "{symbol_class}",
                                                                            stroke_width: "{stroke_width}",
                                                                        }
                                                                    }
                                                                    if matches!(marker.base, DrillBaseShape::Diamond) {
                                                                        polygon {
                                                                            points: "0 {-r}, {r} 0, 0 {r}, {-r} 0",
                                                                            fill: if matches!(marker.modifier, DrillModifier::Filled) { "currentColor" } else { "none" },
                                                                            class: "{symbol_class}",
                                                                            stroke_width: "{stroke_width}",
                                                                        }
                                                                    }
                                                                    if matches!(marker.base, DrillBaseShape::Triangle) {
                                                                        polygon {
                                                                            points: "0 {-r}, {r} {r * 0.85}, {-r} {r * 0.85}",
                                                                            fill: if matches!(marker.modifier, DrillModifier::Filled) { "currentColor" } else { "none" },
                                                                            class: "{symbol_class}",
                                                                            stroke_width: "{stroke_width}",
                                                                        }
                                                                    }
                                                                    if matches!(marker.base, DrillBaseShape::Hexagon) {
                                                                        polygon {
                                                                            points: "0 {-r}, {r * 0.83} {-r * 0.48}, {r * 0.83} {r * 0.48}, 0 {r}, {-r * 0.83} {r * 0.48}, {-r * 0.83} {-r * 0.48}",
                                                                            fill: if matches!(marker.modifier, DrillModifier::Filled) { "currentColor" } else { "none" },
                                                                            class: "{symbol_class}",
                                                                            stroke_width: "{stroke_width}",
                                                                        }
                                                                    }

                                                                    // Interior modifier.
                                                                    if matches!(marker.modifier, DrillModifier::Dot) {
                                                                        circle {
                                                                            cx: "0",
                                                                            cy: "0",
                                                                            r: "{r * (10.0 / 42.0)}",
                                                                            class: "{symbol_class}",
                                                                            fill: "currentColor",
                                                                        }
                                                                    }
                                                                    if matches!(marker.modifier, DrillModifier::Plus) {
                                                                        line {
                                                                            x1: "0",
                                                                            y1: "{-r * 0.75}",
                                                                            x2: "0",
                                                                            y2: "{r * 0.75}",
                                                                            class: "{symbol_class}",
                                                                            stroke_width: "{stroke_width}",
                                                                        }
                                                                        line {
                                                                            x1: "{-r * 0.75}",
                                                                            y1: "0",
                                                                            x2: "{r * 0.75}",
                                                                            y2: "0",
                                                                            class: "{symbol_class}",
                                                                            stroke_width: "{stroke_width}",
                                                                        }
                                                                    }
                                                                    if matches!(marker.modifier, DrillModifier::X) {
                                                                        line {
                                                                            x1: "{-r * 0.66}",
                                                                            y1: "{-r * 0.66}",
                                                                            x2: "{r * 0.66}",
                                                                            y2: "{r * 0.66}",
                                                                            class: "{symbol_class}",
                                                                            stroke_width: "{stroke_width}",
                                                                        }
                                                                        line {
                                                                            x1: "{-r * 0.66}",
                                                                            y1: "{r * 0.66}",
                                                                            x2: "{r * 0.66}",
                                                                            y2: "{-r * 0.66}",
                                                                            class: "{symbol_class}",
                                                                            stroke_width: "{stroke_width}",
                                                                        }
                                                                    }
                                                                    if matches!(marker.modifier, DrillModifier::Bullseye) {
                                                                        circle {
                                                                            cx: "0",
                                                                            cy: "0",
                                                                            r: "{r * (16.0 / 42.0)}",
                                                                            fill: "none",
                                                                            class: "{symbol_class}",
                                                                            stroke_width: "{stroke_width}",
                                                                        }
                                                                    }
                                                                    if matches!(marker.modifier, DrillModifier::HalfFill) {
                                                                        rect {
                                                                            x: "{-half_fill_w}",
                                                                            y: "{-r}",
                                                                            width: "{half_fill_w}",
                                                                            height: "{2.0 * r}",
                                                                            class: "{symbol_class}",
                                                                            fill: "currentColor",
                                                                            fill_opacity: "0.75",
                                                                        }
                                                                    }
                                                                    if matches!(marker.modifier, DrillModifier::QuarterFill) {
                                                                        rect {
                                                                            x: "{-quarter_fill_w}",
                                                                            y: "{-r}",
                                                                            width: "{quarter_fill_w}",
                                                                            height: "{quarter_fill_h}",
                                                                            class: "{symbol_class}",
                                                                            fill: "currentColor",
                                                                            fill_opacity: "0.75",
                                                                        }
                                                                    }

                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            aside { class: "board-drill-legend-panel",
                                                h4 { "Drill size legend" }
                                                if drill_size_legend.is_empty() {
                                                    p { class: "diag-status", "No drilled holes detected" }
                                                } else {
                                                    for (legend_idx , entry) in drill_size_legend.iter().enumerate() {
                                                        {
                                                            let r = 8.0_f64;
                                                            let sw = 1.2_f64;
                                                            rsx! {
                                                                div { key: "drill-legend-entry-{legend_idx}", class: "board-drill-legend-item",
                                                                    svg { class: "board-drill-legend-icon", view_box: "0 0 24 24",
                                                                        g { transform: "translate(12 12) rotate({entry.rotation_deg})",
                                                                            if matches!(entry.base, DrillBaseShape::Circle) {
                                                                                circle {
                                                                                    cx: "0",
                                                                                    cy: "0",
                                                                                    r: "{r}",
                                                                                    fill: if matches!(entry.modifier, DrillModifier::Filled) { "currentColor" } else { "none" },
                                                                                    class: "board-hole-cross board-hole-legend",
                                                                                    stroke_width: "{sw}",
                                                                                }
                                                                            }
                                                                            if matches!(entry.base, DrillBaseShape::Square) {
                                                                                rect {
                                                                                    x: "{-r * 0.95}",
                                                                                    y: "{-r * 0.95}",
                                                                                    width: "{r * 1.9}",
                                                                                    height: "{r * 1.9}",
                                                                                    fill: if matches!(entry.modifier, DrillModifier::Filled) { "currentColor" } else { "none" },
                                                                                    class: "board-hole-cross board-hole-legend",
                                                                                    stroke_width: "{sw}",
                                                                                }
                                                                            }
                                                                            if matches!(entry.base, DrillBaseShape::Diamond) {
                                                                                polygon {
                                                                                    points: "0 {-r}, {r} 0, 0 {r}, {-r} 0",
                                                                                    fill: if matches!(entry.modifier, DrillModifier::Filled) { "currentColor" } else { "none" },
                                                                                    class: "board-hole-cross board-hole-legend",
                                                                                    stroke_width: "{sw}",
                                                                                }
                                                                            }
                                                                            if matches!(entry.base, DrillBaseShape::Triangle) {
                                                                                polygon {
                                                                                    points: "0 {-r}, {r} {r * 0.85}, {-r} {r * 0.85}",
                                                                                    fill: if matches!(entry.modifier, DrillModifier::Filled) { "currentColor" } else { "none" },
                                                                                    class: "board-hole-cross board-hole-legend",
                                                                                    stroke_width: "{sw}",
                                                                                }
                                                                            }
                                                                            if matches!(entry.base, DrillBaseShape::Hexagon) {
                                                                                polygon {
                                                                                    points: "0 {-r}, {r * 0.83} {-r * 0.48}, {r * 0.83} {r * 0.48}, 0 {r}, {-r * 0.83} {r * 0.48}, {-r * 0.83} {-r * 0.48}",
                                                                                    fill: if matches!(entry.modifier, DrillModifier::Filled) { "currentColor" } else { "none" },
                                                                                    class: "board-hole-cross board-hole-legend",
                                                                                    stroke_width: "{sw}",
                                                                                }
                                                                            }
                                                                            if matches!(entry.modifier, DrillModifier::Dot) {
                                                                                circle {
                                                                                    cx: "0",
                                                                                    cy: "0",
                                                                                    r: "{r * (10.0 / 42.0)}",
                                                                                    class: "board-hole-legend",
                                                                                    fill: "currentColor",
                                                                                }
                                                                            }
                                                                            if matches!(entry.modifier, DrillModifier::Plus) {
                                                                                line {
                                                                                    x1: "0",
                                                                                    y1: "{-r * 0.75}",
                                                                                    x2: "0",
                                                                                    y2: "{r * 0.75}",
                                                                                    class: "board-hole-cross board-hole-legend",
                                                                                    stroke_width: "{sw}",
                                                                                }
                                                                                line {
                                                                                    x1: "{-r * 0.75}",
                                                                                    y1: "0",
                                                                                    x2: "{r * 0.75}",
                                                                                    y2: "0",
                                                                                    class: "board-hole-cross board-hole-legend",
                                                                                    stroke_width: "{sw}",
                                                                                }
                                                                            }
                                                                            if matches!(entry.modifier, DrillModifier::X) {
                                                                                line {
                                                                                    x1: "{-r * 0.66}",
                                                                                    y1: "{-r * 0.66}",
                                                                                    x2: "{r * 0.66}",
                                                                                    y2: "{r * 0.66}",
                                                                                    class: "board-hole-cross board-hole-legend",
                                                                                    stroke_width: "{sw}",
                                                                                }
                                                                                line {
                                                                                    x1: "{-r * 0.66}",
                                                                                    y1: "{r * 0.66}",
                                                                                    x2: "{r * 0.66}",
                                                                                    y2: "{-r * 0.66}",
                                                                                    class: "board-hole-cross board-hole-legend",
                                                                                    stroke_width: "{sw}",
                                                                                }
                                                                            }
                                                                            if matches!(entry.modifier, DrillModifier::Bullseye) {
                                                                                circle {
                                                                                    cx: "0",
                                                                                    cy: "0",
                                                                                    r: "{r * (16.0 / 42.0)}",
                                                                                    fill: "none",
                                                                                    class: "board-hole-cross board-hole-legend",
                                                                                    stroke_width: "{sw}",
                                                                                }
                                                                            }
                                                                            if matches!(entry.modifier, DrillModifier::HalfFill) {
                                                                                rect {
                                                                                    x: "{-r}",
                                                                                    y: "{-r}",
                                                                                    width: "{r}",
                                                                                    height: "{2.0 * r}",
                                                                                    class: "board-hole-legend",
                                                                                    fill: "currentColor",
                                                                                    fill_opacity: "0.75",
                                                                                }
                                                                            }
                                                                            if matches!(entry.modifier, DrillModifier::QuarterFill) {
                                                                                rect {
                                                                                    x: "{-r}",
                                                                                    y: "{-r}",
                                                                                    width: "{r}",
                                                                                    height: "{r}",
                                                                                    class: "board-hole-legend",
                                                                                    fill: "currentColor",
                                                                                    fill_opacity: "0.75",
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                    span {
                                                                        {
                                                                            unit_format::format_length_display(
                                                                                Length::from_mm(entry.diameter_mm),
                                                                                snapshot.unit_system,
                                                                            )
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                div { class: "board-drill-legend-note",
                                                    "Size classes are ordered by drill diameter and reuse symbol patterns after 120 combinations."
                                                }
                                                div { class: "board-drill-legend-note", "Hole type colors" }
                                                div { class: "board-drill-legend-item",
                                                    svg {
                                                        class: "board-drill-legend-icon",
                                                        view_box: "0 0 24 24",
                                                        circle {
                                                            cx: "12",
                                                            cy: "12",
                                                            r: "8",
                                                            fill: "none",
                                                            class: "board-hole-cross board-hole-via",
                                                            stroke_width: "1.8",
                                                        }
                                                    }
                                                    span { "Via" }
                                                }
                                                div { class: "board-drill-legend-item",
                                                    svg {
                                                        class: "board-drill-legend-icon",
                                                        view_box: "0 0 24 24",
                                                        circle {
                                                            cx: "12",
                                                            cy: "12",
                                                            r: "8",
                                                            fill: "none",
                                                            class: "board-hole-cross board-hole-pth",
                                                            stroke_width: "1.8",
                                                        }
                                                    }
                                                    span { "PTH" }
                                                }
                                                div { class: "board-drill-legend-item",
                                                    svg {
                                                        class: "board-drill-legend-icon",
                                                        view_box: "0 0 24 24",
                                                        circle {
                                                            cx: "12",
                                                            cy: "12",
                                                            r: "8",
                                                            fill: "none",
                                                            class: "board-hole-cross board-hole-npth",
                                                            stroke_width: "1.8",
                                                        }
                                                    }
                                                    span { "NPTH" }
                                                }
                                                div { class: "board-drill-legend-item",
                                                    svg { class: "board-legend-icon", view_box: "0 0 24 24",
                                                        path {
                                                            d: "M 3 12 L 9 4 L 21 4 L 21 20 L 3 20 Z",
                                                            class: "board-edge-shape",
                                                        }
                                                    }
                                                    span { "Edge cuts" }
                                                }
                                            }
                                        }
                                        p { "Board edge shapes: {board.edge_shapes.len()} · Holes: {board.holes.len()}" }
                                    } else {
                                        div { class: "canvas-mock", "Board bounding box unavailable" }
                                        p { "Open a board in KiCad to render the board graph." }
                                    }
                                } else {
                                    div { class: "canvas-mock", "Board snapshot unavailable" }
                                    p { "Click 'Refresh Board Snapshot' while a PCB is open in KiCad." }
                                }
                            }
    }
}
