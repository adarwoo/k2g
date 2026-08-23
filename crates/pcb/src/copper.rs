//! The copper on a board layer, resolved into plain polygons.
//!
//! Isolation engraving asks one question of a board — *where is the copper, and which net
//! does each piece belong to* — and KiCad answers it in five different shapes: tracks and
//! arcs as centrelines with a width, pads as per-layer outlines, vias as a padstack, zones
//! as a computed fill, and the occasional graphic drawn straight onto a copper layer. This
//! module flattens all five into one list of net-tagged polygons so nothing downstream has
//! to care which was which.
//!
//! **The fill is KiCad's, not ours.** A poured zone arrives with clearance, thermal relief
//! and island removal already applied (`PcbZone::filled_polygons`). Recomputing that would
//! mean reimplementing KiCad's filler, and getting it subtly wrong would mean cutting
//! through a plane that the schematic says is continuous. So an *unfilled* zone is
//! reported rather than passed over: an unfilled pour and no pour at all look identical
//! from here, and only one of them is what the operator meant.
//!
//! Coordinates stay in board nanometres, as `i64`, the way the stitcher's do — this is
//! geometry on its way to Clipper, not lengths on their way to a human.

use kicad_ipc_rs::{
    PcbItem, PcbZoneType, PolyLineNm, PolyLineNodeGeometryNm, PolygonWithHolesNm, Vector2Nm,
};

use crate::snapshot::Client;
use crate::stitching::tessellate;

/// A closed polygon with its holes, in board nanometres.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polygon {
    pub outline: Vec<(i64, i64)>,
    /// Inner rings. A zone fill puts its clearance cut-outs here.
    pub holes: Vec<Vec<(i64, i64)>>,
}

/// Where a piece of copper came from. Kept for diagnostics only — the geometry is the
/// same either way — but "no copper found" is a very different bug depending on which of
/// these is missing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopperSource {
    Track,
    Arc,
    Pad,
    Via,
    Zone,
    Graphic,
}

/// One piece of copper on one layer.
#[derive(Clone, Debug, PartialEq)]
pub struct CopperFeature {
    /// Net name, or empty for copper on no net. Names, not codes: KiCad's proto marks
    /// `Net.code` deprecated and says net codes are no longer used.
    pub net: String,
    pub source: CopperSource,
    pub polygons: Vec<Polygon>,
}

/// Every piece of copper on one layer, with whatever could not be read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CopperSnapshot {
    /// KiCad layer id. Copper is exactly `3..=34` (`BL_F_Cu` .. `BL_B_Cu`).
    pub layer_id: i32,
    pub features: Vec<CopperFeature>,
    /// Things the operator has to know about before trusting the result — an unfilled
    /// zone, a via with no ring, a pad KiCad would not resolve.
    pub warnings: Vec<String>,
    /// **This describes less copper than the board has.** Nothing may be machined from it.
    ///
    /// A read that KiCad refused returns the same empty list as a layer with nothing on it,
    /// and the two are completely different facts. Isolating a partial reading produces
    /// contours that look perfectly reasonable around the copper that was seen, and no
    /// account at all of the copper that was not — a board whose nets come out joined with
    /// every diagnostic silent, which is the one outcome this crate is built to prevent.
    pub partial: bool,
}

/// KiCad's copper layer ids, `BL_F_Cu` through `BL_B_Cu`.
///
/// The enum lays the copper layers out contiguously, so this is a range check rather than
/// a lookup table — see `BoardLayer` in the generated protos.
pub const COPPER_LAYER_IDS: std::ops::RangeInclusive<i32> = 3..=34;

pub fn is_copper_layer(layer_id: i32) -> bool {
    COPPER_LAYER_IDS.contains(&layer_id)
}

/// The two layers a mill can actually reach.
///
/// Isolation engraving cuts the surface, so only the outer copper is a candidate however
/// many layers the board has. A four-layer board is not made this way at all, and saying
/// so is more use than quietly engraving two of its four layers.
pub const FRONT_COPPER: i32 = 3;
pub const BACK_COPPER: i32 = 34;

fn nm(v: Vector2Nm) -> (i64, i64) {
    (v.x_nm, v.y_nm)
}

/// One IPC polyline flattened to points, arcs tessellated.
///
/// KiCad sends polygon rings whose nodes are *either* a point or a whole arc, so a ring
/// cannot simply be read as a vertex list — a pad with a rounded corner would come out
/// with the corner chopped off.
fn ring_points(line: &PolyLineNm) -> Vec<(i64, i64)> {
    let mut pts: Vec<(i64, i64)> = Vec::with_capacity(line.nodes.len());
    for node in &line.nodes {
        match node {
            PolyLineNodeGeometryNm::Point(p) => pts.push(nm(*p)),
            PolyLineNodeGeometryNm::Arc(arc) => {
                let (s, m, e) = (nm(arc.start), nm(arc.mid), nm(arc.end));
                if pts.last() != Some(&s) {
                    pts.push(s);
                }
                tessellate::tessellate_arc(
                    &mut pts, s.0 as f64, s.1 as f64, m.0 as f64, m.1 as f64, e.0 as f64,
                    e.1 as f64,
                );
                pts.push(e);
            }
        }
    }
    pts
}

fn polygon_from_nm(polygon: &PolygonWithHolesNm) -> Option<Polygon> {
    let outline = polygon.outline.as_ref().map(ring_points)?;
    if outline.len() < 3 {
        return None;
    }
    Some(Polygon {
        outline,
        holes: polygon
            .holes
            .iter()
            .map(ring_points)
            .filter(|h| h.len() >= 3)
            .collect(),
    })
}

/// A track or arc centreline swollen to its width.
///
/// A track is drawn as a line with a width, and copper is an area, so the centreline has
/// to be given its body before anything can be offset around it. Round ends because that
/// is how KiCad renders a track end, and a square end here would leave the isolation pass
/// cutting a corner that is not on the board.
fn stroke(points: &[(i64, i64)], width_nm: i64) -> Vec<Polygon> {
    crate::stitching::stroke_open_path(points, width_nm as f64 / 2.0)
        .into_iter()
        .filter(|p| p.len() >= 3)
        .map(|outline| Polygon { outline, holes: Vec::new() })
        .collect()
}

/// How long to wait for KiCad to finish re-pouring before giving up on a read.
///
/// Generous, because the alternative to waiting is reading the wrong board: a big ground
/// pour on a slow machine is seconds, and this only runs when something is about to cut
/// copper. Exceeded, the read is **refused** rather than attempted — see [`wait_until_idle`].
const REFILL_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// How often to ask whether the fill has finished.
const REFILL_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Collects every piece of copper on `layer_id`, retrying while KiCad is busy.
///
/// `refill` asks KiCad to re-pour its zones first. That is worth doing before a cut and
/// worth *not* doing on every refresh.
///
/// # The race this exists to lose safely
///
/// `RefillZones` is documented by KiCad as returning immediately and then answering
/// **`AS_BUSY` to every subsequent request until the fill completes**. Reading straight
/// after it is therefore a race against a pour, and one that was mostly lost: on a board
/// with a ground plane, ten reads in a row returned
///
/// ```text
///   1, 0, 1, 0, 0, 119, 0, 0, 119, 0     features (119 is the right answer)
/// ```
///
/// Every one of those wrong answers reached the isolation pass as *fact*, because each
/// collector turned its refused request into an empty list. Two of them are the dangerous
/// kind — a **partial** read, from which the pass cuts plausible-looking contours round the
/// copper it saw and gives no account whatever of the copper it did not.
///
/// # Retried on the real signal, not on a proxy
///
/// The obvious fix — poll a cheap board-level call until it answers, then read — does not
/// work: `get_board_enabled_layers` is *not* gated by the busy state, so it reports ready
/// while `GetItems` is still refusing. Rather than hunt for a call whose gating matches,
/// this retries the whole read and stops when the read itself comes back clean. That is the
/// condition actually wanted, and it holds however KiCad's gating is arranged in a future
/// version.
///
/// A snapshot whose [`CopperSnapshot::partial`] flag is set describes less copper than the
/// board has and must not be machined from. Nothing here returns an empty snapshot to mean
/// "no copper" without saying which it is.
pub fn collect_copper(client: &Client, layer_id: i32, refill: bool) -> CopperSnapshot {
    if !is_copper_layer(layer_id) {
        return CopperSnapshot {
            layer_id,
            warnings: vec![format!("Layer {layer_id} is not a copper layer.")],
            partial: true,
            ..Default::default()
        };
    }
    if refill {
        // A stale fill is worse than a slow read: it is copper that is no longer there.
        let _ = client.refill_zones(Vec::new());
    }

    let deadline = std::time::Instant::now() + REFILL_WAIT;
    loop {
        let snap = collect_once(client, layer_id);
        if !snap.partial || std::time::Instant::now() >= deadline {
            return snap;
        }
        std::thread::sleep(REFILL_POLL);
    }
}

/// One pass over the board's copper. Partial whenever KiCad refused any part of it.
fn collect_once(client: &Client, layer_id: i32) -> CopperSnapshot {
    let mut snap = CopperSnapshot { layer_id, ..Default::default() };
    collect_tracks_and_arcs(client, layer_id, &mut snap);
    collect_pads(client, layer_id, &mut snap);
    collect_vias(client, layer_id, &mut snap);
    collect_zones(client, layer_id, &mut snap);
    snap
}

/// Records that a read failed, so its emptiness is never read as an absence of copper.
///
/// The one thing this file must not do is return silence. Every collector below asks KiCad
/// for a class of item and every one of those requests can fail — most often with `AS_BUSY`
/// during a pour, but equally if the board is closed mid-read. An unanswered question and
/// an answer of "none" are the same empty list and completely different facts.
fn read_failed(snap: &mut CopperSnapshot, what: &str, err: impl std::fmt::Display) {
    snap.partial = true;
    snap.warnings.push(format!(
        "KiCad would not return the {what} on this layer ({err}), so any copper they carry \
         is missing from this reading of the board."
    ));
}

fn layer_matches(id: i32, layer_id: i32) -> bool {
    id == layer_id
}

fn collect_tracks_and_arcs(client: &Client, layer_id: i32, snap: &mut CopperSnapshot) {
    const KOT_PCB_TRACE: i32 = 11;
    const KOT_PCB_ARC: i32 = 13;

    let items = match client.get_items_by_type_codes(vec![KOT_PCB_TRACE, KOT_PCB_ARC]) {
        Ok(items) => items,
        Err(err) => return read_failed(snap, "tracks and arcs", err),
    };
    for item in items {
        match item {
            PcbItem::Track(track) if layer_matches(track.layer.id, layer_id) => {
                let (Some(start), Some(end)) = (track.start_nm, track.end_nm) else { continue };
                let width = track.width_nm.unwrap_or(0);
                let polygons = stroke(&[nm(start), nm(end)], width);
                if polygons.is_empty() {
                    continue;
                }
                snap.features.push(CopperFeature {
                    net: track.net.map(|n| n.name).unwrap_or_default(),
                    source: CopperSource::Track,
                    polygons,
                });
            }
            PcbItem::Arc(arc) if layer_matches(arc.layer.id, layer_id) => {
                let (Some(s), Some(m), Some(e)) = (arc.start_nm, arc.mid_nm, arc.end_nm) else {
                    continue;
                };
                let (s, m, e) = (nm(s), nm(m), nm(e));
                let mut pts = vec![s];
                tessellate::tessellate_arc(
                    &mut pts, s.0 as f64, s.1 as f64, m.0 as f64, m.1 as f64, e.0 as f64,
                    e.1 as f64,
                );
                pts.push(e);
                let polygons = stroke(&pts, arc.width_nm.unwrap_or(0));
                if polygons.is_empty() {
                    continue;
                }
                snap.features.push(CopperFeature {
                    net: arc.net.map(|n| n.name).unwrap_or_default(),
                    source: CopperSource::Arc,
                    polygons,
                });
            }
            _ => {}
        }
    }
}

fn collect_pads(client: &Client, layer_id: i32, snap: &mut CopperSnapshot) {
    const KOT_PCB_PAD: i32 = 2;

    // Pad ids and their nets, from the items; the *shapes* come from a separate call,
    // because a padstack does not describe its own final outline — a custom pad is a set
    // of primitives KiCad merges, and asking it to do the merging is both correct and
    // very much cheaper than reimplementing it.
    let mut ids: Vec<String> = Vec::new();
    let mut net_of: std::collections::HashMap<String, String> = Default::default();
    let pads = match client.get_items_by_type_codes(vec![KOT_PCB_PAD]) {
        Ok(items) => items,
        Err(err) => return read_failed(snap, "pads", err),
    };
    for item in pads {
        if let PcbItem::Pad(pad) = item {
            let Some(id) = pad.id.clone() else { continue };
            net_of.insert(id.clone(), pad.net.map(|n| n.name).unwrap_or_default());
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return;
    }

    // A pad is not necessarily on every layer — unconnected-layer removal takes it off
    // the ones it is not used on, and a padstack cannot be asked about that in isolation.
    let present: std::collections::HashSet<String> = client
        .check_padstack_presence_on_layers(ids.clone(), vec![layer_id])
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.presence == kicad_ipc_rs::PadstackPresenceState::Present)
        .map(|e| e.item_id)
        .collect();
    let wanted: Vec<String> = if present.is_empty() {
        ids.clone() // presence unavailable: better to over-report copper than to miss it
    } else {
        ids.iter().filter(|id| present.contains(*id)).cloned().collect()
    };
    if wanted.is_empty() {
        return;
    }

    match client.get_pad_shape_as_polygon(wanted, layer_id) {
        Ok(entries) => {
            for entry in entries {
                let Some(polygon) = polygon_from_nm(&entry.polygon) else { continue };
                snap.features.push(CopperFeature {
                    net: net_of.get(&entry.pad_id).cloned().unwrap_or_default(),
                    source: CopperSource::Pad,
                    polygons: vec![polygon],
                });
            }
        }
        Err(err) => snap
            .warnings
            .push(format!("KiCad could not resolve the pad shapes on this layer: {err}")),
    }
}

fn collect_vias(client: &Client, layer_id: i32, snap: &mut CopperSnapshot) {
    let mut ringless = 0usize;
    let vias = match client.get_vias() {
        Ok(vias) => vias,
        Err(err) => return read_failed(snap, "vias", err),
    };
    for via in vias {
        let Some(position) = via.position_nm else { continue };
        let Some(stack) = via.pad_stack.as_ref() else { continue };

        // The ring for this layer, or the stack's single entry when it describes one pad
        // for all layers — which is the common case for a plain through via.
        let ring = stack
            .copper_layers
            .iter()
            .find(|l| layer_matches(l.layer.id, layer_id))
            .or_else(|| stack.copper_layers.first());
        let Some(size) = ring.and_then(|r| r.size_nm) else {
            ringless += 1;
            continue;
        };

        let (cx, cy) = nm(position);
        let (rx, ry) = (size.x_nm as f64 / 2.0, size.y_nm as f64 / 2.0);
        if rx <= 0.0 || ry <= 0.0 {
            ringless += 1;
            continue;
        }
        let mut pts = Vec::new();
        tessellate::tessellate_circle(&mut pts, cx as f64, cy as f64, cx as f64 + rx, cy as f64);
        if pts.len() < 3 {
            continue;
        }
        snap.features.push(CopperFeature {
            net: via.net.map(|n| n.name).unwrap_or_default(),
            source: CopperSource::Via,
            polygons: vec![Polygon { outline: pts, holes: Vec::new() }],
        });
        let _ = ry; // round vias only; an oval via is not a thing KiCad makes
    }
    if ringless > 0 {
        snap.warnings.push(format!(
            "{ringless} via(s) report no copper ring on this layer and were left out. \
             Anything routed around them will not isolate them."
        ));
    }
}

fn collect_zones(client: &Client, layer_id: i32, snap: &mut CopperSnapshot) {
    const KOT_PCB_ZONE: i32 = 16;

    let mut unfilled: Vec<String> = Vec::new();
    let zones = match client.get_items_by_type_codes(vec![KOT_PCB_ZONE]) {
        Ok(items) => items,
        Err(err) => return read_failed(snap, "zones", err),
    };
    for item in zones {
        let PcbItem::Zone(zone) = item else { continue };
        if zone.zone_type != PcbZoneType::Copper {
            continue; // a rule area is not copper, whatever its outline says
        }
        if !zone.layers.iter().any(|l| layer_matches(l.id, layer_id)) {
            continue;
        }

        // An unfilled pour contributes nothing and looks exactly like no pour. Named, so
        // the operator can go and fill it rather than wonder where the plane went.
        if !zone.filled {
            unfilled.push(if zone.name.is_empty() {
                zone.net.as_ref().map(|n| n.name.clone()).unwrap_or_else(|| "unnamed".into())
            } else {
                zone.name.clone()
            });
            continue;
        }

        let net = zone.net.as_ref().map(|n| n.name.clone()).unwrap_or_default();
        for entry in &zone.filled_polygons {
            if !layer_matches(entry.layer.id, layer_id) {
                continue;
            }
            let polygons: Vec<Polygon> =
                entry.shapes.iter().filter_map(polygon_from_nm).collect();
            if polygons.is_empty() {
                continue;
            }
            snap.features.push(CopperFeature {
                net: net.clone(),
                source: CopperSource::Zone,
                polygons,
            });
        }
    }

    if !unfilled.is_empty() {
        snap.warnings.push(format!(
            "{} zone(s) on this layer are not filled and contribute no copper ({}). \
             Fill them in KiCad — an unfilled pour is indistinguishable from no pour here, \
             and anything relying on it would be isolated from nothing.",
            unfilled.len(),
            unfilled.join(", ")
        ));
    }
}
