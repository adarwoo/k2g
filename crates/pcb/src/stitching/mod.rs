//! Edge-cut stitching: chain `BoardEdgeShape` primitives into closed contours,
//! classify their nesting (outer board boundary vs holes), and compute
//! inside/outside routing offsets.
//!
//! # Pipeline
//!
//! 1. **Tessellate** — curved shapes (arcs, beziers, circles, rounded rects)
//!    are converted to polylines of `(i64, i64)` nm coordinates.
//! 2. **Stitch** — open primitives are chained by matching endpoints within a
//!    tolerance of `STITCH_TOLERANCE_NM`.  Closed primitives (circles,
//!    rectangles) are promoted directly.  Unclosed chains are collected as
//!    warnings.
//! 3. **Nesting** — the stitched closed polygons are classified by containment
//!    depth using point-in-polygon tests:
//!    - Depth-0 contours → outer board boundaries
//!    - Depth-1 contours → holes / inner cutouts
//!    - Depth-2+ → floating island (error)
//! 4. **Validation** — the result is checked before it is returned:
//!    - Any unclosed endpoint gap → error (open contour)
//!    - Any contour at depth ≥ 2 → error (floating island inside a hole)
//!    Invalid results carry a non-empty `errors` vec; callers must check
//!    `errors.is_empty()` before using contours for further processing.
//! 5. **Offset** — each contour is offset by `±tool_radius_nm`:
//!    - Outer boundaries → negative offset (route *inside*)
//!    - Inner holes      → positive offset (route *outside*)

pub mod corners;
pub mod tessellate;

use clipper2_rust::core::{Path64, Point64};

use crate::snapshot::{BoardEdgeShape, BoardPoint};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single ordered move along a contour, preserving KiCad's original geometry type
/// (operation-planner.md §3): a straight `Line`, a 3-point `Arc`, or a cubic `Bezier`.
/// A contour is an ordered, closed loop of these; the routing phase renders each as its
/// matching primitive (`linear_cut` / `cut_arc` / `cut_bezier`) so arcs stay arcs
/// instead of being flattened into a fan of G1 chords. All coordinates are nm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    Line {
        start: (i64, i64),
        end: (i64, i64),
    },
    Arc {
        start: (i64, i64),
        mid: (i64, i64),
        end: (i64, i64),
    },
    Bezier {
        start: (i64, i64),
        control1: (i64, i64),
        control2: (i64, i64),
        end: (i64, i64),
    },
}

impl Segment {
    /// The point the move begins at.
    pub fn start(&self) -> (i64, i64) {
        match *self {
            Segment::Line { start, .. }
            | Segment::Arc { start, .. }
            | Segment::Bezier { start, .. } => start,
        }
    }

    /// The point the move ends at.
    pub fn end(&self) -> (i64, i64) {
        match *self {
            Segment::Line { end, .. } | Segment::Arc { end, .. } | Segment::Bezier { end, .. } => {
                end
            }
        }
    }

    /// The same move traversed in the opposite direction — used when stitching has to
    /// flip a fragment to chain it. Endpoints swap; an arc keeps its mid; a bezier
    /// swaps its two control points.
    fn reversed(self) -> Segment {
        match self {
            Segment::Line { start, end } => Segment::Line {
                start: end,
                end: start,
            },
            Segment::Arc { start, mid, end } => Segment::Arc {
                start: end,
                mid,
                end: start,
            },
            Segment::Bezier {
                start,
                control1,
                control2,
                end,
            } => Segment::Bezier {
                start: end,
                control1: control2,
                control2: control1,
                end: start,
            },
        }
    }

    /// The move with its start point moved to `p` (endpoint snapping). The other
    /// defining points are untouched, so a sub-tolerance snap barely perturbs an arc.
    fn with_start(self, p: (i64, i64)) -> Segment {
        match self {
            Segment::Line { end, .. } => Segment::Line { start: p, end },
            Segment::Arc { mid, end, .. } => Segment::Arc { start: p, mid, end },
            Segment::Bezier {
                control1,
                control2,
                end,
                ..
            } => Segment::Bezier {
                start: p,
                control1,
                control2,
                end,
            },
        }
    }
}

/// A closed, ordered contour in nm coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Contour {
    /// The **tessellated** polyline (curves flattened), last point implicitly closing
    /// to the first. This is the topology substrate — used for nesting/containment and
    /// the current clipper offset — kept even though `segments` carries the true shape.
    pub points: Vec<(i64, i64)>,
    /// The ordered, closed loop of **typed** moves (line/arc/bezier) that reproduces
    /// this contour without flattening (operation-planner.md §3). Endpoints are snapped
    /// for perfect continuity — `segments[i].end == segments[i+1].start` (wrapping) —
    /// so the routing phase can emit G1/G2/G3 directly.
    pub segments: Vec<Segment>,
    /// Derived from nesting depth: even → outer boundary, odd → hole.
    pub is_hole: bool,
}

/// Result of the full stitching pipeline for one `BoardSnapshot`.
#[derive(Debug, Clone)]
pub struct StitchResult {
    /// Closed contours sorted outer-first, then their holes.
    /// Only valid (and usable for processing) when `errors` is empty.
    pub contours: Vec<Contour>,
    /// Hard errors that prevent the board from being processed.
    /// An open contour or a floating island (depth ≥ 2) are both errors.
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tolerance
// ---------------------------------------------------------------------------

/// Two endpoints are considered coincident if their distance is within this
/// threshold.  10 µm is generous for PCB edge cuts, which are typically drawn
/// exactly.
const STITCH_TOLERANCE_NM: i64 = 10_000;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn nm(p: &BoardPoint) -> (i64, i64) {
    (p.x.as_nm() as i64, p.y.as_nm() as i64)
}

fn dist_sq(a: (i64, i64), b: (i64, i64)) -> i64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    // Use i128 to avoid overflow for large nm values, then clamp back.
    let d2 = (dx as i128) * (dx as i128) + (dy as i128) * (dy as i128);
    d2.min(i64::MAX as i128) as i64
}

fn close_enough(a: (i64, i64), b: (i64, i64)) -> bool {
    let t = STITCH_TOLERANCE_NM;
    dist_sq(a, b) <= t * t
}

// ---------------------------------------------------------------------------
// Step 1: convert each BoardEdgeShape into a polyline segment
// ---------------------------------------------------------------------------

/// An open polyline fragment generated from a single `BoardEdgeShape`, carrying both
/// its tessellated `points` (for stitching + topology) and its `segments` (the typed
/// moves that reproduce it without flattening). For self-closing shapes (circles,
/// rectangles) `is_closed` is true and `segments` already forms a closed loop.
struct Fragment {
    points: Vec<(i64, i64)>,
    segments: Vec<Segment>,
    is_closed: bool,
    label: String, // shape id or index, for warnings
}

fn shape_label(shape: &BoardEdgeShape, index: usize) -> String {
    match shape {
        BoardEdgeShape::Track { id, .. }
        | BoardEdgeShape::Arc { id, .. }
        | BoardEdgeShape::GraphicSegment { id, .. }
        | BoardEdgeShape::GraphicRectangle { id, .. }
        | BoardEdgeShape::GraphicArc { id, .. }
        | BoardEdgeShape::GraphicCircle { id, .. }
        | BoardEdgeShape::GraphicBezier { id, .. }
        | BoardEdgeShape::GraphicPolygon { id, .. } => id
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("shape#{index}")),
    }
}

/// One shape into however many fragments it contributes.
///
/// Every primitive but the polygon yields exactly one. A polygon set carries an outline
/// and any number of holes, each already a closed ring, so it is the one shape that can
/// produce several — which is why this returns a list rather than an `Option`.
fn shape_to_fragments(shape: &BoardEdgeShape, index: usize) -> Vec<Fragment> {
    if let BoardEdgeShape::GraphicPolygon { rings, .. } = shape {
        let label = shape_label(shape, index);
        return rings
            .iter()
            .enumerate()
            .filter_map(|(r, ring)| polygon_ring_fragment(ring, format!("{label}/ring{r}")))
            .collect();
    }
    shape_to_fragment(shape, index).into_iter().collect()
}

/// A closed polygon ring into a fragment, preserving arc nodes as arcs.
fn polygon_ring_fragment(ring: &crate::snapshot::PolyRing, label: String) -> Option<Fragment> {
    use crate::snapshot::PolyNode;

    let mut pts: Vec<(i64, i64)> = Vec::new();
    let mut segments: Vec<Segment> = Vec::new();
    let mut cursor: Option<(i64, i64)> = None;

    // Bridges the gap when an arc node does not begin where the previous node ended.
    // KiCad's rings are continuous, but a stray gap should become a straight edge rather
    // than a hole in the contour.
    let line_to = |pts: &mut Vec<(i64, i64)>,
                   segments: &mut Vec<Segment>,
                   from: (i64, i64),
                   to: (i64, i64)| {
        if from != to {
            pts.push(to);
            segments.push(Segment::Line {
                start: from,
                end: to,
            });
        }
    };

    for node in &ring.nodes {
        match node {
            PolyNode::Point(p) => {
                let p = nm(p);
                match cursor {
                    None => pts.push(p),
                    Some(prev) => line_to(&mut pts, &mut segments, prev, p),
                }
                cursor = Some(p);
            }
            PolyNode::Arc { start, mid, end } => {
                let (s, m, e) = (nm(start), nm(mid), nm(end));
                match cursor {
                    None => pts.push(s),
                    Some(prev) => line_to(&mut pts, &mut segments, prev, s),
                }
                tessellate::tessellate_arc(
                    &mut pts, s.0 as f64, s.1 as f64, m.0 as f64, m.1 as f64, e.0 as f64,
                    e.1 as f64,
                );
                pts.push(e);
                segments.push(Segment::Arc {
                    start: s,
                    mid: m,
                    end: e,
                });
                cursor = Some(e);
            }
        }
    }

    // Close the ring explicitly: the `closed` flag is advisory and the last node rarely
    // repeats the first.
    let (first, last) = (*pts.first()?, cursor?);
    line_to(&mut pts, &mut segments, last, first);

    (segments.len() >= 3).then_some(Fragment {
        points: pts,
        segments,
        is_closed: true,
        label,
    })
}

fn shape_to_fragment(shape: &BoardEdgeShape, index: usize) -> Option<Fragment> {
    let label = shape_label(shape, index);

    let mut pts: Vec<(i64, i64)> = Vec::new();
    let mut segments: Vec<Segment> = Vec::new();

    match shape {
        // --- open primitives: one typed segment each ---
        BoardEdgeShape::Track { start, end, .. }
        | BoardEdgeShape::GraphicSegment { start, end, .. } => {
            let s = nm(start);
            let e = nm(end);
            pts.push(s);
            pts.push(e);
            segments.push(Segment::Line { start: s, end: e });
        }

        BoardEdgeShape::Arc {
            start, mid, end, ..
        }
        | BoardEdgeShape::GraphicArc {
            start, mid, end, ..
        } => {
            let s = nm(start);
            let m = nm(mid);
            let e = nm(end);
            tessellate::tessellate_arc(
                &mut pts, s.0 as f64, s.1 as f64, m.0 as f64, m.1 as f64, e.0 as f64, e.1 as f64,
            );
            pts.push(e); // include end point
            segments.push(Segment::Arc {
                start: s,
                mid: m,
                end: e,
            });
        }

        BoardEdgeShape::GraphicBezier {
            start,
            control1,
            control2,
            end,
            ..
        } => {
            let s = nm(start);
            let c1 = nm(control1);
            let c2 = nm(control2);
            let e = nm(end);
            tessellate::tessellate_bezier(
                &mut pts,
                s.0 as f64,
                s.1 as f64,
                c1.0 as f64,
                c1.1 as f64,
                c2.0 as f64,
                c2.1 as f64,
                e.0 as f64,
                e.1 as f64,
            );
            pts.push(e);
            segments.push(Segment::Bezier {
                start: s,
                control1: c1,
                control2: c2,
                end: e,
            });
        }

        // --- self-closing primitives: segments already form a closed loop ---
        BoardEdgeShape::GraphicCircle {
            center,
            radius_point,
            ..
        } => {
            let (cx, cy) = nm(center);
            let (rx, ry) = nm(radius_point);
            tessellate::tessellate_circle(&mut pts, cx as f64, cy as f64, rx as f64, ry as f64);
            let segments = circle_segments(cx as f64, cy as f64, rx as f64, ry as f64);
            return Some(Fragment {
                points: pts,
                segments,
                is_closed: true,
                label,
            });
        }

        BoardEdgeShape::GraphicRectangle {
            top_left,
            bottom_right,
            corner_radius,
            ..
        } => {
            let (x0, y0) = nm(top_left);
            let (x1, y1) = nm(bottom_right);
            let r = corner_radius
                .as_ref()
                .map(|l| l.as_nm() as f64)
                .unwrap_or(0.0);
            tessellate::tessellate_rectangle(
                &mut pts, x0 as f64, y0 as f64, x1 as f64, y1 as f64, r,
            );
            let segments = rect_segments(x0 as f64, y0 as f64, x1 as f64, y1 as f64, r);
            return Some(Fragment {
                points: pts,
                segments,
                is_closed: true,
                label,
            });
        }

        // Handled by `shape_to_fragments`, which can return the several rings a polygon
        // set may hold; this function can only return one.
        BoardEdgeShape::GraphicPolygon { .. } => return None,
    }

    if pts.len() < 2 {
        return None;
    }

    Some(Fragment {
        points: pts,
        segments,
        is_closed: false,
        label,
    })
}

/// The two semicircle arcs that make up a full circle, going CCW from the east point.
/// Used so a circular board edge stays two `cut_arc`s, not a flattened polygon.
fn circle_segments(cx: f64, cy: f64, rx: f64, ry: f64) -> Vec<Segment> {
    let r = (rx - cx).hypot(ry - cy).round();
    let pt = |x: f64, y: f64| (x.round() as i64, y.round() as i64);
    let east = pt(cx + r, cy);
    let north = pt(cx, cy + r);
    let west = pt(cx - r, cy);
    let south = pt(cx, cy - r);
    vec![
        Segment::Arc {
            start: east,
            mid: north,
            end: west,
        },
        Segment::Arc {
            start: west,
            mid: south,
            end: east,
        },
    ]
}

/// The typed segments of a rectangle: four `Line`s when the corners are sharp, or four
/// straight edges joined by four quarter-`Arc` corners when it is rounded. The corner
/// layout mirrors [`tessellate::tessellate_rectangle`].
fn rect_segments(x0: f64, y0: f64, x1: f64, y1: f64, corner_radius_nm: f64) -> Vec<Segment> {
    use std::f64::consts::{PI, TAU};
    let r = corner_radius_nm.clamp(0.0, ((x1 - x0).abs().min((y1 - y0).abs())) * 0.5);
    let (lx, rx) = (x0.min(x1), x0.max(x1));
    let (ty, by) = (y0.min(y1), y0.max(y1));
    let pt = |x: f64, y: f64| (x.round() as i64, y.round() as i64);

    if r <= 1.0 {
        let corners = [pt(lx, ty), pt(rx, ty), pt(rx, by), pt(lx, by)];
        return (0..4)
            .map(|i| Segment::Line {
                start: corners[i],
                end: corners[(i + 1) % 4],
            })
            .collect();
    }

    // A quarter-arc corner centred at (acx,acy), swept from t0 to t1.
    let arc = |acx: f64, acy: f64, t0: f64, t1: f64| {
        let p = |t: f64| pt(acx + r * t.cos(), acy + r * t.sin());
        Segment::Arc {
            start: p(t0),
            mid: p((t0 + t1) * 0.5),
            end: p(t1),
        }
    };
    let tl = arc(lx + r, ty + r, PI, PI * 1.5);
    let tr = arc(rx - r, ty + r, PI * 1.5, TAU);
    let br = arc(rx - r, by - r, 0.0, PI * 0.5);
    let bl = arc(lx + r, by - r, PI * 0.5, PI);
    // Straight edges join each corner arc's end to the next arc's start.
    let line = |a: Segment, b: Segment| Segment::Line {
        start: a.end(),
        end: b.start(),
    };
    vec![
        tl,
        line(tl, tr),
        tr,
        line(tr, br),
        br,
        line(br, bl),
        bl,
        line(bl, tl),
    ]
}

// ---------------------------------------------------------------------------
// Step 2: stitch open fragments into closed chains
// ---------------------------------------------------------------------------

/// A stitched closed loop: the tessellated `points` (topology) plus the ordered typed
/// `segments` (line/arc/bezier) that reproduce it without flattening.
struct Chain {
    points: Vec<(i64, i64)>,
    segments: Vec<Segment>,
}

fn stitch_fragments(fragments: Vec<Fragment>) -> (Vec<Chain>, Vec<String>) {
    let mut closed: Vec<Chain> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Separate already-closed shapes from open ones (keep the whole fragment so its
    // segments travel with its points).
    let mut open: Vec<Fragment> = Vec::new();
    for frag in fragments {
        if frag.is_closed {
            closed.push(Chain {
                points: frag.points,
                segments: frag.segments,
            });
        } else {
            open.push(frag);
        }
    }

    // Greedily chain open fragments, carrying points and segments together.
    let mut chains: Vec<Chain> = Vec::new();
    let mut chain_labels: Vec<String> = Vec::new();
    let mut used = vec![false; open.len()];

    for i in 0..open.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut points = open[i].points.clone();
        let mut segments = open[i].segments.clone();
        let seed_label = open[i].label.clone();

        loop {
            let head = *points.first().unwrap();
            let tail = *points.last().unwrap();

            if close_enough(head, tail) && points.len() > 2 {
                break; // already closed
            }

            // Find a fragment whose start or end matches our tail.
            let mut found = false;
            for j in 0..open.len() {
                if used[j] {
                    continue;
                }
                let fstart = *open[j].points.first().unwrap();
                let fend = *open[j].points.last().unwrap();

                if close_enough(tail, fstart) {
                    used[j] = true;
                    points.extend_from_slice(&open[j].points[1..]);
                    segments.extend(open[j].segments.iter().copied());
                    found = true;
                    break;
                } else if close_enough(tail, fend) {
                    used[j] = true;
                    let mut rev = open[j].points.clone();
                    rev.reverse();
                    points.extend_from_slice(&rev[1..]);
                    // A flipped fragment contributes its segments in reverse order, each
                    // individually reversed, so the chain keeps a single direction.
                    segments.extend(open[j].segments.iter().rev().map(|s| s.reversed()));
                    found = true;
                    break;
                }
            }
            if !found {
                break;
            }
        }

        chains.push(Chain { points, segments });
        chain_labels.push(seed_label);
    }

    // Classify chains as closed or open; snap the closed ones' segment endpoints.
    for (idx, mut chain) in chains.into_iter().enumerate() {
        let head = *chain.points.first().unwrap();
        let tail = *chain.points.last().unwrap();
        if close_enough(head, tail) || chain.points.len() < 3 {
            // Treat very-short chains with matching endpoints as closed, but only if
            // they have ≥3 distinct points.
            if chain.points.len() >= 3 {
                snap_segment_endpoints(&mut chain.segments);
                closed.push(chain);
            }
        } else {
            warnings.push(format!(
                "open chain starting at fragment '{}' ({}pts, gap {:.1}µm)",
                chain_labels.get(idx).map(|s| s.as_str()).unwrap_or("?"),
                chain.points.len(),
                (dist_sq(head, tail) as f64).sqrt() / 1_000.0,
            ));
        }
    }

    (closed, warnings)
}

/// Forces perfect endpoint continuity on a closed loop of segments: each segment's
/// start is snapped to the previous segment's end, and the first is snapped to the last
/// to close the loop. Snaps are within `STITCH_TOLERANCE_NM`, so geometry is preserved.
fn snap_segment_endpoints(segments: &mut [Segment]) {
    let n = segments.len();
    if n < 2 {
        return;
    }
    for i in 1..n {
        let prev_end = segments[i - 1].end();
        segments[i] = segments[i].with_start(prev_end);
    }
    let last_end = segments[n - 1].end();
    segments[0] = segments[0].with_start(last_end);
}

// ---------------------------------------------------------------------------
// Step 3 & 4: nesting + stitch result construction
// ---------------------------------------------------------------------------

/// Stitch the raw `edge_shapes` from a `BoardSnapshot` into closed, nested
/// contours.  Each `Contour` carries `is_hole = true` when it is an inner
/// boundary (should be routed *outside*).
pub fn stitch_edge_shapes(shapes: &[BoardEdgeShape]) -> StitchResult {
    // --- tessellate ---
    let fragments: Vec<Fragment> = shapes
        .iter()
        .enumerate()
        .flat_map(|(i, s)| shape_to_fragments(s, i))
        .collect();

    // --- stitch ---
    let (closed_polys, open_chain_warnings) = stitch_fragments(fragments);

    // --- validate: open chains are hard errors ---
    let mut errors: Vec<String> = open_chain_warnings
        .iter()
        .map(|w| format!("Open contour: {w}"))
        .collect();

    if closed_polys.is_empty() {
        return StitchResult {
            contours: Vec::new(),
            errors,
        };
    }

    log::debug!("[stitch] raw closed chains: {}", closed_polys.len());
    for (i, chain) in closed_polys.iter().enumerate() {
        let (xmin, ymin, xmax, ymax) = bbox_nm(&chain.points);
        log::trace!(
            "[stitch]   chain {i}: {} pts / {} seg  bbox ({:.3},{:.3})-({:.3},{:.3}) mm",
            chain.points.len(),
            chain.segments.len(),
            xmin as f64 / 1_000_000.0,
            ymin as f64 / 1_000_000.0,
            xmax as f64 / 1_000_000.0,
            ymax as f64 / 1_000_000.0,
        );
    }
    if !errors.is_empty() {
        for e in &errors {
            log::debug!("[stitch] ERROR: {e}");
        }
    }

    // Classify each chain's nesting depth by how many other chains contain its first
    // point (an odd depth is a hole). The tessellated `points` drive the containment
    // test; the typed `segments` are carried through untouched. Nested contours stay
    // distinct — a boolean union would swallow cutouts inside the outer boundary.
    let mut contours_with_depth: Vec<(usize, Contour)> = closed_polys
        .iter()
        .enumerate()
        .map(|(i, chain)| {
            let sample = chain.points[0];
            let depth = closed_polys
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .filter(|(_, other)| point_in_polygon_nm(sample, &other.points))
                .count();
            (
                depth,
                Contour {
                    points: chain.points.clone(),
                    segments: chain.segments.clone(),
                    is_hole: depth % 2 == 1,
                },
            )
        })
        .collect();
    contours_with_depth.sort_by(|(depth_a, contour_a), (depth_b, contour_b)| {
        depth_a.cmp(depth_b).then_with(|| {
            signed_area_nm2(&contour_b.points)
                .unsigned_abs()
                .cmp(&signed_area_nm2(&contour_a.points).unsigned_abs())
        })
    });
    // --- validate: floating islands (depth ≥ 2) ---
    for (depth, contour) in &contours_with_depth {
        if *depth >= 2 {
            let (xmin, ymin, xmax, ymax) = bbox_nm(&contour.points);
            errors.push(format!(
                "Floating island at depth {depth} (contour inside a hole) — bbox ({:.3},{:.3})-({:.3},{:.3}) mm",
                xmin as f64 / 1_000_000.0, ymin as f64 / 1_000_000.0,
                xmax as f64 / 1_000_000.0, ymax as f64 / 1_000_000.0,
            ));
        }
    }

    let contours: Vec<Contour> = contours_with_depth
        .into_iter()
        .map(|(_, contour)| contour)
        .collect();

    log::debug!("[stitch] contour nesting ({} total):", contours.len());
    for (i, c) in contours.iter().enumerate() {
        let (xmin, ymin, xmax, ymax) = bbox_nm(&c.points);
        let area = signed_area_nm2(&c.points).unsigned_abs();
        log::trace!(
            "[stitch]   #{i} {} {} pts  bbox ({:.3},{:.3})-({:.3},{:.3}) mm  area {:.2} mm^2",
            if c.is_hole { "HOLE " } else { "OUTER" },
            c.points.len(),
            xmin as f64 / 1_000_000.0,
            ymin as f64 / 1_000_000.0,
            xmax as f64 / 1_000_000.0,
            ymax as f64 / 1_000_000.0,
            area as f64 / 1e12,
        );
    }
    if !errors.is_empty() {
        log::debug!(
            "[stitch] {} validation error(s) — board cannot be processed:",
            errors.len()
        );
        for e in &errors {
            log::debug!("[stitch]   ERROR: {e}");
        }
    }

    StitchResult { contours, errors }
}

fn bbox_nm(pts: &[(i64, i64)]) -> (i64, i64, i64, i64) {
    let (mut xmin, mut ymin, mut xmax, mut ymax) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for &(x, y) in pts {
        xmin = xmin.min(x);
        ymin = ymin.min(y);
        xmax = xmax.max(x);
        ymax = ymax.max(y);
    }
    (xmin, ymin, xmax, ymax)
}

fn signed_area_nm2(pts: &[(i64, i64)]) -> i128 {
    let n = pts.len();
    let mut sum: i128 = 0;
    for i in 0..n {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[(i + 1) % n];
        sum += (x0 as i128) * (y1 as i128) - (x1 as i128) * (y0 as i128);
    }
    sum / 2
}

/// Offsets each contour by `tool_radius_nm` into the cutter-centre path that frees the
/// board at its **nominal** size, one entry per input contour and in the same order.
///
/// The sign follows from what survives the cut. A PCB keeps the material inside its outer
/// boundary and loses the material inside each cutout, so the kerf must fall on the waste
/// side of both:
///
/// - **Outer boundary** — the cutter runs *outside* the line (positive delta). Offsetting
///   inward instead would return a board one router diameter undersized.
/// - **Interior cutout** — the cutter runs *inside* the opening (negative delta), leaving
///   the cutout at its drawn size rather than a diameter oversized.
///
/// A contour smaller than the cutter can vanish under the offset, which is why an entry
/// can be `None`: the caller must report that feature as unmillable rather than skip it
/// silently. Where an offset splits a contour in two, the largest part is taken — the
/// others are slivers of a feature the tool cannot resolve.
///
/// Operates on the tessellated [`Contour::points`], so the result is a polyline: joins,
/// concave-corner trimming and self-intersection removal all come from the polygon
/// offset, which is what makes it correct without a special case per corner type. The
/// typed [`Contour::segments`] are preserved for a later arc-fitting pass
/// (operation-planner.md §3) but are not consulted here.
/// Chord tolerance for the arcs an offset generates, matched to the tessellator's own
/// so a path is no coarser after offsetting than it was before. Pinned rather than left
/// at Clipper's proportional default because [`fit_cutout`] measures areas across two
/// offsets and needs a stated error bound rather than a floating one.
const OFFSET_ARC_TOLERANCE_NM: f64 = tessellate::SAGITTA_TOLERANCE_NM;

/// Every path that offsetting `paths` by `delta_nm` produces, as one group.
///
/// Positive grows and negative shrinks **whatever the input winding** — Clipper reads
/// the orientation of each group and flips the delta to suit, which matters here because
/// nothing in this crate normalises the winding of a stitched contour.
///
/// Returned largest-area-first with ties broken by Clipper's own order, so the result is
/// a total function of the input: two runs on the same board give the same program.
fn offset_group(paths: &[Vec<(i64, i64)>], delta_nm: f64) -> Vec<Vec<(i64, i64)>> {
    use clipper2_rust::{
        core::Paths64,
        inflate_paths_64,
        offset::{EndType, JoinType},
    };

    let input: Paths64 = paths
        .iter()
        .filter(|p| p.len() >= 3)
        .map(|p| p.iter().map(|&(x, y)| Point64 { x, y }).collect::<Path64>())
        .collect();
    if input.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<Vec<(i64, i64)>> = inflate_paths_64(
        &input,
        delta_nm,
        JoinType::Round,
        EndType::Polygon,
        2.0,
        OFFSET_ARC_TOLERANCE_NM,
    )
    .iter()
    .map(|p| p.iter().map(|pt| (pt.x, pt.y)).collect::<Vec<(i64, i64)>>())
    .filter(|p| p.len() >= 3)
    .collect();
    out.sort_by_key(|p| std::cmp::Reverse(signed_area_nm2(p).unsigned_abs()));
    out
}

/// Offsets one closed path, returning **every** fragment it breaks into.
///
/// The fragment count is information, not noise: shrinking a shape that has a neck
/// narrower than twice the offset splits it in two, and that is precisely how
/// [`fit_cutout`] detects a cutter that cannot pass. Callers that only want the body of
/// the shape can take the first, which is the largest.
pub fn offset_paths(points: &[(i64, i64)], delta_nm: f64) -> Vec<Vec<(i64, i64)>> {
    offset_group(&[points.to_vec()], delta_nm)
}

pub fn routing_offset(contours: &[Contour], tool_radius_nm: i64) -> Vec<Option<Vec<(i64, i64)>>> {
    contours
        .iter()
        .map(|contour| {
            if contour.points.len() < 3 {
                return None;
            }
            // Grow the boundary, shrink the cutouts — the kerf lands on the waste side
            // of each. `Round` joins are the tool's own radius sweeping a convex corner.
            let delta = if contour.is_hole {
                -(tool_radius_nm as f64)
            } else {
                tool_radius_nm as f64
            };
            // Largest fragment only. That is a real loss of information where the offset
            // splits — a pinched cutout silently becomes one lobe — which is why
            // interior openings go through `fit_cutout` instead of this, and why it
            // refuses a cutter that splits the erosion rather than cutting one half.
            offset_paths(&contour.points, delta).into_iter().next()
        })
        .collect()
}

/// What a candidate cutter would make of one interior opening.
#[derive(Clone, Debug)]
pub struct CutoutFit {
    /// The cutter-centre loop: the boundary of `erode(C, R)`, one closed path.
    pub wall_path: Vec<(i64, i64)>,
    /// The slug or slugs left standing once the wall pass has run: `erode(C, D)`.
    ///
    /// Empty when the pass clears the whole opening, which happens exactly when the
    /// opening is nowhere wider than twice the cutter diameter. More than one when the
    /// opening is wide at both ends and pinched in the middle — a dumbbell leaves two
    /// pieces, and one tab between them holds neither.
    pub slugs: Vec<Vec<(i64, i64)>>,
    /// Material the cutter cannot reach, over and above the fillets its own radius
    /// forces it to leave at every sharp corner. Never usefully zero — see
    /// [`corners::corner_fillet_area_nm2`].
    pub excess_nm2: f64,
}

/// Whether `tool_radius_nm` can cut `contour` as drawn, and what it leaves behind.
///
/// Two questions, answered separately because they fail in different ways and want
/// different remedies.
///
/// **Can the cutter get everywhere?** `erode(C, R)` is the set of places the cutter's
/// centre may stand. Empty means it does not fit at all. *Split* means there is a neck
/// narrower than 2R somewhere: the cut would come out as two disjoint loops that free
/// neither piece, which is worse than refusing the tool. That test needs no tolerance,
/// so it runs first.
///
/// **Does it leave more than it must?** `open(C,R) = dilate(erode(C,R), R)` is exactly
/// the union of every radius-R disc that fits inside `C` — precisely what the cutter can
/// reach, corners and all. Whatever remains is fillet plus genuinely unreachable
/// material, and the fillets have a closed form, so subtracting them leaves an honest
/// measure of the second. A round cutter *always* leaves something at a sharp corner;
/// without that subtraction every rectangular cutout would look unreachable.
///
/// `None` when the cutter does not fit. The area threshold is deliberately **not**
/// applied here — that is tool-selection policy, and it lives with the picker.
pub fn fit_cutout(contour: &Contour, tool_radius_nm: i64) -> Option<CutoutFit> {
    if contour.points.len() < 3 || tool_radius_nm <= 0 {
        return None;
    }
    let r = tool_radius_nm as f64;

    let erosion = offset_paths(&contour.points, -r);
    if erosion.len() != 1 {
        return None; // does not fit, or a neck splits it
    }

    let opening = offset_group(&erosion, r);
    let open_area: f64 = opening
        .iter()
        .map(|p| signed_area_nm2(p).unsigned_abs() as f64)
        .sum();
    let total_area = signed_area_nm2(&contour.points).unsigned_abs() as f64;

    let budget: f64 = corners::convex_corners(contour, corners::MIN_CORNER_TURN_RAD)
        .iter()
        .map(|c| corners::corner_fillet_area_nm2(r, c.interior_rad))
        .sum();

    // The slug is the erosion eroded again by the same radius: the cutter sweeps a disc
    // of radius R along a centreline already R inside the wall, so it clears everything
    // within 2R of the wall. `erode(C, 2R)` is what survives — and it is empty exactly
    // when the opening is nowhere wider than 2R+2R, i.e. twice the cutter *diameter*.
    let slugs = offset_paths(&contour.points, -2.0 * r);

    Some(CutoutFit {
        wall_path: erosion.into_iter().next()?,
        slugs,
        excess_nm2: (total_area - open_area - budget).max(0.0),
    })
}

/// An open polyline given a body: the area a pen of width `2·half_width_nm` would ink.
///
/// This is what turns a track into copper. A track is stored as a centreline and a width,
/// and every question worth asking about copper — how far is it from its neighbour, where
/// does a cutter go to isolate it — is a question about area, not about a line.
///
/// Round ends and round joins, because that is how KiCad draws a track: square ends would
/// invent copper past the end of the trace and make the isolation pass cut a corner that
/// is not on the board.
pub fn stroke_open_path(points: &[(i64, i64)], half_width_nm: f64) -> Vec<Vec<(i64, i64)>> {
    use clipper2_rust::{
        core::Paths64,
        inflate_paths_64,
        offset::{EndType, JoinType},
    };

    if points.len() < 2 || half_width_nm <= 0.0 {
        return Vec::new();
    }
    let input: Paths64 =
        vec![points.iter().map(|&(x, y)| Point64 { x, y }).collect::<Path64>()];

    inflate_paths_64(
        &input,
        half_width_nm,
        JoinType::Round,
        EndType::Round,
        2.0,
        OFFSET_ARC_TOLERANCE_NM,
    )
    .iter()
    .map(|p| p.iter().map(|pt| (pt.x, pt.y)).collect::<Vec<(i64, i64)>>())
    .filter(|p| p.len() >= 3)
    .collect()
}

/// Perimeter of a closed polygon, in nanometres.
pub fn path_perimeter_nm(path: &[(i64, i64)]) -> f64 {
    if path.len() < 2 {
        return 0.0;
    }
    path.iter()
        .zip(path.iter().cycle().skip(1))
        .take(path.len())
        .map(|(&(ax, ay), &(bx, by))| ((bx - ax) as f64).hypot((by - ay) as f64))
        .sum()
}

// ---------------------------------------------------------------------------
// Point-in-polygon test (ray casting, nm coordinates)
// ---------------------------------------------------------------------------

fn point_in_polygon_nm(pt: (i64, i64), poly: &[(i64, i64)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let (px, py) = (pt.0 as f64, pt.1 as f64);
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i].0 as f64, poly[i].1 as f64);
        let (xj, yj) = (poly[j].0 as f64, poly[j].1 as f64);
        if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{PolyNode, PolyRing};
    use units::Length;

    fn pt(x_mm: f64, y_mm: f64) -> BoardPoint {
        BoardPoint {
            x: Length::from_mm(x_mm),
            y: Length::from_mm(y_mm),
        }
    }

    fn segment(id: &str, ax: f64, ay: f64, bx: f64, by: f64) -> BoardEdgeShape {
        BoardEdgeShape::GraphicSegment {
            id: Some(id.into()),
            start: pt(ax, ay),
            end: pt(bx, by),
        }
    }

    /// Asserts the segments form a continuous, closed loop: each segment ends exactly
    /// where the next begins (wrapping) — the post-snap continuity invariant.
    fn assert_closed_loop(segments: &[Segment]) {
        let n = segments.len();
        assert!(n >= 2, "a loop needs at least two segments");
        for i in 0..n {
            let next = (i + 1) % n;
            assert_eq!(
                segments[i].end(),
                segments[next].start(),
                "segment {i} end must meet segment {next} start",
            );
        }
    }

    #[test]
    fn four_line_edges_stitch_into_four_line_segments() {
        // A unit square given with one edge (the top) reversed, so stitching must flip
        // a fragment — the result is still four continuous Line segments.
        let shapes = vec![
            segment("bottom", 0.0, 0.0, 10.0, 0.0),
            segment("right", 10.0, 0.0, 10.0, 10.0),
            segment("top_rev", 0.0, 10.0, 10.0, 10.0), // reversed direction
            segment("left", 0.0, 10.0, 0.0, 0.0),
        ];
        let result = stitch_edge_shapes(&shapes);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.contours.len(), 1);
        let contour = &result.contours[0];
        assert_eq!(
            contour.segments.len(),
            4,
            "one typed segment per edge, not flattened"
        );
        assert!(contour
            .segments
            .iter()
            .all(|s| matches!(s, Segment::Line { .. })));
        assert_closed_loop(&contour.segments);
    }

    #[test]
    fn an_arc_edge_survives_as_an_arc_segment() {
        // A "D": a straight top edge and a semicircular arc below it, stitched into a
        // closed loop that keeps one Line and one Arc (not a fan of chords).
        let shapes = vec![
            segment("top", 0.0, 0.0, 10.0, 0.0),
            BoardEdgeShape::GraphicArc {
                id: Some("bow".into()),
                start: pt(10.0, 0.0),
                mid: pt(5.0, -5.0),
                end: pt(0.0, 0.0),
            },
        ];
        let result = stitch_edge_shapes(&shapes);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.contours.len(), 1);
        let contour = &result.contours[0];
        assert_eq!(contour.segments.len(), 2);
        assert_eq!(
            contour
                .segments
                .iter()
                .filter(|s| matches!(s, Segment::Arc { .. }))
                .count(),
            1
        );
        assert_eq!(
            contour
                .segments
                .iter()
                .filter(|s| matches!(s, Segment::Line { .. }))
                .count(),
            1
        );
        assert_closed_loop(&contour.segments);
    }

    #[test]
    fn a_sharp_rectangle_is_four_line_segments() {
        let shapes = vec![BoardEdgeShape::GraphicRectangle {
            id: Some("rect".into()),
            top_left: pt(0.0, 0.0),
            bottom_right: pt(10.0, 6.0),
            corner_radius: None,
        }];
        let result = stitch_edge_shapes(&shapes);
        assert!(result.errors.is_empty());
        assert_eq!(result.contours.len(), 1);
        let contour = &result.contours[0];
        assert_eq!(contour.segments.len(), 4);
        assert!(contour
            .segments
            .iter()
            .all(|s| matches!(s, Segment::Line { .. })));
        assert_closed_loop(&contour.segments);
    }

    #[test]
    fn a_circle_is_two_arc_segments() {
        let shapes = vec![BoardEdgeShape::GraphicCircle {
            id: Some("hole".into()),
            center: pt(5.0, 5.0),
            radius_point: pt(9.0, 5.0),
        }];
        let result = stitch_edge_shapes(&shapes);
        assert!(result.errors.is_empty());
        assert_eq!(result.contours.len(), 1);
        let contour = &result.contours[0];
        assert_eq!(contour.segments.len(), 2);
        assert!(contour
            .segments
            .iter()
            .all(|s| matches!(s, Segment::Arc { .. })));
        assert_closed_loop(&contour.segments);
    }

    #[test]
    fn nesting_marks_the_inner_rectangle_as_a_hole() {
        // An outer boundary with a rectangular cutout inside it — the containment test
        // (now on point slices) still classifies the inner one as a hole, and both keep
        // their typed segments.
        let shapes = vec![
            BoardEdgeShape::GraphicRectangle {
                id: Some("outer".into()),
                top_left: pt(0.0, 0.0),
                bottom_right: pt(20.0, 20.0),
                corner_radius: None,
            },
            BoardEdgeShape::GraphicRectangle {
                id: Some("inner".into()),
                top_left: pt(5.0, 5.0),
                bottom_right: pt(15.0, 15.0),
                corner_radius: None,
            },
        ];
        let result = stitch_edge_shapes(&shapes);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.contours.len(), 2);
        assert!(
            result.contours.iter().any(|c| !c.is_hole),
            "one outer boundary"
        );
        assert!(result.contours.iter().any(|c| c.is_hole), "one inner hole");
        assert!(
            result.contours.iter().all(|c| c.segments.len() == 4),
            "typed segments kept"
        );
    }

    /// A polygon-drawn cutout must reach the machining path like any other shape.
    ///
    /// KiCad's polygon tool is an ordinary way to draw an opening, and upstream
    /// `kicad-ipc-rs` maps the shape to a vertex *count* with the geometry discarded. We
    /// carried the vertices through the fork precisely so this works; before that, a
    /// board whose cutout was drawn this way lost it with no error anywhere — it simply
    /// never got machined, which is the worst way for a CAM tool to fail.
    #[test]
    fn a_polygon_ring_becomes_a_closed_contour() {
        let ring = |pts: &[(f64, f64)]| PolyRing {
            nodes: pts
                .iter()
                .map(|&(x, y)| PolyNode::Point(pt(x, y)))
                .collect(),
        };
        let shapes = vec![
            BoardEdgeShape::GraphicRectangle {
                id: Some("outer".into()),
                top_left: pt(0.0, 0.0),
                bottom_right: pt(20.0, 20.0),
                corner_radius: None,
            },
            BoardEdgeShape::GraphicPolygon {
                id: Some("poly-cutout".into()),
                rings: vec![ring(&[(5.0, 5.0), (15.0, 5.0), (15.0, 15.0), (5.0, 15.0)])],
            },
        ];

        let result = stitch_edge_shapes(&shapes);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(
            result.contours.len(),
            2,
            "the polygon is a contour, not a dropped shape"
        );

        let hole = result
            .contours
            .iter()
            .find(|c| c.is_hole)
            .expect("the polygon nests as a hole");
        assert_eq!(
            hole.segments.len(),
            4,
            "closed by its own edges, not left open"
        );
        assert_eq!(
            signed_area_nm2(&hole.points).unsigned_abs(),
            100_000_000_000_000,
            "10mm x 10mm in nm^2"
        );
    }

    /// A rectangular nm contour, for the fit tests.
    fn rect_contour(w_mm: f64, h_mm: f64) -> Contour {
        let (w, h) = ((w_mm * 1e6) as i64, (h_mm * 1e6) as i64);
        let pts = vec![(0, 0), (w, 0), (w, h), (0, h)];
        let segments = (0..4)
            .map(|i| Segment::Line {
                start: pts[i],
                end: pts[(i + 1) % 4],
            })
            .collect();
        Contour {
            points: pts,
            segments,
            is_hole: true,
        }
    }

    fn nm(mm: f64) -> i64 {
        (mm * 1e6) as i64
    }

    /// A cutter that fits must not be refused just because it is round.
    ///
    /// `open(C,R)` can never recover a sharp corner, so the raw leftover area of a
    /// rectangular cutout is always about `0.86 R²` however well the cutter fits. A fit
    /// test that did not subtract the corner budget would reject every rectangle on the
    /// board — which is most of them.
    #[test]
    fn a_rectangular_cutout_is_cut_by_a_cutter_that_fits_despite_its_corners() {
        let c = rect_contour(10.0, 6.0);
        let fit = fit_cutout(&c, nm(1.0)).expect("a 2mm cutter fits a 6mm-wide opening");
        let r_sq = 1e6_f64 * 1e6;
        assert!(
            fit.excess_nm2 < r_sq,
            "excess {} nm² should be under one cutter-radius square ({r_sq})",
            fit.excess_nm2
        );
    }

    /// A neck narrower than the cutter must be refused, not silently half-cut.
    ///
    /// Shrinking a pinched shape splits it, and `routing_offset` would hand back only
    /// the larger lobe — so the program would cut one closed loop inside one half and
    /// leave the neck and the other half attached, with nothing to say so. Refusing the
    /// tool is the only safe answer.
    #[test]
    fn a_neck_narrower_than_the_cutter_splits_the_erosion_and_is_refused() {
        // Two 8mm squares joined by a 1mm-wide, 4mm-long waist.
        let pts = vec![
            (nm(0.0), nm(0.0)),
            (nm(8.0), nm(0.0)),
            (nm(8.0), nm(3.5)),
            (nm(12.0), nm(3.5)),
            (nm(12.0), nm(0.0)),
            (nm(20.0), nm(0.0)),
            (nm(20.0), nm(8.0)),
            (nm(12.0), nm(8.0)),
            (nm(12.0), nm(4.5)),
            (nm(8.0), nm(4.5)),
            (nm(8.0), nm(8.0)),
            (nm(0.0), nm(8.0)),
        ];
        let n = pts.len();
        let segments = (0..n)
            .map(|i| Segment::Line {
                start: pts[i],
                end: pts[(i + 1) % n],
            })
            .collect();
        let dumbbell = Contour {
            points: pts,
            segments,
            is_hole: true,
        };

        assert!(
            fit_cutout(&dumbbell, nm(1.0)).is_none(),
            "a 2mm cutter cannot pass a 1mm waist"
        );
        assert!(
            fit_cutout(&dumbbell, nm(0.4)).is_some(),
            "a 0.8mm cutter passes it and should be accepted"
        );
    }

    /// An opening no wider than twice the cutter diameter leaves nothing to hold.
    ///
    /// The wall pass sweeps a disc of radius R along a centreline already R inside the
    /// edge, so it clears everything within 2R of the wall — the whole opening, once the
    /// opening is nowhere wider than 4R. This is the exact condition, not a rule of
    /// thumb, and it is what lets the operation skip pocketing altogether.
    #[test]
    fn an_opening_no_wider_than_twice_the_cutter_diameter_leaves_no_slug() {
        // 1mm cutter (R=0.5): clears anything up to 2mm wide.
        let narrow = rect_contour(20.0, 1.8);
        let fit = fit_cutout(&narrow, nm(0.5)).expect("the cutter fits");
        assert!(
            fit.slugs.is_empty(),
            "1.8mm < 2 x 1mm: the pass clears the whole opening"
        );

        let wide = rect_contour(20.0, 3.0);
        let fit = fit_cutout(&wide, nm(0.5)).expect("the cutter fits");
        assert!(
            !fit.slugs.is_empty(),
            "3mm > 2 x 1mm: a slug survives and must be held"
        );
    }

    /// A dumbbell wide enough for the cutter can still leave two islands.
    ///
    /// The waist passes the cutter but is too narrow to leave material, so the slug
    /// splits while the cut path does not. One tab would hold one lobe and throw the
    /// other, which is why island retention works per slug rather than per cutout.
    #[test]
    fn a_wide_waisted_dumbbell_leaves_two_islands() {
        let pts = vec![
            (nm(0.0), nm(0.0)),
            (nm(10.0), nm(0.0)),
            (nm(10.0), nm(4.4)),
            (nm(14.0), nm(4.4)),
            (nm(14.0), nm(0.0)),
            (nm(24.0), nm(0.0)),
            (nm(24.0), nm(10.0)),
            (nm(14.0), nm(10.0)),
            (nm(14.0), nm(5.6)),
            (nm(10.0), nm(5.6)),
            (nm(10.0), nm(10.0)),
            (nm(0.0), nm(10.0)),
        ];
        let n = pts.len();
        let segments = (0..n)
            .map(|i| Segment::Line {
                start: pts[i],
                end: pts[(i + 1) % n],
            })
            .collect();
        let dumbbell = Contour {
            points: pts,
            segments,
            is_hole: true,
        };

        // R=0.4 (0.8mm cutter) against a 1.2mm waist. The cutter passes it — 1.2 > 2R —
        // but 1.2 < 2 x 0.8mm, so no material survives the waist and the slug splits.
        // That gap between "the cutter fits" and "material survives" is exactly where
        // one cutout turns into two islands.
        let fit = fit_cutout(&dumbbell, nm(0.4)).expect("0.8mm cutter passes the 1.2mm waist");
        assert_eq!(
            fit.slugs.len(),
            2,
            "two lobes of material, each needing its own tab"
        );
    }

    /// The slug is the shape the tab is sized against, not the drawing.
    ///
    /// A 20mm square cut with a 1mm cutter leaves a ~18mm square: the tab is a fraction
    /// of *that* perimeter. Sizing off the drawn contour would scale the tab with the
    /// cutter, which has nothing to do with how big the loose piece is.
    #[test]
    fn the_slug_is_the_opening_eroded_by_the_cutter_diameter() {
        let fit = fit_cutout(&rect_contour(20.0, 20.0), nm(0.5)).expect("fits");
        assert_eq!(fit.slugs.len(), 1);
        let perimeter_mm = path_perimeter_nm(&fit.slugs[0]) / 1e6;
        // Eroded by the cutter diameter (2 x 0.5mm), so 18mm a side. The corners stay
        // sharp: round joins apply to the outside of a turn, and shrinking a convex
        // shape turns every corner the other way.
        assert!(
            (perimeter_mm - 72.0).abs() < 0.2,
            "expected a 4 x 18mm slug perimeter, got {perimeter_mm}"
        );
    }

    /// A track becomes copper of its own width, with round ends.
    ///
    /// A track is stored as a centreline and a width, and every question isolation asks —
    /// how far is this from its neighbour, where does the cutter go — is about area. The
    /// ends must be round because that is how KiCad draws them: square ends invent copper
    /// past the end of the trace, and the isolation pass would cut a corner that is not on
    /// the board.
    #[test]
    fn stroking_a_track_gives_it_its_width_and_rounds_its_ends() {
        // A 10mm run at 1mm wide: 10 x 1 in the body, plus a half-circle at each end.
        let path = vec![(0, 0), (nm(10.0), 0)];
        let stroked = stroke_open_path(&path, nm(0.5) as f64);
        assert_eq!(stroked.len(), 1, "one track, one polygon");

        let area_mm2 = signed_area_nm2(&stroked[0]).unsigned_abs() as f64 / 1e12;
        let expected = 10.0 * 1.0 + std::f64::consts::PI * 0.5 * 0.5;
        assert!(
            (area_mm2 - expected).abs() < 0.02,
            "expected ~{expected:.3}mm^2 (body + two round ends), got {area_mm2:.3}"
        );

        let (xmin, _, xmax, _) = bbox_nm(&stroked[0]);
        assert!(xmin <= nm(-0.49), "the round end reaches back past the start: {xmin}");
        assert!(xmax >= nm(10.49), "and past the end: {xmax}");
    }

    /// A zero-width or single-point track strokes to nothing rather than to a degenerate
    /// polygon that Clipper would later hand back as an empty offset.
    #[test]
    fn a_track_with_no_length_or_no_width_strokes_to_nothing() {
        assert!(stroke_open_path(&[(0, 0)], 500_000.0).is_empty(), "one point is not a run");
        assert!(
            stroke_open_path(&[(0, 0), (nm(5.0), 0)], 0.0).is_empty(),
            "a zero-width track has no body"
        );
    }

    /// A polygon's holes are rings in their own right.
    ///
    /// A single polygon shape can carry an outline *and* interior holes. Flattening them
    /// into one ring list means nesting is decided by the same containment pass that
    /// handles every other contour, rather than the polygon carrying a private opinion
    /// about which of its rings are openings.
    #[test]
    fn a_polygon_carrying_its_own_hole_yields_both_rings() {
        let ring = |pts: &[(f64, f64)]| PolyRing {
            nodes: pts
                .iter()
                .map(|&(x, y)| PolyNode::Point(pt(x, y)))
                .collect(),
        };
        let shapes = vec![BoardEdgeShape::GraphicPolygon {
            id: Some("poly".into()),
            rings: vec![
                ring(&[(0.0, 0.0), (30.0, 0.0), (30.0, 30.0), (0.0, 30.0)]),
                ring(&[(10.0, 10.0), (20.0, 10.0), (20.0, 20.0), (10.0, 20.0)]),
            ],
        }];

        let result = stitch_edge_shapes(&shapes);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.contours.len(), 2);
        assert_eq!(
            result.contours.iter().filter(|c| c.is_hole).count(),
            1,
            "the inner ring"
        );
        assert_eq!(
            result.contours.iter().filter(|c| !c.is_hole).count(),
            1,
            "the outer ring"
        );
    }
}
