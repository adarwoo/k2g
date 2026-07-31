//! **Locating pins** — where the two registration holes go, and how much room the job's
//! coordinate frame has to make for them.
//!
//! Pure geometry over the placed board rectangle, so all of it is testable without a
//! store, a board or a fixture. The rules are fixed and there is deliberately nothing to
//! configure but the pin diameter: a registration scheme with knobs on is one an operator
//! can get wrong, and getting it wrong is a scrapped board with no symptom until the
//! second setup is already cut.
//!
//! ## The rules
//!
//! A pin pair sits **on the fixture's flip mirror line**, centred on the board and one
//! diameter clear of it, one pin each side. That single sentence is what makes
//! double-sided work: the board turns over about that line, so each pin maps onto
//! *itself* and the flipped board lands back in the frame it left.
//!
//! The line follows the fixture's `board_flip_axis`, which names the axis the board turns
//! **about**. A page turn (about Y) mirrors X, so its mirror line is vertical and the pins
//! sit above and below the board. A tumble (about X) is the transpose.
//!
//! ```text
//!            BoardFlip::AboutY                    BoardFlip::AboutX
//!         (page turn, X mirrors)              (tumble, Y mirrors)
//!
//!                  o  pin                      mirror line
//!            ┌───────────────┐            ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈
//!            │               │              ┌───────────────┐
//!            │     board     │            o │     board     │ o
//!            │               │              └───────────────┘
//!            └───────────────┘            ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈
//!                  o  pin                    pin           pin
//!            ┆               ┆
//!            mirror line (vertical)
//! ```
//!
//! ## Why the margin is knowable before the placement is
//!
//! [`margin`] takes no geometry at all: the growth is a fixed multiple of the diameter.
//! That is what breaks what would otherwise be a circular dependency — the origin has to
//! clear the pins, the pins are measured from the placed board, and the placed board
//! depends on the origin. Because the *amount* of clearance needs no board, the margin can
//! be handed to [`Placement::new`](super::placement::Placement::new) up front and the pin
//! centres read back out of the finished placement afterwards.

use units::Length;

use super::placement::{BoardFlip, Margin, Rect};
use super::plan::Point;

/// How far a pin centre sits from the board's bounding box, as a multiple of the pin
/// diameter.
///
/// One diameter: close enough that the pin pair spans little more than the board (a longer
/// lever arm between the pins would be *better* for angular registration, but it is also
/// more blank to buy and more overhang to clamp), and far enough that the pin hole never
/// breaks into the routed channel around the edge.
const CLEARANCE_DIAMETERS: f64 = 1.0;

/// How far the envelope grows beyond the board on each pin side: the centre clearance plus
/// the pin's own radius, so the hole is inside the frame and not merely its centre.
const GROWTH_DIAMETERS: f64 = CLEARANCE_DIAMETERS + 0.5;

/// The room the work origin has to make for a pin pair of this diameter.
///
/// Grows the two sides the pins are on and leaves the other two alone — a page-turn
/// fixture puts them above and below, so the board keeps its X extents exactly.
///
/// Needs no board geometry, which is the whole point: see the module note.
pub fn margin(diameter: Length, axis: BoardFlip) -> Margin {
    let growth = GROWTH_DIAMETERS * diameter.as_mm();
    if axis.mirrors_x() {
        // Turned about Y: the mirror line is vertical, so the pins are above and below.
        Margin { x_min: 0.0, x_max: 0.0, y_min: growth, y_max: growth }
    } else {
        Margin { x_min: growth, x_max: growth, y_min: 0.0, y_max: 0.0 }
    }
}

/// The two pin centres in the finished machine frame, given the placed board rectangle
/// ([`Placement::board_rect_mm`](super::placement::Placement::board_rect_mm)).
///
/// Ordered low-coordinate first along the axis they are spread on, so a plan is the same
/// plan every time it is built.
pub fn centres(rect: Rect, axis: BoardFlip, diameter: Length) -> [Point; 2] {
    let clearance = CLEARANCE_DIAMETERS * diameter.as_mm();
    let at = |x: f64, y: f64| Point::new(Length::from_mm(x), Length::from_mm(y));

    if axis.mirrors_x() {
        // On the vertical centre line, which is what maps them onto themselves when X
        // mirrors — a pin anywhere else would move under the flip, and then it is not
        // registration, it is two holes that happen to be round.
        let x = rect.centre_x();
        [at(x, rect.min_y - clearance), at(x, rect.max_y + clearance)]
    } else {
        let y = rect.centre_y();
        [at(rect.min_x - clearance, y), at(rect.max_x + clearance, y)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcode::placement::{BoardOrigin, Placement, PlacementSpec};

    fn rect(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Rect {
        Rect { min_x, min_y, max_x, max_y }
    }

    fn mm(v: f64) -> Length {
        Length::from_mm(v)
    }

    /// A page turn spreads the pins along Y and leaves X alone; a tumble is the
    /// transpose. Growing the wrong pair of sides would put the origin in the middle of
    /// the pin holes.
    #[test]
    fn the_margin_grows_only_the_sides_the_pins_are_on() {
        let page = margin(mm(3.2), BoardFlip::AboutY);
        assert_eq!((page.x_min, page.x_max), (0.0, 0.0), "a page turn does not widen X");
        assert!((page.y_min - 4.8).abs() < 1e-9, "1.5 x 3.2: {}", page.y_min);
        assert_eq!(page.y_min, page.y_max, "symmetric pins, symmetric growth");

        let tumble = margin(mm(3.2), BoardFlip::AboutX);
        assert_eq!((tumble.y_min, tumble.y_max), (0.0, 0.0));
        assert!((tumble.x_min - 4.8).abs() < 1e-9);
    }

    /// Centre clearance is one diameter; the envelope has to clear the *hole*, so it grows
    /// by a further radius. Getting the two confused leaves half a pin hole outside the
    /// frame — and the operator only finds out when the cutter reaches it.
    #[test]
    fn the_envelope_clears_the_hole_and_not_merely_its_centre() {
        let d = mm(3.0);
        let board = rect(0.0, 0.0, 20.0, 10.0);
        let [low, _] = centres(board, BoardFlip::AboutY, d);

        let gap = board.min_y - low.y.as_mm();
        assert!((gap - 3.0).abs() < 1e-9, "centre is one diameter clear: {gap}");
        let outer_edge = gap + d.as_mm() / 2.0;
        assert!(
            (margin(d, BoardFlip::AboutY).y_min - outer_edge).abs() < 1e-9,
            "the margin reaches the far side of the hole"
        );
    }

    /// Both pins sit on the mirror line, centred on the board, one each side.
    #[test]
    fn the_pins_sit_on_the_mirror_line_one_each_side() {
        let board = rect(0.0, 0.0, 20.0, 10.0);

        let [a, b] = centres(board, BoardFlip::AboutY, mm(3.0));
        assert_eq!(a.x, b.x, "both on the vertical centre line");
        assert!((a.x.as_mm() - 10.0).abs() < 1e-9, "centred on the board");
        assert!(a.y.as_mm() < board.min_y && b.y.as_mm() > board.max_y, "one each side");
        assert!(a.y.as_mm() < b.y.as_mm(), "ordered low first");

        let [a, b] = centres(board, BoardFlip::AboutX, mm(3.0));
        assert_eq!(a.y, b.y, "both on the horizontal centre line");
        assert!((a.y.as_mm() - 5.0).abs() < 1e-9);
        assert!(a.x.as_mm() < board.min_x && b.x.as_mm() > board.max_x);
        assert!(a.x.as_mm() < b.x.as_mm(), "ordered low first");
    }

    /// **The property the whole scheme rests on.** Turn the board over and each pin lands
    /// on itself, so the holes drilled in setup 1 are the holes the pins sit in for setup
    /// 2. A pin that moved by even a fraction would still *look* like registration in
    /// every view k2g has — and the second side would be cut off-register.
    ///
    /// Asserted end to end through a real [`Placement`], not against a restatement of the
    /// mirror arithmetic, because the two could otherwise agree with each other while both
    /// being wrong.
    #[test]
    fn a_pin_maps_onto_itself_when_the_board_is_turned_over() {
        let board = pcb::BoardBoundingBox {
            x: mm(0.0),
            y: mm(0.0),
            width: mm(37.0),
            height: mm(23.0),
        };
        let diameter = mm(3.2);

        for (axis, origin) in [
            (BoardFlip::AboutY, BoardOrigin::default()),
            (BoardFlip::AboutX, BoardOrigin::default()),
            // Registered into the far-right corner, where the board runs negative — the
            // case where a sign error hides.
            (BoardFlip::AboutY, BoardOrigin { x_at_right: true, y_at_far: true }),
        ] {
            let base = PlacementSpec {
                bounds: Some(&board),
                orientation_deg: 0.0,
                origin,
                margin: margin(diameter, axis),
                flip: None,
                scale_x: 1.0,
                scale_y: 1.0,
                z_retract: mm(2.0),
                z_safe: mm(5.0),
            };
            let top = Placement::new(&base);
            let bottom = Placement::new(&PlacementSpec { flip: Some(axis), ..base });

            for pin in centres(top.board_rect_mm(), axis, diameter) {
                // Where the top-side program drilled it, expressed as a board point…
                let as_drilled = top.unplace(&pin);
                // …and where the bottom-side program would find that same feature.
                let after_flip = bottom.xy(&as_drilled);
                assert!(
                    (after_flip.x.as_mm() - pin.x.as_mm()).abs() < 1e-9
                        && (after_flip.y.as_mm() - pin.y.as_mm()).abs() < 1e-9,
                    "{axis:?}: pin at {:?} moved to {:?} under the flip",
                    (pin.x.as_mm(), pin.y.as_mm()),
                    (after_flip.x.as_mm(), after_flip.y.as_mm())
                );
            }
        }
    }

    /// The margin is what keeps the pins inside the frame; with it applied, neither pin
    /// hole strays outside the envelope the origin anchors.
    #[test]
    fn both_pin_holes_land_inside_the_envelope_the_margin_opened() {
        let board = pcb::BoardBoundingBox {
            x: mm(0.0),
            y: mm(0.0),
            width: mm(37.0),
            height: mm(23.0),
        };
        let diameter = mm(3.2);
        let axis = BoardFlip::AboutY;
        let placement = Placement::new(&PlacementSpec {
            bounds: Some(&board),
            orientation_deg: 0.0,
            origin: BoardOrigin::default(),
            margin: margin(diameter, axis),
            flip: None,
            scale_x: 1.0,
            scale_y: 1.0,
            z_retract: mm(2.0),
            z_safe: mm(5.0),
        });

        for pin in centres(placement.board_rect_mm(), axis, diameter) {
            let radius = diameter.as_mm() / 2.0;
            assert!(
                pin.y.as_mm() - radius >= -1e-9,
                "the hole reaches below the zero: {}",
                pin.y.as_mm() - radius
            );
        }
    }
}
