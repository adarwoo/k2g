//! Routing toolpath geometry — the pockets a router mills when a drill cannot make the
//! feature: a **round hole** too big to drill (or whose drill point would reach the
//! bed), and an **oblong slot**.
//!
//! **One strategy, by design** (settled 2026-07-25): plunge inside the feature, clear
//! outward removing *all* the material, and finish with a lap on the wall. This is the
//! only approach that never leaves a centre island (the flying-debris hazard of a
//! tangential contour) and leaves no entry witness mark on the finished wall — for a
//! through feature the interior is removed material anyway. There is deliberately **no**
//! user option here: no strategy choice, no island toggle, and the radial stepover is
//! fixed at [`STEPOVER_FRACTION`] of the tool diameter (a standard roughing pitch).
//!
//! The two shapes are the same construction about different medial sets:
//!
//! | feature | medial set | offset pass at radius `r` |
//! |---|---|---|
//! | round hole ([`spiral_hole`]) | the centre point | a full circle |
//! | oblong slot ([`slot_route`]) | the centre-line segment | a stadium (racetrack) |
//!
//! so a slot whose travel collapses to zero *is* a round hole, and [`slot_route`]
//! delegates to [`spiral_hole`] in that case rather than duplicating the degenerate
//! handling. Passes are stepped out by the pitch; the scallops between them are interior
//! material and the final pass sits exactly on the wall.
//!
//! **Arc direction is climb milling.** With an M03 (clockwise, seen from above) spindle,
//! the cut is climb when the material lies to the **right** of the direction of travel —
//! which inside a pocket means travelling **counter-clockwise**. Every wall pass here is
//! therefore CCW (`G3`). Climb is what keeps FR4's top copper from lifting.
//!
//! This holds because [`Placement`](super::placement::Placement) delivers a right-handed,
//! Y-up machine frame — it flips KiCad's Y-down board frame precisely so that a
//! handedness argument like this one can be made here at all.

use units::Length;

use crate::gcode::plan::Point;

/// Radial stepover as a fraction of the router diameter. Not user-exposed (KISS).
pub const STEPOVER_FRACTION: f64 = 0.5;

/// Plunge feed as a fraction of the tool's rated (lateral) feed.
///
/// A stock tool carries **one** rated feed, which is its *lateral* cutting feed — but a
/// straight plunge engages the tool's weak end-cutting geometry over its full diameter
/// at once. Driving a 0.4 mm router into FR4 at its lateral feed snaps it. A third is
/// the conventional derating for a straight (non-ramped) plunge.
///
/// Like [`STEPOVER_FRACTION`] this is a fixed constant, not a preference: it follows
/// from the move, and a wrong value here is a broken cutter. It disappears the day the
/// stock model carries a rated plunge feed of its own.
pub const PLUNGE_FEED_FRACTION: f64 = 1.0 / 3.0;

/// Below this radial reach (mm) the tool fills the feature across, so there is nothing
/// to step out to and the medial pass alone finishes it.
const MIN_REACH_MM: f64 = 1e-3;

/// A length in millimetres, with negative zero folded to zero.
///
/// `-0.0` is a perfectly ordinary `f64` — it falls out of `-r · nₓ` whenever the normal
/// has a zero component, i.e. for every axis-aligned slot — and it formats as the GCode
/// word `I-0`. Harmless to a controller, but noise in the program and a spurious textual
/// difference between two paths that are geometrically the same.
fn mm(value: f64) -> Length {
    // `== 0.0` is true of −0.0, so this replaces it and leaves every other value alone.
    Length::from_mm(if value == 0.0 { 0.0 } else { value })
}

/// One move of a routing toolpath, in **machine coordinates** (mm). The body renderer
/// maps each to a CNC primitive: `Rapid`→`rapid_move`, `Plunge`/`Cut`→`linear_cut`,
/// `Arc`→`cut_arc`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RouteMove {
    /// Rapid (G0) to a point — positioning above the work, and the final retract.
    Rapid { x: Length, y: Length, z: Length },
    /// Feed (G1) straight down into the material — the entry. Distinct from [`Self::Cut`]
    /// only so it can be rendered at the derated plunge feed ([`PLUNGE_FEED_FRACTION`]).
    Plunge { x: Length, y: Length, z: Length },
    /// Feed (G1) laterally at depth — a radial step-out or a straight run of a pass.
    Cut { x: Length, y: Length, z: Length },
    /// An arc (G2/G3) whose centre is at the start plus `(i, j)`. Always the
    /// incremental-centre (I/J) form; a full circle is the case `end == start`.
    Arc { x: Length, y: Length, i: Length, j: Length, ccw: bool },
}

/// Builds the spiral-pocket toolpath for one hole (single depth pass — PCBs are thin;
/// multi-pass depth is a later refinement that repeats the spiral per level).
///
/// - `center` — hole centre in machine coordinates.
/// - `z_retract` — height above the surface to rapid at (the fixture R-plane).
/// - `z_bottom` — the cutting depth, a negative machine-Z (board top is Z0).
/// - `hole_diameter` / `tool_diameter` — the router must be smaller (the assigner
///   guarantees it); the tool centre sweeps out to `(hole − tool)/2` so the tool's
///   outer edge meets the wall.
pub fn spiral_hole(
    center: Point,
    z_retract: Length,
    z_bottom: Length,
    hole_diameter: Length,
    tool_diameter: Length,
) -> Vec<RouteMove> {
    let cx = center.x.as_mm();
    let cy = center.y.as_mm();
    let reach = (hole_diameter.as_mm() - tool_diameter.as_mm()) / 2.0; // max tool-centre radius
    let pitch = (STEPOVER_FRACTION * tool_diameter.as_mm()).max(MIN_REACH_MM);

    // Rapid to the centre above the work, then plunge — this removes the centre so no
    // island can survive.
    let mut moves = vec![
        RouteMove::Rapid { x: mm(cx), y: mm(cy), z: z_retract },
        RouteMove::Plunge { x: mm(cx), y: mm(cy), z: z_bottom },
    ];

    // Concentric circles stepped out by the pitch, the last exactly on the wall. When
    // the tool nearly fills the hole (`reach ≈ 0`) the plunge alone finishes it.
    if reach > MIN_REACH_MM {
        let mut radii: Vec<f64> = Vec::new();
        let mut rad = pitch;
        while rad < reach - MIN_REACH_MM {
            radii.push(rad);
            rad += pitch;
        }
        radii.push(reach); // finishing lap on the wall

        for rad in &radii {
            // Step out to (cx+rad, cy) at depth, then a full CCW circle back to it.
            moves.push(RouteMove::Cut {
                x: mm(cx + rad),
                y: mm(cy),
                z: z_bottom,
            });
            moves.push(RouteMove::Arc {
                x: mm(cx + rad),
                y: mm(cy),
                i: mm(-rad),
                j: mm(0.0),
                ccw: true,
            });
        }

        // Retract straight up from the wall.
        moves.push(RouteMove::Rapid {
            x: mm(cx + reach),
            y: mm(cy),
            z: z_retract,
        });
    } else {
        moves.push(RouteMove::Rapid { x: mm(cx), y: mm(cy), z: z_retract });
    }

    moves
}

/// Builds the stadium-pocket toolpath for one oblong slot (single depth pass, as
/// [`spiral_hole`]).
///
/// - `a` / `b` — the slot's **medial-axis end centres** in machine coordinates: the two
///   points a drill making the slot's end holes would sit on. Taking the axis as two
///   placed points rather than a centre-plus-angle means the slot's orientation comes
///   through [`Placement`](super::placement::Placement) exactly once, with no board-space
///   angle to re-derive (and no chance of re-deriving it in the wrong frame).
/// - `slot_width` — the slot across its short axis; the tool must not exceed it.
/// - `from_solid` — `true` when the router meets full material (the `route` and
///   `drill_ends_then_route` strategies): the pocket is cleared from the axis outward.
///   `false` when a drill chain already opened the channel (`drill_chain_then_route`):
///   only the finishing lap on the wall is cut, which is all that is left to remove.
///
/// The passes step out from the axis by [`STEPOVER_FRACTION`] of the tool, the last
/// landing exactly on the wall at `(slot_width − tool)/2`. Each is a CCW stadium loop
/// (two straights + two 180° end arcs); the axis pass itself is a single straight run,
/// and it is what guarantees no island is left behind.
pub fn slot_route(
    a: Point,
    b: Point,
    z_retract: Length,
    z_bottom: Length,
    slot_width: Length,
    tool_diameter: Length,
    from_solid: bool,
) -> Vec<RouteMove> {
    let (ax, ay) = (a.x.as_mm(), a.y.as_mm());
    let (bx, by) = (b.x.as_mm(), b.y.as_mm());
    let travel = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();

    // No travel means the "slot" is a circle — that is precisely the spiral pocket, so
    // reuse it rather than carry a degenerate stadium through the rest of this function.
    if travel <= MIN_REACH_MM {
        return spiral_hole(a, z_retract, z_bottom, slot_width, tool_diameter);
    }

    // Axis unit vector; passes are offset along its normal (taken per loop, below).
    let (ux, uy) = ((bx - ax) / travel, (by - ay) / travel);
    let reach = (slot_width.as_mm() - tool_diameter.as_mm()) / 2.0; // max offset from the axis

    // Which offsets to cut. Clearing from solid runs the axis (r = 0) and then steps out;
    // a chain-opened slot needs only the wall lap — unless the tool fills the slot, when
    // the axis pass *is* the wall lap.
    let mut offsets: Vec<f64> = Vec::new();
    let clears_interior = from_solid || reach <= MIN_REACH_MM;
    if clears_interior {
        offsets.push(0.0);
        let pitch = (STEPOVER_FRACTION * tool_diameter.as_mm()).max(MIN_REACH_MM);
        let mut r = pitch;
        while r < reach - MIN_REACH_MM {
            offsets.push(r);
            r += pitch;
        }
    }
    if reach > MIN_REACH_MM {
        offsets.push(reach); // finishing lap on the wall
    }

    // Enter on the axis at `a` — inside the slot either way, and for the drilled-ends
    // strategies it is an already-open hole.
    let mut moves = vec![
        RouteMove::Rapid { x: a.x, y: a.y, z: z_retract },
        RouteMove::Plunge { x: a.x, y: a.y, z: z_bottom },
    ];

    // The axis pass leaves the tool at `b`, so every loop after it enters and closes at
    // the `b` end; without it the loops stay at the `a` end where the plunge left off.
    // Either way the entry end never changes, so consecutive loops only step out.
    let mut at_b = false;
    for &r in &offsets {
        if r <= MIN_REACH_MM {
            moves.push(RouteMove::Cut { x: b.x, y: b.y, z: z_bottom });
            at_b = true;
            continue;
        }
        // The loop runs `from` → `to`, so its normal is taken from *that* direction —
        // which is what makes the traversal counter-clockwise from either end.
        let (from, to) = if at_b { ((bx, by), (ax, ay)) } else { ((ax, ay), (bx, by)) };
        let n = if at_b { (uy, -ux) } else { (-uy, ux) };
        // Step out perpendicular, onto the -n side of the entry end: the loop's start.
        moves.push(RouteMove::Cut {
            x: mm(from.0 - r * n.0),
            y: mm(from.1 - r * n.1),
            z: z_bottom,
        });
        moves.extend(stadium_loop_ccw(from, to, n, r, z_bottom));
    }

    // Retract from wherever the last pass finished.
    let last = moves
        .iter()
        .rev()
        .find_map(|m| match *m {
            RouteMove::Cut { x, y, .. } | RouteMove::Plunge { x, y, .. } => Some((x, y)),
            RouteMove::Arc { x, y, .. } => Some((x, y)),
            RouteMove::Rapid { .. } => None,
        })
        .unwrap_or((a.x, a.y));
    moves.push(RouteMove::Rapid { x: last.0, y: last.1, z: z_retract });

    moves
}

/// One counter-clockwise stadium loop at offset `r` about the axis `from`–`to`, starting
/// and ending at `from − r·n`.
///
/// `n` **must** be the 90°-CCW normal of the `from`→`to` direction; that is what makes
/// the traversal counter-clockwise in absolute terms whichever end the caller entered
/// from. The order is: the straight down the `−n` side, the 180° cap *outward* around
/// `to`, the straight back up the `+n` side, then the cap outward around `from`. Each cap
/// bulges away from the other end — going the short way round would cut back across the
/// slot's middle. Both caps are half circles, so the incremental centre offset is just
/// `±r·n`. All four moves are at `z_bottom`; the caller has already plunged.
fn stadium_loop_ccw(
    from: (f64, f64),
    to: (f64, f64),
    n: (f64, f64),
    r: f64,
    z_bottom: Length,
) -> Vec<RouteMove> {
    let (nx, ny) = n;
    let offset = |p: (f64, f64), sign: f64| {
        (mm(p.0 + sign * r * nx), mm(p.1 + sign * r * ny))
    };
    let (from_minus, from_plus) = (offset(from, -1.0), offset(from, 1.0));
    let (to_minus, to_plus) = (offset(to, -1.0), offset(to, 1.0));

    vec![
        RouteMove::Cut { x: to_minus.0, y: to_minus.1, z: z_bottom },
        // Cap about `to`: −n side to +n side, the centre one r·n step along +n.
        RouteMove::Arc {
            x: to_plus.0,
            y: to_plus.1,
            i: mm(r * nx),
            j: mm(r * ny),
            ccw: true,
        },
        RouteMove::Cut { x: from_plus.0, y: from_plus.1, z: z_bottom },
        // Cap about `from`: +n side back to −n side.
        RouteMove::Arc {
            x: from_minus.0,
            y: from_minus.1,
            i: mm(-r * nx),
            j: mm(-r * ny),
            ccw: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(center_mm: (f64, f64)) -> Point {
        Point::new(Length::from_mm(center_mm.0), Length::from_mm(center_mm.1))
    }

    /// The signature property: the very first cut is a plunge at the exact centre, so
    /// no island can survive (the flying-debris hazard is designed out).
    #[test]
    fn plunges_the_centre_first_so_no_island_remains() {
        let moves = spiral_hole(at((5.0, 5.0)), Length::from_mm(2.0), Length::from_mm(-2.4),
            Length::from_mm(3.2), Length::from_mm(1.0));
        assert_eq!(moves[0], RouteMove::Rapid { x: Length::from_mm(5.0), y: Length::from_mm(5.0), z: Length::from_mm(2.0) });
        assert_eq!(moves[1], RouteMove::Plunge { x: Length::from_mm(5.0), y: Length::from_mm(5.0), z: Length::from_mm(-2.4) },
            "the centre is plunged before any spiral");
    }

    /// Passes step out by the stepover and the final circle sits exactly on the wall
    /// radius `(hole − tool)/2`, with none exceeding it.
    #[test]
    fn circles_step_out_to_the_wall_without_overshooting() {
        // 3.2 mm hole, 1.0 mm tool → reach 1.1 mm, pitch 0.5 mm → radii 0.5, 1.0, 1.1.
        let moves = spiral_hole(at((0.0, 0.0)), Length::from_mm(2.0), Length::from_mm(-2.0),
            Length::from_mm(3.2), Length::from_mm(1.0));
        let radii: Vec<f64> = moves
            .iter()
            .filter_map(|m| match m {
                RouteMove::Arc { i, .. } => Some(-i.as_mm()),
                _ => None,
            })
            .collect();
        assert_eq!(radii.len(), 3, "two rough passes + a wall lap: {radii:?}");
        assert!((radii[0] - 0.5).abs() < 1e-6 && (radii[1] - 1.0).abs() < 1e-6);
        assert!((radii[2] - 1.1).abs() < 1e-6, "final lap is on the wall");
        // Consecutive passes never gap by more than the pitch → full coverage.
        assert!(radii[0] <= 0.5 + 1e-9);
        assert!(radii[1] - radii[0] <= 0.5 + 1e-9);
        assert!(radii[2] - radii[1] <= 0.5 + 1e-9);
    }

    /// Every arc is a full circle centred on the hole (end point == its own start, and
    /// the centre offset points back to the hole centre).
    #[test]
    fn arcs_are_full_circles_about_the_hole_centre() {
        let moves = spiral_hole(at((10.0, 20.0)), Length::from_mm(2.0), Length::from_mm(-2.0),
            Length::from_mm(3.2), Length::from_mm(1.0));
        for m in &moves {
            if let RouteMove::Arc { x, y, i, j, ccw } = m {
                let start_x = x.as_mm();
                let center_x = start_x + i.as_mm();
                assert!((center_x - 10.0).abs() < 1e-6, "circle centred on the hole X");
                assert!((y.as_mm() - 20.0).abs() < 1e-6 && j.as_mm().abs() < 1e-9, "start/centre on the hole Y line");
                assert!(*ccw);
            }
        }
    }

    /// Degenerate guard: a tool the same size as the hole (no reach) is a plain
    /// plunge, never a zero/negative-radius spiral. The assigner keeps the router
    /// strictly smaller, so this only guards against bad input.
    #[test]
    fn a_tool_with_no_reach_is_a_plain_plunge() {
        let moves = spiral_hole(at((0.0, 0.0)), Length::from_mm(2.0), Length::from_mm(-2.0),
            Length::from_mm(1.0), Length::from_mm(1.0));
        assert!(!moves.iter().any(|m| matches!(m, RouteMove::Arc { .. })), "no spiral without reach");
        assert!(matches!(moves[1], RouteMove::Plunge { .. }), "still plunges the centre");
    }

    // -- slot routing ------------------------------------------------------------

    /// Every point the tool centre visits, in order — the path as geometry rather than
    /// as moves. Arcs contribute their end point; their bulge is checked separately.
    fn path_points(moves: &[RouteMove]) -> Vec<(f64, f64)> {
        moves
            .iter()
            .map(|m| match *m {
                RouteMove::Rapid { x, y, .. }
                | RouteMove::Plunge { x, y, .. }
                | RouteMove::Cut { x, y, .. }
                | RouteMove::Arc { x, y, .. } => (x.as_mm(), y.as_mm()),
            })
            .collect()
    }

    /// A 6 × 2 mm slot lying along +X from (0,0) to (4,0) — travel 4, so `a`/`b` are the
    /// end centres of a slot 6 mm long overall.
    fn slot_along_x(tool_mm: f64, from_solid: bool) -> Vec<RouteMove> {
        slot_route(
            at((0.0, 0.0)),
            at((4.0, 0.0)),
            Length::from_mm(2.0),
            Length::from_mm(-2.0),
            Length::from_mm(2.0),
            Length::from_mm(tool_mm),
            from_solid,
        )
    }

    /// The signature safety property: the tool centre never strays outside the stadium
    /// at the wall radius, so the cutter's edge never passes the finished wall. Checked
    /// against the distance to the axis *segment*, which is what the stadium is.
    #[test]
    fn the_tool_centre_never_leaves_the_slot() {
        for (tool, from_solid) in [(0.5, true), (1.0, true), (2.0, true), (0.8, false)] {
            let reach = (2.0 - tool) / 2.0;
            for (x, y) in path_points(&slot_along_x(tool, from_solid)) {
                // Distance from (x,y) to the segment (0,0)–(4,0).
                let d = if x < 0.0 {
                    (x * x + y * y).sqrt()
                } else if x > 4.0 {
                    ((x - 4.0).powi(2) + y * y).sqrt()
                } else {
                    y.abs()
                };
                assert!(d <= reach + 1e-9, "({x},{y}) is {d} from the axis, past the {reach} wall");
            }
        }
    }

    /// A router exactly the slot width mills it in one pass down the centre line — no
    /// loops, no arcs, just plunge and run.
    #[test]
    fn a_full_width_router_makes_a_single_axis_pass() {
        let moves = slot_along_x(2.0, true);
        assert_eq!(
            moves,
            vec![
                RouteMove::Rapid { x: Length::from_mm(0.0), y: Length::from_mm(0.0), z: Length::from_mm(2.0) },
                RouteMove::Plunge { x: Length::from_mm(0.0), y: Length::from_mm(0.0), z: Length::from_mm(-2.0) },
                RouteMove::Cut { x: Length::from_mm(4.0), y: Length::from_mm(0.0), z: Length::from_mm(-2.0) },
                RouteMove::Rapid { x: Length::from_mm(4.0), y: Length::from_mm(0.0), z: Length::from_mm(2.0) },
            ]
        );
    }

    /// Clearing from solid runs the axis first (so no island survives), then steps out by
    /// the stepover, the last pass landing exactly on the wall.
    #[test]
    fn clearing_from_solid_runs_the_axis_then_steps_out_to_the_wall() {
        // 2.0 slot, 1.0 tool → reach 0.5, pitch 0.5 → offsets 0 (axis) and 0.5 (wall).
        let moves = slot_along_x(1.0, true);
        assert!(matches!(moves[1], RouteMove::Plunge { .. }));
        assert_eq!(
            moves[2],
            RouteMove::Cut { x: Length::from_mm(4.0), y: Length::from_mm(0.0), z: Length::from_mm(-2.0) },
            "the axis pass comes first"
        );
        // Arc radii, from the incremental centre offsets: one pair per loop.
        let radii: Vec<f64> = moves
            .iter()
            .filter_map(|m| match m {
                RouteMove::Arc { i, j, .. } => Some((i.as_mm().powi(2) + j.as_mm().powi(2)).sqrt()),
                _ => None,
            })
            .collect();
        assert_eq!(radii.len(), 2, "one loop = two end caps: {radii:?}");
        for r in radii {
            assert!((r - 0.5).abs() < 1e-9, "the lap sits on the wall at 0.5, got {r}");
        }
    }

    /// A narrower tool needs interior passes as well, none gapping by more than the
    /// stepover — that is what makes the pocket fully cleared.
    #[test]
    fn interior_passes_never_gap_by_more_than_the_stepover() {
        // 2.0 slot, 0.4 tool → reach 0.8, pitch 0.2 → offsets 0, .2, .4, .6, .8.
        let moves = slot_along_x(0.4, true);
        let mut radii: Vec<f64> = vec![0.0];
        radii.extend(moves.iter().filter_map(|m| match m {
            RouteMove::Arc { i, j, .. } => Some((i.as_mm().powi(2) + j.as_mm().powi(2)).sqrt()),
            _ => None,
        }));
        radii.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        assert_eq!(radii.len(), 5, "axis + four offsets: {radii:?}");
        for pair in radii.windows(2) {
            assert!(pair[1] - pair[0] <= 0.2 + 1e-9, "gap {:?} exceeds the stepover", pair);
        }
        assert!((radii[radii.len() - 1] - 0.8).abs() < 1e-9, "the last pass is on the wall");
    }

    /// After a drill chain there is no interior left, so the router cuts the wall lap and
    /// nothing else — the whole point of `drill_chain_then_route`.
    #[test]
    fn a_chain_opened_slot_gets_only_the_wall_lap() {
        let solid = slot_along_x(0.4, true);
        let cleaned = slot_along_x(0.4, false);
        let arcs = |ms: &[RouteMove]| ms.iter().filter(|m| matches!(m, RouteMove::Arc { .. })).count();
        assert_eq!(arcs(&cleaned), 2, "exactly one loop");
        assert!(arcs(&solid) > arcs(&cleaned), "clearing from solid takes more passes");
        assert!(
            !cleaned.iter().any(|m| matches!(m, RouteMove::Cut { x, y, .. }
                if (x.as_mm() - 4.0).abs() < 1e-9 && y.as_mm().abs() < 1e-9)),
            "no axis pass — the chain already removed that material"
        );
    }

    /// Every cap is a true half circle centred on an axis end — so it bulges *away* from
    /// the far end. Cutting the short way round would slice back across the slot's middle
    /// and leave the ends unfinished.
    ///
    /// `I`/`J` are the centre offset from the arc's **start**, as GCode defines them, so
    /// this walks the path to know where each arc begins.
    #[test]
    fn every_cap_is_a_half_circle_centred_on_an_axis_end() {
        for tool in [1.0, 0.4] {
            let moves = slot_along_x(tool, true);
            let reach = (2.0 - tool) / 2.0;
            let mut here = (0.0, 0.0);
            let mut caps = 0;
            for m in &moves {
                let (end, arc) = match *m {
                    RouteMove::Arc { x, y, i, j, .. } => ((x.as_mm(), y.as_mm()), Some((i.as_mm(), j.as_mm()))),
                    RouteMove::Rapid { x, y, .. }
                    | RouteMove::Plunge { x, y, .. }
                    | RouteMove::Cut { x, y, .. } => ((x.as_mm(), y.as_mm()), None),
                };
                if let Some((i, j)) = arc {
                    let centre = (here.0 + i, here.1 + j);
                    assert!(
                        centre.1.abs() < 1e-9 && (centre.0.abs() < 1e-9 || (centre.0 - 4.0).abs() < 1e-9),
                        "a cap must be centred on an axis end, got {centre:?}"
                    );
                    let r_start = ((here.0 - centre.0).powi(2) + (here.1 - centre.1).powi(2)).sqrt();
                    let r_end = ((end.0 - centre.0).powi(2) + (end.1 - centre.1).powi(2)).sqrt();
                    assert!((r_start - r_end).abs() < 1e-9, "start and end share the radius");
                    assert!(r_start <= reach + 1e-9, "cap radius {r_start} is past the {reach} wall");
                    // A half circle: the end is the start reflected through the centre.
                    assert!(
                        (end.0 - (2.0 * centre.0 - here.0)).abs() < 1e-9
                            && (end.1 - (2.0 * centre.1 - here.1)).abs() < 1e-9,
                        "a cap spans 180°, {here:?} → {end:?} about {centre:?}"
                    );
                    caps += 1;
                }
                here = end;
            }
            assert!(caps >= 2 && caps % 2 == 0, "caps come in pairs, got {caps}");
        }
    }

    /// Wall passes run counter-clockwise, which with an M03 spindle is climb milling
    /// inside a pocket. Measured as the signed area swept by the loop's corner points.
    #[test]
    fn wall_passes_run_counter_clockwise() {
        for from_solid in [true, false] {
            let moves = slot_along_x(1.0, from_solid);
            // The loop's four corners, in traversal order, are the points between the
            // step-out and the retract.
            let pts = path_points(&moves);
            let corners = &pts[pts.len() - 5..pts.len() - 1];
            let mut area = 0.0;
            for k in 0..corners.len() {
                let (x0, y0) = corners[k];
                let (x1, y1) = corners[(k + 1) % corners.len()];
                area += x0 * y1 - x1 * y0;
            }
            assert!(area > 0.0, "signed area {area} should be positive (CCW), {corners:?}");
        }
    }

    /// A slot whose ends coincide is a round hole; it must not degenerate into a
    /// zero-length stadium but reuse the spiral pocket.
    #[test]
    fn a_slot_with_no_travel_is_routed_as_a_round_hole() {
        let slot = slot_route(
            at((3.0, 3.0)), at((3.0, 3.0)),
            Length::from_mm(2.0), Length::from_mm(-2.0),
            Length::from_mm(3.2), Length::from_mm(1.0), true,
        );
        let hole = spiral_hole(at((3.0, 3.0)), Length::from_mm(2.0), Length::from_mm(-2.0),
            Length::from_mm(3.2), Length::from_mm(1.0));
        assert_eq!(slot, hole);
    }

    /// Orientation comes from the two placed end points alone, so a slot at any angle is
    /// the same path rotated — no board-space angle is re-derived.
    #[test]
    fn a_rotated_slot_is_the_same_path_rotated() {
        let along_x = slot_along_x(1.0, true);
        // The same slot turned 90°: (0,0)→(0,4).
        let along_y = slot_route(
            at((0.0, 0.0)), at((0.0, 4.0)),
            Length::from_mm(2.0), Length::from_mm(-2.0),
            Length::from_mm(2.0), Length::from_mm(1.0), true,
        );
        assert_eq!(along_x.len(), along_y.len());
        for (a, b) in path_points(&along_x).iter().zip(path_points(&along_y).iter()) {
            // Rotating (x,y) by +90° gives (−y, x).
            assert!((b.0 - -a.1).abs() < 1e-9 && (b.1 - a.0).abs() < 1e-9, "{a:?} → {b:?}");
        }
    }
}
