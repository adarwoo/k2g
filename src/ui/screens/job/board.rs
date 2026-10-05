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
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use pcb::{BoardEdgeShape, BoardSnapshot, Contour, HoleKind};
use units::Length;

use crate::runtime::isolation::Isolation;
use crate::runtime::tooling::{engrave_target, step_targets, EngraveTarget, StepTargets};
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

/// Width of the SVG user space the board is drawn into. The height follows from the
/// board's aspect, so this one number sets the scale for everything in the view.
const BOARD_VIEW_WIDTH: f64 = 1000.0;

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

/// A thing the legend names and the operator can switch off.
///
/// Only the rows that name a *layer* — something drawn as its own group with its own
/// colour. The drill-size and slot-size rows are deliberately absent: they classify
/// symbols by diameter rather than naming a layer, so a tickbox there would be a size
/// filter, which is a different feature and would put a dozen more boxes on a dense board.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum BoardLayer {
    CopperFront,
    CopperBack,
    /// The isolation pass: the channels milled round each net, and the tool that cuts them.
    ///
    /// **One row, not one per face.** The pass shown is the one the selected step engraves
    /// (see [`engrave_target`]), so a two-sided profile shows its front pass while a front
    /// step is selected and its back pass while a back one is — the same rule the rest of this
    /// view follows. A row per face would leave one of them permanently empty on the single-
    /// sided boards that are most of the work.
    ///
    /// This row is the only one that governs something outside itself: switching it off also
    /// takes the mask off the copper, restoring the board to whole copper. That is deliberate —
    /// "show me the isolation" and "show me the copper it leaves" are one question.
    Isolation,
    Via,
    Pth,
    Npth,
    OutsideRoute,
    EdgeCut,
}

impl BoardLayer {
    /// The layer a hole kind belongs to. One row governs both the drilled markers and the
    /// routed slots of its kind, which is what the "Hole type colours" row already meant —
    /// a PTH slot is hatched in the PTH colour.
    fn of_hole(kind: &HoleKind) -> Self {
        match kind {
            HoleKind::Via => Self::Via,
            HoleKind::PadPth => Self::Pth,
            HoleKind::PadNpth => Self::Npth,
        }
    }
}

/// Whether `layer` is drawn, given the set the operator has switched off.
///
/// Hidden rather than shown, so "everything visible" is the empty set and a layer added
/// later is visible without anyone having to remember to list it.
fn layer_visible(hidden: &BTreeSet<BoardLayer>, layer: BoardLayer) -> bool {
    !hidden.contains(&layer)
}

/// The order the layers are painted in, bottom first.
///
/// Copper underneath because that is where it is on the board, then the kerf band, the
/// edge line, and the drilled and routed features on top of everything — those are what
/// the view is read for, and they are small marks that must not be buried.
///
/// The isolation overlay sits directly on the copper, because it is a statement *about* the
/// copper: the groove and the tool's line belong in the channel they were cut in. It still
/// goes under the drill marks, which stay the smallest and topmost marks on the board.
const DEFAULT_DRAW_ORDER: [BoardLayer; 8] = [
    BoardLayer::CopperBack,
    BoardLayer::CopperFront,
    BoardLayer::Isolation,
    BoardLayer::OutsideRoute,
    BoardLayer::EdgeCut,
    BoardLayer::Via,
    BoardLayer::Pth,
    BoardLayer::Npth,
];

/// The paint order with `raised` moved to the end, so it lands on top of the rest.
///
/// SVG paints in document order and honours neither `z-index` nor `order`, so bringing a
/// layer to the front means genuinely emitting it last — which is why the marks are built
/// per layer and shuffled rather than written out in a fixed sequence.
///
/// Everything else keeps its relative order. Raising the bottom copper should slide it
/// over the top copper and nothing else; it should not also rearrange the drill marks.
fn draw_order(raised: Option<BoardLayer>) -> Vec<BoardLayer> {
    let mut order: Vec<BoardLayer> = DEFAULT_DRAW_ORDER.to_vec();
    if let Some(raised) = raised {
        order.retain(|layer| *layer != raised);
        order.push(raised);
    }
    order
}

/// The tickbox at the head of a legend row that names a layer.
///
/// The legend is the layer control, so the row that explains a thing is the row that
/// switches it off — rather than a separate panel of toggles the reader has to match up
/// against it by name.
#[component]
fn LegendToggle(layer: BoardLayer, hidden: Signal<BTreeSet<BoardLayer>>) -> Element {
    let shown = layer_visible(&hidden.read(), layer);
    rsx! {
        input {
            r#type: "checkbox",
            class: "board-legend-check",
            checked: shown,
            // The row around this raises the layer to the front; the box switches it off.
            // Two meanings on one row, so the click must not reach both — ticking a box
            // would otherwise also reorder the drawing, which is not what the tick says.
            onclick: move |event| event.stop_propagation(),
            onchange: move |_| {
                hidden
                    .with_mut(|set| {
                        if !set.remove(&layer) {
                            set.insert(layer);
                        }
                    });
            },
        }
    }
}

/// A legend row for a layer: a tickbox that shows and hides it, and a body that brings it
/// to the front.
///
/// The legend is the layer control, so the row that explains a thing is the row that
/// arranges it. Clicking the row again puts the drawing back in its natural order rather
/// than leaving the reader to hunt for how to undo it.
#[component]
fn LegendLayerRow(
    layer: BoardLayer,
    hidden: Signal<BTreeSet<BoardLayer>>,
    top: Signal<Option<BoardLayer>>,
    children: Element,
) -> Element {
    let raised = *top.read() == Some(layer);
    rsx! {
        div {
            class: if raised {
                "board-drill-legend-item is-layer is-top"
            } else {
                "board-drill-legend-item is-layer"
            },
            title: if raised { "Click to restore the drawing order" } else { "Click to draw this layer on top" },
            onclick: move |_| {
                let mut top = top;
                top.set((!raised).then_some(layer));
            },
            LegendToggle { layer, hidden }
            {children}
        }
    }
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
/// One copper layer flattened into as few SVG paths as it can honestly be.
#[derive(Clone, PartialEq)]
struct CopperLayerSvg {
    /// Everything with no holes — tracks, pads, vias — as one path.
    ///
    /// One element rather than one per feature, because a dense board has well over a
    /// thousand of them and the DOM is the cost here, not the geometry. They are all in
    /// one path under the **nonzero** rule, which is what makes overlaps union: a track
    /// running across its own pad must read as copper, and under `evenodd` the overlap
    /// would come out as a hole.
    solid: String,
    /// The poured zones, one path each, filled **evenodd** so their clearance cut-outs
    /// and thermal reliefs read as holes.
    ///
    /// Not folded into the path above for exactly that reason — the two need opposite
    /// fill rules — and separate from each other because evenodd across two overlapping
    /// zones would punch a hole where they meet.
    zones: Vec<String>,
}

/// Both outer layers as SVG, in the view's coordinates.
#[derive(Clone, Default, PartialEq)]
struct CopperSvg {
    front: Option<CopperLayerSvg>,
    back: Option<CopperLayerSvg>,
}

/// The board's copper as paths, or nothing when there is no copper or no bounds.
///
/// Pulled out of the component and memoized on the context, because it is the most
/// expensive thing the view builds and **none of it depends on zoom or pan** — those
/// move the `viewBox`, not the geometry. Rebuilt inline it would regenerate and re-diff
/// most of a megabyte of path data on every frame of a drag.
fn build_copper_svg(ctx: &AppCtx) -> CopperSvg {
    let Some(bbox) = ctx.board.as_ref().and_then(|board| board.bounding_box.as_ref()) else {
        return CopperSvg::default();
    };
    let (min_x, min_y) = (bbox.x.as_mm(), bbox.y.as_mm());
    let (width, height) = (bbox.width.as_mm(), bbox.height.as_mm());
    if width <= 0.0 || height <= 0.0 {
        return CopperSvg::default();
    }
    let view_height = BOARD_VIEW_WIDTH * (height / width);

    // Unclamped, unlike the edge shapes: copper legitimately sits outside the board's own
    // bounding box (a pad's clearance, a pour run right to the edge), and clamping would
    // smear it along the border instead of letting the frame clip it.
    let tx = move |px: f64| ((px - min_x) / width) * BOARD_VIEW_WIDTH;
    let ty = move |py: f64| ((py - min_y) / height) * view_height;

    CopperSvg {
        front: ctx.copper.front.as_ref().map(|layer| copper_layer_svg(layer, tx, ty)),
        back: ctx.copper.back.as_ref().map(|layer| copper_layer_svg(layer, tx, ty)),
    }
}

/// Turns a layer's copper into paths, mapping board mm through `tx`/`ty`.
fn copper_layer_svg(
    copper: &pcb::CopperSnapshot,
    tx: impl Fn(f64) -> f64,
    ty: impl Fn(f64) -> f64,
) -> CopperLayerSvg {
    let ring = |out: &mut String, ring: &[(i64, i64)]| {
        for (i, &(x, y)) in ring.iter().enumerate() {
            let (x, y) = (tx(x as f64 / 1e6), ty(y as f64 / 1e6));
            out.push_str(if i == 0 { "M" } else { "L" });
            out.push_str(&format!("{x:.2} {y:.2} "));
        }
        out.push_str("Z ");
    };

    let mut solid = String::new();
    let mut zones = Vec::new();
    for feature in &copper.features {
        if feature.source == pcb::CopperSource::Zone {
            let mut path = String::new();
            for polygon in &feature.polygons {
                ring(&mut path, &polygon.outline);
                for hole in &polygon.holes {
                    ring(&mut path, hole);
                }
            }
            if !path.is_empty() {
                zones.push(path);
            }
            continue;
        }
        for polygon in &feature.polygons {
            ring(&mut solid, &polygon.outline);
        }
    }
    CopperLayerSvg { solid, zones }
}

/// Every stretch the pass cut at one width, as one path.
///
/// Grouped by width because width is a *stroke property*, so one element can carry as many
/// stretches as share it — and the ladder has a handful of rungs, so a board whose 349 loops
/// became 3416 slivers still draws in four paths. Ungrouped it would be one element per
/// contour, which is the cost the copper render already went out of its way to avoid.
#[derive(Clone, PartialEq)]
struct ChannelGroup {
    /// The channel's width in view units.
    ///
    /// A **real** width, so it scales with zoom — unlike the drill symbols and edge lines,
    /// which are annotations and carry `non-scaling-stroke`. A channel drawn at a fixed screen
    /// width would be a picture of a board that does not exist.
    width_units: f64,
    /// True when this rung is below the width the bit was chosen for — a stretch the ladder
    /// had to squeeze through a tight gap.
    narrowed: bool,
    d: String,
}

/// The isolation pass as SVG, in the view's coordinates.
#[derive(Clone, PartialEq)]
struct IsolationSvg {
    /// The copper face this cuts — `pcb::FRONT_COPPER` or `pcb::BACK_COPPER`. Only that face's
    /// copper is masked by it; the other side of the board is untouched by this pass.
    layer_id: i32,
    channels: Vec<ChannelGroup>,
    /// The tool centre's own line, every stretch merged. Hairline, and the only thing that
    /// distinguishes two passes whose channels have merged into one — which is what an overlap
    /// looks like from above.
    centrelines: String,
    /// The flat at the bottom of the groove, in view units; zero for a flat-ended tool, whose
    /// bottom *is* its channel.
    tip_units: f64,
    /// Free copper a dedicated milling cutter took out, merged into one path — empty when
    /// the step does not run the pass. Kept apart from `channels` because it is a different
    /// tool at a different, single width: folding it into that width-keyed map would need a
    /// `narrowed` flag that means nothing here, since this pass has no ladder to squeeze.
    clearing_d: String,
    /// The milling cutter's own diameter, in view units. Zero (and `clearing_d` empty) when
    /// the pass did not run.
    clearing_width_units: f64,
}

/// What the isolation render is derived from, identified cheaply.
///
/// The point of the type is its [`PartialEq`]: the held contours are compared by **pointer**,
/// not by walking a few hundred thousand points, so the expensive builder downstream re-runs
/// when the answer changes and not merely when the context does. `BoardCopper` is held the
/// same way and for the same reason.
#[derive(Clone)]
struct IsolationInput {
    target: EngraveTarget,
    /// `None` while the worker is still on it — which the legend says out loud rather than
    /// showing whole copper as though nothing were going to be cut.
    held: Option<Arc<Isolation>>,
    /// The board's bounds in mm, which is all the mapping needs.
    bbox: (f64, f64, f64, f64),
}

impl PartialEq for IsolationInput {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target
            && self.bbox == other.bbox
            && match (&self.held, &other.held) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

/// The isolation pass to draw, and the answer held for it.
///
/// Cheap enough to run on every render — a datastore read of the profile's steps and a bit
/// choice, the same two the Board view already makes for [`step_targets`] — which is what lets
/// it stand between the context and the path builder.
fn isolation_input(ctx: &AppCtx) -> Option<IsolationInput> {
    let target = engrave_target(ctx, ctx.selected_step)?;
    let bbox = ctx.board.as_ref()?.bounding_box.as_ref()?;
    let (width, height) = (bbox.width.as_mm(), bbox.height.as_mm());
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let held = ctx.isolation.matching(&target.spec).cloned();
    Some(IsolationInput { target, held, bbox: (bbox.x.as_mm(), bbox.y.as_mm(), width, height) })
}

/// One chain of nested clearing rings as the single open path the cutter actually walks.
///
/// Each ring contributes its own vertices plus a repeat of its first — the closing edge
/// `pcb`'s geometry leaves implied — and the step from that repeat to the next ring's first
/// vertex *is* the connector, cut at depth across ground the pass has already cleared. So
/// the whole chain is one unbroken subpath, which is the point: drawn as separate closed
/// loops it is indistinguishable from a path that retracts between every ring, and that is
/// motion this program does not contain (`gcode::program`'s `continues_from_previous`).
///
/// Rings too degenerate to draw are skipped rather than breaking the chain around them.
fn chain_path(rings: &[Vec<(i64, i64)>]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for ring in rings.iter().filter(|ring| ring.len() >= 3) {
        out.extend_from_slice(ring);
        out.push(ring[0]);
    }
    out
}

/// The isolation contours as paths, mapped into the view the way the copper is.
///
/// **The channels are what the tool sweeps, not what it traces.** A round tool run along a
/// polyline sweeps exactly a round-capped, round-joined stroke of its own width — so stroking
/// the contour at the width it achieved is not an approximation of the cut, it *is* the cut.
/// That is what lets the copper be shown as it will be left without a single boolean op: the
/// same paths go into a mask, and the copper under them is gone.
fn build_isolation_svg(input: &IsolationInput) -> Option<IsolationSvg> {
    let held = input.held.as_ref()?;
    let (min_x, min_y, width, height) = input.bbox;
    let view_height = BOARD_VIEW_WIDTH * (height / width);
    let units_per_mm = BOARD_VIEW_WIDTH / width;
    let tx = |px: f64| ((px - min_x) / width) * BOARD_VIEW_WIDTH;
    let ty = |py: f64| ((py - min_y) / height) * view_height;

    let append = |out: &mut String, path: &[(i64, i64)], closed: bool| {
        for (i, &(x, y)) in path.iter().enumerate() {
            let (x, y) = (tx(x as f64 / 1e6), ty(y as f64 / 1e6));
            out.push_str(if i == 0 { "M" } else { "L" });
            out.push_str(&format!("{x:.2} {y:.2} "));
        }
        // Per subpath, so one `d` can hold closed loops and open spans together — which it has
        // to, because a contour that took one width the whole way round stays a loop while one
        // that had to narrow arrives as spans, and both may be the same rung of the ladder.
        if closed {
            out.push_str("Z ");
        }
    };

    let mut by_width: BTreeMap<i64, String> = BTreeMap::new();
    let mut centrelines = String::new();
    for contour in &held.result.contours {
        if contour.path.len() < 2 {
            continue;
        }
        append(by_width.entry(contour.width_nm).or_default(), &contour.path, contour.closed);
        append(&mut centrelines, &contour.path, contour.closed);
    }
    // The island rings are cut by the same bit at its nominal width — they are the pass at full
    // depth, not a narrowed stretch — so they join that rung rather than getting a group of
    // their own. Stroked, the concentric rings union into the cleared island, which is exactly
    // what `pcb::clearing::fill` laid them out to do.
    //
    // Drawn a whole chain at a time, in `pcb::Clearing::chains`' own order, because that is
    // what the machine really does: one drop-in per island, then inward from ring to ring
    // across ground already cleared, never lifting. Ring-at-a-time — as this drew them until
    // an operator quite reasonably read the picture as a retract between every ring — a single
    // continuous pass looked like a stack of unrelated loops.
    for rings in held.clearing.chains() {
        let path = chain_path(&rings);
        if path.is_empty() {
            continue;
        }
        append(by_width.entry(input.target.spec.width_nm).or_default(), &path, false);
        append(&mut centrelines, &path, false);
    }

    // The dedicated mill's own rings, kept out of `by_width`: a different tool, at a width
    // that has no relationship to the isolation channel's, so grouping it in there would
    // make `narrowed` (which means "the isolation ladder squeezed here") answer a question
    // this pass never asks.
    let mut clearing_d = String::new();
    for rings in held.mill_clearing.chains() {
        let path = chain_path(&rings);
        if path.is_empty() {
            continue;
        }
        append(&mut clearing_d, &path, false);
    }

    if by_width.is_empty() && clearing_d.is_empty() {
        return None;
    }

    Some(IsolationSvg {
        layer_id: input.target.spec.layer_id,
        // Widest first, so a narrowed stretch is painted over the full-width channel it
        // branches off rather than under it.
        channels: by_width
            .into_iter()
            .rev()
            .map(|(width_nm, d)| ChannelGroup {
                width_units: (width_nm as f64 / 1e6) * units_per_mm,
                narrowed: width_nm < input.target.spec.width_nm,
                d,
            })
            .collect(),
        centrelines,
        // A flat-ended tool cuts its own width however deep it goes, so its groove has no
        // narrower bottom to draw; drawing one would flood the channel with a single tone and
        // say something about the cut that is not true.
        tip_units: if input.target.point_angle.as_degrees() >= 180.0 {
            0.0
        } else {
            input.target.tip.as_mm() * units_per_mm
        },
        clearing_width_units: (input.target.spec.clearing_mill_diameter_nm as f64 / 1e6)
            * units_per_mm,
        clearing_d,
    })
}

/// What the isolation legend row says: its title, the line beneath the heading, and the facts.
///
/// Split out because it is prose about numbers rather than markup, and because it is the half
/// of this feature that has to be right when the picture is *not*: a channel drawn 0.35 mm
/// wide when 0.40 was asked for looks like any other channel, and this is where that is said.
///
/// Returns `("", "", vec![])` when there is no pass, which the caller reads as "no row".
fn isolation_legend(
    target: Option<&EngraveTarget>,
    held: Option<&Isolation>,
    unit: units::UserUnitSystem,
) -> (String, String, Vec<String>) {
    let Some(target) = target else {
        return (String::new(), String::new(), Vec::new());
    };
    let len = |length: Length| unit_format::format_length_display(length, unit);

    let face = if target.spec.layer_id == pcb::BACK_COPPER {
        "Bottom copper (B.Cu)"
    } else {
        "Top copper (F.Cu)"
    };
    let label = format!("{face} · {} channel", len(target.achieved));

    let Some(held) = held else {
        // Not an error and not an empty pass — the worker is on it. Saying nothing here is how
        // a board once sat on screen looking as though its copper would be left whole.
        return (
            label,
            "Working out the isolation contours…".to_string(),
            Vec::new(),
        );
    };

    // A cone widens with depth, so its tip is not its channel and both numbers are worth
    // having; a flat tool has one width there is, and quoting a "tip" for it invites the reader
    // to look for a second band that is not there.
    let bit = if target.point_angle.as_degrees() >= 180.0 {
        format!("{} ⌀{}, flat", target.tool_label, len(target.tip))
    } else {
        format!(
            "{} · tip ⌀{} at {:.0}°",
            target.tool_label,
            len(target.tip),
            target.point_angle.as_degrees(),
        )
    };

    let mut facts = vec![bit];
    facts.push(format!(
        "Asked for at least {}; this bit cuts {}.",
        len(target.requested),
        len(target.achieved),
    ));

    let narrowed = held.result.contours.iter().filter(|c| c.width_nm < target.spec.width_nm).count();
    if narrowed > 0 {
        facts.push(format!(
            "{narrowed} stretch(es) narrowed to fit between close nets — marked on the board.",
        ));
    }

    // The islands, said the way the step says them: what was taken, and what a router would
    // still be needed for.
    let clearing = &held.clearing;
    if target.removes_islands {
        if clearing.removed > 0 {
            facts.push(format!("{} stranded island(s) trimmed.", clearing.removed));
        }
        if clearing.left > 0 {
            facts.push(format!(
                "{} piece(s) of free copper left standing, the widest {}.",
                clearing.left,
                len(Length::from_mm(clearing.widest_left_nm as f64 / 1e6)),
            ));
        }
    } else {
        facts.push("Island trimming is off for this step.".to_string());
    }

    // The dedicated mill, said the same way: what it took, and what it could not reach.
    if target.clears_narrow_copper {
        match target.clearing_tool_label.as_deref() {
            Some(tool_label) => {
                let mill = &held.mill_clearing;
                facts.push(format!(
                    "{tool_label} clears free copper below {}.",
                    len(target.clearing_threshold),
                ));
                if mill.removed > 0 {
                    facts.push(format!(
                        "{} piece(s) of free copper cleared by the mill.",
                        mill.removed,
                    ));
                }
                if mill.unreachable > 0 {
                    facts.push(format!(
                        "{} piece(s) too narrow for the mill to enter, the widest {}.",
                        mill.unreachable,
                        len(Length::from_mm(mill.widest_unreachable_nm as f64 / 1e6)),
                    ));
                }
            }
            None => facts.push(format!(
                "No milling bit in stock is narrow enough to clear copper below {} — the \
                 pass is skipped.",
                len(target.clearing_threshold),
            )),
        }
    }

    let note = "Channels are cut to scale, so the copper shown is the copper the board is \
                left with. The field between nets stays standing — isolation milling only \
                takes the channel."
        .to_string();
    (label, note, facts)
}

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
/// One drilled hole's symbol, placed and rotated.
///
/// Lifted out of the view so the listing can be reordered: the marks are now emitted per
/// layer and shuffled by which one the reader has brought to the front, and a 150-line
/// block inlined in the middle of the tree cannot be moved about.
fn hole_marker_element(idx: usize, marker: &BoardHoleMarker) -> Element {
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

/// One routed slot: the hatched stadium it sweeps, and the cutter's line through it.
fn slot_element(idx: usize, slot: &BoardSlotFeature) -> Element {
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
    // What the operator has switched off in the legend. Hidden rather than shown, so
    // everything starts visible — the copper included, since it is most of what a PCB is
    // and a board view that omits it until asked is a board view of the drill file.
    //
    // Local to this view, like zoom and pan, and forgotten on leaving it for the same
    // reason: it is how the picture is being looked at, not something about the job.
    let hidden_layers = use_signal(BTreeSet::<BoardLayer>::new);
    // Which layer the reader has brought to the front, if any. Also local to the view:
    // it is how the picture is being looked at, not a fact about the job.
    let top_layer = use_signal(|| Option::<BoardLayer>::None);
    let hidden = hidden_layers.read().clone();
    let mut board_last_pointer = use_signal(|| (0.0_f64, 0.0_f64));
    let board_view_width = BOARD_VIEW_WIDTH;
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
                        // Each ring drawn as a closed path. Arc nodes are drawn as
                        // their chord here — the preview only needs the shape to read,
                        // and the machining path takes its arcs from the stitched
                        // contour, not from this.
                        BoardEdgeShape::GraphicPolygon { rings, .. } => {
                            let mut d = String::new();
                            for ring in rings {
                                for (i, node) in ring.nodes.iter().enumerate() {
                                    let p = match node {
                                        pcb::PolyNode::Point(p) => p,
                                        pcb::PolyNode::Arc { start, .. } => start,
                                    };
                                    let (x, y) = (tx(p.x.as_mm()), ty(p.y.as_mm()));
                                    d.push_str(&format!("{} {x} {y} ", if i == 0 { "M" } else { "L" }));
                                }
                                d.push_str("Z ");
                            }
                            (!d.is_empty()).then(|| SvgShape::Path(d.trim_end().to_string()))
                        }
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

    // Memoized, not built inline: it is the most expensive thing here and none of it
    // moves with zoom or pan. It rebuilds when the context does — which is when the board
    // is re-read — and when the layer is switched on.
    // Built whenever the board has copper, whatever is switched on. Keying it on the
    // toggles instead would rebuild most of a megabyte of path data every time one was
    // ticked; the render skips a hidden layer, which costs nothing.
    let copper_svg = use_memo(move || build_copper_svg(&state.read()));
    let copper_svg = copper_svg.read();

    // The isolation pass, in two stages. The first is cheap and runs whenever the context
    // moves; the second is the expensive one and runs only when the first says the *answer*
    // moved. Splitting them is the whole reason `IsolationInput` compares its contours by
    // pointer — a bare memo over the context would rebuild every channel path each time a
    // setting anywhere in the app was touched.
    let iso_input = use_memo(move || isolation_input(&state.read()));
    let isolation_svg = use_memo(move || iso_input.read().as_ref().and_then(build_isolation_svg));
    let iso_input = iso_input.read();
    let isolation_svg = isolation_svg.read();

    // Nothing else asks. The machining plan requests contours too, but it runs on the
    // generation path — which is gated on the job being ready to machine — and from the
    // Machining tab, so a board sitting on this tab would wait forever. Asking twice is free
    // (`request_isolation` drops a repeat of what it is already working on), and a failed run
    // being asked again is the deliberate behaviour: KiCad answers `AS_BUSY` while it re-pours
    // and the retry is what waits for the pour.
    if let Some(input) = iso_input.as_ref() {
        if input.held.is_none() {
            crate::runtime::isolation::request_isolation(input.target.spec.clone());
        }
    }
    let iso_target = iso_input.as_ref().map(|input| &input.target);
    let iso_held = iso_input.as_ref().and_then(|input| input.held.as_ref());
    let (isolation_label, isolation_note, isolation_facts) =
        isolation_legend(iso_target, iso_held.map(Arc::as_ref), snapshot.unit_system);
    let iso_shown = isolation_svg
        .as_ref()
        .filter(|_| layer_visible(&hidden, BoardLayer::Isolation));
    // The copper layer whose face the pass cuts. The other face keeps its copper whole — a
    // front channel takes no copper off the back of the board, and painting one opaquely over
    // both is exactly the lie the mask exists to avoid.
    let iso_face = iso_shown.map(|iso| {
        if iso.layer_id == pcb::BACK_COPPER {
            BoardLayer::CopperBack
        } else {
            BoardLayer::CopperFront
        }
    });

    // The stitched contours, in view units — the source for the outside-route band, and for
    // the field-copper wash beneath the isolation pass.
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

    // Each layer's marks, built up front so they can be emitted in whatever order the
    // reader has asked for. Hidden layers build nothing at all rather than building
    // something empty, which keeps the cost of a switched-off copper layer at zero.
    let copper_marks = |which: BoardLayer, class: &'static str, layer: Option<&CopperLayerSvg>| {
        let layer = layer.filter(|_| layer_visible(&hidden, which))?;
        // Isolation milling starts from a face that is **entirely** copper and takes a channel
        // out around each net, so the copper between nets stays standing. Drawing only the
        // design's copper would be a picture of an etched board — which is why the wash goes on
        // beneath it, cut by the same mask: it is the field the pass leaves behind, and it is
        // what makes every channel edge a visible copper boundary right across the face.
        let cut = iso_face == Some(which);
        let field = cut.then(|| stitched_outline.as_ref()).flatten();
        Some(rsx! {
            g {
                class: "{class}",
                mask: if cut { "url(#board-isolation-mask)" } else { "none" },
                if let Some(field) = field {
                    path { d: "{field}", class: "board-copper-field", fill_rule: "evenodd" }
                }
                for (zone_idx , zone) in layer.zones.iter().enumerate() {
                    path {
                        key: "{class}-zone-{zone_idx}",
                        d: "{zone}",
                        class: "board-copper-fill",
                        fill_rule: "evenodd",
                    }
                }
                if !layer.solid.is_empty() {
                    path { d: "{layer.solid}", class: "board-copper-fill", fill_rule: "nonzero" }
                }
            }
        })
    };

    // The features of one hole kind: the slots it routes and the holes it drills, as one
    // layer, because that is what the legend row governs and what raising it should move.
    let hole_marks = |which: BoardLayer| {
        if !layer_visible(&hidden, which) {
            return None;
        }
        Some(rsx! {
            for (idx , slot) in board_slot_features
                .iter()
                .enumerate()
                .filter(|(_, slot)| BoardLayer::of_hole(&slot.kind) == which)
            {
                {slot_element(idx, slot)}
            }
            for (idx , marker) in board_hole_markers
                .iter()
                .enumerate()
                .filter(|(_, marker)| BoardLayer::of_hole(&marker.kind) == which)
            {
                {hole_marker_element(idx, marker)}
            }
        })
    };

    // The tool's own signature, over the copper the mask has already cut.
    //
    // The channel itself is not painted here — it is absent copper, and absence is what the
    // mask makes. What is left to say is what the *tool* did: how deep the groove goes, where
    // it had to be squeezed, and where its centre ran.
    //
    // Ghosted when another step engraves this face, the way the outline band ghosts for a step
    // that does not route it. The mask is not ghosted with it: the copper genuinely will be
    // cut, whichever step happens to be on screen.
    let isolation_marks = || {
        let iso = iso_shown?;
        let ghost = iso_target.is_some_and(|target| !target.is_selected_step);
        Some(rsx! {
            g { class: if ghost { "board-step-ghost" } else { "" },
                // The flat at the bottom of the V, drawn inside the channel it sits in. Two
                // tones for a cone, one band for a flat cutter — and that falls out of the
                // geometry rather than a branch on tool kind, because a flat tool's tip *is*
                // its channel and `tip_units` comes back zero for it.
                if iso.tip_units > 0.0 && !iso.centrelines.is_empty() {
                    path {
                        d: "{iso.centrelines}",
                        class: "board-iso-groove",
                        stroke_width: "{iso.tip_units}",
                    }
                }
                // Where the ladder had to give up width. The step already says *that* it
                // narrowed and between which nets; this says **where**, which is the half no
                // amount of prose can give you.
                for (idx , group) in iso.channels.iter().filter(|group| group.narrowed).enumerate() {
                    path {
                        key: "iso-narrow-{idx}",
                        d: "{group.d}",
                        class: "board-iso-narrowed",
                        stroke_width: "{group.width_units}",
                    }
                }
                // The tool centre. A hairline, and the only mark that survives two channels
                // merging into one — two centre lines in a single channel is what an overlap
                // looks like from directly above.
                if !iso.centrelines.is_empty() {
                    path { d: "{iso.centrelines}", class: "board-iso-centerline" }
                }
                // The dedicated mill's own centre — a different colour, since it is a
                // different tool cutting a block of its own, not a rung of the same ladder.
                if !iso.clearing_d.is_empty() {
                    path { d: "{iso.clearing_d}", class: "board-clearing-centerline" }
                }
            }
        })
    };

    // Outside routing: the kerf the outline cutter sweeps. It lies wholly beyond the edge
    // cut, so the finished board keeps its nominal size. Without a clean stitch there is
    // no inside to keep out of, so fall back to a band centred on the raw edge fragments —
    // still "this edge is routed", just unsided.
    //
    // Ghosted when the selected step does not cut the outline: the kerf is drawn so the
    // board still reads as a board, but it is plainly another step's work. The ghost class
    // goes on a wrapping group, never beside `board-outline-band` on the band itself —
    // that class sets its own `opacity`, and two single-class rules have equal
    // specificity, so whichever the sheet declares last wins and the ghost is silently
    // ignored.
    let outline_marks = layer_visible(&hidden, BoardLayer::OutsideRoute).then(|| {
        rsx! {
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
                        {edge_shape_element(shape, "board-outline-band", Some(outline_band_width))}
                    }
                }
            }
        }
    });

    // The visible edge pass only. The identical loop inside the mask must NOT be gated: it
    // defines where the kerf band is allowed to draw, so hiding it there would move the
    // band rather than hide a line.
    let edge_marks = layer_visible(&hidden, BoardLayer::EdgeCut).then(|| {
        rsx! {
            for shape in board_edge_shapes_svg.iter() {
                {edge_shape_element(shape, "board-edge-shape", None)}
            }
        }
    });

    let layer_marks: Vec<(BoardLayer, Element)> = draw_order(*top_layer.read())
        .into_iter()
        .filter_map(|layer| {
            let marks = match layer {
                BoardLayer::CopperBack => {
                    copper_marks(layer, "board-copper-back", copper_svg.back.as_ref())
                }
                BoardLayer::CopperFront => {
                    copper_marks(layer, "board-copper-front", copper_svg.front.as_ref())
                }
                BoardLayer::Isolation => isolation_marks(),
                BoardLayer::OutsideRoute => outline_marks.clone(),
                BoardLayer::EdgeCut => edge_marks.clone(),
                BoardLayer::Via | BoardLayer::Pth | BoardLayer::Npth => hole_marks(layer),
            }?;
            Some((layer, marks))
        })
        .collect();

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
                                                if !snapshot.copper.is_empty() {
                                                    " · {snapshot.copper.feature_count()} copper"
                                                }
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

                                                        // The isolation swatch's own channel,
                                                        // pitched for the 24×24 legend user
                                                        // space. Mask ids resolve document-wide,
                                                        // which is why the swatch can use one
                                                        // declared in the board's `defs`.
                                                        mask {
                                                            id: "board-isolation-legend-mask",
                                                            mask_units: "userSpaceOnUse",
                                                            x: "0",
                                                            y: "0",
                                                            width: "24",
                                                            height: "24",
                                                            rect { x: "0", y: "0", width: "24", height: "24", fill: "white" }
                                                            path {
                                                                d: "M 3 12 L 21 12",
                                                                fill: "none",
                                                                stroke: "black",
                                                                stroke_width: "5",
                                                                stroke_linecap: "round",
                                                            }
                                                        }

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

                                                        // The copper the isolation pass takes
                                                        // away. White is kept; each channel is
                                                        // stroked in black at the width it was
                                                        // actually cut, round-capped and
                                                        // round-joined — precisely the region a
                                                        // round tool sweeps along a polyline.
                                                        // Hung on the copper group of the face
                                                        // this pass cuts, it leaves the copper
                                                        // the board will be left with: no
                                                        // boolean geometry, and exact.
                                                        if let Some(iso) = iso_shown {
                                                            mask {
                                                                id: "board-isolation-mask",
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
                                                                for (idx , group) in iso.channels.iter().enumerate() {
                                                                    path {
                                                                        key: "iso-cut-{idx}",
                                                                        d: "{group.d}",
                                                                        fill: "none",
                                                                        stroke: "black",
                                                                        stroke_width: "{group.width_units}",
                                                                        stroke_linecap: "round",
                                                                        stroke_linejoin: "round",
                                                                    }
                                                                }
                                                                // The dedicated mill's own copper, taken out the same way — one
                                                                // more stroke in the same mask, since it is absent copper too.
                                                                if !iso.clearing_d.is_empty() {
                                                                    path {
                                                                        d: "{iso.clearing_d}",
                                                                        fill: "none",
                                                                        stroke: "black",
                                                                        stroke_width: "{iso.clearing_width_units}",
                                                                        stroke_linecap: "round",
                                                                        stroke_linejoin: "round",
                                                                    }
                                                                }
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

                                                    // Every layer's marks, emitted in the order the reader has chosen. SVG paints
                                                    // in document order and honours no z-index, so "bring this to the front" has to
                                                    // mean genuinely last — hence a list of elements that gets shuffled rather than
                                                    // a fixed sequence written out in the tree.
                                                    {layer_marks.into_iter().map(|(_, marks)| marks)}
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
                                                                    span { class: "board-legend-check-gap" }
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
                                                                    span { class: "board-legend-check-gap" }
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
                                                LegendLayerRow {
                                                    layer: BoardLayer::OutsideRoute,
                                                    hidden: hidden_layers,
                                                    top: top_layer,
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
                                                LegendLayerRow {
                                                    layer: BoardLayer::Via,
                                                    hidden: hidden_layers,
                                                    top: top_layer,
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
                                                LegendLayerRow {
                                                    layer: BoardLayer::Pth,
                                                    hidden: hidden_layers,
                                                    top: top_layer,
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
                                                LegendLayerRow {
                                                    layer: BoardLayer::Npth,
                                                    hidden: hidden_layers,
                                                    top: top_layer,
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
                                                LegendLayerRow {
                                                    layer: BoardLayer::EdgeCut,
                                                    hidden: hidden_layers,
                                                    top: top_layer,
                                                    svg { class: "board-legend-icon", view_box: "0 0 24 24",
                                                        path {
                                                            d: "M 3 12 L 9 4 L 21 4 L 21 20 L 3 20 Z",
                                                            class: "board-edge-shape",
                                                        }
                                                    }
                                                    span { "Edge cut line" }
                                                }

                                                // Copper last, and only when there is any:
                                                // a heading over two rows that explain
                                                // nothing would be worse than no heading.
                                                // KiCad's own colours, so an operator
                                                // reading this against the layout has
                                                // nothing to translate.
                                                if !snapshot.copper.is_empty() {
                                                    h4 { "Copper" }
                                                    div { class: "board-drill-legend-note",
                                                        "Shaded beneath everything else, as it is on the board. The far side is drawn fainter."
                                                    }
                                                    for (layer , label , swatch) in [
                                                        (BoardLayer::CopperFront, "Top copper (F.Cu)", "board-copper-front"),
                                                        (BoardLayer::CopperBack, "Bottom copper (B.Cu)", "board-copper-back"),
                                                    ] {
                                                        LegendLayerRow {
                                                            key: "copper-legend-{label}",
                                                            layer,
                                                            hidden: hidden_layers,
                                                            top: top_layer,
                                                            svg { class: "board-drill-legend-icon", view_box: "0 0 24 24",
                                                                g { class: "{swatch}",
                                                                    rect {
                                                                        x: "3",
                                                                        y: "5",
                                                                        width: "18",
                                                                        height: "14",
                                                                        rx: "2",
                                                                        class: "board-copper-fill",
                                                                    }
                                                                }
                                                            }
                                                            span { "{label}" }
                                                        }
                                                    }
                                                }

                                                // The isolation pass. Only when the profile
                                                // actually engraves — a row explaining a pass
                                                // that is not in the job is a row that has to be
                                                // read to find that out.
                                                if let Some(target) = iso_target {
                                                    h4 { "Isolation" }
                                                    div { class: "board-drill-legend-note", "{isolation_note}" }
                                                    LegendLayerRow {
                                                        layer: BoardLayer::Isolation,
                                                        hidden: hidden_layers,
                                                        top: top_layer,
                                                        // The swatch is the board's own picture
                                                        // in miniature: copper, a channel taken
                                                        // out of it, the groove in the channel
                                                        // and the tool's line down the middle.
                                                        svg { class: "board-drill-legend-icon", view_box: "0 0 24 24",
                                                            g { class: "board-copper-front",
                                                                rect {
                                                                    x: "3",
                                                                    y: "5",
                                                                    width: "18",
                                                                    height: "14",
                                                                    rx: "2",
                                                                    class: "board-copper-fill",
                                                                    mask: "url(#board-isolation-legend-mask)",
                                                                }
                                                            }
                                                            path {
                                                                d: "M 3 12 L 21 12",
                                                                class: "board-iso-groove",
                                                                stroke_width: "2.4",
                                                            }
                                                            path { d: "M 3 12 L 21 12", class: "board-iso-centerline" }
                                                        }
                                                        span { "{isolation_label}" }
                                                    }
                                                    for line in isolation_facts.iter() {
                                                        div {
                                                            key: "iso-fact-{line}",
                                                            class: "board-drill-legend-note",
                                                            "{line}"
                                                        }
                                                    }
                                                    if !target.is_selected_step {
                                                        div { class: "board-drill-legend-note",
                                                            "Cut by another step, so the tool marks are drawn faint. The copper is shown as that step will leave it."
                                                        }
                                                    }
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

    /// An island's rings are one continuous cut, so they have to be one continuous path:
    /// every ring closed back to its own start, and the step from there to the next ring's
    /// start left in as the connector the tool really cuts. Drawn as separate closed loops
    /// — which is what this did until an operator read the picture as a retract between
    /// every ring — the preview cannot show the pass it is previewing.
    #[test]
    fn a_chain_of_rings_draws_as_one_unbroken_path() {
        let outer = vec![(0, 0), (1_000_000, 0), (1_000_000, 1_000_000), (0, 1_000_000)];
        let inner = vec![(200_000, 200_000), (800_000, 200_000), (800_000, 800_000), (200_000, 800_000)];

        let path = chain_path(&[outer.clone(), inner.clone()]);

        assert_eq!(path.len(), 10, "four vertices and a closing repeat, twice: {path:?}");
        assert_eq!(&path[..5], &[(0, 0), (1_000_000, 0), (1_000_000, 1_000_000), (0, 1_000_000), (0, 0)]);
        assert_eq!(path[4], outer[0], "the outer ring closes back to its own start");
        assert_eq!(path[5], inner[0], "and the very next step is the connector into the inner ring");
        assert_eq!(path[9], inner[0], "which then closes too");
    }

    /// A ring too degenerate to draw is skipped without breaking the chain around it.
    #[test]
    fn a_degenerate_ring_does_not_break_its_chain() {
        let ring = vec![(0, 0), (1_000_000, 0), (1_000_000, 1_000_000)];
        let path = chain_path(&[vec![(5, 5)], ring.clone(), Vec::new()]);
        assert_eq!(path, vec![(0, 0), (1_000_000, 0), (1_000_000, 1_000_000), (0, 0)]);
        assert!(chain_path(&[vec![(5, 5)]]).is_empty(), "nothing drawable is no path at all");
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
            copper_thickness: Default::default(),
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
            copper_thickness: Default::default(),
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


#[cfg(test)]
mod isolation_tests {
    use super::*;
    use crate::runtime::isolation::IsolationSpec;
    use units::{Angle, UserUnitSystem};

    /// A 100 mm board, so the mapping is a round ten view units per millimetre and every
    /// expected width below can be read off by eye.
    const UNITS_PER_MM: f64 = BOARD_VIEW_WIDTH / 100.0;

    fn spec(width_nm: i64) -> IsolationSpec {
        IsolationSpec {
            board_name: "b".into(),
            board_epoch: 1,
            layer_id: pcb::FRONT_COPPER,
            width_nm,
            min_width_nm: 150_000,
            remove_islands: true,
            clear_narrow_copper: false,
            clearing_threshold_nm: 0,
            clearing_guard_band_nm: 0,
            clearing_mill_diameter_nm: 0,
        }
    }

    fn target(width_nm: i64, tip_mm: f64, angle_deg: f64) -> EngraveTarget {
        EngraveTarget {
            spec: spec(width_nm),
            is_selected_step: true,
            requested: Length::from_mm(0.3),
            achieved: Length::from_mm(width_nm as f64 / 1e6),
            tip: Length::from_mm(tip_mm),
            point_angle: Angle::from_degrees(angle_deg),
            tool_label: "V-bit".into(),
            removes_islands: true,
            clears_narrow_copper: false,
            clearing_threshold: Length::from_mm(2.0),
            clearing_tool_label: None,
        }
    }

    fn contour(width_nm: i64, closed: bool) -> pcb::IsolationContour {
        pcb::IsolationContour {
            net: "GND".into(),
            path: vec![(0, 0), (10_000_000, 0), (10_000_000, 10_000_000)],
            closed,
            width_nm,
        }
    }

    fn input(
        target: EngraveTarget,
        contours: Vec<pcb::IsolationContour>,
        islands: Vec<Vec<(i64, i64)>>,
    ) -> IsolationInput {
        let held = Isolation {
            spec: target.spec.clone(),
            result: pcb::IsolationResult {
                layer_id: target.spec.layer_id,
                contours,
                ..Default::default()
            },
            copper_warnings: Vec::new(),
            copper_layer_count: 2,
            // One group per ring, because `paths` and `groups` are a pair — `pcb::fill`
            // never produces one without the other, and `Clearing::chains` reads the
            // grouping to know which rings are cut as one continuous pass.
            clearing: pcb::Clearing {
                groups: vec![1; islands.len()],
                paths: islands,
                ..Default::default()
            },
            mill_clearing: pcb::Clearing::default(),
        };
        IsolationInput {
            target,
            held: Some(Arc::new(held)),
            bbox: (0.0, 0.0, 100.0, 100.0),
        }
    }

    /// **Why the grouping is by width.** Width is a stroke property, so one element carries
    /// every stretch that shares it — which is what keeps a board whose loops fragmented into
    /// thousands of slivers down to a handful of paths. The copper render already went out of
    /// its way to avoid one element per feature; this must not reintroduce it.
    #[test]
    fn each_achieved_width_becomes_one_path_widest_first() {
        let iso = build_isolation_svg(&input(
            target(400_000, 0.1, 30.0),
            vec![contour(400_000, true), contour(400_000, true), contour(250_000, false)],
            Vec::new(),
        ))
        .expect("three contours over two widths");

        assert_eq!(iso.channels.len(), 2, "two rungs of the ladder, two paths");
        assert!((iso.channels[0].width_units - 0.4 * UNITS_PER_MM).abs() < 1e-9);
        assert!((iso.channels[1].width_units - 0.25 * UNITS_PER_MM).abs() < 1e-9);
        assert_eq!(iso.channels[0].d.matches('M').count(), 2, "both full-width loops in one path");
    }

    /// A stretch below the width the bit was chosen for is one the ladder had to squeeze, and
    /// the view marks it. The full-width rung is not marked — every contour would light up,
    /// which is the same as none of them lighting up.
    #[test]
    fn only_a_rung_below_the_chosen_width_counts_as_narrowed() {
        let iso = build_isolation_svg(&input(
            target(400_000, 0.1, 30.0),
            vec![contour(400_000, true), contour(250_000, true)],
            Vec::new(),
        ))
        .expect("contours");

        assert!(!iso.channels[0].narrowed, "the width the bit was chosen for is not a compromise");
        assert!(iso.channels[1].narrowed, "a rung below it is");
    }

    /// A contour that took one width the whole way round is a closed loop; one that had to
    /// narrow arrives as open spans. Both may be the same rung, so one `d` has to hold both —
    /// which works only because `Z` is per subpath.
    #[test]
    fn a_loop_closes_its_subpath_and_a_span_does_not() {
        let closed = build_isolation_svg(&input(
            target(400_000, 0.1, 30.0),
            vec![contour(400_000, true)],
            Vec::new(),
        ))
        .expect("contours");
        let open = build_isolation_svg(&input(
            target(400_000, 0.1, 30.0),
            vec![contour(400_000, false)],
            Vec::new(),
        ))
        .expect("contours");

        assert!(closed.channels[0].d.ends_with("Z "));
        assert!(!open.channels[0].d.contains('Z'));
    }

    /// **Islands are the pass at full depth, not a narrowed stretch.** They are cut by the same
    /// bit at the width it was chosen for, so they join that rung rather than opening one of
    /// their own — and a group of their own would be drawn as though the tool had backed off
    /// for them, which is the opposite of what happened.
    #[test]
    fn island_rings_join_the_rung_the_bit_was_chosen_for() {
        let iso = build_isolation_svg(&input(
            target(400_000, 0.1, 30.0),
            vec![contour(400_000, true)],
            vec![vec![(0, 0), (1_000_000, 0), (1_000_000, 1_000_000)]],
        ))
        .expect("contours");

        assert_eq!(iso.channels.len(), 1, "the ring is cut at the nominal width, not its own");
        assert_eq!(iso.channels[0].d.matches('M').count(), 2, "contour and ring in one path");
        assert!(!iso.channels[0].narrowed);
    }

    /// A flat-ended tool cuts its own width however deep it goes, so its groove has no narrower
    /// bottom. Drawing one would flood the channel with a single tone and assert a depth the
    /// cut does not have — and this is the whole of the "two tones for a V-bit, one for a
    /// milling bit" behaviour, with no branch on tool kind anywhere.
    #[test]
    fn a_flat_tool_has_no_groove_and_a_cone_does() {
        let flat = build_isolation_svg(&input(
            target(400_000, 0.4, 180.0),
            vec![contour(400_000, true)],
            Vec::new(),
        ))
        .expect("contours");
        let cone = build_isolation_svg(&input(
            target(400_000, 0.1, 30.0),
            vec![contour(400_000, true)],
            Vec::new(),
        ))
        .expect("contours");

        assert_eq!(flat.tip_units, 0.0);
        assert!((cone.tip_units - 0.1 * UNITS_PER_MM).abs() < 1e-9);
        assert!(cone.tip_units < cone.channels[0].width_units, "the groove sits inside its channel");
    }

    /// Contours still being computed draw nothing rather than drawing whole copper — which
    /// would say the board comes off the machine uncut.
    #[test]
    fn nothing_held_draws_nothing() {
        let mut pending = input(target(400_000, 0.1, 30.0), vec![contour(400_000, true)], Vec::new());
        pending.held = None;

        assert!(build_isolation_svg(&pending).is_none());
    }

    /// The held answer is compared by **pointer**, so the expensive path builder downstream
    /// re-runs when the contours change and not merely when the context does. Comparing the
    /// contents would walk a few hundred thousand points on every render.
    #[test]
    fn the_input_compares_its_contours_by_pointer() {
        let a = input(target(400_000, 0.1, 30.0), vec![contour(400_000, true)], Vec::new());
        let same = IsolationInput { held: a.held.clone(), ..a.clone() };
        // Byte-identical contours, built separately.
        let other = input(target(400_000, 0.1, 30.0), vec![contour(400_000, true)], Vec::new());

        // Compared rather than `assert_eq!`-ed: a failure message here would print every
        // contour the input holds, which is the very thing this exists to avoid touching.
        assert!(a == same, "the same answer is the same input");
        assert!(a != other, "a different Arc is a different answer, cheaply");
    }

    /// While the worker is still on it the legend says so. Silence here is how a board once sat
    /// on screen looking as though its copper would be left whole.
    #[test]
    fn a_pass_with_no_contours_yet_says_it_is_working() {
        let target = target(400_000, 0.1, 30.0);
        let (label, note, facts) = isolation_legend(Some(&target), None, UserUnitSystem::Metric);

        assert!(label.contains("Top copper"), "the face is named even before the answer lands");
        assert!(note.contains("Working out"));
        assert!(facts.is_empty());
    }

    /// The narrowing count is read off the contours rather than off `narrowed`, which counts
    /// *pairs of nets* — a different question, and one that cannot be pointed at on the canvas.
    #[test]
    fn the_legend_counts_the_stretches_it_marks() {
        let target = target(400_000, 0.1, 30.0);
        let held = input(
            target.clone(),
            vec![contour(400_000, true), contour(250_000, true), contour(200_000, true)],
            Vec::new(),
        );
        let (_, _, facts) = isolation_legend(
            Some(&target),
            held.held.as_deref(),
            UserUnitSystem::Metric,
        );

        assert!(
            facts.iter().any(|line| line.starts_with("2 stretch(es) narrowed")),
            "two of the three contours are below the chosen width: {facts:?}",
        );
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;

    fn hidden(layers: &[BoardLayer]) -> BTreeSet<BoardLayer> {
        layers.iter().copied().collect()
    }

    /// SVG paints in document order and honours no z-index, so bringing a layer to the
    /// front means genuinely emitting it last. Nothing else may move: raising the bottom
    /// copper should slide it over the top copper and leave the drill marks where they
    /// were.
    #[test]
    fn raising_a_layer_moves_only_that_layer_to_the_end() {
        let order = draw_order(Some(BoardLayer::CopperBack));

        assert_eq!(order.last(), Some(&BoardLayer::CopperBack));
        assert_eq!(order.len(), DEFAULT_DRAW_ORDER.len(), "nothing gained or lost");

        let rest: Vec<BoardLayer> = order[..order.len() - 1].to_vec();
        let expected: Vec<BoardLayer> = DEFAULT_DRAW_ORDER
            .iter()
            .copied()
            .filter(|l| *l != BoardLayer::CopperBack)
            .collect();
        assert_eq!(rest, expected, "the others keep their order");
    }

    /// Copper underneath and the drilled marks on top is the natural reading of a board,
    /// and it is what the view goes back to when nothing is raised.
    #[test]
    fn nothing_raised_is_the_natural_order() {
        assert_eq!(draw_order(None), DEFAULT_DRAW_ORDER.to_vec());
        assert_eq!(
            DEFAULT_DRAW_ORDER.first(),
            Some(&BoardLayer::CopperBack),
            "copper is at the bottom, as it is on the board"
        );
        assert_eq!(DEFAULT_DRAW_ORDER.last(), Some(&BoardLayer::Npth));
    }

    /// Raising the layer that is already last is a no-op rather than a reshuffle.
    #[test]
    fn raising_the_topmost_layer_changes_nothing() {
        assert_eq!(draw_order(Some(BoardLayer::Npth)), DEFAULT_DRAW_ORDER.to_vec());
    }

    /// Every layer the legend offers has a place in the order, or raising it from the
    /// legend would drop it out of the drawing entirely.
    #[test]
    fn every_layer_the_legend_offers_is_painted() {
        for layer in [
            BoardLayer::CopperFront,
            BoardLayer::CopperBack,
            BoardLayer::Isolation,
            BoardLayer::Via,
            BoardLayer::Pth,
            BoardLayer::Npth,
            BoardLayer::OutsideRoute,
            BoardLayer::EdgeCut,
        ] {
            assert!(DEFAULT_DRAW_ORDER.contains(&layer), "{layer:?} is never painted");
            assert_eq!(draw_order(Some(layer)).len(), DEFAULT_DRAW_ORDER.len());
        }
    }

    /// The isolation overlay is a statement *about* the copper, so it goes on top of it — but
    /// it must stay under the drill marks, which are the smallest marks on the board and the
    /// ones it is read for. Getting this the other way round buries a 0.3 mm hole symbol under
    /// a channel drawn at true width.
    #[test]
    fn the_isolation_overlay_sits_between_the_copper_and_the_drill_marks() {
        let at = |layer: BoardLayer| DEFAULT_DRAW_ORDER.iter().position(|l| *l == layer).unwrap();

        assert!(at(BoardLayer::CopperFront) < at(BoardLayer::Isolation));
        assert!(at(BoardLayer::CopperBack) < at(BoardLayer::Isolation));
        for mark in [BoardLayer::Via, BoardLayer::Pth, BoardLayer::Npth] {
            assert!(at(BoardLayer::Isolation) < at(mark), "{mark:?} must stay on top");
        }
    }

    /// The default has to be "everything", and it has to stay that way when a layer is
    /// added later. Tracking what is *hidden* is what guarantees both — a set of what is
    /// shown would leave a new layer invisible until someone remembered to list it.
    #[test]
    fn nothing_hidden_shows_everything() {
        let none = hidden(&[]);
        for layer in [
            BoardLayer::CopperFront,
            BoardLayer::CopperBack,
            BoardLayer::Via,
            BoardLayer::Pth,
            BoardLayer::Npth,
            BoardLayer::OutsideRoute,
            BoardLayer::EdgeCut,
        ] {
            assert!(layer_visible(&none, layer), "{layer:?} should start visible");
        }
    }

    /// One row, one layer. Switching a hole type off must not take its neighbours with it
    /// — the whole point of a per-row control is that the rest of the picture stays put.
    #[test]
    fn hiding_one_layer_leaves_the_others_alone() {
        let only_pth = hidden(&[BoardLayer::Pth]);

        assert!(!layer_visible(&only_pth, BoardLayer::Pth));
        assert!(layer_visible(&only_pth, BoardLayer::Via));
        assert!(layer_visible(&only_pth, BoardLayer::Npth));
        assert!(layer_visible(&only_pth, BoardLayer::CopperFront));
        assert!(layer_visible(&only_pth, BoardLayer::EdgeCut));
    }

    /// A hole type governs both the drilled markers and the routed slots of that kind,
    /// because that is what the row has always meant: a PTH slot is hatched in the PTH
    /// colour. Both the marker loop and the slot loop reach the layer through this, so
    /// one mapping keeps them from disagreeing.
    #[test]
    fn a_hole_kind_maps_to_the_row_that_names_its_colour() {
        assert_eq!(BoardLayer::of_hole(&HoleKind::Via), BoardLayer::Via);
        assert_eq!(BoardLayer::of_hole(&HoleKind::PadPth), BoardLayer::Pth);
        assert_eq!(BoardLayer::of_hole(&HoleKind::PadNpth), BoardLayer::Npth);
    }

    /// The two copper layers are independent: seeing the far side alone is the reason to
    /// have two rows rather than one.
    #[test]
    fn the_two_copper_sides_switch_independently() {
        let front_off = hidden(&[BoardLayer::CopperFront]);
        assert!(!layer_visible(&front_off, BoardLayer::CopperFront));
        assert!(layer_visible(&front_off, BoardLayer::CopperBack));
    }
}
