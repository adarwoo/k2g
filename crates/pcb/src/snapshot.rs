//! The **PCB record**: the subset of a board's geometry that k2g actually needs.
//!
//! A [`BoardSnapshot`] is collected once from an open KiCad PCB and then handed
//! to the UI (to draw the board) and to the GCode generator (to iterate holes
//! and boundaries). It intentionally keeps only items of interest — edge cuts,
//! drilled holes (vias and plated/non-plated pads), the bounding box, and the
//! board thickness — rather than the full KiCad object graph.
//!
//! All coordinates are decoded into typed [`Length`]s (KiCad IPC reports
//! nanometres) so downstream code never juggles raw `i64` nm.

use kicad_ipc_rs::{
    BoardStackupLayerType, DocumentType, KiCadClientBlocking, PcbGraphicShapeGeometry, PcbItem,
    PcbPadStack, PcbPadType, Vector2Nm,
};

use units::Length;

/// The raw KiCad blocking client. Kept crate-private: callers go through
/// [`crate::KiCad`], which owns instance discovery and routing.
pub(crate) type Client = KiCadClientBlocking;

/// Everything k2g keeps about one PCB, collected from a KiCad document.
#[derive(Clone, Debug, PartialEq)]
pub struct BoardSnapshot {
    /// Short, user-facing PCB name (the board file's name), set at collection
    /// from the enumerated `PcbInfo`; empty when unknown.
    pub name: String,
    pub thickness: Option<Length>,
    pub bounding_box: Option<BoardBoundingBox>,
    pub edge_shapes: Vec<BoardEdgeShape>,
    pub holes: Vec<BoardHole>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoardBoundingBox {
    pub x: Length,
    pub y: Length,
    pub width: Length,
    pub height: Length,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoardPoint {
    pub x: Length,
    pub y: Length,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BoardEdgeShape {
    Track {
        id: Option<String>,
        start: BoardPoint,
        end: BoardPoint,
        width: Option<Length>,
    },
    Arc {
        id: Option<String>,
        start: BoardPoint,
        mid: BoardPoint,
        end: BoardPoint,
        width: Option<Length>,
    },
    GraphicSegment {
        id: Option<String>,
        start: BoardPoint,
        end: BoardPoint,
    },
    GraphicRectangle {
        id: Option<String>,
        top_left: BoardPoint,
        bottom_right: BoardPoint,
        corner_radius: Option<Length>,
    },
    GraphicArc {
        id: Option<String>,
        start: BoardPoint,
        mid: BoardPoint,
        end: BoardPoint,
    },
    GraphicCircle {
        id: Option<String>,
        center: BoardPoint,
        radius_point: BoardPoint,
    },
    GraphicBezier {
        id: Option<String>,
        start: BoardPoint,
        control1: BoardPoint,
        control2: BoardPoint,
        end: BoardPoint,
    },
    GraphicPolygon {
        id: Option<String>,
        polygon_count: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum HoleKind {
    Via,
    PadPth,
    PadNpth,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoardHole {
    pub id: Option<String>,
    pub kind: HoleKind,
    pub position: BoardPoint,
    pub drill_x: Option<Length>,
    pub drill_y: Option<Length>,
    pub plated: Option<bool>,
    /// Absolute board orientation of the drill, in degrees, when KiCad reports
    /// one. For an **oblong** hole (`drill_x != drill_y`) this is the rotation
    /// of the slot on the board: `drill_x`/`drill_y` give the slot's size in the
    /// pad's own frame, so both are needed to machine a rotated slot. Round
    /// holes and vias leave this `None` — orientation is immaterial to them.
    pub orientation_deg: Option<f64>,
}

/// Two drill axes closer than this are the same size. KiCad reports drills in nm, so a
/// nominally round hole can carry sub-micron noise between its axes; a genuine slot is
/// never this close to round.
pub const OBLONG_TOLERANCE_UM: f64 = 1.0;

/// A milled slot — an oblong hole resolved into the geometry every consumer needs.
///
/// `angle_deg` is the long axis in the **board frame**, measured from +X toward +Y. The
/// board frame is Y-down, the same as SVG's, so this angle is directly an SVG
/// `rotate()`; in board millimetres the axis unit vector is `(cos, sin)` of it.
///
/// Two conversions happen here and nowhere else. KiCad reports the pad angle
/// counter-clockwise *as displayed*, which is the opposite sense in a Y-down frame, so
/// it is negated. And `drill_x`/`drill_y` are the slot's size in the pad's **own** frame,
/// so a slot running along the pad's local Y stands a quarter turn off the pad angle.
/// Resolving both once, here, is what keeps the board preview and the machining plan
/// pointing the same way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    /// Slot centre, in board coordinates.
    pub center_x: Length,
    pub center_y: Length,
    /// The long axis, end to end (the major drill axis).
    pub length: Length,
    /// The across-axis width (the minor drill axis) — the widest cutter that fits.
    pub width: Length,
    /// Board-frame angle of the long axis, in degrees.
    pub angle_deg: f64,
}

impl Slot {
    /// How far the cutter/drill centre travels along the axis: the length less one
    /// width, since the end features are centred a half-width in from each end.
    pub fn travel(&self) -> Length {
        Length::from_mm((self.length.as_mm() - self.width.as_mm()).max(0.0))
    }

    /// A point `offset` along the long axis from the centre, in board coordinates.
    /// Negative offsets run toward the other end.
    pub fn point_at(&self, offset: Length) -> BoardPoint {
        let (sin, cos) = self.angle_deg.to_radians().sin_cos();
        BoardPoint {
            x: Length::from_mm(self.center_x.as_mm() + offset.as_mm() * cos),
            y: Length::from_mm(self.center_y.as_mm() + offset.as_mm() * sin),
        }
    }
}

impl BoardHole {
    /// The hole's drill axes as `(major, minor)`, or `None` when it reports no drill.
    /// A hole reporting only one axis is round on that axis.
    pub fn drill_axes(&self) -> Option<(Length, Length)> {
        let dx = self.drill_x.or(self.drill_y)?;
        let dy = self.drill_y.or(self.drill_x)?;
        Some(if dx.as_mm() >= dy.as_mm() { (dx, dy) } else { (dy, dx) })
    }

    /// This hole as a milled [`Slot`], or `None` when it is round. The single place a
    /// hole is classified oblong, so the tooling adapter, the machining plan and the
    /// board preview cannot disagree about which holes are slots.
    pub fn slot(&self) -> Option<Slot> {
        let (major, minor) = self.drill_axes()?;
        if major.as_um() - minor.as_um() <= OBLONG_TOLERANCE_UM {
            return None;
        }
        // Negated: KiCad's pad angle is CCW as displayed, the board frame is Y-down.
        let mut angle_deg = -self.orientation_deg.unwrap_or(0.0);
        // The long axis is the pad's local Y when `drill_y` is the major one.
        if self.drill_y.zip(self.drill_x).is_some_and(|(dy, dx)| dy.as_mm() > dx.as_mm()) {
            angle_deg += 90.0;
        }
        Some(Slot {
            center_x: self.position.x,
            center_y: self.position.y,
            length: major,
            width: minor,
            angle_deg,
        })
    }
}

/// Collect a [`BoardSnapshot`] from one KiCad instance client.
///
/// The client must already be pointed at the intended instance (see
/// [`crate::KiCad::collect_snapshot`]). Returns an empty snapshot when the
/// instance has no open board rather than erroring.
pub(crate) fn collect(client: &Client) -> Result<BoardSnapshot, String> {
    let has_board = !client
        .get_open_documents(DocumentType::Pcb)
        .map_err(|e| format!("failed to query open board state: {e}"))?
        .is_empty();
    if !has_board {
        return Ok(BoardSnapshot {
            name: String::new(),
            thickness: None,
            bounding_box: None,
            edge_shapes: Vec::new(),
            holes: Vec::new(),
        });
    }

    let board_thickness = collect_board_thickness_from_stackup(client);

    let mut edge_shapes = Vec::new();
    let mut edge_item_ids = Vec::new();
    let mut holes = Vec::new();

    // Query only item families we need instead of requesting every KiCad object
    // type. This avoids AS_BAD_REQUEST on versions that reject broad type lists.

    const KOT_PCB_PAD: i32 = 2;
    const KOT_PCB_SHAPE: i32 = 3;
    const KOT_PCB_TRACE: i32 = 11;
    const KOT_PCB_ARC: i32 = 13;

    let vias = client
        .get_vias()
        .map_err(|e| format!("failed to fetch vias: {e}"))?;
    for via in vias {
        if let Some(position_nm) = via.position_nm {
            let (drill_x, drill_y) = extract_drill_diameter(&via.pad_stack);
            holes.push(BoardHole {
                id: via.id,
                kind: HoleKind::Via,
                position: point_from_nm(position_nm),
                drill_x,
                drill_y,
                plated: Some(true),
                // KiCad only supports circular via drills, so a via has no
                // meaningful slot orientation.
                orientation_deg: None,
            });
        }
    }

    let pad_items = safe_get_items_by_type_codes(client, vec![KOT_PCB_PAD]);
    for item in pad_items {
        if let PcbItem::Pad(pad) = item {
            if let Some(position_nm) = pad.position_nm {
                let kind = match pad.pad_type {
                    PcbPadType::Pth => Some((HoleKind::PadPth, Some(true))),
                    PcbPadType::Npth => Some((HoleKind::PadNpth, Some(false))),
                    _ => None, // SMD, EdgeConnector, Unknown — no drill
                };
                if let Some((kind, plated)) = kind {
                    let (drill_x, drill_y) = extract_drill_diameter(&pad.pad_stack);
                    let orientation_deg =
                        oblong_orientation(extract_pad_angle(&pad.pad_stack), drill_x, drill_y);
                    holes.push(BoardHole {
                        id: pad.id,
                        kind,
                        position: point_from_nm(position_nm),
                        drill_x,
                        drill_y,
                        plated,
                        orientation_deg,
                    });
                }
            }
        }
    }

    let track_items = safe_get_items_by_type_codes(client, vec![KOT_PCB_TRACE]);
    let mut layers_id: Vec<String> = Vec::new();
    for item in track_items {
        if let PcbItem::Track(track) = item {
            let layer_name = track.layer.name.as_str();

            if !layers_id.contains(&layer_name.to_string()) {
                layers_id.push(layer_name.to_string());
            }

            if track.layer.name == "BL_Edge_Cuts" {
                if let (Some(start), Some(end)) = (track.start_nm, track.end_nm) {
                    edge_shapes.push(BoardEdgeShape::Track {
                        id: track.id.clone(),
                        start: point_from_nm(start),
                        end: point_from_nm(end),
                        width: track.width_nm.map(Length::from_nm),
                    });
                }
                if let Some(id) = track.id {
                    edge_item_ids.push(id);
                }
            }
        }
    }

    let arc_items = safe_get_items_by_type_codes(client, vec![KOT_PCB_ARC]);
    for item in arc_items {
        if let PcbItem::Arc(arc) = item {
            if arc.layer.name == "BL_Edge_Cuts" {
                if let (Some(start), Some(mid), Some(end)) = (arc.start_nm, arc.mid_nm, arc.end_nm) {
                    edge_shapes.push(BoardEdgeShape::Arc {
                        id: arc.id.clone(),
                        start: point_from_nm(start),
                        mid: point_from_nm(mid),
                        end: point_from_nm(end),
                        width: arc.width_nm.map(Length::from_nm),
                    });
                }
                if let Some(id) = arc.id {
                    edge_item_ids.push(id);
                }
            }
        }
    }

    let shape_items = safe_get_items_by_type_codes(client, vec![KOT_PCB_SHAPE]);
    for item in shape_items {
        if let PcbItem::BoardGraphicShape(shape) = item {
            if shape.layer.name == "BL_Edge_Cuts" {
                if let Some(edge_shape) = edge_shape_from_graphic(&shape.id, &shape.geometry) {
                    edge_shapes.push(edge_shape);
                }
                if let Some(id) = shape.id {
                    edge_item_ids.push(id);
                }
            }
        }
    }

    // Try to compute bounding box from Edge.Cuts items via IPC bounding-box query.
    let bounding_box = if !edge_item_ids.is_empty() {
        let bboxes = client
            .get_item_bounding_boxes(edge_item_ids, false)
            .unwrap_or_default();

        let mut min_x: Option<i64> = None;
        let mut min_y: Option<i64> = None;
        let mut max_x: Option<i64> = None;
        let mut max_y: Option<i64> = None;

        for bb in bboxes {
            let right = bb.x_nm + bb.width_nm;
            let bottom = bb.y_nm + bb.height_nm;

            min_x = Some(min_x.map_or(bb.x_nm, |v| v.min(bb.x_nm)));
            min_y = Some(min_y.map_or(bb.y_nm, |v| v.min(bb.y_nm)));
            max_x = Some(max_x.map_or(right, |v| v.max(right)));
            max_y = Some(max_y.map_or(bottom, |v| v.max(bottom)));
        }

        match (min_x, min_y, max_x, max_y) {
            (Some(x0), Some(y0), Some(x1), Some(y1)) => Some(BoardBoundingBox {
                x: Length::from_nm(x0),
                y: Length::from_nm(y0),
                width: Length::from_nm((x1 - x0).max(0)),
                height: Length::from_nm((y1 - y0).max(0)),
            }),
            _ => None,
        }
    } else {
        None
    };

    // Fall back: derive bounding box from hole positions when Edge.Cuts returned nothing.
    let bounding_box = bounding_box.or_else(|| {
        let mut min_x: Option<f64> = None;
        let mut min_y: Option<f64> = None;
        let mut max_x: Option<f64> = None;
        let mut max_y: Option<f64> = None;
        for hole in &holes {
            let x = hole.position.x.as_nm();
            let y = hole.position.y.as_nm();
            min_x = Some(min_x.map_or(x, |v: f64| v.min(x)));
            min_y = Some(min_y.map_or(y, |v: f64| v.min(y)));
            max_x = Some(max_x.map_or(x, |v: f64| v.max(x)));
            max_y = Some(max_y.map_or(y, |v: f64| v.max(y)));
        }
        match (min_x, min_y, max_x, max_y) {
            (Some(x0), Some(y0), Some(x1), Some(y1)) => {
                // Add 5% padding on each side so edge holes aren't clipped.
                let w = (x1 - x0).max(1.0);
                let h = (y1 - y0).max(1.0);
                let pad_x = w * 0.05;
                let pad_y = h * 0.05;
                Some(BoardBoundingBox {
                    x: Length::from_nm((x0 - pad_x) as i64),
                    y: Length::from_nm((y0 - pad_y) as i64),
                    width: Length::from_nm((w + pad_x * 2.0) as i64),
                    height: Length::from_nm((h + pad_y * 2.0) as i64),
                })
            }
            _ => None,
        }
    });

    Ok(BoardSnapshot {
        name: String::new(),
        thickness: board_thickness,
        bounding_box,
        edge_shapes,
        holes,
    })
}

fn collect_board_thickness_from_stackup(client: &Client) -> Option<Length> {
    let stackup = client.get_board_stackup().ok()?;

    let sum_nm: i64 = stackup
        .layers
        .iter()
        .filter(|layer| {
            matches!(
                layer.layer_type,
                BoardStackupLayerType::Copper | BoardStackupLayerType::Dielectric
            )
        })
        .filter_map(|layer| layer.thickness_nm)
        .filter(|thickness_nm| *thickness_nm > 0)
        .sum();

    if sum_nm > 0 {
        return Some(Length::from_nm(sum_nm));
    }

    None
}

fn safe_get_items_by_type_codes(client: &Client, type_codes: Vec<i32>) -> Vec<PcbItem> {
    client
        .get_items_by_type_codes(type_codes)
        .unwrap_or_else(|_| Vec::new())
}

fn point_from_nm(v: Vector2Nm) -> BoardPoint {
    BoardPoint {
        x: Length::from_nm(v.x_nm),
        y: Length::from_nm(v.y_nm),
    }
}

fn extract_drill_diameter(pad_stack: &Option<PcbPadStack>) -> (Option<Length>, Option<Length>) {
    let drill = pad_stack.as_ref().and_then(|s| s.drill.as_ref());
    let d = drill.and_then(|d| d.diameter_nm);
    match d {
        Some(v) => (Some(Length::from_nm(v.x_nm)), Some(Length::from_nm(v.y_nm))),
        None => (None, None),
    }
}

/// The pad-stack orientation of a pad, in degrees, or `None`.
///
/// Reads the pad orientation surfaced by our kicad-ipc-rs fork
/// (`PcbPadStack.angle_degrees`); returns `None` when the pad reports none.
fn extract_pad_angle(pad_stack: &Option<PcbPadStack>) -> Option<f64> {
    pad_stack.as_ref().and_then(|s| s.angle_degrees)
}

/// The slot orientation for an **oblong** pad, in degrees (absolute board
/// frame), or `None` for a round drill.
///
/// KiCad reports a pad-stack angle for *every* pad — including round ones on a
/// rotated footprint — but the angle only matters for a milled slot, whose
/// length runs along the pad's local X (`drill_x`) rotated by it. We therefore
/// keep the orientation only when the drill is genuinely oblong (both axes
/// known and unequal), leaving round holes orientation-free per
/// [`BoardHole::orientation_deg`]. Kept pure (no KiCad types) so it is unit
/// testable.
fn oblong_orientation(
    angle_deg: Option<f64>,
    drill_x: Option<Length>,
    drill_y: Option<Length>,
) -> Option<f64> {
    let is_oblong = drill_x.is_some() && drill_y.is_some() && drill_x != drill_y;
    if is_oblong {
        angle_deg
    } else {
        None
    }
}

fn edge_shape_from_graphic(
    id: &Option<String>,
    geometry: &Option<PcbGraphicShapeGeometry>,
) -> Option<BoardEdgeShape> {
    let geometry = geometry.as_ref()?;
    match geometry {
        PcbGraphicShapeGeometry::Segment { start_nm, end_nm } => {
            Some(BoardEdgeShape::GraphicSegment {
                id: id.clone(),
                start: point_from_nm(start_nm.to_owned()?),
                end: point_from_nm(end_nm.to_owned()?),
            })
        }
        PcbGraphicShapeGeometry::Rectangle {
            top_left_nm,
            bottom_right_nm,
            corner_radius_nm,
        } => Some(BoardEdgeShape::GraphicRectangle {
            id: id.clone(),
            top_left: point_from_nm(top_left_nm.to_owned()?),
            bottom_right: point_from_nm(bottom_right_nm.to_owned()?),
            corner_radius: corner_radius_nm.map(Length::from_nm),
        }),
        PcbGraphicShapeGeometry::Arc {
            start_nm,
            mid_nm,
            end_nm,
        } => Some(BoardEdgeShape::GraphicArc {
            id: id.clone(),
            start: point_from_nm(start_nm.to_owned()?),
            mid: point_from_nm(mid_nm.to_owned()?),
            end: point_from_nm(end_nm.to_owned()?),
        }),
        PcbGraphicShapeGeometry::Circle {
            center_nm,
            radius_point_nm,
        } => Some(BoardEdgeShape::GraphicCircle {
            id: id.clone(),
            center: point_from_nm(center_nm.to_owned()?),
            radius_point: point_from_nm(radius_point_nm.to_owned()?),
        }),
        PcbGraphicShapeGeometry::Bezier {
            start_nm,
            control1_nm,
            control2_nm,
            end_nm,
        } => Some(BoardEdgeShape::GraphicBezier {
            id: id.clone(),
            start: point_from_nm(start_nm.to_owned()?),
            control1: point_from_nm(control1_nm.to_owned()?),
            control2: point_from_nm(control2_nm.to_owned()?),
            end: point_from_nm(end_nm.to_owned()?),
        }),
        PcbGraphicShapeGeometry::Polygon { polygon_count } => {
            Some(BoardEdgeShape::GraphicPolygon {
                id: id.clone(),
                polygon_count: *polygon_count,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mm(v: f64) -> Option<Length> {
        Some(Length::from_mm(v))
    }

    #[test]
    fn a_round_drill_stays_orientation_free_even_when_the_pad_is_rotated() {
        // Equal axes → round; a rotated footprint still reports an angle, but it
        // is meaningless for a round hole, so we drop it.
        assert_eq!(oblong_orientation(Some(90.0), mm(1.0), mm(1.0)), None);
    }

    #[test]
    fn an_oblong_drill_keeps_its_reported_orientation() {
        assert_eq!(oblong_orientation(Some(37.5), mm(3.0), mm(1.2)), Some(37.5));
    }

    #[test]
    fn an_oblong_drill_without_a_reported_angle_is_none() {
        assert_eq!(oblong_orientation(None, mm(3.0), mm(1.2)), None);
    }

    #[test]
    fn incomplete_drill_dimensions_are_not_treated_as_oblong() {
        // A single known axis is not enough to call it a slot.
        assert_eq!(oblong_orientation(Some(45.0), mm(3.0), None), None);
        assert_eq!(oblong_orientation(Some(45.0), None, mm(1.2)), None);
    }

    // --- BoardHole::drill_axes / slot -------------------------------------

    fn hole(drill_x_mm: f64, drill_y_mm: f64, angle_deg: Option<f64>) -> BoardHole {
        BoardHole {
            id: None,
            kind: HoleKind::PadPth,
            position: BoardPoint { x: Length::from_mm(0.0), y: Length::from_mm(0.0) },
            drill_x: mm(drill_x_mm),
            drill_y: mm(drill_y_mm),
            plated: Some(true),
            orientation_deg: angle_deg,
        }
    }

    /// The axes come back largest-first whichever way round KiCad reports them, so the
    /// long axis is always the slot's length.
    #[test]
    fn drill_axes_are_ordered_major_then_minor() {
        for (x, y) in [(3.2, 1.6), (1.6, 3.2)] {
            let (major, minor) = hole(x, y, None).drill_axes().expect("both axes known");
            assert_eq!((major.as_mm(), minor.as_mm()), (3.2, 1.6));
        }
    }

    /// A via reporting only one axis is round on that axis, not a hair-thin slot.
    #[test]
    fn a_single_reported_axis_reads_as_round() {
        let mut via = hole(0.6, 0.6, None);
        via.drill_y = None;
        assert!(via.drill_axes().is_some(), "one axis is enough to size it");
        assert!(via.slot().is_none(), "and it is not a slot");
    }

    /// Equal axes are round even carrying the sub-micron noise KiCad's nm units allow.
    #[test]
    fn near_equal_axes_are_not_slots() {
        assert!(hole(1.0, 1.0, None).slot().is_none());
        assert!(hole(1.0, 1.0 - OBLONG_TOLERANCE_UM / 2000.0, None).slot().is_none());
        assert!(hole(3.2, 1.6, None).slot().is_some());
    }

    /// The pad angle is negated into the board's Y-down frame, and a slot whose length
    /// runs along the pad's local Y stands a quarter turn off it.
    #[test]
    fn the_slot_axis_folds_in_the_pad_angle_and_the_long_axis() {
        assert_eq!(hole(3.2, 1.6, Some(30.0)).slot().unwrap().angle_deg, -30.0);
        assert_eq!(hole(1.6, 3.2, Some(30.0)).slot().unwrap().angle_deg, 60.0);
        // No reported angle → axis-aligned, not a silent rotation.
        assert_eq!(hole(3.2, 1.6, None).slot().unwrap().angle_deg, 0.0);
    }

    /// Travel is the length less one width — the end features are centred a half-width
    /// in from each end — and points walk the slot's own axis.
    #[test]
    fn slot_travel_and_points_follow_the_axis() {
        let slot = hole(3.2, 1.6, Some(-90.0)).slot().expect("oblong");
        assert!((slot.travel().as_mm() - 1.6).abs() < 1e-9);

        // -90° in KiCad negates to +90° in the board frame: the axis runs along +Y.
        let end = slot.point_at(Length::from_mm(0.8));
        assert!(end.x.as_mm().abs() < 1e-9, "x is unchanged: {}", end.x.as_mm());
        assert!((end.y.as_mm() - 0.8).abs() < 1e-9);
    }
}
