//! [`Placement`] — the single **board → machine** coordinate mapping
//! (operation-planner.md §6). Built once per step and threaded through the planner
//! so every op is emitted in machine space; the offset / rotation / scaling math
//! lives here instead of scattering through the planner and the Coder.
//!
//! **XY** is a composed affine: flip the board's Y-down frame into the machine's Y-up
//! one, rotate by the job's board orientation, translate the rotated board's min corner
//! to the work origin, then apply the CNC's per-axis scaling calibration. Because ops
//! are placed here, the ordering TSP (op-planner §4) minimises *physical* travel.
//!
//! **The Y flip is not optional.** KiCad's board frame is Y-**down** (it is a screen
//! frame — [`pcb::Slot`] documents the same convention, and the board preview renders
//! board coordinates straight into SVG for exactly that reason). A CNC's work frame is
//! Y-**up**. Carrying board Y through unchanged therefore mirrors the whole board about
//! the X axis: every hole lands on the wrong side, and every arc runs the wrong way
//! round. The flip happens *first*, before the rotation, so the job's orientation angle
//! means a counter-clockwise turn **as seen on the machine** — which is what an operator
//! setting it means by it.
//!
//! **Z** context (retract / safe heights) is carried for op building; the full Z
//! stack-up (fixture backboard + board thickness, bed-relative Z0) firms up when the
//! fixture model gains that geometry (op-planner §6, and the plan's Phase-3 gaps).
//! Until then the origin corner is the board's own min corner (a sane default) — the
//! fixture-selectable corner (the fixture `origin`) is not yet in the runtime
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
    /// The pivot is the same point before and after the Y flip, because the flip is
    /// about the bounds' own centre line.
    center_x_mm: f64,
    center_y_mm: f64,
    /// The rotated board's min corner, in board mm — subtracted so it lands on the
    /// work origin.
    min_x_mm: f64,
    min_y_mm: f64,
    /// Whether to flip the board's Y-down frame into the machine's Y-up one.
    ///
    /// True whenever there are bounds — that is, whenever there is a real board. With
    /// no bounds there is no board frame to flip and no origin to flip about, so the
    /// transform stays an identity (save for scaling) and the pure planner tests can
    /// state their points in machine space directly.
    flip_y: bool,
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
            flip_y: bounds.is_some(),
            scale_x,
            scale_y,
            z_retract,
            z_safe,
        }
    }

    /// Maps a board point into machine coordinates: flip the Y-down board frame into
    /// the machine's Y-up one, rotate about the board centre, shift the rotated min
    /// corner to the origin, then apply per-axis scaling.
    pub fn xy(&self, p: &BoardPoint) -> Point {
        // Mirror about the bounds' horizontal centre line. Doing it about the centre
        // rather than about y=0 is what keeps the rotation pivot — and hence the
        // corner extents computed in `new` — unchanged by the flip.
        let flipped_y = if self.flip_y {
            2.0 * self.center_y_mm - p.y.as_mm()
        } else {
            p.y.as_mm()
        };
        let (rx, ry) = rotate_about(
            p.x.as_mm(),
            flipped_y,
            self.center_x_mm,
            self.center_y_mm,
            self.cos,
            self.sin,
        );
        let mx = (rx - self.min_x_mm) * self.scale_x;
        let my = (ry - self.min_y_mm) * self.scale_y;
        Point::new(Length::from_mm(mx), Length::from_mm(my))
    }

    /// Maps a machine point back into board coordinates — the exact inverse of
    /// [`Self::xy`], undoing scaling, translation, rotation and the Y flip in turn.
    ///
    /// Needed wherever geometry is derived in machine space but consumed by something
    /// that takes board coordinates: the outline's mouse-bite centres are found on the
    /// placed toolpath, then handed to the drill planner, which places its own targets.
    /// A round trip through both is the identity, which is what the test asserts.
    pub fn unplace(&self, p: &Point) -> BoardPoint {
        let rx = p.x.as_mm() / self.scale_x + self.min_x_mm;
        let ry = p.y.as_mm() / self.scale_y + self.min_y_mm;
        // Rotate back: the inverse rotation is the same matrix with sin negated.
        let (bx, by) = rotate_about(rx, ry, self.center_x_mm, self.center_y_mm, self.cos, -self.sin);
        let unflipped = if self.flip_y { 2.0 * self.center_y_mm - by } else { by };
        BoardPoint { x: Length::from_mm(bx), y: Length::from_mm(unflipped) }
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
        // Board bounds start at (10,20), 30 × 40. With no rotation/scaling, X is simply
        // offset; Y is measured from the board's *bottom* edge, because board Y counts
        // downward and machine Y counts up. The point sits 4 mm below the top edge, so
        // 40 − 4 = 36 mm above the bottom.
        let p = Placement::new(Some(&bounds(10.0, 20.0, 30.0, 40.0)), 0.0, 1.0, 1.0, Length::from_mm(2.0), Length::from_mm(5.0));
        let out = p.xy(&pt(13.0, 24.0));
        assert!((out.x.as_mm() - 3.0).abs() < 1e-6, "x: {}", out.x.as_mm());
        assert!((out.y.as_mm() - 36.0).abs() < 1e-6, "y: {}", out.y.as_mm());
    }

    /// The whole board lands in the positive quadrant with its corners on the bounds,
    /// and the *order* of Y is reversed — the board's top edge is the machine's far
    /// side. Getting this backwards mirrors every hole.
    #[test]
    fn the_y_down_board_frame_is_flipped_into_the_machines_y_up_one() {
        let p = Placement::new(Some(&bounds(0.0, 0.0, 20.0, 10.0)), 0.0, 1.0, 1.0, Length::from_mm(2.0), Length::from_mm(5.0));
        // Board (0,0) is the top-left corner; on the machine that is the far-left one.
        let top_left = p.xy(&pt(0.0, 0.0));
        assert!((top_left.y.as_mm() - 10.0).abs() < 1e-6, "the board's top edge is machine Y max");
        let bottom_left = p.xy(&pt(0.0, 10.0));
        assert!(bottom_left.y.as_mm().abs() < 1e-6, "the board's bottom edge is machine Y 0");
        // And the mapping is an isometry: a shape's handedness survives, so an arc
        // that is counter-clockwise on the machine really is climb milling.
        let a = p.xy(&pt(0.0, 0.0));
        let b = p.xy(&pt(4.0, 0.0));
        let c = p.xy(&pt(0.0, 3.0));
        let cross = (b.x.as_mm() - a.x.as_mm()) * (c.y.as_mm() - a.y.as_mm())
            - (b.y.as_mm() - a.y.as_mm()) * (c.x.as_mm() - a.x.as_mm());
        assert!(cross < 0.0, "board-frame CCW becomes machine-frame CW, as a mirror must");
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

    /// `unplace` is the exact inverse of `xy`, including the flip and an odd rotation —
    /// geometry found on the placed toolpath has to come back to board space unchanged.
    #[test]
    fn placing_and_unplacing_is_the_identity() {
        for (angle, sx, sy) in [(0.0, 1.0, 1.0), (90.0, 1.0, 1.0), (37.0, 1.01, 0.99)] {
            let p = Placement::new(Some(&bounds(3.0, 7.0, 12.0, 9.0)), angle, sx, sy,
                Length::from_mm(2.0), Length::from_mm(5.0));
            for board in [pt(3.0, 7.0), pt(9.0, 11.5), pt(15.0, 16.0)] {
                let back = p.unplace(&p.xy(&board));
                assert!(
                    (back.x.as_mm() - board.x.as_mm()).abs() < 1e-9
                        && (back.y.as_mm() - board.y.as_mm()).abs() < 1e-9,
                    "at {angle}°: {:?} → {:?}",
                    (board.x.as_mm(), board.y.as_mm()),
                    (back.x.as_mm(), back.y.as_mm())
                );
            }
        }
    }

    #[test]
    fn is_deterministic() {
        let p = Placement::new(Some(&bounds(3.0, 7.0, 12.0, 9.0)), 37.0, 1.0, 1.0, Length::from_mm(2.0), Length::from_mm(5.0));
        assert_eq!(p.xy(&pt(6.0, 9.0)), p.xy(&pt(6.0, 9.0)));
    }
}
