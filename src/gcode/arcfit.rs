//! Curve fitting and curve flattening — the two directions between a polyline and the
//! arcs a controller can express.
//!
//! # Why fitting exists
//!
//! A curved board edge reaches the generator as a polyline and nothing else. KiCad's arcs
//! and beziers survive stitching as typed `pcb::Segment`s, but the routing offset
//! (`pcb::routing_offset`) is points-in/points-out: Clipper does the work that actually
//! matters for correctness — self-intersection removal, concave-corner trimming, choosing
//! the surviving path when an offset splits a contour — and it does it on a polygon. So a
//! rounded board edge came out as hundreds of `G1` chords.
//!
//! Fitting arcs back to that polyline is not a lossy shortcut, because **the polyline is
//! not the source of truth about the curve either**. Two facts make it sound:
//!
//! - The polyline is tessellated to a 1 µm sagitta (`SAGITTA_TOLERANCE_NM` in
//!   `crates/pcb/src/stitching/tessellate.rs`), which is an order of magnitude finer than
//!   the fit tolerance a router profile asks for, and three below what one repeats.
//! - **Offsetting does not preserve curve type.** A line offsets to a line and an arc to a
//!   concentric arc, but the offset of a bezier is not a bezier — it has no exact form in
//!   any G-code word. Arcs are therefore not a degraded stand-in for the offset of a
//!   curve; within a tolerance they are the best exact description of it that exists.
//!
//! Clipper's round joins come back for free: a `JoinType::Round` corner *is* an arc of
//! exactly the tool radius, and reads as one to the fitter.
//!
//! # Why flattening exists
//!
//! The other direction, for a controller with no arc word at all. Once outlines emit
//! `G2`/`G3`, a profile whose `cut_arc` is empty would silently lose its outline — an
//! empty motion template renders to nothing. Flattening is what lets the fallback
//! (`cut_arc` → `cut_linear`) end somewhere every machine can reach.
//!
//! # The safety property
//!
//! Every function here is **tolerance-bounded and conservative**. A fit is accepted only
//! when every source point lies within `tolerance` of the fitted arc, so the failure mode
//! is "stayed a line", which is exactly today's output. Nothing here can widen a path or
//! cut outside it.

use units::Length;

use crate::gcode::plan::Point;

/// Below this the fitter treats two points as one. Well under the 1 µm the polyline is
/// tessellated to, so it only ever collapses genuine duplicates.
const EPS_MM: f64 = 1e-9;

/// Fewer points than this never become an arc.
///
/// Three points define a circle exactly, so a three-point run would "fit" any corner
/// perfectly and turn every sharp vertex into a tiny arc. Four is the shortest run where
/// the fit is over-determined and so can actually be wrong — which is what makes accepting
/// it meaningful.
const MIN_ARC_POINTS: usize = 4;

/// A fitted run of a polyline: what one move of the path became.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathSeg {
    /// A straight move to `to`.
    Line { to: Point },
    /// An arc to `to` about `centre`. `ccw` is the direction travelled, matching
    /// [`RouteMove::Arc`](crate::gcode::routing::RouteMove)'s own convention.
    Arc { to: Point, centre: Point, ccw: bool },
}

impl PathSeg {
    /// Where this segment ends, whichever kind it is.
    pub fn end(&self) -> Point {
        match *self {
            Self::Line { to } | Self::Arc { to, .. } => to,
        }
    }
}

/// A plain 2-D point in millimetres. The fitter works in `f64` throughout — [`Length`] is
/// a units-checked type whose arithmetic is not free, and the geometry here is dense
/// (every candidate run is re-tested as it grows).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pt {
    x: f64,
    y: f64,
}

impl Pt {
    fn of(p: Point) -> Self {
        Self { x: p.x.as_mm(), y: p.y.as_mm() }
    }

    fn into_point(self) -> Point {
        Point::new(Length::from_mm(self.x), Length::from_mm(self.y))
    }

    fn dist(self, other: Pt) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

/// The circle through three points, or `None` when they are collinear (or coincident).
///
/// Solved from the perpendicular-bisector intersection in the form that keeps the
/// determinant explicit, so the collinear case is a numeric test on one value rather than
/// a division that quietly returns an infinity.
fn circle_through(a: Pt, b: Pt, c: Pt) -> Option<(Pt, f64)> {
    let det = 2.0 * ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x));
    if det.abs() < 1e-12 {
        return None;
    }
    let ab = (b.x - a.x) * (b.x + a.x) + (b.y - a.y) * (b.y + a.y);
    let ac = (c.x - a.x) * (c.x + a.x) + (c.y - a.y) * (c.y + a.y);
    let cx = ((c.y - a.y) * ab - (b.y - a.y) * ac) / det;
    let cy = ((b.x - a.x) * ac - (c.x - a.x) * ab) / det;
    let centre = Pt { x: cx, y: cy };
    Some((centre, centre.dist(a)))
}

/// Whether the run `a..=c` turns counter-clockwise, from the sign of the cross product.
fn turns_ccw(a: Pt, b: Pt, c: Pt) -> bool {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x) > 0.0
}

/// Whether the arc through `run` stays within `tol` of **the polyline**, and sweeps
/// consistently in one direction.
///
/// Three independent things are checked, and every one of them is load-bearing:
///
/// 1. **Each vertex is on the circle.** The obvious one, and on its own badly insufficient.
/// 2. **Each chord's sagitta is within tolerance.** The four corners of a square lie
///    *exactly* on a circle, so a vertex-only test fits a square as an arc and the cutter
///    rounds off every corner of the board. What matters is not whether the points are on
///    the arc but whether the *path between them* is: a chord subtending θ bows
///    `r(1 − cos(θ/2))` away from it. On a polyline tessellated to 1 µm the chords are
///    short and this is nothing; on a square's 5 mm sides it is a millimetre, and the fit
///    is refused.
/// 3. **The sweep is monotonic and under a full turn.** A zig-zag straddling the circle
///    would pass (1) and (2) while doubling back, and a closed loop would collapse into a
///    single arc whose start and end coincide — which `RouteMove::Arc` reads as a full
///    circle, a different path from the one asked for.
fn run_fits(run: &[Pt], centre: Pt, radius: f64, tol: f64, ccw: bool) -> bool {
    if radius <= tol {
        return false;
    }
    let angle = |p: Pt| (p.y - centre.y).atan2(p.x - centre.x);

    let mut swept = 0.0_f64;
    let mut previous = angle(run[0]);
    for (index, point) in run.iter().enumerate().skip(1) {
        if (centre.dist(*point) - radius).abs() > tol {
            return false;
        }
        // (2): how far the straight move to this point bows away from the arc it would
        // replace. `half` is clamped because a chord marginally longer than the diameter
        // is reachable through rounding on a near-half-turn step.
        let chord = run[index - 1].dist(*point);
        let half = (chord * 0.5 / radius).clamp(0.0, 1.0);
        if radius * (1.0 - (1.0 - half * half).max(0.0).sqrt()) > tol {
            return false;
        }
        let current = angle(*point);
        // Step into (-PI, PI], then require it to go the way the run started.
        let mut step = current - previous;
        while step > std::f64::consts::PI {
            step -= std::f64::consts::TAU;
        }
        while step <= -std::f64::consts::PI {
            step += std::f64::consts::TAU;
        }
        if (ccw && step <= 0.0) || (!ccw && step >= 0.0) {
            return false;
        }
        swept += step.abs();
        // A sweep at or past a full turn would put the arc's end back on its start, which
        // `RouteMove::Arc` reads as a full circle — a different path from the one asked
        // for. Kept strictly under, so such a run is split into two arcs instead.
        if swept >= std::f64::consts::TAU * 0.995 {
            return false;
        }
        previous = current;
    }
    true
}

/// Fits `points` with arcs wherever an arc holds to within `tolerance`, and lines
/// everywhere else.
///
/// Greedy longest-run: from each start, the run is extended while the circle through its
/// first, middle and last points still contains every point of it. Greedy rather than
/// optimal because the input is a machine path, not a drawing — a marginally longer arc
/// somewhere is worth nothing, and a fitter that can be reasoned about line by line is
/// worth a lot.
///
/// The returned segments are the moves *after* `points[0]`, which is the path's start and
/// is already where the tool is. An input of fewer than two points yields nothing.
pub fn fit(points: &[Point], tolerance: Length) -> Vec<PathSeg> {
    let tol = tolerance.as_mm().max(EPS_MM);

    // Collapse duplicates up front: a repeated point has no direction, and would make
    // every angular test below meaningless.
    let mut pts: Vec<Pt> = Vec::with_capacity(points.len());
    for point in points {
        let p = Pt::of(*point);
        if pts.last().is_none_or(|last: &Pt| last.dist(p) > EPS_MM) {
            pts.push(p);
        }
    }
    if pts.len() < 2 {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    while start + 1 < pts.len() {
        let mut best: Option<(usize, Pt, bool)> = None;

        if pts.len() - start >= MIN_ARC_POINTS {
            // Grow the run one point at a time, re-testing the circle through the current
            // endpoints and midpoint. Re-deriving the circle as the run grows (rather than
            // keeping the first one) is what lets a long gentle curve be found: the first
            // three points of a fine polyline are nearly collinear and their circle is
            // meaningless.
            let mut end = start + MIN_ARC_POINTS - 1;
            while end < pts.len() {
                let run = &pts[start..=end];
                let mid = run[run.len() / 2];
                match circle_through(run[0], mid, run[run.len() - 1]) {
                    Some((centre, radius)) => {
                        let ccw = turns_ccw(run[0], mid, run[run.len() - 1]);
                        if run_fits(run, centre, radius, tol, ccw) {
                            best = Some((end, centre, ccw));
                            end += 1;
                        } else {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }

        match best {
            Some((end, centre, ccw)) => {
                out.push(PathSeg::Arc {
                    to: pts[end].into_point(),
                    centre: centre.into_point(),
                    ccw,
                });
                start = end;
            }
            None => {
                out.push(PathSeg::Line { to: pts[start + 1].into_point() });
                start += 1;
            }
        }
    }
    out
}

/// The number of chords an arc of `radius` sweeping `sweep` radians needs to stay within
/// `tol` of the true arc.
///
/// The same sagitta relation used for the 3-D preview (`scene.rs`) and the spiral
/// (`routing.rs`): a chord subtending θ sits `r(1 − cos(θ/2))` inside the arc.
fn chord_count(radius: f64, sweep: f64, tol: f64) -> usize {
    if radius <= tol {
        return 1;
    }
    let half = (1.0 - tol / radius).clamp(-1.0, 1.0).acos();
    if half <= f64::EPSILON {
        return 1;
    }
    ((sweep.abs() / (2.0 * half)).ceil() as usize).max(1)
}

/// Flattens an arc into the chord end points that approximate it to within `tolerance`.
///
/// `from` is where the tool already is; `centre` is absolute (not the incremental I/J
/// form). Returns the points to feed to, **excluding** `from` and ending exactly on `to`
/// — so the path's endpoint is preserved bit for bit and cannot drift by the accumulated
/// error of the chords.
///
/// `from == to` is read as a **full circle**, matching `RouteMove::Arc`'s own convention.
pub fn flatten_arc(
    from: Point,
    to: Point,
    centre: Point,
    ccw: bool,
    tolerance: Length,
) -> Vec<Point> {
    let tol = tolerance.as_mm().max(EPS_MM);
    let (f, t, c) = (Pt::of(from), Pt::of(to), Pt::of(centre));
    let radius = c.dist(f);
    if radius <= tol {
        return vec![to];
    }

    let start = (f.y - c.y).atan2(f.x - c.x);
    let end = (t.y - c.y).atan2(t.x - c.x);
    let mut sweep = end - start;
    if ccw {
        while sweep <= 0.0 {
            sweep += std::f64::consts::TAU;
        }
    } else {
        while sweep >= 0.0 {
            sweep -= std::f64::consts::TAU;
        }
    }
    // A closed arc is a full turn, not a zero one.
    if f.dist(t) <= EPS_MM {
        sweep = if ccw { std::f64::consts::TAU } else { -std::f64::consts::TAU };
    }

    let n = chord_count(radius, sweep, tol);
    let mut out = Vec::with_capacity(n);
    for i in 1..n {
        let angle = start + sweep * (i as f64) / (n as f64);
        out.push(
            Pt { x: c.x + radius * angle.cos(), y: c.y + radius * angle.sin() }.into_point(),
        );
    }
    // Exactly the caller's endpoint, never a recomputed one.
    out.push(to);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOL: f64 = 0.01;

    fn tol() -> Length {
        Length::from_mm(TOL)
    }

    fn pt(x: f64, y: f64) -> Point {
        Point::new(Length::from_mm(x), Length::from_mm(y))
    }

    /// Points sampled off a circle, `sweep` radians from `start`, at a fine step.
    fn arc_points(cx: f64, cy: f64, r: f64, start: f64, sweep: f64, n: usize) -> Vec<Point> {
        (0..=n)
            .map(|i| {
                let a = start + sweep * (i as f64) / (n as f64);
                pt(cx + r * a.cos(), cy + r * a.sin())
            })
            .collect()
    }

    /// The property the whole module rests on: a fitted arc never departs from the points
    /// it replaced by more than the tolerance. Everything else is an optimisation; this is
    /// the part that decides whether the cutter goes where the board needs it to.
    fn assert_within_tolerance(source: &[Point], fitted: &[PathSeg]) {
        let mut from = Pt::of(source[0]);
        let mut cursor = 0usize;
        for seg in fitted {
            let end = Pt::of(seg.end());
            // Every source point up to this segment's end must sit on it.
            let stop = source
                .iter()
                .position(|p| Pt::of(*p).dist(end) < 1e-6)
                .expect("every fitted endpoint is a source point");
            if let PathSeg::Arc { centre, .. } = *seg {
                let c = Pt::of(centre);
                let radius = c.dist(from);
                for p in &source[cursor..=stop] {
                    let err = (c.dist(Pt::of(*p)) - radius).abs();
                    assert!(err <= TOL + 1e-9, "point {p:?} is {err} mm off its fitted arc");
                }
            }
            cursor = stop;
            from = end;
        }
    }

    #[test]
    fn a_sampled_circle_arc_fits_as_one_arc() {
        let source = arc_points(10.0, 5.0, 3.0, 0.0, std::f64::consts::FRAC_PI_2, 40);
        let fitted = fit(&source, tol());

        assert_eq!(fitted.len(), 1, "a quarter circle is one arc, got {fitted:?}");
        let PathSeg::Arc { to, centre, ccw } = fitted[0] else { panic!("not an arc") };
        assert!(ccw, "sampled anticlockwise");
        assert!(Pt::of(centre).dist(Pt { x: 10.0, y: 5.0 }) < 1e-6, "centre recovered");
        assert!(Pt::of(to).dist(Pt::of(source[source.len() - 1])) < 1e-9, "ends where it should");
        assert_within_tolerance(&source, &fitted);
    }

    #[test]
    fn direction_is_recovered_both_ways() {
        for (sweep, expect_ccw) in
            [(std::f64::consts::FRAC_PI_2, true), (-std::f64::consts::FRAC_PI_2, false)]
        {
            let source = arc_points(0.0, 0.0, 4.0, 0.0, sweep, 40);
            let fitted = fit(&source, tol());
            let PathSeg::Arc { ccw, .. } = fitted[0] else { panic!("not an arc: {fitted:?}") };
            assert_eq!(ccw, expect_ccw, "sweep {sweep}");
        }
    }

    /// A straight run must stay straight. An arc through three nearly-collinear points has
    /// an enormous radius and would "fit" beautifully — emitting a G2 with a metre-scale
    /// I/J for what is a straight edge.
    #[test]
    fn a_straight_run_stays_lines() {
        let source: Vec<Point> = (0..=20).map(|i| pt(i as f64 * 0.5, 3.0)).collect();
        let fitted = fit(&source, tol());
        assert!(
            fitted.iter().all(|s| matches!(s, PathSeg::Line { .. })),
            "a straight edge must not become an arc: {fitted:?}"
        );
    }

    /// A sharp corner is not a tiny arc. Three points define a circle exactly, so without
    /// the four-point floor every vertex in the path would fit one.
    #[test]
    fn a_sharp_corner_stays_two_lines() {
        let source = vec![pt(0.0, 0.0), pt(5.0, 0.0), pt(5.0, 5.0), pt(0.0, 5.0)];
        let fitted = fit(&source, tol());
        assert!(
            fitted.iter().all(|s| matches!(s, PathSeg::Line { .. })),
            "square corners must stay corners: {fitted:?}"
        );
    }

    /// A coarsely sampled arc is refused, however perfectly its vertices sit on the circle.
    ///
    /// The same rule that keeps a square square, in its other guise: the emitted path must
    /// agree with the polyline the planner actually offset and placed tabs against. Points
    /// on the circle are not enough — the straight moves *between* them have to be on it
    /// too, or the fitted arc cuts somewhere the plan never went.
    #[test]
    fn a_coarsely_sampled_arc_is_refused() {
        // 8 points over a quarter circle of radius 20: vertices exact, chords bow ~0.15 mm.
        let coarse = arc_points(0.0, 0.0, 20.0, 0.0, std::f64::consts::FRAC_PI_2, 8);
        assert!(
            fit(&coarse, tol()).iter().all(|s| matches!(s, PathSeg::Line { .. })),
            "chords bowing past tolerance must stay lines"
        );

        // The same arc sampled the way the stitcher actually tessellates it: fitted.
        let fine = arc_points(0.0, 0.0, 20.0, 0.0, std::f64::consts::FRAC_PI_2, 400);
        assert!(
            matches!(fit(&fine, tol()).as_slice(), [PathSeg::Arc { .. }]),
            "a finely tessellated arc is exactly what this exists to recover"
        );
    }

    /// A closed loop must not collapse to one arc whose end sits on its start — that is
    /// `RouteMove::Arc`'s full-circle form, and it would describe a different path.
    #[test]
    fn a_full_circle_does_not_become_a_single_closed_arc() {
        let source = arc_points(0.0, 0.0, 6.0, 0.0, std::f64::consts::TAU, 120);
        let fitted = fit(&source, tol());
        assert!(fitted.len() >= 2, "a full circle needs splitting, got {fitted:?}");
        assert_within_tolerance(&source, &fitted);
    }

    #[test]
    fn degenerate_input_never_panics_and_never_invents_an_arc() {
        assert!(fit(&[], tol()).is_empty());
        assert!(fit(&[pt(1.0, 1.0)], tol()).is_empty());
        assert_eq!(fit(&[pt(0.0, 0.0), pt(1.0, 0.0)], tol()).len(), 1);
        // All the same point: collapsed to one, so there is no move to make.
        assert!(fit(&[pt(2.0, 2.0), pt(2.0, 2.0), pt(2.0, 2.0)], tol()).is_empty());
    }

    #[test]
    fn flattening_stays_within_tolerance_and_lands_exactly_on_the_end() {
        let (from, to, centre) = (pt(3.0, 0.0), pt(0.0, 3.0), pt(0.0, 0.0));
        let chords = flatten_arc(from, to, centre, true, tol());

        assert!(chords.len() > 4, "a quarter circle needs several chords: {}", chords.len());
        for p in &chords {
            let r = Pt::of(*p).dist(Pt { x: 0.0, y: 0.0 });
            assert!((r - 3.0).abs() <= TOL + 1e-9, "chord vertex {r} mm from centre");
        }
        let last = chords[chords.len() - 1];
        assert!(
            Pt::of(last).dist(Pt::of(to)) < 1e-12,
            "the endpoint must be the caller's, not a recomputed one"
        );
    }

    /// `from == to` means a full circle, as `RouteMove::Arc` defines it — not a zero-length
    /// move. Getting this wrong would drop every routed hole's finishing lap.
    #[test]
    fn flattening_a_closed_arc_walks_the_whole_circle() {
        let centre = pt(0.0, 0.0);
        let start = pt(5.0, 0.0);
        let chords = flatten_arc(start, start, centre, true, tol());
        assert!(chords.len() > 20, "a full circle, not a no-op: {}", chords.len());
        // It must actually go round: some vertex on the far side.
        assert!(
            chords.iter().any(|p| p.x.as_mm() < -4.9),
            "never reached the opposite side of the circle"
        );
    }

    #[test]
    fn a_finer_tolerance_gives_more_chords() {
        let (from, to, centre) = (pt(3.0, 0.0), pt(0.0, 3.0), pt(0.0, 0.0));
        let coarse = flatten_arc(from, to, centre, true, Length::from_mm(0.05));
        let fine = flatten_arc(from, to, centre, true, Length::from_mm(0.001));
        assert!(fine.len() > coarse.len(), "{} vs {}", fine.len(), coarse.len());
    }

    /// Round trip: fit a sampled arc, flatten it back, and the result must still describe
    /// the same curve. This is the composition the fallback chain actually performs when a
    /// controller has no arc word.
    #[test]
    fn fitting_then_flattening_returns_to_the_same_curve() {
        let source = arc_points(2.0, 2.0, 5.0, 0.3, 1.2, 60);
        let fitted = fit(&source, tol());

        let mut from = source[0];
        for seg in &fitted {
            if let PathSeg::Arc { to, centre, ccw } = *seg {
                for p in flatten_arc(from, to, centre, ccw, tol()) {
                    let r = Pt::of(p).dist(Pt { x: 2.0, y: 2.0 });
                    assert!((r - 5.0).abs() <= 2.0 * TOL, "{r} mm from the true centre");
                }
            }
            from = seg.end();
        }
        assert!(
            Pt::of(from).dist(Pt::of(source[source.len() - 1])) < 1e-9,
            "the path must still end where it did"
        );
    }

}
