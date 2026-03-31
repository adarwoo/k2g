/// Tessellate curved `BoardEdgeShape` primitives into polylines of `i64` nm
/// coordinates, suitable for feeding into clipper2-rust.
///
/// Caller convention: every function appends points to a caller-supplied `Vec`.
/// The last point is NOT repeated at the end — the caller closes the ring when
/// it needs to.

use std::f64::consts::{PI, TAU};

/// Minimum chord-height (sagitta) in nm before we stop subdividing an arc.
/// 1 000 nm = 1 µm — sub-micron accuracy, far finer than any router bit.
const SAGITTA_TOLERANCE_NM: f64 = 1_000.0;

/// Maximum number of segments per full circle (prevents degenerate cases on
/// very large radii).
const MAX_SEGMENTS_PER_CIRCLE: usize = 1024;
/// Minimum segments per full circle.
const MIN_SEGMENTS_PER_CIRCLE: usize = 8;

// ---------------------------------------------------------------------------
// Arc  (3-point: start / mid / end)
// ---------------------------------------------------------------------------

/// Append tessellated arc points from `start` through `mid` to `end` (all in
/// nm).  The arc direction (CW vs CCW) is derived from the sign of the
/// cross-product at `mid`, matching KiCad's convention.
///
/// The *start* point is **included**; the *end* point is **excluded** so that
/// callers can chain primitives without duplicating junction vertices.
pub fn tessellate_arc(
    out: &mut Vec<(i64, i64)>,
    sx: f64, sy: f64,
    mx: f64, my: f64,
    ex: f64, ey: f64,
) {
    // Circumcircle centre.
    let d = 2.0 * (sx * (my - ey) + mx * (ey - sy) + ex * (sy - my));
    if d.abs() < 1e-6 {
        // Collinear — degenerate arc, emit start only.
        out.push((sx.round() as i64, sy.round() as i64));
        return;
    }
    let sq = |v: f64| v * v;
    let m1 = sq(sx) + sq(sy);
    let m2 = sq(mx) + sq(my);
    let m3 = sq(ex) + sq(ey);
    let cx = (m1 * (my - ey) + m2 * (ey - sy) + m3 * (sy - my)) / d;
    let cy = (m1 * (ex - mx) + m2 * (sx - ex) + m3 * (mx - sx)) / d;
    let r = ((sx - cx).powi(2) + (sy - cy).powi(2)).sqrt();

    if r < 1.0 {
        out.push((sx.round() as i64, sy.round() as i64));
        return;
    }

    let angle = |px: f64, py: f64| (py - cy).atan2(px - cx);
    let t_start = angle(sx, sy);
    let t_mid   = angle(mx, my);
    let t_end   = angle(ex, ey);

    // Determine the angular span that goes through mid.
    let cw_to_end = (t_end - t_start).rem_euclid(TAU);
    let cw_to_mid = (t_mid - t_start).rem_euclid(TAU);
    // If mid sits on the CW arc from start→end, sweep is CW in screen coords
    // (positive angles).  Otherwise sweep CCW.
    let (span, ccw) = if cw_to_mid <= cw_to_end {
        (cw_to_end, false)
    } else {
        (TAU - cw_to_end, true)
    };

    // Number of segments based on sagitta tolerance.
    // sagitta = r * (1 - cos(half_angle))  ≈  r * half_angle² / 2
    // Solve for n: span/n = half_angle  =>  n = span / (2 * acos(1 - tol/r))
    let half_angle = (1.0 - SAGITTA_TOLERANCE_NM / r).clamp(-1.0, 1.0).acos();
    let n = if half_angle < 1e-9 {
        MAX_SEGMENTS_PER_CIRCLE
    } else {
        ((span / (2.0 * half_angle)).ceil() as usize)
            .clamp(MIN_SEGMENTS_PER_CIRCLE, MAX_SEGMENTS_PER_CIRCLE)
    };

    let step = if ccw { -span / n as f64 } else { span / n as f64 };
    for i in 0..n {
        let t = t_start + step * i as f64;
        let px = cx + r * t.cos();
        let py = cy + r * t.sin();
        out.push((px.round() as i64, py.round() as i64));
    }
}

// ---------------------------------------------------------------------------
// Full circle (centre + radius point)
// ---------------------------------------------------------------------------

pub fn tessellate_circle(
    out: &mut Vec<(i64, i64)>,
    cx: f64, cy: f64,
    rx: f64, ry: f64,
) {
    let r = ((rx - cx).powi(2) + (ry - cy).powi(2)).sqrt();
    if r < 1.0 {
        out.push((cx.round() as i64, cy.round() as i64));
        return;
    }
    let half_angle = (1.0 - SAGITTA_TOLERANCE_NM / r).clamp(-1.0, 1.0).acos();
    let n = if half_angle < 1e-9 {
        MAX_SEGMENTS_PER_CIRCLE
    } else {
        (TAU / (2.0 * half_angle)).ceil() as usize
    }
    .clamp(MIN_SEGMENTS_PER_CIRCLE, MAX_SEGMENTS_PER_CIRCLE);

    for i in 0..n {
        let t = TAU * i as f64 / n as f64;
        let px = cx + r * t.cos();
        let py = cy + r * t.sin();
        out.push((px.round() as i64, py.round() as i64));
    }
}

// ---------------------------------------------------------------------------
// Rectangle (with optional corner radius)
// ---------------------------------------------------------------------------

pub fn tessellate_rectangle(
    out: &mut Vec<(i64, i64)>,
    x0: f64, y0: f64, // top-left in nm
    x1: f64, y1: f64, // bottom-right in nm
    corner_radius_nm: f64,
) {
    let r = corner_radius_nm.clamp(0.0, ((x1 - x0).abs().min((y1 - y0).abs())) * 0.5);
    let (lx, rx) = (x0.min(x1), x0.max(x1));
    let (ty, by) = (y0.min(y1), y0.max(y1));

    if r <= 1.0 {
        // Sharp corners — 4 points.
        out.push((lx.round() as i64, ty.round() as i64));
        out.push((rx.round() as i64, ty.round() as i64));
        out.push((rx.round() as i64, by.round() as i64));
        out.push((lx.round() as i64, by.round() as i64));
        return;
    }

    // Each rounded corner is a quarter-arc.
    let corners: [(f64, f64, f64, f64, f64, f64, f64, f64); 4] = [
        // (arc_cx, arc_cy,  from_angle, to_angle, straight_sx, straight_sy, straight_ex, straight_ey)
        (lx + r, ty + r, PI,         PI * 1.5, lx,      by - r, lx + r,  ty     ), // top-left
        (rx - r, ty + r, PI * 1.5,   TAU,      lx + r,  ty,     rx,      ty + r ), // top-right
        (rx - r, by - r, 0.0,        PI * 0.5, rx,      ty + r, rx - r,  by     ), // bottom-right
        (lx + r, by - r, PI * 0.5,   PI,       rx - r,  by,     lx,      by - r ), // bottom-left
    ];

    let half_angle = (1.0 - SAGITTA_TOLERANCE_NM / r).clamp(-1.0, 1.0).acos();
    let segs_per_quarter = ((PI * 0.5 / (2.0 * half_angle)).ceil() as usize)
        .clamp(2, MAX_SEGMENTS_PER_CIRCLE / 4);

    for (acx, acy, t0, t1, _, _, _, _) in &corners {
        let span = t1 - t0;
        for i in 0..segs_per_quarter {
            let t = t0 + span * i as f64 / segs_per_quarter as f64;
            let px = acx + r * t.cos();
            let py = acy + r * t.sin();
            out.push((px.round() as i64, py.round() as i64));
        }
    }
}

// ---------------------------------------------------------------------------
// Cubic Bezier (4 control points)
// ---------------------------------------------------------------------------

/// Adaptive subdivision of a cubic Bezier curve.  Subdivides until the
/// control-polygon deviates less than `SAGITTA_TOLERANCE_NM` from the chord.
pub fn tessellate_bezier(
    out: &mut Vec<(i64, i64)>,
    p0x: f64, p0y: f64,
    p1x: f64, p1y: f64,
    p2x: f64, p2y: f64,
    p3x: f64, p3y: f64,
) {
    // Flatness check: max distance of control points from the chord p0→p3.
    fn flatness(p0x:f64, p0y:f64, p1x:f64, p1y:f64, p2x:f64, p2y:f64, p3x:f64, p3y:f64) -> f64 {
        let dx = p3x - p0x;
        let dy = p3y - p0y;
        let len = (dx*dx + dy*dy).sqrt().max(1e-9);
        let d1 = ((dy * p1x - dx * p1y + p3x * p0y - p3y * p0x) / len).abs();
        let d2 = ((dy * p2x - dx * p2y + p3x * p0y - p3y * p0x) / len).abs();
        d1.max(d2)
    }

    fn subdivide(
        out: &mut Vec<(i64, i64)>,
        p0x:f64, p0y:f64, p1x:f64, p1y:f64,
        p2x:f64, p2y:f64, p3x:f64, p3y:f64,
        depth: u32,
    ) {
        if depth > 16 || flatness(p0x,p0y,p1x,p1y,p2x,p2y,p3x,p3y) <= SAGITTA_TOLERANCE_NM {
            out.push((p0x.round() as i64, p0y.round() as i64));
            return;
        }
        // De Casteljau split at t=0.5.
        let m01x = (p0x+p1x)*0.5; let m01y = (p0y+p1y)*0.5;
        let m12x = (p1x+p2x)*0.5; let m12y = (p1y+p2y)*0.5;
        let m23x = (p2x+p3x)*0.5; let m23y = (p2y+p3y)*0.5;
        let m012x = (m01x+m12x)*0.5; let m012y = (m01y+m12y)*0.5;
        let m123x = (m12x+m23x)*0.5; let m123y = (m12y+m23y)*0.5;
        let mx = (m012x+m123x)*0.5; let my = (m012y+m123y)*0.5;
        subdivide(out, p0x,p0y, m01x,m01y, m012x,m012y, mx,my, depth+1);
        subdivide(out, mx,my, m123x,m123y, m23x,m23y, p3x,p3y, depth+1);
    }

    subdivide(out, p0x,p0y, p1x,p1y, p2x,p2y, p3x,p3y, 0);
}
