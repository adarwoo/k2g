//! [`Placement`] — the single **board → machine** coordinate mapping
//! (operation-planner.md §6). Built once per step and threaded through the planner
//! so every op is emitted in machine space; the offset / rotation / scaling math
//! lives here instead of scattering through the planner and the Coder.
//!
//! **XY** is a composed affine: flip the board's Y-down frame into the machine's Y-up
//! one, rotate by the job's board orientation, translate the corner the fixture
//! registers against onto the work zero, then apply the CNC's per-axis scaling
//! calibration. Because ops are placed here, the ordering TSP (op-planner §4) minimises
//! *physical* travel.
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
//! The **origin corner** now comes from the fixture (`origin.x0`/`origin.y0`) — see
//! [`BoardOrigin`]. It moves the zero only; the axes keep the machine's directions, so a
//! board registered against a right-hand stop legitimately runs into negative X.
//!
//! ## Two additions for double-sided work
//!
//! [`Margin`] is work the origin has to clear that is **not** the board: today only the
//! locating pins, which sit outside the board's own bounds. It moves the zero outward and
//! nothing else — the board does not move relative to the pins, the *zero* moves relative
//! to both. A zero margin therefore reproduces the previous transform exactly, which is
//! what makes a job without pins byte-identical to one generated before this existed.
//!
//! [`BoardFlip`] is the board being physically turned over for a back-face step. It is
//! applied **last**, in the finished machine frame, as a mirror about the board's own
//! centre line — the line the registration pins sit on, which is why the pins map to
//! themselves and the flipped work occupies exactly the frame the front side did.
//!
//! Note that this makes **two** mirrors in the composition for a back-face step (the
//! Y-down→Y-up flip, then this), so such a step's machine frame has the same handedness as
//! the board frame, where a front-face step's is reversed. That does not disturb climb
//! milling: pockets and slots are generated counter-clockwise *in machine space* by
//! construction (`super::routing`), not derived from this transform's handedness.

use pcb::{BoardBoundingBox, BoardPoint};
use units::Length;

use super::plan::Point;

/// Which corner of the **bed** the work origin sits on (the fixture's `origin`).
///
/// Named in the bed's own directions — `left`/`right` as the operator sees them, `near`
/// for the operator's side and `far` for across the bed. Deliberately not the board's
/// `front`/`back`, which name the PCB's two faces: sharing a word between "the operator's
/// end of the table" and "the component side of the board" is how a job comes out mirrored.
///
/// This moves the **zero**, it does not mirror anything: the axes keep the machine's own
/// directions, X growing right and Y away from the operator. So a board zeroed on its
/// right edge occupies negative X — which is exactly right when the fixture registers
/// against a right-hand stop, and would be wrong if the axis flipped with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardOrigin {
    /// `false` = the left edge is X0, `true` = the right edge.
    pub x_at_right: bool,
    /// `false` = the near edge (the operator's side) is Y0, `true` = the far edge.
    pub y_at_far: bool,
}

impl Default for BoardOrigin {
    /// Near-left, which is what every program emitted before the fixture could say
    /// otherwise — so a fixture that does not care keeps its existing output.
    fn default() -> Self {
        Self { x_at_right: false, y_at_far: false }
    }
}

impl BoardOrigin {
    /// Reads the fixture's two edge names, defaulting anything unrecognised to near-left.
    pub fn from_edges(x0: &str, y0: &str) -> Self {
        Self {
            x_at_right: x0.eq_ignore_ascii_case("right"),
            y_at_far: y0.eq_ignore_ascii_case("far"),
        }
    }
}

/// Work outside the board's own bounds that the origin has to clear, per side, in
/// machine millimetres before scaling.
///
/// Today this is the locating pins and only the locating pins. It is **not** the routed
/// channel: the cutter centre runs one radius outside the edge, so a routed job has always
/// cut into negative coordinates, and folding that in here would move every existing
/// routed program. That is a separate decision from this one.
///
/// [`Default`] is all zeros, which reproduces the transform exactly as it was before
/// margins existed — the property every "a job without pins is unchanged" test rests on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Margin {
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
}

/// Which axis the board is turned about when a step machines the bottom side.
///
/// Named for the axis the board *rotates about*, matching the fixture schema's
/// `board_flip_axis`, so the two cannot be read as saying different things. The
/// coordinate that ends up mirrored is the other one: turning about Y — a page turn —
/// mirrors X.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BoardFlip {
    /// Tumbled near-to-far about the machine's X axis: **Y** is mirrored, the left edge
    /// stays on the left.
    AboutX,
    /// Turned left-to-right about the machine's Y axis, like a page: **X** is mirrored,
    /// the near edge stays near.
    ///
    /// The default, matching the schema: it is the common case, and it is what a profile
    /// written before `board_flip_axis` existed is taken to mean.
    #[default]
    AboutY,
}

impl BoardFlip {
    /// Reads the fixture's `board_flip_axis`, defaulting anything unrecognised to the page
    /// turn — which is what the schema says a profile predating the field assumes.
    pub fn from_axis(axis: &str) -> Self {
        if axis.eq_ignore_ascii_case("x") {
            Self::AboutX
        } else {
            Self::AboutY
        }
    }

    /// Whether this flip mirrors the X coordinate (as against Y).
    pub fn mirrors_x(self) -> bool {
        matches!(self, Self::AboutY)
    }
}

/// An axis-aligned rectangle in machine millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Rect {
    pub fn centre_x(&self) -> f64 {
        (self.min_x + self.max_x) / 2.0
    }

    pub fn centre_y(&self) -> f64 {
        (self.min_y + self.max_y) / 2.0
    }
}

/// Everything [`Placement::new`] needs.
///
/// A parameters struct rather than nine positional arguments — the shape `AssignConfig`,
/// `Setup` and `ProgramPrimitives` already use here. Two of these (`margin`, `flip`) are
/// zero/absent for every top-side job, and a caller that passes them by position would be
/// counting `1.0, 1.0` scaling factors to find out which is which.
///
/// `Copy` so a caller can build one base spec and derive the front- and back-face
/// placements from it with `..base` — which is exactly how the two are meant to differ.
#[derive(Clone, Copy)]
pub struct PlacementSpec<'a> {
    pub bounds: Option<&'a BoardBoundingBox>,
    /// The job's board placement angle, degrees counter-clockwise as seen on the machine.
    pub orientation_deg: f64,
    pub origin: BoardOrigin,
    /// Work outside the board the origin must clear. [`Margin::default`] for a job with no
    /// locating pins.
    pub margin: Margin,
    /// Set when this step machines the board's back face, i.e. it is turned over.
    pub flip: Option<BoardFlip>,
    pub scale_x: f64,
    pub scale_y: f64,
    pub z_retract: Length,
    pub z_safe: Length,
}

/// The mirror a [`BoardFlip`] becomes once the line it acts about is known.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Mirror {
    /// Mirror X (as against Y).
    on_x: bool,
    /// The mirror line, in the **finished** machine frame (post-origin, post-scaling), so
    /// applying it is one subtraction with nothing left to compose.
    line_mm: f64,
}

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
    /// The rotated board's **origin corner**, in board mm — subtracted so it lands on
    /// the work zero. Which corner that is comes from the fixture; for the front-left
    /// default it is the rotated bounds' minimum, as it always used to be, pushed outward
    /// by whatever [`Margin`] the job's pins need.
    origin_x_mm: f64,
    origin_y_mm: f64,
    /// The board's own rotated extents in the **finished** frame — what
    /// [`Self::board_rect_mm`] hands out, so the pin geometry does not have to re-derive
    /// the transform to find the board's centre line.
    rect: Rect,
    /// The turn-over mirror, when this step machines the board's back face.
    mirror: Option<Mirror>,
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
    /// Builds the placement from the board bounds, the job's board orientation, the
    /// fixture's work-origin corner and flip axis, the pin margin, the CNC's per-axis
    /// scaling, and the step's retract/safe heights.
    ///
    /// With no bounds (no board), XY is identity save for scaling — enough for the
    /// pure planner tests and a graceful no-board path. A flip is ignored in that case:
    /// there is no board centre to mirror about, and mirroring about zero would move the
    /// work rather than turn it over.
    pub fn new(spec: &PlacementSpec) -> Self {
        let theta = spec.orientation_deg.to_radians();
        let cos = theta.cos();
        let sin = theta.sin();

        let (center_x_mm, center_y_mm, extents) = match spec.bounds {
            Some(b) => {
                let x0 = b.x.as_mm();
                let y0 = b.y.as_mm();
                let x1 = x0 + b.width.as_mm();
                let y1 = y0 + b.height.as_mm();
                let cx = (x0 + x1) / 2.0;
                let cy = (y0 + y1) / 2.0;
                // Rotate the four corners about the centre, then take the extent the
                // fixture registers against. Corners are taken *after* rotation because
                // a turned board's "left edge" is whichever edge ends up leftmost.
                let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
                let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
                let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
                for (px, py) in corners {
                    let (rx, ry) = rotate_about(px, py, cx, cy, cos, sin);
                    min_x = min_x.min(rx);
                    min_y = min_y.min(ry);
                    max_x = max_x.max(rx);
                    max_y = max_y.max(ry);
                }
                // These extents are of the *transformed* board even though the flip is
                // not applied here: mirroring about the bounds' own centre line maps the
                // four corners onto each other, so the set — and hence its min and max —
                // is unchanged. Which is the whole reason the flip is about the centre.
                //
                // So this is simply the machine frame: X grows right, Y away from the
                // operator. Left/near are the minima, right/far the maxima. Zeroing on a
                // maximum puts the board in negative coordinates, which is correct — the
                // axis directions belong to the machine, not to the fixture.
                (cx, cy, Some(Rect { min_x, min_y, max_x, max_y }))
            }
            None => (0.0, 0.0, None),
        };

        // The zero lands on the registered corner of the *envelope*, not of the board:
        // pushed outward by the margin so the pins, which sit beyond the board, are
        // inside the frame too. With no margin this is the corner it always was.
        let (origin_x_mm, origin_y_mm) = match extents {
            Some(rect) => (
                if spec.origin.x_at_right {
                    rect.max_x + spec.margin.x_max
                } else {
                    rect.min_x - spec.margin.x_min
                },
                if spec.origin.y_at_far {
                    rect.max_y + spec.margin.y_max
                } else {
                    rect.min_y - spec.margin.y_min
                },
            ),
            None => (0.0, 0.0),
        };

        // The board's own extents, expressed in the finished frame. Not the envelope's:
        // the pins are placed *from* the board, so this is the rectangle they measure out
        // from, and it is also the rectangle whose centre line the board turns about.
        let rect = extents
            .map(|r| Rect {
                min_x: (r.min_x - origin_x_mm) * spec.scale_x,
                min_y: (r.min_y - origin_y_mm) * spec.scale_y,
                max_x: (r.max_x - origin_x_mm) * spec.scale_x,
                max_y: (r.max_y - origin_y_mm) * spec.scale_y,
            })
            // With `x0: right` the origin is the board's *maximum*, so the placed board
            // runs negative and what was the max is now the min. Normalise, so a Rect is
            // always a rectangle rather than sometimes an inside-out one.
            .map(|r| Rect {
                min_x: r.min_x.min(r.max_x),
                min_y: r.min_y.min(r.max_y),
                max_x: r.min_x.max(r.max_x),
                max_y: r.min_y.max(r.max_y),
            })
            .unwrap_or(Rect { min_x: 0.0, min_y: 0.0, max_x: 0.0, max_y: 0.0 });

        // The board turns about its own centre line — which is where the registration
        // pins sit, and is what makes them map to themselves.
        let mirror = spec.flip.filter(|_| spec.bounds.is_some()).map(|flip| Mirror {
            on_x: flip.mirrors_x(),
            line_mm: if flip.mirrors_x() { rect.centre_x() } else { rect.centre_y() },
        });

        Self {
            cos,
            sin,
            center_x_mm,
            center_y_mm,
            origin_x_mm,
            origin_y_mm,
            rect,
            mirror,
            flip_y: spec.bounds.is_some(),
            scale_x: spec.scale_x,
            scale_y: spec.scale_y,
            z_retract: spec.z_retract,
            z_safe: spec.z_safe,
        }
    }

    /// Maps a board point into machine coordinates: flip the Y-down board frame into
    /// the machine's Y-up one, rotate about the board centre, shift the rotated corner to
    /// the origin, apply per-axis scaling, then — for a back-face step — mirror about the
    /// board's centre line.
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
        let (mx, my) = self.turn_over(
            (rx - self.origin_x_mm) * self.scale_x,
            (ry - self.origin_y_mm) * self.scale_y,
        );
        Point::new(Length::from_mm(mx), Length::from_mm(my))
    }

    /// Maps a machine point back into board coordinates — the exact inverse of
    /// [`Self::xy`], undoing the turn-over mirror, scaling, translation, rotation and the
    /// Y flip in turn.
    ///
    /// Needed wherever geometry is derived in machine space but consumed by something
    /// that takes board coordinates: the outline's mouse-bite centres are found on the
    /// placed toolpath, and the locating pins are measured out from the placed board, and
    /// both are then handed to the drill planner, which places its own targets. A round
    /// trip through both is the identity, which is what the test asserts.
    pub fn unplace(&self, p: &Point) -> BoardPoint {
        // The mirror is its own inverse, so undoing it is applying it again.
        let (mx, my) = self.turn_over(p.x.as_mm(), p.y.as_mm());
        let rx = mx / self.scale_x + self.origin_x_mm;
        let ry = my / self.scale_y + self.origin_y_mm;
        // Rotate back: the inverse rotation is the same matrix with sin negated.
        let (bx, by) = rotate_about(rx, ry, self.center_x_mm, self.center_y_mm, self.cos, -self.sin);
        let unflipped = if self.flip_y { 2.0 * self.center_y_mm - by } else { by };
        BoardPoint { x: Length::from_mm(bx), y: Length::from_mm(unflipped) }
    }

    /// The board's own extents in the finished machine frame.
    ///
    /// Invariant under the turn-over mirror, because that mirror acts about this
    /// rectangle's own centre line — so a caller need not ask which face is being
    /// machined to know where the board is.
    pub fn board_rect_mm(&self) -> Rect {
        self.rect
    }

    /// Applies the turn-over mirror, or returns the point unchanged when there is none.
    /// An involution: applying it twice is the identity, which is what lets
    /// [`Self::unplace`] undo it by calling the same function.
    fn turn_over(&self, x: f64, y: f64) -> (f64, f64) {
        match self.mirror {
            Some(Mirror { on_x: true, line_mm }) => (2.0 * line_mm - x, y),
            Some(Mirror { on_x: false, line_mm }) => (x, 2.0 * line_mm - y),
            None => (x, y),
        }
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

    /// A spec with everything that is not the subject of a test at its neutral value:
    /// no margin, no flip, unity scaling. Every test then names only what it is about,
    /// which is also what makes "no margin and no flip reproduces the old transform"
    /// readable as the regression guard it is.
    fn spec(bounds: Option<&BoardBoundingBox>) -> PlacementSpec<'_> {
        PlacementSpec {
            bounds,
            orientation_deg: 0.0,
            origin: BoardOrigin::default(),
            margin: Margin::default(),
            flip: None,
            scale_x: 1.0,
            scale_y: 1.0,
            z_retract: Length::from_mm(2.0),
            z_safe: Length::from_mm(5.0),
        }
    }

    #[test]
    fn identity_shifts_the_min_corner_to_the_origin() {
        // Board bounds start at (10,20), 30 × 40. With no rotation/scaling, X is simply
        // offset; Y is measured from the board's *bottom* edge, because board Y counts
        // downward and machine Y counts up. The point sits 4 mm below the top edge, so
        // 40 − 4 = 36 mm above the bottom.
        let p = Placement::new(&spec(Some(&bounds(10.0, 20.0, 30.0, 40.0))));
        let out = p.xy(&pt(13.0, 24.0));
        assert!((out.x.as_mm() - 3.0).abs() < 1e-6, "x: {}", out.x.as_mm());
        assert!((out.y.as_mm() - 36.0).abs() < 1e-6, "y: {}", out.y.as_mm());
    }

    /// The whole board lands in the positive quadrant with its corners on the bounds,
    /// and the *order* of Y is reversed — the board's top edge is the machine's far
    /// side. Getting this backwards mirrors every hole.
    #[test]
    fn the_y_down_board_frame_is_flipped_into_the_machines_y_up_one() {
        let p = Placement::new(&spec(Some(&bounds(0.0, 0.0, 20.0, 10.0))));
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

    /// The fixture's registered corner is what lands on zero. It moves the **origin**,
    /// not the axes: a board zeroed on its right edge runs into negative X, because X
    /// still grows to the right — that is the machine's business, not the fixture's.
    ///
    /// Until this worked, the origin was editable in the UI, hardcoded to front-left in
    /// the crosswalk, and ignored here — a control that looked like it did something.
    #[test]
    fn the_work_origin_lands_on_the_corner_the_fixture_registers_against() {
        // A 20 x 10 board. Its four corners in machine space are (0,0) and (20,10) at
        // the extremes, whichever one is chosen as zero.
        let placed = |origin: BoardOrigin| {
            let b = bounds(0.0, 0.0, 20.0, 10.0);
            let p = Placement::new(&PlacementSpec { origin, ..spec(Some(&b)) });
            // Board (0,0) is its top-left corner, which the Y flip puts at the far side.
            let far_left = p.xy(&pt(0.0, 0.0));
            let near_right = p.xy(&pt(20.0, 10.0));
            (
                (far_left.x.as_mm(), far_left.y.as_mm()),
                (near_right.x.as_mm(), near_right.y.as_mm()),
            )
        };

        // Near-left: the default, and what every program emitted before this worked.
        assert_eq!(
            placed(BoardOrigin::default()),
            ((0.0, 10.0), (20.0, 0.0)),
            "the board sits wholly in the positive quadrant"
        );

        // Right-hand stop: X is measured back from the right edge, so the board is at
        // negative X. Y is untouched.
        assert_eq!(
            placed(BoardOrigin { x_at_right: true, y_at_far: false }),
            ((-20.0, 10.0), (0.0, 0.0))
        );

        // Far stop: likewise for Y.
        assert_eq!(
            placed(BoardOrigin { x_at_right: false, y_at_far: true }),
            ((0.0, 0.0), (20.0, -10.0))
        );

        // Both, i.e. registered into the far-right corner.
        assert_eq!(
            placed(BoardOrigin { x_at_right: true, y_at_far: true }),
            ((-20.0, 0.0), (0.0, -10.0))
        );
    }

    /// The edge names come straight from the schema, and anything unrecognised falls
    /// back to near-left rather than silently picking the far corner.
    #[test]
    fn origin_edges_parse_from_the_schema_words() {
        assert_eq!(BoardOrigin::from_edges("left", "near"), BoardOrigin::default());
        assert_eq!(
            BoardOrigin::from_edges("RIGHT", "Far"),
            BoardOrigin { x_at_right: true, y_at_far: true },
            "case is not the operator's problem"
        );
        assert_eq!(BoardOrigin::from_edges("nonsense", ""), BoardOrigin::default());
    }

    #[test]
    fn scaling_multiplies_each_axis() {
        let b = bounds(0.0, 0.0, 10.0, 10.0);
        let p = Placement::new(&PlacementSpec { scale_x: 1.01, scale_y: 0.99, ..spec(Some(&b)) });
        let out = p.xy(&pt(5.0, 5.0));
        assert!((out.x.as_mm() - 5.05).abs() < 1e-6);
        assert!((out.y.as_mm() - 4.95).abs() < 1e-6);
    }

    #[test]
    fn quarter_turn_keeps_the_board_in_the_positive_quadrant() {
        // A 20×10 board rotated 90° becomes 10×20; every mapped point stays within
        // [0,10]×[0,20] and the rotated min corner sits at the origin.
        let b = bounds(0.0, 0.0, 20.0, 10.0);
        let p = Placement::new(&PlacementSpec { orientation_deg: 90.0, ..spec(Some(&b)) });
        for (bx, by) in [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)] {
            let out = p.xy(&pt(bx, by));
            assert!(out.x.as_mm() >= -1e-6 && out.x.as_mm() <= 10.0 + 1e-6, "x in [0,10]: {}", out.x.as_mm());
            assert!(out.y.as_mm() >= -1e-6 && out.y.as_mm() <= 20.0 + 1e-6, "y in [0,20]: {}", out.y.as_mm());
        }
    }

    /// `unplace` is the exact inverse of `xy`, including the Y flip, an odd rotation, a
    /// margin and a back-face turn-over — geometry found on the placed toolpath has to
    /// come back to board space unchanged.
    ///
    /// The locating pins depend on this directly: their centres are measured out in
    /// machine space and then unplaced to become drill targets, so an inverse that was
    /// only approximately right would put the second setup's registration in the wrong
    /// place — and by a margin far too small to see.
    #[test]
    fn placing_and_unplacing_is_the_identity() {
        let margin = Margin { x_min: 2.0, x_max: 0.0, y_min: 4.8, y_max: 4.8 };
        for (angle, sx, sy) in [(0.0, 1.0, 1.0), (90.0, 1.0, 1.0), (37.0, 1.01, 0.99)] {
            for flip in [None, Some(BoardFlip::AboutY), Some(BoardFlip::AboutX)] {
                let b = bounds(3.0, 7.0, 12.0, 9.0);
                let p = Placement::new(&PlacementSpec {
                    orientation_deg: angle,
                    scale_x: sx,
                    scale_y: sy,
                    margin,
                    flip,
                    ..spec(Some(&b))
                });
                for board in [pt(3.0, 7.0), pt(9.0, 11.5), pt(15.0, 16.0)] {
                    let back = p.unplace(&p.xy(&board));
                    assert!(
                        (back.x.as_mm() - board.x.as_mm()).abs() < 1e-9
                            && (back.y.as_mm() - board.y.as_mm()).abs() < 1e-9,
                        "at {angle}°, flip {flip:?}: {:?} → {:?}",
                        (board.x.as_mm(), board.y.as_mm()),
                        (back.x.as_mm(), back.y.as_mm())
                    );
                }
            }
        }
    }

    #[test]
    fn is_deterministic() {
        let b = bounds(3.0, 7.0, 12.0, 9.0);
        let p = Placement::new(&PlacementSpec { orientation_deg: 37.0, ..spec(Some(&b)) });
        assert_eq!(p.xy(&pt(6.0, 9.0)), p.xy(&pt(6.0, 9.0)));
    }

    // -----------------------------------------------------------------------
    // Margin — the envelope the locating pins need
    // -----------------------------------------------------------------------

    /// **The regression guard the whole "board-anchored" decision rests on.**
    ///
    /// A job with no locating pins has a zero margin, and must place every point exactly
    /// where it did before margins existed. If this ever fails, every board machined
    /// against a previously-saved program is cut in the wrong place — and the programs
    /// still look entirely reasonable.
    #[test]
    fn a_zero_margin_changes_nothing() {
        let b = bounds(10.0, 20.0, 30.0, 40.0);
        let plain = Placement::new(&spec(Some(&b)));
        let margined =
            Placement::new(&PlacementSpec { margin: Margin::default(), ..spec(Some(&b)) });
        for board in [pt(13.0, 24.0), pt(10.0, 20.0), pt(40.0, 60.0)] {
            assert_eq!(plain.xy(&board), margined.xy(&board));
        }
    }

    /// A margin moves the **zero**, never the board: every point shifts by exactly the
    /// margin on the side the origin is registered against, and the board's own size is
    /// untouched. The pins sit in the space that opens up.
    #[test]
    fn a_margin_moves_the_origin_and_not_the_board() {
        let b = bounds(0.0, 0.0, 20.0, 10.0);
        let plain = Placement::new(&spec(Some(&b)));
        let p = Placement::new(&PlacementSpec {
            // The shape a Y-axis pin pair makes: clearance below and above, none sideways.
            margin: Margin { x_min: 0.0, x_max: 0.0, y_min: 4.8, y_max: 4.8 },
            ..spec(Some(&b))
        });

        let corner = pt(0.0, 10.0); // board bottom-left → machine (0,0) with no margin
        assert_eq!(plain.xy(&corner), Point::new(Length::from_mm(0.0), Length::from_mm(0.0)));
        let moved = p.xy(&corner);
        assert!((moved.x.as_mm() - 0.0).abs() < 1e-9, "X has no margin: {}", moved.x.as_mm());
        assert!((moved.y.as_mm() - 4.8).abs() < 1e-9, "Y clears the pin: {}", moved.y.as_mm());

        // The board is the same board, 20 × 10, just further from the zero.
        let rect = p.board_rect_mm();
        assert!((rect.max_x - rect.min_x - 20.0).abs() < 1e-9);
        assert!((rect.max_y - rect.min_y - 10.0).abs() < 1e-9);
    }

    /// Registered against a right-hand stop, the board runs into negative X — so the
    /// margin has to push the zero the *other* way, or it would eat into the work instead
    /// of clearing the pins.
    #[test]
    fn a_margin_pushes_outward_whichever_corner_is_registered() {
        let b = bounds(0.0, 0.0, 20.0, 10.0);
        let margin = Margin { x_min: 3.0, x_max: 3.0, y_min: 0.0, y_max: 0.0 };
        let front_left = Placement::new(&PlacementSpec { margin, ..spec(Some(&b)) });
        let back_right = Placement::new(&PlacementSpec {
            margin,
            origin: BoardOrigin { x_at_right: true, y_at_far: true },
            ..spec(Some(&b))
        });

        // Front-left: the board's left edge sits 3 mm *up* from zero.
        assert!((front_left.board_rect_mm().min_x - 3.0).abs() < 1e-9);
        // Back-right: its right edge sits 3 mm *down* from zero, i.e. at −3.
        assert!((back_right.board_rect_mm().max_x + 3.0).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // The board turned over for its back face
    // -----------------------------------------------------------------------

    /// The axis names come from the fixture schema, and name the axis the board *turns
    /// about* — so the coordinate that mirrors is the other one. Reading this backwards
    /// mirrors the solder side along the wrong axis, which is a scrapped board that looks
    /// perfectly plausible in the preview.
    #[test]
    fn the_flip_axis_names_the_axis_turned_about_not_the_one_mirrored() {
        assert_eq!(BoardFlip::from_axis("y"), BoardFlip::AboutY);
        assert!(BoardFlip::AboutY.mirrors_x(), "a page turn mirrors X");
        assert_eq!(BoardFlip::from_axis("X"), BoardFlip::AboutX);
        assert!(!BoardFlip::AboutX.mirrors_x(), "a tumble mirrors Y");
        assert_eq!(BoardFlip::from_axis("nonsense"), BoardFlip::AboutY, "the page turn is the default");
    }

    /// `X' = 2L − X` about the board's centre line, Y untouched (and the transpose for a
    /// tumble). Stated as the spec states it, because this one line is the whole of
    /// double-sided registration.
    #[test]
    fn a_bottom_side_step_mirrors_about_the_boards_centre_line() {
        let b = bounds(0.0, 0.0, 20.0, 10.0);
        let top = Placement::new(&spec(Some(&b)));
        let bottom =
            Placement::new(&PlacementSpec { flip: Some(BoardFlip::AboutY), ..spec(Some(&b)) });

        let line = top.board_rect_mm().centre_x();
        assert!((line - 10.0).abs() < 1e-9, "the centre line of a 20 mm board");

        for board in [pt(0.0, 0.0), pt(3.0, 4.0), pt(20.0, 10.0)] {
            let front = top.xy(&board);
            let back = bottom.xy(&board);
            assert!(
                (back.x.as_mm() - (2.0 * line - front.x.as_mm())).abs() < 1e-9,
                "X mirrors: {} vs {}",
                back.x.as_mm(),
                front.x.as_mm()
            );
            assert!((back.y.as_mm() - front.y.as_mm()).abs() < 1e-9, "Y is untouched");
        }
    }

    /// The flipped board occupies **exactly** the frame the front side did. It has to:
    /// the operator turns one physical board over in one physical fixture, so if the
    /// envelope moved, the second setup would be cutting somewhere the first was not.
    #[test]
    fn turning_the_board_over_leaves_it_in_the_same_envelope() {
        let b = bounds(0.0, 0.0, 20.0, 10.0);
        for flip in [BoardFlip::AboutY, BoardFlip::AboutX] {
            let top = Placement::new(&spec(Some(&b)));
            let bottom = Placement::new(&PlacementSpec { flip: Some(flip), ..spec(Some(&b)) });
            assert_eq!(
                top.board_rect_mm(),
                bottom.board_rect_mm(),
                "{flip:?} must map the board onto itself"
            );
        }
    }

    /// With no board there is no centre line to turn about, and mirroring about zero
    /// would translate the work rather than flip it. The flip is dropped instead.
    #[test]
    fn a_flip_with_no_board_is_ignored_rather_than_applied_about_zero() {
        let p = Placement::new(&PlacementSpec {
            flip: Some(BoardFlip::AboutY),
            ..spec(None)
        });
        let out = p.xy(&pt(7.0, 3.0));
        assert_eq!(out, Point::new(Length::from_mm(7.0), Length::from_mm(3.0)));
    }
}
