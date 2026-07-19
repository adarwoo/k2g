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
                    holes.push(BoardHole {
                        id: pad.id,
                        kind,
                        position: point_from_nm(position_nm),
                        drill_x,
                        drill_y,
                        plated,
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
