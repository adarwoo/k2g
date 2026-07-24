//! [`Placement`] — the single **board → machine** coordinate mapping
//! (operation-planner.md §6). Built once per step and threaded through the planner
//! so every op is emitted in machine space; the offset / rotation / scaling math
//! lives here instead of scattering through the planner and the Coder.
//!
//! **XY** is a composed affine: rotate by the job's board orientation, translate the
//! rotated board's min corner to the work origin, then apply the CNC's per-axis
//! scaling calibration. Because ops are placed here, the ordering TSP (op-planner §4)
//! minimises *physical* travel.
//!
//! **Z** context (retract / safe heights) is carried for op building; the full Z
//! stack-up (fixture backboard + board thickness, bed-relative Z0) firms up when the
//! fixture model gains that geometry (op-planner §6, and the plan's Phase-3 gaps).
//! Until then the origin corner is the board's own min corner (a sane default) — the
//! fixture-selectable corner (`work_origin_reference`) is not yet in the runtime
//! fixture model.

use pcb::{BoardBoundingBox, BoardPoint};
use units::Length;

use super::plan::Point;

/// The board→machine affine + the step's Z reference heights. A pure value: same
/// inputs → same transform (op-planner §8 determinism).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Rotation about the board-bounds centre (radians, as cos/sin).
    cos: f64,
    sin: f64,
    /// Board-bounds centre, in board mm — the pivot the orientation rotates about.
    center_x_mm: f64,
    center_y_mm: f64,
    /// The rotated board's min corner, in board mm — subtracted so it lands on the
    /// work origin.
    min_x_mm: f64,
    min_y_mm: f64,
    /// Per-axis CNC scaling calibration (`machine.scaling.x/y`).
    scale_x: f64,
    scale_y: f64,
    /// R-plane the tool retracts to between features.
    z_retract: Length,
    /// Safe height clear of the work and fixtures.
    z_safe: Length,
}

impl Placement {
    /// Builds the placement from the board bounds, the job's board orientation
    /// (degrees), the CNC's per-axis scaling, and the step's retract/safe heights.
    ///
    /// With no bounds (no board), XY is identity save for scaling — enough for the
    /// pure planner tests and a graceful no-board path.
    pub fn new(
        bounds: Option<&BoardBoundingBox>,
        orientation_deg: f64,
        scale_x: f64,
        scale_y: f64,
        z_retract: Length,
        z_safe: Length,
    ) -> Self {
        let theta = orientation_deg.to_radians();
        let cos = theta.cos();
        let sin = theta.sin();

        let (center_x_mm, center_y_mm, min_x_mm, min_y_mm) = match bounds {
            Some(b) => {
                let x0 = b.x.as_mm();
                let y0 = b.y.as_mm();
                let x1 = x0 + b.width.as_mm();
                let y1 = y0 + b.height.as_mm();
                let cx = (x0 + x1) / 2.0;
                let cy = (y0 + y1) / 2.0;
                // Rotate the four corners about the centre and take the min per axis,
                // so the rotated bounding box hugs the work origin after translation.
                let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
                let mut min_x = f64::INFINITY;
                let mut min_y = f64::INFINITY;
                for (px, py) in corners {
                    let (rx, ry) = rotate_about(px, py, cx, cy, cos, sin);
                    min_x = min_x.min(rx);
                    min_y = min_y.min(ry);
                }
                (cx, cy, min_x, min_y)
            }
            None => (0.0, 0.0, 0.0, 0.0),
        };

        Self {
            cos,
            sin,
            center_x_mm,
            center_y_mm,
            min_x_mm,
            min_y_mm,
            scale_x,
            scale_y,
            z_retract,
            z_safe,
        }
    }

    /// Maps a board point into machine coordinates: rotate about the board centre,
    /// shift the rotated min corner to the origin, then apply per-axis scaling.
    pub fn xy(&self, p: &BoardPoint) -> Point {
        let (rx, ry) = rotate_about(
            p.x.as_mm(),
            p.y.as_mm(),
            self.center_x_mm,
            self.center_y_mm,
            self.cos,
            self.sin,
        );
        let mx = (rx - self.min_x_mm) * self.scale_x;
        let my = (ry - self.min_y_mm) * self.scale_y;
        Point::new(Length::from_mm(mx), Length::from_mm(my))
    }

    pub fn z_retract(&self) -> Length {
        self.z_retract
    }

    /// Safe height clear of the work and fixtures. Part of the Placement's Z contract
    /// (op-planner §6); consumed by the Coder handoff (§7) for rapid moves, which is
    /// not wired yet — hence unused today.
    #[allow(dead_code)]
    pub fn z_safe(&self) -> Length {
        self.z_safe
    }
}

/// Rotates `(px, py)` about the pivot `(cx, cy)` by the angle given as `(cos, sin)`.
fn rotate_about(px: f64, py: f64, cx: f64, cy: f64, cos: f64, sin: f64) -> (f64, f64) {
    let dx = px - cx;
    let dy = py - cy;
    (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f64, y: f64, w: f64, h: f64) -> BoardBoundingBox {
        BoardBoundingBox {
            x: Length::from_mm(x),
            y: Length::from_mm(y),
            width: Length::from_mm(w),
            height: Length::from_mm(h),
        }
    }

    fn pt(x: f64, y: f64) -> BoardPoint {
        BoardPoint { x: Length::from_mm(x), y: Length::from_mm(y) }
    }

    #[test]
    fn identity_shifts_the_min_corner_to_the_origin() {
        // Board bounds start at (10,20); with no rotation/scaling a point is offset
        // so the board's min corner lands on (0,0).
        let p = Placement::new(Some(&bounds(10.0, 20.0, 30.0, 40.0)), 0.0, 1.0, 1.0, Length::from_mm(2.0), Length::from_mm(5.0));
        let out = p.xy(&pt(13.0, 24.0));
        assert!((out.x.as_mm() - 3.0).abs() < 1e-6);
        assert!((out.y.as_mm() - 4.0).abs() < 1e-6);
    }

    #[test]
    fn scaling_multiplies_each_axis() {
        let p = Placement::new(Some(&bounds(0.0, 0.0, 10.0, 10.0)), 0.0, 1.01, 0.99, Length::from_mm(2.0), Length::from_mm(5.0));
        let out = p.xy(&pt(5.0, 5.0));
        assert!((out.x.as_mm() - 5.05).abs() < 1e-6);
        assert!((out.y.as_mm() - 4.95).abs() < 1e-6);
    }

    #[test]
    fn quarter_turn_keeps_the_board_in_the_positive_quadrant() {
        // A 20×10 board rotated 90° becomes 10×20; every mapped point stays within
        // [0,10]×[0,20] and the rotated min corner sits at the origin.
        let p = Placement::new(Some(&bounds(0.0, 0.0, 20.0, 10.0)), 90.0, 1.0, 1.0, Length::from_mm(2.0), Length::from_mm(5.0));
        for (bx, by) in [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)] {
            let out = p.xy(&pt(bx, by));
            assert!(out.x.as_mm() >= -1e-6 && out.x.as_mm() <= 10.0 + 1e-6, "x in [0,10]: {}", out.x.as_mm());
            assert!(out.y.as_mm() >= -1e-6 && out.y.as_mm() <= 20.0 + 1e-6, "y in [0,20]: {}", out.y.as_mm());
        }
    }

    #[test]
    fn is_deterministic() {
        let p = Placement::new(Some(&bounds(3.0, 7.0, 12.0, 9.0)), 37.0, 1.0, 1.0, Length::from_mm(2.0), Length::from_mm(5.0));
        assert_eq!(p.xy(&pt(6.0, 9.0)), p.xy(&pt(6.0, 9.0)));
    }
}
