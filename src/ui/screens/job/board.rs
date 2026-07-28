//! Job "Board" view — the PCB visualization: renders the active board's edge
//! graph, drill holes and routed features as pan/zoom-able SVG, with a legend.
//! Reads the active board snapshot; all view state (zoom, pan, selected document)
//! is local to this component.
//!
//! **Drilled vs routed.** A round hole is made by plunging a drill, so it is drawn
//! as a symbol keyed to its size class. Everything a *router* makes — an oblong
//! slot, the board outline — removes a swept band of material, so it is drawn as
//! that band, hatched: the hatch is the cutting tool path, and its width is the
//! feature's own width (the slot width; a fixed nominal kerf for the outline,
//! whose router is a per-step choice this view cannot see).

use dioxus::prelude::*;
use std::path::Path;

use pcb::{BoardEdgeShape, BoardSnapshot, Contour, HoleKind};
use units::Length;

use crate::runtime::tooling::{step_targets, StepTargets};
use crate::runtime::AppCtx;
use units::user_format as unit_format;

/// Nominal kerf width drawn for the board-outline route, in mm.
///
/// The outline router is a per-step tool choice this view has no access to — it renders
/// raw board geometry, not a machining plan — so the outside-route band uses a fixed
/// nominal width (2 mm is the usual PCB outline cutter). It states "this edge is
/// routed", not "this exact tool cuts it".
const OUTLINE_ROUTE_WIDTH_MM: f64 = 2.0;

/// Smallest drill-marker radius, in view units at zoom 1 (~2 screen px). Only so a hole
/// that would land under a pixel still leaves a mark — it is deliberately tiny, because
/// markers are drawn **true to size**: a 0.3 mm via should read as smaller than the
/// 0.8 mm pad beside it, not be inflated to match it.
const MIN_MARKER_RADIUS_UNITS: f64 = 2.0;

/// Largest drill-marker radius, so one oversize drill cannot swamp the render.
const MAX_MARKER_RADIUS_UNITS: f64 = 28.0;

/// Hatch line pitch expressed in board mm, so the texture keeps a constant physical
/// scale whatever the board's size.
const HATCH_PITCH_MM: f64 = 0.35;

/// View-unit floor on the hatch pitch: a very large board would otherwise hatch below a
/// pixel and read as a flat tint.
const HATCH_PITCH_MIN_UNITS: f64 = 1.2;

/// View-unit ceiling on the hatch pitch: a very small board would otherwise hatch
/// coarser than its own slots.
const HATCH_PITCH_MAX_UNITS: f64 = 6.0;

/// Hatch pitch inside the 24×24 legend swatches, which have their own user space.
const HATCH_PITCH_LEGEND: f64 = 3.0;

/// The hole kinds, as the slug used to build their per-kind hatch pattern ids and CSS
/// classes. One list keeps the pattern defs, the band fills and the legend swatches in
/// lockstep — a routed slot hatches in its own PTH/NPTH/via colour, exactly as a drilled
/// hole's symbol does.
const HOLE_KIND_SLUGS: [&str; 3] = ["via", "pth", "npth"];

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
    /// Whether the selected machining step makes this feature. `false` draws it ghosted:
    /// still there, because a board is unreadable without its own geometry, but plainly
    /// not this step's work.
    machined: bool,
}

#[derive(Clone)]
struct DrillLegendEntry {
    diameter_mm: f64,
    base: DrillBaseShape,
    modifier: DrillModifier,
    rotation_deg: f64,
}

/// A routed oblong hole ("slot"), pre-resolved into view space.
///
/// A slot is milled, not drilled: the cutter's centre sweeps the long axis between the
/// two end centres, and the material it removes is exactly the stadium of the slot
/// outline. The view therefore hatches that stadium — the hatched band *is* the cutting
/// tool path, and its width is the slot width by construction.
#[derive(Clone)]
struct BoardSlotFeature {
    /// Slot centre, in view units.
    x: f64,
    y: f64,
    /// SVG rotation (degrees) laying the slot's long axis on the local +X axis.
    rotation_deg: f64,
    /// Half the cutter-centre travel along the long axis, in view units.
    half_travel: f64,
    /// The stadium outline, in the slot's own (unrotated) frame.
    outline_path: String,
    /// Hole kind, for the boundary colour.
    kind: HoleKind,
    /// See [`BoardHoleMarker::machined`].
    machined: bool,
}

/// One distinct slot size present on the board, for the legend. Keyed by kind as well
/// as size: the hatch is kind-coloured, so the same size plated and unplated are two
/// visually different things and each earns its own key entry.
#[derive(Clone)]
struct SlotLegendEntry {
    length_mm: f64,
    width_mm: f64,
    kind: HoleKind,
}

/// Everything the SVG render needs from the board's holes, resolved into view units.
/// Both legends are built from the same classification as the markers, so the picture
/// and the key can never disagree.
#[derive(Default)]
struct BoardFeatures {
    /// Round drilled holes, as symbol markers.
    holes: Vec<BoardHoleMarker>,
    /// Oblong holes, as routed (hatched) slot bands.
    slots: Vec<BoardSlotFeature>,
    /// Distinct round-drill diameters, in symbol-class order.
    drill_legend: Vec<DrillLegendEntry>,
    /// Distinct slot sizes, smallest first.
    slot_legend: Vec<SlotLegendEntry>,
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

/// The colour-only class for a hole kind. Routed slots bring their own stroke geometry
/// (a thin non-scaling boundary), so they take the kind colour without the marker
/// stroke rules that [`hole_marker_class`] carries.
fn hole_kind_class(kind: &HoleKind) -> &'static str {
    match kind {
        HoleKind::Via => "board-hole-via",
        HoleKind::PadPth => "board-hole-pth",
        HoleKind::PadNpth => "board-hole-npth",
    }
}

/// The [`HOLE_KIND_SLUGS`] entry for a kind — the stem of its hatch pattern id and band
/// class.
fn hole_kind_slug(kind: &HoleKind) -> &'static str {
    match kind {
        HoleKind::Via => HOLE_KIND_SLUGS[0],
        HoleKind::PadPth => HOLE_KIND_SLUGS[1],
        HoleKind::PadNpth => HOLE_KIND_SLUGS[2],
    }
}

/// The stadium swept by a cutter of width `2 * half_width` travelling `2 * half_travel`
/// along the local +X axis — that is, the slot outline in the slot's own frame. Both
/// arcs sweep the same way round so the path closes as one convex region.
fn stadium_path(half_travel: f64, half_width: f64) -> String {
    let (h, r) = (half_travel, half_width);
    let (nh, nr) = (-half_travel, -half_width);
    format!("M {nh} {nr} L {h} {nr} A {r} {r} 0 0 1 {h} {r} L {nh} {r} A {r} {r} 0 0 1 {nh} {nr} Z")
}

/// Resolves the board's holes into view-space render features: round holes become drill
/// symbols keyed by a size class, oblong holes become routed slot bands.
///
/// `zoom` only feeds the drill-symbol legibility floor; slot bands stay geometrically
/// true at every zoom, since their whole point is showing the real swept area.
/// Whether the selected step makes `hole`. No targets means no step to filter by — every
/// feature is drawn at full strength, which is what a board with no machining profile
/// selected should look like.
fn machines(targets: Option<&StepTargets>, hole: &pcb::BoardHole) -> bool {
    targets.map(|t| t.machines(hole)).unwrap_or(true)
}

fn resolve_board_features(
    board: &BoardSnapshot,
    view_width: f64,
    view_height: f64,
    zoom: f64,
    targets: Option<&StepTargets>,
) -> BoardFeatures {
    let Some(bbox) = board.bounding_box.as_ref() else {
        return BoardFeatures::default();
    };
    let (min_x, min_y) = (bbox.x.as_mm(), bbox.y.as_mm());
    let (width, height) = (bbox.width.as_mm(), bbox.height.as_mm());
    if width <= 0.0 || height <= 0.0 {
        return BoardFeatures::default();
    }
    let units_per_mm = view_width / width;

    // Size classes cover *drilled* holes only — a slot is milled to size, so it carries
    // no drill symbol and gets its own legend below.
    let mut drill_size_classes = board
        .holes
        .iter()
        .filter(|hole| hole.slot().is_none())
        .filter_map(|hole| hole.drill_axes())
        .map(|(major, _)| major.as_mm())
        .collect::<Vec<_>>();
    drill_size_classes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    drill_size_classes.dedup_by(|a, b| (*a - *b).abs() < 1e-6);

    let mut features = BoardFeatures {
        drill_legend: drill_size_classes
            .iter()
            .enumerate()
            .map(|(class_idx, diameter_mm)| {
                let (base, modifier, rotation_deg) = drill_symbol_from_index(class_idx);
                DrillLegendEntry { diameter_mm: *diameter_mm, base, modifier, rotation_deg }
            })
            .collect(),
        ..BoardFeatures::default()
    };

    // The floor shrinks as you zoom in, so zooming always reveals true relative sizes.
    let min_marker_radius = MIN_MARKER_RADIUS_UNITS / zoom.max(1.0);

    for hole in &board.holes {
        let x = ((hole.position.x.as_mm() - min_x) / width).clamp(0.0, 1.0) * view_width;
        let y = ((hole.position.y.as_mm() - min_y) / height).clamp(0.0, 1.0) * view_height;
        // A milled slot: the band is the material the cutter sweeps. `Slot` carries the
        // board-frame axis angle, which in this Y-down view is directly an SVG rotation.
        if let Some(slot) = hole.slot() {
            let (length_mm, width_mm) = (slot.length.as_mm(), slot.width.as_mm());
            let half_width = width_mm * 0.5 * units_per_mm;
            let half_travel = slot.travel().as_mm() * 0.5 * units_per_mm;
            features.slots.push(BoardSlotFeature {
                x,
                y,
                rotation_deg: slot.angle_deg,
                half_travel,
                outline_path: stadium_path(half_travel, half_width),
                kind: hole.kind.clone(),
                machined: machines(targets, hole),
            });
            if !features.slot_legend.iter().any(|entry| {
                (entry.length_mm - length_mm).abs() < 1e-6
                    && (entry.width_mm - width_mm).abs() < 1e-6
                    && entry.kind == hole.kind
            }) {
                features.slot_legend.push(SlotLegendEntry {
                    length_mm,
                    width_mm,
                    kind: hole.kind.clone(),
                });
            }
            continue;
        }

        // A hole with no drill data at all still gets a marker, at a token size.
        let major = hole.drill_axes().map(|(major, _)| major.as_mm()).unwrap_or(0.1);
        let hole_diameter = major.max(0.05);
        let marker_radius = ((hole_diameter / width) * view_width * 0.5)
            .max(min_marker_radius)
            .min(MAX_MARKER_RADIUS_UNITS);
        let class_idx = drill_size_classes
            .iter()
            .position(|d| (*d - hole_diameter).abs() < 1e-6)
            .unwrap_or(0);
        let (base, modifier, rotation_deg) = drill_symbol_from_index(class_idx);
        features.holes.push(BoardHoleMarker {
            x,
            y,
            marker_radius,
            rotation_deg,
            kind: hole.kind.clone(),
            base,
            modifier,
            machined: machines(targets, hole),
        });
    }

    features.slot_legend.sort_by(|a, b| {
        a.length_mm
            .partial_cmp(&b.length_mm)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.width_mm.partial_cmp(&b.width_mm).unwrap_or(std::cmp::Ordering::Equal))
    });
    features
}

/// One hatch pattern, tiled in the user space of whichever SVG references it — so the
/// board render and the legend swatches each get a tile sized for their own coordinate
/// system.
///
/// `colour_class` goes on the `<pattern>` itself and only needs to set `color`: pattern
/// content inherits from the pattern element, and the hatch lines paint with
/// `currentColor`. That lets the existing hole-kind colour classes drive the hatch
/// directly, so a routed slot is hatched in its own PTH/NPTH/via colour.
///
/// `cross` adds the perpendicular family. Slots hatch in one direction (the tile rotates
/// with the slot, so it can never align with the band), but the outline band follows
/// arbitrary edge angles — a single family would run lengthwise along a 45° chamfer and
/// stop reading as a hatch, so the outline cross-hatches.
fn hatch_pattern(
    id: &str,
    colour_class: &str,
    pitch: f64,
    line_width: f64,
    cross: bool,
) -> Element {
    rsx! {
        pattern {
            id: "{id}",
            class: "{colour_class}",
            pattern_units: "userSpaceOnUse",
            width: "{pitch}",
            height: "{pitch}",
            pattern_transform: "rotate(45)",
            line {
                x1: "0",
                y1: "0",
                x2: "0",
                y2: "{pitch}",
                class: "board-hatch-line",
                stroke_width: "{line_width}",
            }
            if cross {
                line {
                    x1: "0",
                    y1: "0",
                    x2: "{pitch}",
                    y2: "0",
                    class: "board-hatch-line",
                    stroke_width: "{line_width}",
                }
            }
        }
    }
}

/// The stitched board outline as one SVG path, every contour a closed subpath, in view
/// units. `None` when the stitcher has not produced a usable result.
///
/// This is what makes the outside-route band possible: the raw `edge_shapes` are
/// unordered fragments, so there is no "inside" to offset away from, whereas a stitched
/// contour is a closed, nested loop. The tessellated polyline is enough here — the true
/// arcs still draw the nominal cut line on top.
fn stitched_outline_path(
    contours: &[Contour],
    to_view: impl Fn(f64, f64) -> (f64, f64),
) -> Option<String> {
    let mut path = String::new();
    for contour in contours {
        let mut points = contour.points.iter();
        let Some((first_x, first_y)) = points.next() else { continue };
        let (x, y) = to_view(*first_x as f64 / 1e6, *first_y as f64 / 1e6);
        path.push_str(&format!("M {x} {y}"));
        for (px, py) in points {
            let (x, y) = to_view(*px as f64 / 1e6, *py as f64 / 1e6);
            path.push_str(&format!(" L {x} {y}"));
        }
        path.push_str(" Z ");
    }
    (!path.is_empty()).then_some(path)
}

/// Renders one edge-shape primitive. Called twice per shape: once as the hatched
/// outside-route band (an explicit `stroke_width`, in view units so it scales with the
/// geometry), then as the nominal cut line on top (`None` — its class carries a
/// non-scaling stroke width).
fn edge_shape_element(shape: &SvgShape, class: &str, stroke_width: Option<f64>) -> Element {
    match shape {
        SvgShape::Line { x1, y1, x2, y2 } => rsx! {
            line {
                x1: "{x1}",
                y1: "{y1}",
                x2: "{x2}",
                y2: "{y2}",
                class: "{class}",
                stroke_width: stroke_width,
            }
        },
        SvgShape::Path(d) => rsx! {
            path { d: "{d}", class: "{class}", stroke_width: stroke_width }
        },
        SvgShape::Rect { x, y, w, h, rx } => rsx! {
            rect {
                x: "{x}",
                y: "{y}",
                width: "{w}",
                height: "{h}",
                rx: "{rx}",
                class: "{class}",
                stroke_width: stroke_width,
            }
        },
        SvgShape::Circle { cx, cy, r } => rsx! {
            circle { cx: "{cx}", cy: "{cy}", r: "{r}", class: "{class}", stroke_width: stroke_width }
        },
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
    // What the selected step actually machines. `None` — no profile, or no such step —
    // draws the whole board at full strength, which is the right picture when there is no
    // step to be showing.
    let step_targets = step_targets(&snapshot, snapshot.selected_step);
    let routes_outline = step_targets.map(|t| t.outline).unwrap_or(true);
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

    // View units per board mm. `tx`/`ty` scale both axes by the same factor (the view
    // height is the width times the board aspect), so one factor serves both — it turns
    // physical widths (hatch pitch, the outline kerf) into view units.
    let units_per_mm = snapshot
        .board
        .as_ref()
        .and_then(|board| board.bounding_box.as_ref())
        .map(|bbox| bbox.width.as_mm())
        .filter(|width| *width > 0.0)
        .map(|width| board_view_width / width)
        .unwrap_or(1.0);
    let hatch_pitch =
        (HATCH_PITCH_MM * units_per_mm).clamp(HATCH_PITCH_MIN_UNITS, HATCH_PITCH_MAX_UNITS);
    let hatch_line_width = hatch_pitch * 0.4;
    let outline_band_width = OUTLINE_ROUTE_WIDTH_MM * units_per_mm;

    // The outside-route band lies wholly beyond the edge cut, so the drawing is the
    // board's bounds grown by one kerf on every side. Pan/zoom work over this content
    // box, not the bare board box, or the band would be clipped at full extent.
    let content_x = -outline_band_width;
    let content_y = -outline_band_width;
    let content_w = board_view_width + 2.0 * outline_band_width;
    let content_h = board_view_height + 2.0 * outline_band_width;

    let viewport_w = (content_w / zoom_value).clamp(10.0, content_w);
    let viewport_h = (content_h / zoom_value).clamp(10.0, content_h);
    let max_pan_x = (content_w - viewport_w).max(0.0);
    let max_pan_y = (content_h - viewport_h).max(0.0);
    // Pan is stored relative to the content box's top-left, so the offset only enters
    // when the viewBox is written.
    let pan_x_clamped = pan_x_value.clamp(0.0, max_pan_x);
    let pan_y_clamped = pan_y_value.clamp(0.0, max_pan_y);
    let view_x = content_x + pan_x_clamped;
    let view_y = content_y + pan_y_clamped;
    let board_view_box = format!("{view_x} {view_y} {viewport_w} {viewport_h}");
    let zoom_percent = (zoom_value * 100.0).round() as i32;
    let features = snapshot
        .board
        .as_ref()
        .map(|board| {
            resolve_board_features(
                board,
                board_view_width,
                board_view_height,
                zoom_value,
                step_targets.as_ref(),
            )
        })
        .unwrap_or_default();
    let board_hole_markers = &features.holes;
    let board_slot_features = &features.slots;
    let drill_size_legend = &features.drill_legend;
    let slot_size_legend = &features.slot_legend;

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

    // The stitched contours, in view units — the source for the outside-route band.
    // Only a clean stitch is usable: with errors the contours are not closed, so there
    // is no reliable inside to keep the band out of.
    let stitched_outline = snapshot
        .stitched_board_data
        .as_ref()
        .filter(|stitched| stitched.errors.is_empty())
        .zip(snapshot.board.as_ref().and_then(|b| b.bounding_box.as_ref()))
        .filter(|(_, bbox)| bbox.width.as_mm() > 0.0 && bbox.height.as_mm() > 0.0)
        .and_then(|(stitched, bbox)| {
            let min_x = bbox.x.as_mm();
            let min_y = bbox.y.as_mm();
            let width = bbox.width.as_mm();
            let height = bbox.height.as_mm();
            stitched_outline_path(&stitched.contours, |px, py| {
                (
                    ((px - min_x) / width) * board_view_width,
                    ((py - min_y) / height) * board_view_height,
                )
            })
        });

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
                                            span { class: "board-view-status",
                                                "{board_hole_markers.len()} drilled · {board_slot_features.len()} routed slots · {board.edge_shapes.len()} edges"
                                            }
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
                                                    let unit_per_px_x = viewport_w / content_w;
                                                    let unit_per_px_y = viewport_h / content_h;

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

                                                    // Keep the viewport's centre fixed across the zoom. Pan is
                                                    // content-box-relative, so the centre is too.
                                                    let old_vw = (content_w / old_zoom).clamp(10.0, content_w);
                                                    let old_vh = (content_h / old_zoom).clamp(10.0, content_h);
                                                    let new_vw = (content_w / new_zoom).clamp(10.0, content_w);
                                                    let new_vh = (content_h / new_zoom).clamp(10.0, content_h);
                                                    let center_x = pan_x_clamped + old_vw * 0.5;
                                                    let center_y = pan_y_clamped + old_vh * 0.5;
                                                    let new_max_pan_x = (content_w - new_vw).max(0.0);
                                                    let new_max_pan_y = (content_h - new_vh).max(0.0);
                                                    board_zoom.set(new_zoom);
                                                    board_pan_x.set((center_x - new_vw * 0.5).clamp(0.0, new_max_pan_x));
                                                    board_pan_y.set((center_y - new_vh * 0.5).clamp(0.0, new_max_pan_y));
                                                },
                                                svg {
                                                    class: "board-svg",
                                                    view_box: "{board_view_box}",
                                                    preserve_aspect_ratio: "xMidYMid meet",

                                                    // Hatch textures for the routed bands: one per hole kind, so a
                                                    // routed slot hatches in the same colour its drilled symbol
                                                    // would carry. Board-scale tiles are pitched in board mm; the
                                                    // `-legend` tiles are pitched for the 24×24 swatch user space
                                                    // (pattern ids resolve document-wide).
                                                    defs {
                                                        for slug in HOLE_KIND_SLUGS {
                                                            {hatch_pattern(&format!("board-route-hatch-{slug}"), &format!("board-hole-{slug}"), hatch_pitch, hatch_line_width, false)}
                                                        }
                                                        for slug in HOLE_KIND_SLUGS {
                                                            {hatch_pattern(&format!("board-route-hatch-{slug}-legend"), &format!("board-hole-{slug}"), HATCH_PITCH_LEGEND, HATCH_PITCH_LEGEND * 0.4, false)}
                                                        }
                                                        {hatch_pattern("board-outline-hatch", "board-hatch-outline", hatch_pitch, hatch_line_width, true)}
                                                        {hatch_pattern("board-outline-hatch-legend", "board-hatch-outline", HATCH_PITCH_LEGEND, HATCH_PITCH_LEGEND * 0.4, true)}

                                                        // Keeps the outside-route band out of the board: white is
                                                        // kept, and the stitched material region is painted black.
                                                        // Stroking the contours at twice the kerf then masking the
                                                        // inner half leaves exactly one kerf outside the edge cut —
                                                        // and it falls out the right way for interior cut-outs too,
                                                        // where "outside the board" is inside the opening.
                                                        if let Some(outline) = stitched_outline.as_ref() {
                                                            mask {
                                                                id: "board-outside-route-mask",
                                                                mask_units: "userSpaceOnUse",
                                                                x: "{content_x}",
                                                                y: "{content_y}",
                                                                width: "{content_w}",
                                                                height: "{content_h}",
                                                                rect {
                                                                    x: "{content_x}",
                                                                    y: "{content_y}",
                                                                    width: "{content_w}",
                                                                    height: "{content_h}",
                                                                    fill: "white",
                                                                }
                                                                path { d: "{outline}", fill: "black", fill_rule: "evenodd", stroke: "none" }
                                                            }
                                                        }
                                                    }

                                                    rect {
                                                        x: "0",
                                                        y: "0",
                                                        width: "{board_view_width}",
                                                        height: "{board_view_height}",
                                                        class: "board-svg-frame",
                                                    }

                                                    // Outside routing: the kerf the outline cutter sweeps. It lies
                                                    // wholly beyond the edge cut, so the finished board keeps its
                                                    // nominal size. Without a clean stitch there is no inside to
                                                    // keep out of, so fall back to a band centred on the raw edge
                                                    // fragments — still "this edge is routed", just unsided.
                                                    // Ghosted when the selected step does not cut the outline: the
                                                    // kerf is drawn so the board still reads as a board, but it is
                                                    // plainly another step's work.
                                                    // The ghost class goes on a wrapping group, never beside
                                                    // `board-outline-band` on the band itself: that class sets its
                                                    // own `opacity`, and two single-class rules have equal
                                                    // specificity, so whichever the sheet declares last wins and
                                                    // the ghost is silently ignored. The slot and marker groups
                                                    // below take this shape for the same reason.
                                                    g { class: if routes_outline { "" } else { "board-step-ghost" },
                                                        if let Some(outline) = stitched_outline.as_ref() {
                                                            {
                                                                let double_kerf = outline_band_width * 2.0;
                                                                rsx! {
                                                                    path {
                                                                        d: "{outline}",
                                                                        class: "board-outline-band",
                                                                        stroke_width: "{double_kerf}",
                                                                        mask: "url(#board-outside-route-mask)",
                                                                    }
                                                                }
                                                            }
                                                        } else {
                                                            for shape in board_edge_shapes_svg.iter() {
                                                                {edge_shape_element(
                                                                    shape,
                                                                    "board-outline-band",
                                                                    Some(outline_band_width),
                                                                )}
                                                            }
                                                        }
                                                    }
                                                    for shape in board_edge_shapes_svg.iter() {
                                                        {edge_shape_element(shape, "board-edge-shape", None)}
                                                    }

                                                    // Routed slots: the hatched stadium is the swept material,
                                                    // the dashed centreline the cutter's own path through it.
                                                    for (idx , slot) in board_slot_features.iter().enumerate() {
                                                        {
                                                            let kind_class = hole_kind_class(&slot.kind);
                                                            let band_class = format!("board-route-band-{}", hole_kind_slug(&slot.kind));
                                                            let travel_start = -slot.half_travel;
                                                            let travel_end = slot.half_travel;
                                                            rsx! {
                                                                g {
                                                                    key: "board-slot-{idx}",
                                                                    class: if slot.machined { "" } else { "board-step-ghost" },
                                                                    transform: "translate({slot.x} {slot.y}) rotate({slot.rotation_deg})",

                                                                    path { d: "{slot.outline_path}", class: "{band_class}" }
                                                                    path {
                                                                        d: "{slot.outline_path}",
                                                                        class: "board-route-outline {kind_class}",
                                                                    }
                                                                    if slot.half_travel > 0.0 {
                                                                        line {
                                                                            x1: "{travel_start}",
                                                                            y1: "0",
                                                                            x2: "{travel_end}",
                                                                            y2: "0",
                                                                            class: "board-route-centerline {kind_class}",
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }

                                                    for (idx , marker) in board_hole_markers.iter().enumerate() {
                                                        {
                                                            let r = marker.marker_radius;
                                                            let stroke_width = 1.0_f64;
                                                            let symbol_class = hole_marker_class(&marker.kind);
                                                            let half_fill_w = r;
                                                            let quarter_fill_w = r;
                                                            let quarter_fill_h = r;
                                                            rsx! {
                                                                g {
                                                                    key: "hole-marker-{idx}",
                                                                    class: if marker.machined { "" } else { "board-step-ghost" },
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
                                                    p { class: "diag-status", "No round drilled holes detected" }
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
                                                    "Size classes cover round drilled holes only; they are ordered by diameter and reuse symbol patterns after 120 combinations."
                                                }

                                                h4 { "Routed features" }
                                                div { class: "board-drill-legend-note",
                                                    "Hatched bands are material a router removes — the hatch is the cutting tool path, in the feature's own hole-type colour."
                                                }
                                                if slot_size_legend.is_empty() {
                                                    p { class: "diag-status", "No oblong slots detected" }
                                                } else {
                                                    for (slot_idx , entry) in slot_size_legend.iter().enumerate() {
                                                        {
                                                            // A swatch at the entry's own aspect, clamped so a long
                                                            // slot still fits the 24×24 icon.
                                                            let half_width = 4.0_f64;
                                                            let half_travel = (half_width
                                                                * (entry.length_mm / entry.width_mm.max(1e-6) - 1.0))
                                                                .clamp(0.0, 7.0);
                                                            let outline = stadium_path(half_travel, half_width);
                                                            let slug = hole_kind_slug(&entry.kind);
                                                            let band_class = format!("board-route-band-{slug}-legend");
                                                            let kind_class = hole_kind_class(&entry.kind);
                                                            let kind_label = slug.to_uppercase();
                                                            let length_text = unit_format::format_length_display(
                                                                Length::from_mm(entry.length_mm),
                                                                snapshot.unit_system,
                                                            );
                                                            let width_text = unit_format::format_length_display(
                                                                Length::from_mm(entry.width_mm),
                                                                snapshot.unit_system,
                                                            );
                                                            rsx! {
                                                                div {
                                                                    key: "slot-legend-entry-{slot_idx}",
                                                                    class: "board-drill-legend-item",
                                                                    svg { class: "board-drill-legend-icon", view_box: "0 0 24 24",
                                                                        g { transform: "translate(12 12)",
                                                                            path { d: "{outline}", class: "{band_class}" }
                                                                            path {
                                                                                d: "{outline}",
                                                                                class: "board-route-outline {kind_class}",
                                                                            }
                                                                        }
                                                                    }
                                                                    span { "{kind_label} slot {length_text} × {width_text}" }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                div { class: "board-drill-legend-item",
                                                    svg { class: "board-drill-legend-icon", view_box: "0 0 24 24",
                                                        // The kerf sits wholly on one side of the cut line, as it
                                                        // does on the board.
                                                        path {
                                                            d: "M 2 9 L 22 9",
                                                            class: "board-outline-band-legend",
                                                            stroke_width: "6",
                                                        }
                                                        path { d: "M 2 12 L 22 12", class: "board-edge-shape" }
                                                    }
                                                    span { "Outside route ({OUTLINE_ROUTE_WIDTH_MM} mm nominal, outside the edge)" }
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
                                                    span { "Edge cut line" }
                                                }
                                            }
                                        }
                                        p {
                                            "Board edge shapes: {board.edge_shapes.len()} · Drilled holes: {board_hole_markers.len()} · Routed slots: {board_slot_features.len()}"
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcb::{BoardHole, BoardPoint};

    /// A hole at the origin with the given drill axes and pad angle.
    fn hole(drill_x_mm: f64, drill_y_mm: f64, angle_deg: Option<f64>) -> BoardHole {
        BoardHole {
            id: None,
            kind: HoleKind::PadPth,
            position: BoardPoint { x: Length::from_mm(0.0), y: Length::from_mm(0.0) },
            drill_x: Some(Length::from_mm(drill_x_mm)),
            drill_y: Some(Length::from_mm(drill_y_mm)),
            plated: Some(true),
            orientation_deg: angle_deg,
        }
    }

    /// The hatched band is the material the cutter sweeps: as long as the slot and
    /// exactly as wide, so the "hatch width == slot width" contract holds by geometry.
    #[test]
    fn the_stadium_band_spans_the_whole_slot() {
        // 3.2 × 1.6 mm slot at 10 view units/mm → half-width 8, half-travel 8.
        let half_width = 1.6 * 0.5 * 10.0;
        let half_travel = (3.2 - 1.6) * 0.5 * 10.0;
        assert_eq!(
            stadium_path(half_travel, half_width),
            "M -8 -8 L 8 -8 A 8 8 0 0 1 8 8 L -8 8 A 8 8 0 0 1 -8 -8 Z"
        );
        // Overall extents: length = 2*travel + 2*radius = 32 units = 3.2 mm; the band's
        // thickness = 2*radius = 16 units = 1.6 mm.
        assert!((2.0 * half_travel + 2.0 * half_width - 32.0).abs() < 1e-9);
        assert!((2.0 * half_width - 16.0).abs() < 1e-9);
    }

    /// Every contour becomes one closed subpath in view units. Closing each subpath is
    /// what lets the mask fill the material region with an even-odd rule, which is in
    /// turn what keeps the outside-route band out of the board.
    #[test]
    fn each_stitched_contour_becomes_one_closed_subpath() {
        // 1 mm square at the bbox origin; the transform below is 10 view units per mm.
        let square = Contour {
            points: vec![(0, 0), (1_000_000, 0), (1_000_000, 1_000_000), (0, 1_000_000)],
            segments: Vec::new(),
            is_hole: false,
        };
        let path = stitched_outline_path(&[square.clone(), square], |x, y| (x * 10.0, y * 10.0))
            .expect("two contours produce a path");
        assert_eq!(path.matches('M').count(), 2, "one subpath per contour");
        assert_eq!(path.matches('Z').count(), 2, "each subpath is closed");
        assert!(path.starts_with("M 0 0 L 10 0 L 10 10 L 0 10 Z"), "scaled to view units: {path}");
    }

    /// No contours (an unstitchable board) yields no path, so the caller falls back to
    /// the unsided band instead of emitting an empty mask.
    #[test]
    fn an_empty_stitch_yields_no_outline_path() {
        assert!(stitched_outline_path(&[], |x, y| (x, y)).is_none());
    }

    /// Slots never reach the drill legend, and round holes never reach the slot legend —
    /// the two keys partition the board's holes.
    #[test]
    fn the_two_legends_partition_the_holes() {
        let board = BoardSnapshot {
            name: "t".into(),
            thickness: None,
            bounding_box: Some(pcb::BoardBoundingBox {
                x: Length::from_mm(0.0),
                y: Length::from_mm(0.0),
                width: Length::from_mm(100.0),
                height: Length::from_mm(100.0),
            }),
            edge_shapes: Vec::new(),
            holes: vec![hole(0.8, 0.8, None), hole(3.2, 1.6, Some(0.0)), hole(0.8, 0.8, None)],
        };
        let features = resolve_board_features(&board, 1000.0, 1000.0, 1.0, None);
        assert_eq!(features.holes.len(), 2, "two round holes get drill symbols");
        assert_eq!(features.slots.len(), 1, "the oblong becomes a routed band");
        assert_eq!(features.drill_legend.len(), 1, "one 0.8 mm size class, no slot entry");
        assert_eq!(features.slot_legend.len(), 1);
        assert!((features.slot_legend[0].length_mm - 3.2).abs() < 1e-9);
        assert!((features.slot_legend[0].width_mm - 1.6).abs() < 1e-9);
        assert!(
            features.holes.iter().all(|marker| marker.machined),
            "with no step to filter by, every feature is the job's work"
        );
    }

    /// The board is drawn per step: a step that drills only plated holes ghosts the
    /// non-plated ones rather than hiding them, so the board still reads as a board.
    #[test]
    fn a_step_ghosts_the_features_it_does_not_machine() {
        let mut npth = hole(1.2, 1.2, None);
        npth.kind = HoleKind::PadNpth;
        let board = BoardSnapshot {
            name: "t".into(),
            thickness: None,
            bounding_box: Some(pcb::BoardBoundingBox {
                x: Length::from_mm(0.0),
                y: Length::from_mm(0.0),
                width: Length::from_mm(100.0),
                height: Length::from_mm(100.0),
            }),
            edge_shapes: Vec::new(),
            holes: vec![hole(0.8, 0.8, None), npth],
        };
        let pth_only = StepTargets { pth: true, npth: false, outline: false };
        let features = resolve_board_features(&board, 1000.0, 1000.0, 1.0, Some(&pth_only));

        assert_eq!(features.holes.len(), 2, "both holes are still drawn");
        assert!(features.holes[0].machined, "the plated hole is this step's work");
        assert!(!features.holes[1].machined, "the non-plated one is not");
    }
}
