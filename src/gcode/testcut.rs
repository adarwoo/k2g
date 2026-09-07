//! **The engraving depth test cut** — how wide a band it needs, and where the L goes in it.
//!
//! Pure geometry over the placed board rectangle, so all of it is testable without a store, a
//! board or a fixture — the same shape as [`super::pins`], and deliberately the same *pattern*:
//! a [`band`] that says how much room is needed while knowing nothing about the board, and an
//! [`l_path`] that reads the finished placement back out.
//!
//! ## Why there is a test cut at all
//!
//! Isolation engraving is the one operation whose quality is a *depth tolerance*. A V-bit's
//! channel is `tip + 2·depth·tan(angle/2)` ([`super::assigner::engrave_width_mm`]), and the
//! depth asked for is a few tens of microns of substrate under a whisker of copper. The pass
//! then runs at one fixed Z for its whole length: there is no probing and no height map. So if
//! the controller's Z0 is not the board surface, nothing anywhere finds out until the board is
//! engraved — nets still joined, or traces undercut and the laminate ploughed.
//!
//! The L is cut first, in scrap, at exactly the depth the pass will use, and the program stops
//! so it can be looked at. What the operator reads is the channel's **width**: a V-bit cuts
//! wider as it goes deeper, so width and depth are one measurement, and width is the one a
//! loupe with a scale resolves.
//!
//! ## The rules
//!
//! Two legs meeting at a right angle, one as long as the board is wide and one as long as it is
//! high, running down the middle of a band just outside the board's bounding box.
//!
//! Long legs are the point. A short scratch says what the depth is at one spot; two legs across
//! the board's own span say whether it is the same depth everywhere, and work that is not held
//! flat fails an isolation pass exactly as reliably as a mis-set tool does.
//!
//! ## Which band, and why it is usually free
//!
//! A job that routes its own outline **already** removes a band of material round the board —
//! `kerf + finishing` wide, waste by construction. If that band has room for the trough with a
//! trough-width of material either side of it, the test cut goes down its middle and the job's
//! frame does not grow by a single micron. That is the common case: a 2 mm cutter leaves
//! 2.1 mm, and a 0.25 mm trough needs 0.75 mm.
//!
//! Only when there is no such band — a board that is engraved but not cut out — does [`band`]
//! open one of its own, `3 × trough` wide, and the board moves out by that much. Three, so the
//! cut spans `[trough, 2·trough]` from the bounding box: one trough-width of material between
//! the L and the board, and another between the L and the work clearance.
//!
//! ## Where this ends up in the frame, and two placements that were wrong
//!
//! The band is an **extent** — how far the test cut reaches from the board's bounding box — and
//! it competes with the other extents rather than adding to them ([`Margin::widest`]). The
//! fixture's `work_clearance` is then stacked outside the widest of them, so the board sits at
//! `clearance + max(extents)` and nothing the job cuts comes nearer the zero than the clearance.
//!
//! Two earlier placements are worth recording, because both look right and neither is:
//!
//! **A fixed offset *past* the zero.** The literal reading of "outside the work", and it puts
//! the cut at negative X and Y on an ordinary near-left fixture — the far side of the corner the
//! board is registered into, which is where the fixture's stop is (`scene::FixtureMark`'s arms
//! are documented as running *away from the stop*), and possibly outside the controller's
//! travel. k2g carries no bed or envelope geometry, so nothing would have caught it.
//!
//! **The corner on the zero.** Tidy, and wrong for a reason that is easy to miss: a cutter has
//! width. A 0.2 mm channel centred on the origin puts half of itself at −0.1 mm. The zero is not
//! a place to cut; it is a place to measure from.
//!
//! ## What is still not knowable here
//!
//! k2g holds no stock, bed or keep-out geometry, so whether the blank actually extends far
//! enough is not decidable in this module. What the band *does* guarantee is that the L is
//! inside the frame the job already required — for a routed job, inside material the job itself
//! removes.

use units::Length;

use super::placement::{Margin, Rect};
use super::plan::Point;

/// How many trough-widths wide a band opened purely for the test cut is.
///
/// Three: the cut spans `[trough, 2·trough]` from the bounding box, leaving one trough-width of
/// material on the board side and one on the clearance side. Two would put the cut hard against
/// an edge; four would push the board out for no gain.
const BAND_TROUGHS: f64 = 3.0;

/// How wide a band the test cut needs, as an extent from the board's bounding box.
///
/// `waste` is the material the outline router removes (`kerf + finishing`), or zero when the job
/// does not cut its own outline. Reusing it when it is wide enough is what makes the test cut
/// free on an ordinary routed job — the frame does not grow at all, and the L is cut in material
/// that was going to be swarf anyway.
///
/// `trough` is the isolation channel's width. Note it is the width the operator *asked for*, not
/// the one the chosen V-bit actually cuts: this has to be answerable before a bit is picked, for
/// the same reason [`super::pins::margin`] has to be answerable before the board is placed.
pub fn band(waste: Length, trough: Length) -> Length {
    let opened = Length::from_mm(trough.as_mm() * BAND_TROUGHS);
    if waste.as_mm() >= opened.as_mm() {
        waste
    } else {
        opened
    }
}

/// The band expressed as an extent on the two sides the origin is on — the only sides that can
/// move the zero, and the only ones the L is cut on.
pub fn margin(band: Length, origin: super::placement::BoardOrigin) -> Margin {
    let mm = band.as_mm();
    Margin {
        x_min: if origin.x_at_right { 0.0 } else { mm },
        x_max: if origin.x_at_right { mm } else { 0.0 },
        y_min: if origin.y_at_far { 0.0 } else { mm },
        y_max: if origin.y_at_far { mm } else { 0.0 },
    }
}

/// The L's cutter-centre polyline in machine coordinates — X-leg end, corner, Y-leg end.
///
/// Three points and one path, deliberately: the right angle at the vertex is the most readable
/// part of the witness, and two separate strokes would leave a step in it.
///
/// The corner sits half a band inside the board's bounding-box corner, on the zero side, so the
/// cut runs down the band's centre line. Which direction the legs run is taken from the **placed
/// rectangle itself** — whichever side of the zero the board is on — rather than from the
/// fixture's origin corner a second time; reading it back off the geometry is what makes it
/// impossible for the direction and the band to disagree.
///
/// `None` when the placement did not make room: no board, an inside-out rect (see
/// [`Rect::width`]), a board straddling the zero, or one that does not clear the zero by the
/// whole band. That last case is the guard that matters — it is what a caller that did not fold
/// [`margin`] into the frame looks like, and cutting anyway would put the L across the clearance
/// the fixture declared, or through the board's own edge.
pub fn l_path(rect: Rect, band: Length) -> Option<[Point; 3]> {
    let (w, h) = (rect.width(), rect.height());
    let band_mm = band.as_mm();
    if !(w > 0.0 && h > 0.0 && band_mm > 0.0) {
        return None;
    }

    // The board has to sit wholly on one side of the zero, at least a full band clear of it.
    // Anything else means the band was never folded into the placement.
    let side = |min: f64, max: f64| -> Option<f64> {
        if min >= band_mm - 1e-9 {
            Some(1.0)
        } else if max <= -band_mm + 1e-9 {
            Some(-1.0)
        } else {
            None
        }
    };
    let sx = side(rect.min_x, rect.max_x)?;
    let sy = side(rect.min_y, rect.max_y)?;

    let at = |x: f64, y: f64| Point::new(Length::from_mm(x), Length::from_mm(y));

    // Half a band inside the bounding-box corner, on the zero side.
    let cx = if sx > 0.0 { rect.min_x } else { rect.max_x } - sx * band_mm / 2.0;
    let cy = if sy > 0.0 { rect.min_y } else { rect.max_y } - sy * band_mm / 2.0;

    // Legs run from the corner *towards* the board, each spanning that board dimension.
    Some([at(cx + sx * w, cy), at(cx, cy), at(cx, cy + sy * h)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcode::pins;
    use crate::gcode::placement::{BoardFlip, BoardOrigin, Placement, PlacementSpec};

    fn mm(v: f64) -> Length {
        Length::from_mm(v)
    }

    fn rect(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Rect {
        Rect { min_x, min_y, max_x, max_y }
    }

    fn origin(x_at_right: bool, y_at_far: bool) -> BoardOrigin {
        BoardOrigin { x_at_right, y_at_far }
    }

    const ORIGINS: [(&str, bool, bool); 4] = [
        ("left/near", false, false),
        ("right/near", true, false),
        ("left/far", false, true),
        ("right/far", true, true),
    ];

    /// A 40 x 25 board placed the way a real job places it: the extents combined with `widest`,
    /// the fixture's clearance stacked outside them.
    fn placed(
        board_origin: BoardOrigin,
        axis: BoardFlip,
        pin: Option<f64>,
        waste_mm: f64,
        band_mm: f64,
        clearance_mm: f64,
    ) -> Placement {
        let bounds = pcb::BoardBoundingBox {
            x: mm(10.0),
            y: mm(20.0),
            width: mm(40.0),
            height: mm(25.0),
        };
        let extents = pin
            .map(|d| pins::margin(mm(d), axis))
            .unwrap_or_default()
            .widest(Margin::uniform(waste_mm))
            .widest(margin(mm(band_mm), board_origin));
        Placement::new(&PlacementSpec {
            bounds: Some(&bounds),
            orientation_deg: 0.0,
            origin: board_origin,
            margin: extents.stack(margin(mm(clearance_mm), board_origin)),
            flip: None,
            scale_x: 1.0,
            scale_y: 1.0,
            z_retract: mm(5.0),
            z_safe: mm(20.0),
        })
    }

    /// **An existing waste band is used rather than a new one opened.** A job that routes its
    /// own outline already removes 2.1 mm of material round the board; asking it to move the
    /// board out again for a 0.25 mm trough would cost blank, move every coordinate and buy
    /// nothing. This is what makes the option free on the common job.
    #[test]
    fn an_existing_waste_band_is_used_rather_than_opening_a_new_one() {
        // 2 mm cutter + 0.1 mm finishing against a 0.25 mm trough: 2.1 >= 0.75.
        assert_eq!(band(mm(2.1), mm(0.25)), mm(2.1), "the routed waste is wide enough");
        // A 1/8" cutter leaves even more.
        assert_eq!(band(mm(3.275), mm(0.25)), mm(3.275));
        // A board that is engraved but never cut out has no waste at all.
        assert_eq!(band(mm(0.0), mm(0.25)), mm(0.75), "so a band is opened");
        // And a waste band too narrow to hold the trough with material either side is not used.
        assert_eq!(band(mm(0.5), mm(0.25)), mm(0.75));
    }

    /// A band opened for the test cut leaves one trough-width of material between the L and the
    /// board, and another between the L and the work clearance. Two would put the cut hard
    /// against an edge — either shaving the board or crossing the clearance the fixture
    /// declared.
    #[test]
    fn a_band_opened_for_the_test_cut_leaves_a_trough_width_of_material_each_side() {
        let trough = 0.25;
        let opened = band(mm(0.0), mm(trough));
        let placed = placed(origin(false, false), BoardFlip::AboutY, None, 0.0, opened.as_mm(), 2.0);
        let l = l_path(placed.board_rect_mm(), opened).expect("a placed board gets a test cut");
        let board = placed.board_rect_mm();

        // The cut's near edge and far edge, measured from the bounding box.
        let from_board = board.min_y - l[1].y.as_mm();
        assert!(
            (from_board - 1.5 * trough).abs() < 1e-9,
            "the cut centre should be 1.5 troughs from the board, got {from_board}",
        );
        assert!(
            (from_board - trough / 2.0 - trough).abs() < 1e-9,
            "leaving a trough-width of material on the board side",
        );
    }

    /// **Nothing the job cuts comes nearer the zero than the work clearance.** The single
    /// statement the whole change exists to make true, swept over the things that can move it —
    /// and measured to the cutting *edge*, because a tool has width and that is precisely what
    /// the "corner on the zero" placement got wrong.
    #[test]
    fn nothing_is_cut_inside_the_work_clearance() {
        for clearance in [0.5, 2.0, 5.0] {
            for waste in [0.0, 2.1, 3.275] {
                for pin in [None, Some(1.0), Some(3.2), Some(6.0)] {
                    for trough in [0.1, 0.25, 0.6] {
                        let b = band(mm(waste), mm(trough));
                        for (name, x_at_right, y_at_far) in ORIGINS {
                            let o = origin(x_at_right, y_at_far);
                            let p = placed(o, BoardFlip::AboutY, pin, waste, b.as_mm(), clearance);
                            let board = p.board_rect_mm();
                            let l = l_path(board, b).expect("a placed board gets a test cut");
                            let label = format!(
                                "{name} C={clearance} waste={waste} pin={pin:?} trough={trough}"
                            );

                            // The test cut's own edge, half a trough beyond its centre line.
                            for point in l {
                                for (v, at_max) in
                                    [(point.x.as_mm(), x_at_right), (point.y.as_mm(), y_at_far)]
                                {
                                    let edge = v.abs() - trough / 2.0;
                                    assert!(
                                        edge >= clearance - 1e-9,
                                        "{label}: a test-cut edge is {edge:.4} from the zero",
                                    );
                                    assert!(
                                        (v >= 0.0) != at_max || v == 0.0,
                                        "{label}: {v} is on the wrong side of the zero",
                                    );
                                }
                            }

                            // The routed channel's outer edge, at the waste extent.
                            if waste > 0.0 {
                                let channel = board.min_x.abs().min(board.max_x.abs()) - waste;
                                assert!(
                                    channel >= -1e-9,
                                    "{label}: the routed channel reaches {channel:.4} past zero",
                                );
                            }

                            // Every pin hole's outer edge.
                            if let Some(d) = pin {
                                for centre in pins::centres(board, BoardFlip::AboutY, mm(d)) {
                                    let edge = centre.y.as_mm().abs() - d / 2.0;
                                    assert!(
                                        edge >= -1e-9,
                                        "{label}: a pin hole reaches {edge:.4} past zero",
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// The cut runs down the band's centre line, on every fixture. Off-centre would spend the
    /// material margin on one side and leave none on the other.
    #[test]
    fn the_test_cut_sits_in_the_middle_of_its_band() {
        let b = band(mm(2.1), mm(0.25));
        for (name, x_at_right, y_at_far) in ORIGINS {
            let o = origin(x_at_right, y_at_far);
            let board = placed(o, BoardFlip::AboutY, Some(3.2), 2.1, b.as_mm(), 2.0).board_rect_mm();
            let l = l_path(board, b).expect("a placed board gets a test cut");

            let edge_x = if x_at_right { board.max_x } else { board.min_x };
            let edge_y = if y_at_far { board.max_y } else { board.min_y };
            assert!(
                ((edge_x - l[1].x.as_mm()).abs() - b.as_mm() / 2.0).abs() < 1e-9,
                "{name}: the corner is not half a band inside the bbox in X",
            );
            assert!(
                ((edge_y - l[1].y.as_mm()).abs() - b.as_mm() / 2.0).abs() < 1e-9,
                "{name}: the corner is not half a band inside the bbox in Y",
            );
        }
    }

    /// **Matched to the board**, which is what makes it a flatness check and not just a depth
    /// check: legs that stopped at some fixed length would say nothing about the far corner of
    /// the travel, which is where a board that is not held flat goes wrong.
    #[test]
    fn the_legs_are_the_boards_own_width_and_height() {
        let b = band(mm(2.1), mm(0.25));
        for (name, x_at_right, y_at_far) in ORIGINS {
            let o = origin(x_at_right, y_at_far);
            let board = placed(o, BoardFlip::AboutY, Some(3.2), 2.1, b.as_mm(), 2.0).board_rect_mm();
            let l = l_path(board, b).expect("a placed board gets a test cut");

            assert!(
                ((l[0].x.as_mm() - l[1].x.as_mm()).abs() - board.width()).abs() < 1e-9,
                "{name}: the X leg is not the board's width",
            );
            assert!(
                ((l[2].y.as_mm() - l[1].y.as_mm()).abs() - board.height()).abs() < 1e-9,
                "{name}: the Y leg is not the board's height",
            );
            // Legs, not a diagonal: each keeps the corner's other coordinate.
            assert!((l[0].y.as_mm() - l[1].y.as_mm()).abs() < 1e-9, "{name}: X leg is not axial");
            assert!((l[2].x.as_mm() - l[1].x.as_mm()).abs() < 1e-9, "{name}: Y leg is not axial");
        }
    }

    /// **The band has to have been folded in.** A caller that builds the placement without the
    /// test cut's extent gets no test cut rather than one laid across the work clearance or the
    /// board's own edge — the failure that would otherwise be silent, because the L would still
    /// look like an L in every view.
    #[test]
    fn a_board_that_does_not_clear_its_band_gets_no_test_cut() {
        let b = mm(2.1);
        assert!(l_path(rect(2.0, 5.0, 42.0, 30.0), b).is_none(), "2.0 mm is not 2.1 mm of room");
        assert!(l_path(rect(2.1, 2.1, 42.1, 27.1), b).is_some(), "exactly the band is enough");
        assert!(l_path(rect(-1.0, 5.0, 42.0, 30.0), b).is_none(), "a board straddling the zero");
    }

    /// No board, no test cut — and no panic. `Placement` with no bounds hands out a zero rect,
    /// and a hand-built rect can be inside-out, because `Rect` is only normalised inside
    /// `Placement::new`.
    #[test]
    fn a_board_with_no_size_gets_no_test_cut() {
        let b = mm(2.1);
        assert!(l_path(rect(0.0, 0.0, 0.0, 0.0), b).is_none(), "a zero rect is not a board");
        assert!(l_path(rect(5.0, 5.0, 45.0, 5.0), b).is_none(), "nor is a board with no height");
        assert!(l_path(rect(5.0, 5.0, -45.0, 30.0), b).is_none(), "nor an inside-out one");
        assert!(l_path(rect(5.0, 5.0, 45.0, 30.0), mm(0.0)).is_none(), "nor is a zero band");
    }

    /// The band claims only the sides the origin is on. Growing all four would move the zero
    /// away from the board on a side the L never visits, which costs blank for nothing.
    #[test]
    fn the_band_claims_only_the_two_sides_the_origin_is_on() {
        let near_left = margin(mm(2.1), origin(false, false));
        assert_eq!((near_left.x_min, near_left.y_min), (2.1, 2.1));
        assert_eq!((near_left.x_max, near_left.y_max), (0.0, 0.0));

        let far_right = margin(mm(2.1), origin(true, true));
        assert_eq!((far_right.x_max, far_right.y_max), (2.1, 2.1));
        assert_eq!((far_right.x_min, far_right.y_min), (0.0, 0.0));
    }
}
