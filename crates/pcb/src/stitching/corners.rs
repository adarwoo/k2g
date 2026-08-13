//! Corners of a stitched contour: where they are, how sharp, and what a round cutter
//! must leave at each one.
//!
//! Two callers, both needing the same thing for opposite reasons. The cutout fit test
//! ([`super::fit_cutout`]) needs the corner *budget* — the material a round cutter is
//! always going to leave behind — so it can tell an unreachable arm from the fillets
//! that are simply what a round tool does. Corner relief needs the corners themselves,
//! to drill into.
//!
//! Angles are read off the **typed segments**, never the tessellated points. A flattened
//! arc is a few dozen vertices each turning a degree or two; summing those as corners
//! would produce thousands of near-straight "corners" and a budget made entirely of
//! rounding error.

use super::{signed_area_nm2, Contour, Segment};

/// A corner the cutter has to negotiate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    /// The vertex itself, nanometres.
    pub at: (i64, i64),
    /// Interior angle in radians, measured **inside the contour**, in (0, π) — only
    /// convex corners are reported, since those are the ones a round tool cannot enter.
    pub interior_rad: f64,
    /// Unit vector along the angle bisector, pointing into the contour's interior.
    pub bisector: (f64, f64),
    /// Index of the segment that ends at this vertex.
    pub segment: usize,
}

/// Joins turning less than this read as straight.
///
/// An arc meeting its own tangent line is a join of exactly π, and floating point will
/// not say so exactly. Counting it as a corner would drill a relief hole into a corner
/// that is already round — which is both pointless and, on a tight radius, a hole that
/// does not fit.
pub const MIN_CORNER_TURN_RAD: f64 = 0.02; // ~1.15°

/// The area a cutter of radius `r_nm` must leave at a corner of interior angle `θ`.
///
/// The cutter rolls into the corner until it touches both edges and can go no further,
/// leaving the region between the two edges and the arc:
///
/// ```text
///     fillet(R, θ) = R² · ( cot(θ/2) − (π − θ)/2 )
/// ```
///
/// the tangent kite (`R²·cot(θ/2)`) less the sector the cutter actually sweeps
/// (`R²·(π−θ)/2`). At θ = π/2 this is `R²(1 − π/4) ≈ 0.215 R²`, the familiar square
/// corner; at θ = π it is zero, as a straight edge should be.
///
/// This exists so a fit test can tell "the cutter cannot reach in there" from "the
/// cutter is round". Without it, every rectangular cutout looks unreachable.
pub fn corner_fillet_area_nm2(r_nm: f64, interior_rad: f64) -> f64 {
    if !(0.0..=std::f64::consts::PI).contains(&interior_rad) {
        return 0.0;
    }
    let half = interior_rad / 2.0;
    let tan_half = half.tan();
    if tan_half.abs() < 1e-12 {
        return 0.0;
    }
    let area = r_nm * r_nm * (1.0 / tan_half - (std::f64::consts::PI - interior_rad) / 2.0);
    area.max(0.0)
}

/// Every convex corner of `contour`, in contour order.
///
/// "Convex" is measured against the contour's own interior, which is read from the sign
/// of its signed area rather than assumed: nothing in this crate normalises winding, and
/// a square stitched clockwise must report the same four 90° corners as the same square
/// stitched anticlockwise. Get that wrong and every relief drill lands outside the board.
pub fn convex_corners(contour: &Contour, min_turn_rad: f64) -> Vec<Corner> {
    let n = contour.segments.len();
    if n < 3 {
        return Vec::new();
    }
    // Positive area means the points run one way round; the interior is then on a known
    // side of every turn. The sign folds that into the angle so the caller never has to.
    let sign = if signed_area_nm2(&contour.points) >= 0 {
        1.0
    } else {
        -1.0
    };

    let mut corners = Vec::new();
    for i in 0..n {
        let incoming = &contour.segments[i];
        let outgoing = &contour.segments[(i + 1) % n];

        let u = tangent_out(incoming);
        let v = tangent_in(outgoing);
        let (Some(u), Some(v)) = (unit(u), unit(v)) else {
            continue;
        };

        let cross = u.0 * v.1 - u.1 * v.0;
        let dot = u.0 * v.0 + u.1 * v.1;
        let turn = cross.atan2(dot);
        if turn.abs() < min_turn_rad {
            continue; // straight through: not a corner
        }

        let interior = std::f64::consts::PI - sign * turn;
        if interior <= 0.0 || interior >= std::f64::consts::PI {
            continue; // reflex: the cutter reaches into these unaided
        }

        // The bisector of the two edge directions, pointing into the interior. `-u` and
        // `v` both point away from the vertex along an edge, so their sum bisects the
        // angle between them.
        let Some(bisector) = unit((v.0 - u.0, v.1 - u.1)) else {
            continue;
        };

        corners.push(Corner {
            at: segment_end(incoming),
            interior_rad: interior,
            bisector,
            segment: i,
        });
    }
    corners
}

fn unit(v: (f64, f64)) -> Option<(f64, f64)> {
    let len = v.0.hypot(v.1);
    (len > 1e-9).then(|| (v.0 / len, v.1 / len))
}

fn segment_end(seg: &Segment) -> (i64, i64) {
    match seg {
        Segment::Line { end, .. } => *end,
        Segment::Arc { end, .. } => *end,
        Segment::Bezier { end, .. } => *end,
    }
}

/// Direction of travel as the segment *arrives* at its end point.
fn tangent_out(seg: &Segment) -> (f64, f64) {
    match seg {
        Segment::Line { start, end } => diff(*end, *start),
        Segment::Arc { start, mid, end } => arc_tangent(*start, *mid, *end, false),
        Segment::Bezier {
            control2,
            end,
            start,
            ..
        } => {
            let d = diff(*end, *control2);
            if d.0.hypot(d.1) > 1e-9 {
                d
            } else {
                diff(*end, *start)
            }
        }
    }
}

/// Direction of travel as the segment *leaves* its start point.
fn tangent_in(seg: &Segment) -> (f64, f64) {
    match seg {
        Segment::Line { start, end } => diff(*end, *start),
        Segment::Arc { start, mid, end } => arc_tangent(*start, *mid, *end, true),
        Segment::Bezier {
            start,
            control1,
            end,
            ..
        } => {
            let d = diff(*control1, *start);
            if d.0.hypot(d.1) > 1e-9 {
                d
            } else {
                diff(*end, *start)
            }
        }
    }
}

fn diff(a: (i64, i64), b: (i64, i64)) -> (f64, f64) {
    ((a.0 - b.0) as f64, (a.1 - b.1) as f64)
}

/// Tangent to a three-point arc at its start (`at_start`) or end.
///
/// Falls back to the chord when the three points are collinear and there is no
/// circumcentre — which is what a degenerate "arc" is.
fn arc_tangent(start: (i64, i64), mid: (i64, i64), end: (i64, i64), at_start: bool) -> (f64, f64) {
    let chord = diff(end, start);
    let Some((cx, cy)) = arc_centre(start, mid, end) else {
        return chord;
    };

    let p = if at_start { start } else { end };
    let radial = (p.0 as f64 - cx, p.1 as f64 - cy);
    // Rotate the radius a quarter turn; the sweep direction picks which quarter.
    let ccw = (mid.0 - start.0) as f64 * (end.1 - mid.1) as f64
        - (mid.1 - start.1) as f64 * (end.0 - mid.0) as f64
        >= 0.0;
    if ccw {
        (-radial.1, radial.0)
    } else {
        (radial.1, -radial.0)
    }
}

fn arc_centre(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> Option<(f64, f64)> {
    let (ax, ay) = (a.0 as f64, a.1 as f64);
    let (bx, by) = (b.0 as f64, b.1 as f64);
    let (cx, cy) = (c.0 as f64, c.1 as f64);
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d.abs() < 1e-6 {
        return None;
    }
    let a2 = ax * ax + ay * ay;
    let b2 = bx * bx + by * by;
    let c2 = cx * cx + cy * cy;
    Some((
        (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d,
        (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// nm-space square, given as four typed line segments.
    fn square(ccw: bool) -> Contour {
        let mm = 1_000_000_i64;
        let mut pts = vec![(0, 0), (10 * mm, 0), (10 * mm, 10 * mm), (0, 10 * mm)];
        if !ccw {
            pts.reverse();
        }
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

    /// The interior angle must not depend on which way the contour happened to stitch.
    ///
    /// `stitch_fragments` joins shapes in whatever order it finds them and flips
    /// fragments to make ends meet, so a contour's winding is not something a caller
    /// chose — and nothing in this crate normalises it. Read the interior from the wrong
    /// side and a square reports four 270° corners, which puts every relief drill
    /// outside the board instead of in it.
    #[test]
    fn the_interior_angle_is_the_same_whichever_way_the_contour_was_stitched() {
        for ccw in [true, false] {
            let corners = convex_corners(&square(ccw), MIN_CORNER_TURN_RAD);
            assert_eq!(corners.len(), 4, "a square has four corners (ccw={ccw})");
            for c in &corners {
                assert!(
                    (c.interior_rad - PI / 2.0).abs() < 1e-9,
                    "expected 90°, got {}° (ccw={ccw})",
                    c.interior_rad.to_degrees()
                );
            }
        }
    }

    /// The bisector must point into the shape, whichever way it was wound.
    ///
    /// It is what a relief drill is placed along. Pointing outward puts the hole in the
    /// board rather than in the waste — cutting metal the drawing says to keep.
    #[test]
    fn the_bisector_points_into_the_contour() {
        for ccw in [true, false] {
            for c in convex_corners(&square(ccw), MIN_CORNER_TURN_RAD) {
                // Step a micron along the bisector; it must land inside the square.
                let x = c.at.0 as f64 + c.bisector.0 * 1_000.0;
                let y = c.at.1 as f64 + c.bisector.1 * 1_000.0;
                assert!(
                    x > 0.0 && x < 10_000_000.0 && y > 0.0 && y < 10_000_000.0,
                    "bisector left the square at {:?} (ccw={ccw})",
                    (x, y)
                );
            }
        }
    }

    /// A right angle's fillet is the square minus the quarter circle.
    ///
    /// The one corner with a closed form anyone can check by eye, and the anchor for the
    /// whole corner budget: without that budget the cutout fit test subtracts nothing for
    /// the fillets a round cutter always leaves and refuses every rectangular cutout as
    /// unreachable.
    #[test]
    fn the_fillet_area_of_a_right_angle_is_the_square_minus_the_quarter_circle() {
        let r = 1_000_000.0; // 1 mm in nm
        let expected = r * r * (1.0 - PI / 4.0);
        assert!((corner_fillet_area_nm2(r, PI / 2.0) - expected).abs() < 1.0);
        // A straight edge leaves nothing.
        assert!(corner_fillet_area_nm2(r, PI) < 1.0);
        // Sharper corners strand more material.
        assert!(corner_fillet_area_nm2(r, PI / 4.0) > corner_fillet_area_nm2(r, PI / 2.0));
    }

    /// A reflex corner is not a corner the cutter struggles with.
    ///
    /// Where the opening turns back on itself the tool has the whole outside of the turn
    /// to swing through, so there is nothing to relieve. Drilling there would cut into
    /// board the drawing keeps.
    #[test]
    fn a_reflex_corner_is_not_reported() {
        let mm = 1_000_000_i64;
        // An L: five convex corners and one reflex.
        let pts = vec![
            (0, 0),
            (10 * mm, 0),
            (10 * mm, 4 * mm),
            (4 * mm, 4 * mm),
            (4 * mm, 10 * mm),
            (0, 10 * mm),
        ];
        let segments = (0..6)
            .map(|i| Segment::Line {
                start: pts[i],
                end: pts[(i + 1) % 6],
            })
            .collect();
        let contour = Contour {
            points: pts,
            segments,
            is_hole: true,
        };

        let corners = convex_corners(&contour, MIN_CORNER_TURN_RAD);
        assert_eq!(
            corners.len(),
            5,
            "five convex corners, the reflex one skipped"
        );
        assert!(corners.iter().all(|c| c.interior_rad < PI));
    }
}
