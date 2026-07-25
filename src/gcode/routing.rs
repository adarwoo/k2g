//! Hole-routing toolpath geometry — the spiral-from-centre pocket that mills a hole no
//! drill can make (the assigner's route-fallback: too big to drill, or its point would
//! reach the bed).
//!
//! **One strategy, by design** (settled 2026-07-25): plunge at the hole centre, then
//! spiral outward removing *all* the material, finishing with a lap on the wall. This
//! is the only approach that never leaves a centre island (the flying-debris hazard of
//! a tangential contour) and leaves no entry witness mark on the finished wall — for a
//! through hole the centre is removed material. There is deliberately **no** user
//! option here: no strategy choice, no island toggle, and the radial stepover is fixed
//! at [`STEPOVER_FRACTION`] of the tool diameter (a standard roughing pitch).
//!
//! The pocket is approximated with concentric full circles stepped out by the pitch —
//! ample for PCB-scale holes (a handful of turns) and trivially correct. The scallops
//! between passes are interior material; the final circle sits exactly on the wall.

use units::Length;

use crate::gcode::plan::Point;

/// Radial stepover as a fraction of the router diameter. Not user-exposed (KISS).
pub const STEPOVER_FRACTION: f64 = 0.5;

/// Below this radial reach (mm) the tool nearly fills the hole, so a single centre
/// plunge finishes it — no spiral is generated.
const MIN_REACH_MM: f64 = 1e-3;

/// One move of a routed-hole toolpath, in **machine coordinates** (mm). The body
/// renderer maps each to a CNC primitive: `Rapid`→`rapid_move`, `Cut`→`linear_cut`,
/// `Arc`→`cut_arc`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RouteMove {
    /// Rapid (G0) to a point — positioning above the work, and the final retract.
    Rapid { x: Length, y: Length, z: Length },
    /// Feed (G1) to a point — the centre plunge and each radial step-out, at depth.
    Cut { x: Length, y: Length, z: Length },
    /// A full circle (G2/G3) ending where it began; centre is at the start plus
    /// `(i, j)`. Always the incremental-centre (I/J) arc form.
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
        RouteMove::Rapid { x: Length::from_mm(cx), y: Length::from_mm(cy), z: z_retract },
        RouteMove::Cut { x: Length::from_mm(cx), y: Length::from_mm(cy), z: z_bottom },
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
                x: Length::from_mm(cx + rad),
                y: Length::from_mm(cy),
                z: z_bottom,
            });
            moves.push(RouteMove::Arc {
                x: Length::from_mm(cx + rad),
                y: Length::from_mm(cy),
                i: Length::from_mm(-rad),
                j: Length::from_mm(0.0),
                ccw: true,
            });
        }

        // Retract straight up from the wall.
        moves.push(RouteMove::Rapid {
            x: Length::from_mm(cx + reach),
            y: Length::from_mm(cy),
            z: z_retract,
        });
    } else {
        moves.push(RouteMove::Rapid { x: Length::from_mm(cx), y: Length::from_mm(cy), z: z_retract });
    }

    moves
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
        assert_eq!(moves[1], RouteMove::Cut { x: Length::from_mm(5.0), y: Length::from_mm(5.0), z: Length::from_mm(-2.4) },
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
        assert!(matches!(moves[1], RouteMove::Cut { .. }), "still plunges the centre");
    }
}
