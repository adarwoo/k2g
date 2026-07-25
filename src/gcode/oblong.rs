//! Oblong-slot drill decomposition — where the drills go when a slot is made by
//! drilling rather than milling (op-planner §3, the `drill_chain*` and
//! `drill_ends_then_route` strategies).
//!
//! **The chain drill is the slot width.** The holes *are* the slot walls, so the drill
//! the assigner picks for the oblong's minor axis is the one that walks the major axis.
//! That makes the spacing naturally a fraction of the drill diameter.
//!
//! **The pitch is derived, not fixed.** A fixed pitch would leave the last hole short of
//! the slot end. Instead the two constants below are *ceilings*: the travel is divided
//! into `ceil(travel / max_pitch)` equal steps, so the end holes land exactly on the end
//! centres and the real pitch is never coarser than the ceiling. Following
//! [`crate::gcode::routing`], the ceilings are fixed constants with no user option —
//! they follow from the strategy, and a wrong value here is a broken drill, not a
//! preference.
//!
//! **The order is a bisection, not a sweep.** Drilling left-to-right makes every plunge
//! after the first bite into the open edge of its neighbour, with material on one side
//! and void on the other; the bit deflects toward the void, which on the sub-millimetre
//! drills slots need is a broken tool. [`chain_order`] drills the two ends and then
//! always halves the widest remaining gap, so every later hole lands *between* two
//! already-drilled ones — engaged symmetrically whether the web is thin enough to cut
//! or still full material.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use pcb::{BoardPoint, Slot};
use units::Length;

/// Pitch ceiling, as a fraction of the drill diameter, when the chain **is** the
/// finished wall (`drill_chain`). The scallop left between two holes of diameter `d`
/// spaced `p` apart is `d/2 − √((d/2)² − (p/2)²)`; at 0.40 that is ~4% of `d`, which is
/// as close to a milled wall as drilling gets without doubling the hole count again.
pub const CHAIN_PITCH_FINISH: f64 = 0.40;

/// Pitch ceiling when a router cleans up afterwards (`drill_chain_then_route`). The
/// chain only has to remove bulk, so this is as coarse as it can be while still leaving
/// the router a light, even finishing cut (~17% of `d` radially) instead of interrupted
/// full-width bites.
pub const CHAIN_PITCH_ROUGH: f64 = 0.75;

/// Offsets along the slot's long axis, from the centre, for a chain of drills.
///
/// The first and last are always the slot's end centres (`±travel/2`), so the chain
/// reaches the full length whatever the pitch works out to. A slot whose travel is zero
/// or whose drill covers it in one step yields the two ends alone; a degenerate drill
/// diameter yields just the ends rather than dividing by zero.
pub fn chain_offsets(travel: Length, drill_diameter: Length, max_pitch_fraction: f64) -> Vec<Length> {
    let travel_mm = travel.as_mm();
    let max_pitch_mm = drill_diameter.as_mm() * max_pitch_fraction;
    let half = travel_mm / 2.0;

    if travel_mm <= 0.0 {
        return vec![Length::from_mm(0.0)];
    }
    if max_pitch_mm <= 0.0 {
        return vec![Length::from_mm(-half), Length::from_mm(half)];
    }

    // Equal steps, never coarser than the ceiling — so both ends land exactly.
    let steps = (travel_mm / max_pitch_mm).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|i| Length::from_mm(-half + travel_mm * (i as f64) / (steps as f64)))
        .collect()
}

/// The order to drill `count` chained positions: both ends, then repeatedly the middle
/// of the widest remaining gap. Returns indices into [`chain_offsets`]' output.
///
/// The invariant this buys is that every hole after the first two has an already-drilled
/// hole on *each* side, so the bit is loaded symmetrically — either it is cutting a thin
/// web with support both sides, or it is in full material. A plain sweep instead leaves
/// every plunge half-over the previous hole, unsupported on that side.
///
/// Alternate-then-fill-in is not enough: its fill-in pass drops each hole beside a
/// single drilled neighbour, which is exactly the asymmetric case to avoid.
pub fn chain_order(count: usize) -> Vec<usize> {
    if count <= 2 {
        return (0..count).collect();
    }
    let mut order = Vec::with_capacity(count);
    order.push(0);
    order.push(count - 1);

    // Widest gap first, ties to the leftmost — deterministic, and it keeps drilled holes
    // as far apart as possible for as long as possible.
    let mut gaps = BinaryHeap::new();
    gaps.push((count - 1, Reverse(0usize)));
    while let Some((span, Reverse(lo))) = gaps.pop() {
        if span < 2 {
            continue;
        }
        let mid = lo + span / 2;
        order.push(mid);
        gaps.push((mid - lo, Reverse(lo)));
        gaps.push((lo + span - mid, Reverse(mid)));
    }
    order
}

/// The board-coordinate drill positions for one slot, already in drilling order.
///
/// `max_pitch_fraction` selects the strategy's ceiling ([`CHAIN_PITCH_FINISH`] or
/// [`CHAIN_PITCH_ROUGH`]); pass `None` for `drill_ends_then_route`, which drills only
/// the two end centres and leaves the web to the router.
pub fn chain_positions(
    slot: &Slot,
    drill_diameter: Length,
    max_pitch_fraction: Option<f64>,
) -> Vec<BoardPoint> {
    let offsets = match max_pitch_fraction {
        Some(fraction) => chain_offsets(slot.travel(), drill_diameter, fraction),
        // Ends only: a chain of exactly one step.
        None => chain_offsets(slot.travel(), slot.travel(), 1.0),
    };
    chain_order(offsets.len())
        .into_iter()
        .map(|i| slot.point_at(offsets[i]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mm(v: f64) -> Length {
        Length::from_mm(v)
    }

    fn offsets_mm(travel_mm: f64, drill_mm: f64, fraction: f64) -> Vec<f64> {
        chain_offsets(mm(travel_mm), mm(drill_mm), fraction)
            .into_iter()
            .map(|l| l.as_mm())
            .collect()
    }

    /// The chain must span the whole slot: the outermost holes sit exactly on the end
    /// centres, whatever the pitch rounds to.
    #[test]
    fn the_end_holes_land_exactly_on_the_slot_ends() {
        // 3.0 × 0.4 mm slot → travel 2.6 mm, ceiling 0.16 mm.
        let offsets = offsets_mm(2.6, 0.4, CHAIN_PITCH_FINISH);
        assert!((offsets[0] + 1.3).abs() < 1e-9, "first hole at -travel/2: {offsets:?}");
        assert!((offsets[offsets.len() - 1] - 1.3).abs() < 1e-9, "last hole at +travel/2");
    }

    /// The derived pitch is uniform and never exceeds the ceiling — that is the whole
    /// point of dividing rather than stepping.
    #[test]
    fn the_pitch_is_uniform_and_within_the_ceiling() {
        let offsets = offsets_mm(2.6, 0.4, CHAIN_PITCH_FINISH);
        let ceiling = 0.4 * CHAIN_PITCH_FINISH;
        let first_gap = offsets[1] - offsets[0];
        for pair in offsets.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(gap <= ceiling + 1e-9, "gap {gap} exceeds ceiling {ceiling}");
            assert!((gap - first_gap).abs() < 1e-9, "gaps are equal: {offsets:?}");
        }
        // 2.6 / 0.16 = 16.25 → 17 steps → 18 holes.
        assert_eq!(offsets.len(), 18);
    }

    /// The rough ceiling drills materially fewer holes than the finish one — the reason
    /// the two strategies have different constants.
    #[test]
    fn the_rough_ceiling_needs_fewer_holes_than_the_finish_one() {
        let rough = offsets_mm(2.6, 0.4, CHAIN_PITCH_ROUGH);
        let finish = offsets_mm(2.6, 0.4, CHAIN_PITCH_FINISH);
        assert_eq!(rough.len(), 10, "2.6 / 0.30 = 8.67 → 9 steps → 10 holes");
        assert!(rough.len() < finish.len());
    }

    /// Holes always overlap: the derived pitch is at most 0.75 d, so consecutive holes
    /// of diameter d always intersect and the chain is a continuous channel.
    #[test]
    fn consecutive_holes_always_overlap() {
        for (travel, drill, fraction) in
            [(2.6, 0.4, CHAIN_PITCH_ROUGH), (2.6, 0.4, CHAIN_PITCH_FINISH), (0.05, 0.4, CHAIN_PITCH_ROUGH)]
        {
            let offsets = offsets_mm(travel, drill, fraction);
            for pair in offsets.windows(2) {
                assert!(pair[1] - pair[0] < drill, "gap must be under one diameter");
            }
        }
    }

    /// A drill that already covers the travel still gives both ends — one step, two
    /// holes — never a single hole short of the slot.
    #[test]
    fn a_short_slot_still_gets_both_ends() {
        let offsets = offsets_mm(0.05, 0.4, CHAIN_PITCH_ROUGH);
        assert_eq!(offsets.len(), 2);
        assert!((offsets[0] + 0.025).abs() < 1e-9 && (offsets[1] - 0.025).abs() < 1e-9);
    }

    /// Degenerate inputs produce a point, not a panic or a divide-by-zero.
    #[test]
    fn degenerate_slots_do_not_divide_by_zero() {
        assert_eq!(offsets_mm(0.0, 0.4, CHAIN_PITCH_ROUGH), vec![0.0]);
        assert_eq!(offsets_mm(2.0, 0.0, CHAIN_PITCH_ROUGH), vec![-1.0, 1.0]);
    }

    /// The load-symmetry invariant: after the two ends, every hole is drilled with an
    /// already-drilled hole on both sides, so the bit is never unsupported on one flank.
    #[test]
    fn every_hole_after_the_ends_is_drilled_between_two_already_drilled_ones() {
        for count in 3..20 {
            let order = chain_order(count);
            assert_eq!(order.len(), count, "every position is drilled exactly once");
            let mut sorted = order.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, (0..count).collect::<Vec<_>>(), "no duplicates or gaps");
            assert_eq!(&order[..2], &[0, count - 1], "both ends go first");

            let mut drilled = vec![order[0], order[1]];
            for &position in &order[2..] {
                assert!(
                    drilled.iter().any(|&d| d < position),
                    "position {position} has no drilled hole below it: {order:?}"
                );
                assert!(
                    drilled.iter().any(|&d| d > position),
                    "position {position} has no drilled hole above it: {order:?}"
                );
                drilled.push(position);
            }
        }
    }

    /// One or two holes have no interior to bisect — they are drilled as they lie.
    #[test]
    fn a_one_or_two_hole_chain_is_drilled_in_place() {
        assert_eq!(chain_order(1), vec![0]);
        assert_eq!(chain_order(2), vec![0, 1]);
    }

    /// Positions are laid along the slot's own axis: a 90° slot walks in Y, not X.
    #[test]
    fn positions_follow_the_slot_axis() {
        let slot = Slot {
            center_x: mm(10.0),
            center_y: mm(20.0),
            length: mm(3.0),
            width: mm(0.4),
            angle_deg: 90.0,
        };
        let points = chain_positions(&slot, mm(0.4), Some(CHAIN_PITCH_ROUGH));
        for point in &points {
            assert!((point.x.as_mm() - 10.0).abs() < 1e-9, "a 90° slot holds X constant");
        }
        let ys: Vec<f64> = points.iter().map(|p| p.y.as_mm()).collect();
        let min = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!((min - (20.0 - 1.3)).abs() < 1e-9 && (max - (20.0 + 1.3)).abs() < 1e-9);
    }

    /// `drill_ends_then_route` drills the two end centres and nothing between.
    #[test]
    fn the_ends_only_strategy_drills_exactly_two_holes() {
        let slot = Slot {
            center_x: mm(0.0),
            center_y: mm(0.0),
            length: mm(3.0),
            width: mm(0.4),
            angle_deg: 0.0,
        };
        let points = chain_positions(&slot, mm(0.4), None);
        assert_eq!(points.len(), 2);
        let xs: Vec<f64> = points.iter().map(|p| p.x.as_mm()).collect();
        assert!(xs.iter().any(|x| (x + 1.3).abs() < 1e-9));
        assert!(xs.iter().any(|x| (x - 1.3).abs() < 1e-9));
    }
}
