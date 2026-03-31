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

pub mod tessellate;

use clipper2_rust::core::{Path64, Point64};

use crate::board::{BoardEdgeShape, BoardPoint};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A closed, ordered polygon in nm coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Contour {
    /// Points in order (last point implicitly connects to first).
    pub points: Vec<(i64, i64)>,
    /// Derived from `PolyTree` depth: even → outer boundary, odd → hole.
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

/// An open polyline fragment generated from a single `BoardEdgeShape`.
/// For self-closing shapes (circles, full rectangles) `is_closed` is true.
struct Fragment {
    points: Vec<(i64, i64)>,
    is_closed: bool,
    label: String, // shape id or index, for warnings
}

fn shape_to_fragment(shape: &BoardEdgeShape, index: usize) -> Option<Fragment> {
    let label = match shape {
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
    };

    let mut pts: Vec<(i64, i64)> = Vec::new();

    match shape {
        // --- open primitives ---
        BoardEdgeShape::Track { start, end, .. }
        | BoardEdgeShape::GraphicSegment { start, end, .. } => {
            pts.push(nm(start));
            pts.push(nm(end));
        }

        BoardEdgeShape::Arc { start, mid, end, .. }
        | BoardEdgeShape::GraphicArc { start, mid, end, .. } => {
            let (sx, sy) = nm(start);
            let (mx, my) = nm(mid);
            let (ex, ey) = nm(end);
            tessellate::tessellate_arc(
                &mut pts,
                sx as f64, sy as f64,
                mx as f64, my as f64,
                ex as f64, ey as f64,
            );
            pts.push((ex, ey)); // include end point
        }

        BoardEdgeShape::GraphicBezier { start, control1, control2, end, .. } => {
            let (sx, sy) = nm(start);
            let (c1x, c1y) = nm(control1);
            let (c2x, c2y) = nm(control2);
            let (ex, ey) = nm(end);
            tessellate::tessellate_bezier(
                &mut pts,
                sx as f64, sy as f64,
                c1x as f64, c1y as f64,
                c2x as f64, c2y as f64,
                ex as f64, ey as f64,
            );
            pts.push((ex, ey));
        }

        // --- self-closing primitives ---
        BoardEdgeShape::GraphicCircle { center, radius_point, .. } => {
            let (cx, cy) = nm(center);
            let (rx, ry) = nm(radius_point);
            tessellate::tessellate_circle(
                &mut pts,
                cx as f64, cy as f64,
                rx as f64, ry as f64,
            );
            return Some(Fragment { points: pts, is_closed: true, label });
        }

        BoardEdgeShape::GraphicRectangle { top_left, bottom_right, corner_radius, .. } => {
            let (x0, y0) = nm(top_left);
            let (x1, y1) = nm(bottom_right);
            let r = corner_radius
                .as_ref()
                .map(|l| l.as_nm() as f64)
                .unwrap_or(0.0);
            tessellate::tessellate_rectangle(
                &mut pts,
                x0 as f64, y0 as f64,
                x1 as f64, y1 as f64,
                r,
            );
            return Some(Fragment { points: pts, is_closed: true, label });
        }

        // Polygon: we have only a count, not the actual geometry — skip.
        BoardEdgeShape::GraphicPolygon { .. } => return None,
    }

    if pts.len() < 2 {
        return None;
    }

    Some(Fragment { points: pts, is_closed: false, label })
}

// ---------------------------------------------------------------------------
// Step 2: stitch open fragments into closed chains
// ---------------------------------------------------------------------------

fn stitch_fragments(
    fragments: Vec<Fragment>,
) -> (Vec<Vec<(i64, i64)>>, Vec<String>) {
    let mut closed: Vec<Vec<(i64, i64)>> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Separate already-closed shapes from open ones.
    let mut open: Vec<Vec<(i64, i64)>> = Vec::new();
    let mut open_labels: Vec<String> = Vec::new();

    for frag in fragments {
        if frag.is_closed {
            closed.push(frag.points);
        } else {
            open.push(frag.points);
            open_labels.push(frag.label);
        }
    }

    // Greedily chain open fragments.
    // Each entry in `chains` is a sequence of points with known start/end.
    let mut chains: Vec<Vec<(i64, i64)>> = Vec::new();
    let mut used = vec![false; open.len()];

    for i in 0..open.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut chain = open[i].clone();

        loop {
            let head = *chain.first().unwrap();
            let tail = *chain.last().unwrap();

            if close_enough(head, tail) && chain.len() > 2 {
                // Already closed.
                break;
            }

            // Try to find a fragment whose start or end matches our tail.
            let mut found = false;
            for j in 0..open.len() {
                if used[j] {
                    continue;
                }
                let fstart = *open[j].first().unwrap();
                let fend   = *open[j].last().unwrap();

                if close_enough(tail, fstart) {
                    used[j] = true;
                    chain.extend_from_slice(&open[j][1..]);
                    found = true;
                    break;
                } else if close_enough(tail, fend) {
                    used[j] = true;
                    let mut rev = open[j].clone();
                    rev.reverse();
                    chain.extend_from_slice(&rev[1..]);
                    found = true;
                    break;
                }
            }
            if !found {
                break;
            }
        }

        chains.push(chain);
    }

    // Classify chains as closed or open.
    for (idx, chain) in chains.into_iter().enumerate() {
        let head = *chain.first().unwrap();
        let tail = *chain.last().unwrap();
        if close_enough(head, tail) || chain.len() < 3 {
            // Treat very-short chains that happen to have matching endpoints as
            // closed, but only if they have ≥3 distinct points.
            if chain.len() >= 3 {
                closed.push(chain);
            }
        } else {
            warnings.push(format!(
                "open chain starting at fragment '{}' ({}pts, gap {:.1}µm)",
                open_labels.get(idx).map(|s| s.as_str()).unwrap_or("?"),
                chain.len(),
                (dist_sq(head, tail) as f64).sqrt() / 1_000.0,
            ));
        }
    }

    (closed, warnings)
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
        .filter_map(|(i, s)| shape_to_fragment(s, i))
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

    println!("[stitch] raw closed chains: {}", closed_polys.len());
    for (i, poly) in closed_polys.iter().enumerate() {
        let (xmin, ymin, xmax, ymax) = bbox_nm(poly);
        println!(
            "[stitch]   chain {i}: {} pts  bbox ({:.3},{:.3})-({:.3},{:.3}) mm",
            poly.len(),
            xmin as f64 / 1_000_000.0, ymin as f64 / 1_000_000.0,
            xmax as f64 / 1_000_000.0, ymax as f64 / 1_000_000.0,
        );
    }
    if !errors.is_empty() {
        for e in &errors {
            println!("[stitch] ERROR: {e}");
        }
    }

    // Nested contours must remain distinct. A boolean union would treat the
    // outer board loop as a filled region and swallow any cutouts inside it.
    let paths: Vec<Path64> = closed_polys
        .iter()
        .map(|poly| {
            poly.iter()
                .map(|&(x, y)| Point64 { x, y })
                .collect::<Path64>()
        })
        .collect();
    println!("[stitch] after nesting prep: {} contour(s)", paths.len());

    let mut contours_with_depth: Vec<(usize, Contour)> = paths
        .iter()
        .map(|path| {
            let pts: Vec<(i64, i64)> = path.iter().map(|p| (p.x, p.y)).collect();
            let sample = pts[0];
            let depth = paths
                .iter()
                .filter(|other| !std::ptr::eq(*other, path))
                .filter(|other| point_in_polygon_nm(sample, other))
                .count();
            (
                depth,
                Contour {
                    points: pts,
                    is_hole: depth % 2 == 1,
                },
            )
        })
        .collect();
    contours_with_depth.sort_by(|(depth_a, contour_a), (depth_b, contour_b)| {
        depth_a
            .cmp(depth_b)
            .then_with(|| signed_area_nm2(&contour_b.points).unsigned_abs().cmp(&signed_area_nm2(&contour_a.points).unsigned_abs()))
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

    let contours: Vec<Contour> = contours_with_depth.into_iter().map(|(_, contour)| contour).collect();

    println!("[stitch] contour nesting ({} total):", contours.len());
    for (i, c) in contours.iter().enumerate() {
        let (xmin, ymin, xmax, ymax) = bbox_nm(&c.points);
        let area = signed_area_nm2(&c.points).unsigned_abs();
        println!(
            "[stitch]   #{i} {} {} pts  bbox ({:.3},{:.3})-({:.3},{:.3}) mm  area {:.2} mm^2",
            if c.is_hole { "HOLE " } else { "OUTER" },
            c.points.len(),
            xmin as f64 / 1_000_000.0, ymin as f64 / 1_000_000.0,
            xmax as f64 / 1_000_000.0, ymax as f64 / 1_000_000.0,
            area as f64 / 1e12,
        );
    }
    if !errors.is_empty() {
        println!("[stitch] {} validation error(s) — board cannot be processed:", errors.len());
        for e in &errors {
            println!("[stitch]   ERROR: {e}");
        }
    }

    StitchResult { contours, errors }
}

fn bbox_nm(pts: &[(i64, i64)]) -> (i64, i64, i64, i64) {
    let (mut xmin, mut ymin, mut xmax, mut ymax) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for &(x, y) in pts {
        xmin = xmin.min(x); ymin = ymin.min(y);
        xmax = xmax.max(x); ymax = ymax.max(y);
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


/// Offset each contour by `tool_radius_nm`, returning the compensated paths
/// that a CNC router should follow.
///
/// - Outer boundaries are offset **inward** (negative delta).
/// - Inner holes are offset **outward** (positive delta).
///
/// Returns `(outer_paths, hole_paths)` both in nm coordinates.
pub fn routing_offset(
    contours: &[Contour],
    tool_radius_nm: i64,
) -> (Vec<Vec<(i64, i64)>>, Vec<Vec<(i64, i64)>>) {
    use clipper2_rust::{
        core::Paths64,
        inflate_paths_64,
        offset::{EndType, JoinType},
    };

    let to_paths64 = |pts: &[(i64, i64)]| -> Path64 {
        pts.iter().map(|&(x, y)| Point64 { x, y }).collect()
    };

    let mut outer_paths: Vec<Vec<(i64, i64)>> = Vec::new();
    let mut hole_paths: Vec<Vec<(i64, i64)>> = Vec::new();

    for contour in contours {
        if contour.points.is_empty() {
            continue;
        }
        let input: Paths64 = vec![to_paths64(&contour.points)];
        // delta sign: negative = shrink (route inside), positive = grow (route outside)
        let delta = if contour.is_hole {
            tool_radius_nm as f64
        } else {
            -(tool_radius_nm as f64)
        };
        let offset = inflate_paths_64(&input, delta, JoinType::Round, EndType::Polygon, 2.0, 0.0);
        let converted: Vec<Vec<(i64, i64)>> =
            offset.iter().map(|p| p.iter().map(|pt| (pt.x, pt.y)).collect()).collect();

        if contour.is_hole {
            hole_paths.extend(converted);
        } else {
            outer_paths.extend(converted);
        }
    }

    (outer_paths, hole_paths)
}

// ---------------------------------------------------------------------------
// Point-in-polygon test (ray casting, nm coordinates)
// ---------------------------------------------------------------------------

fn point_in_polygon_nm(pt: (i64, i64), poly: &Path64) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let (px, py) = (pt.0 as f64, pt.1 as f64);
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i].x as f64, poly[i].y as f64);
        let (xj, yj) = (poly[j].x as f64, poly[j].y as f64);
        if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}
