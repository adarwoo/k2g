//! Regions of the board plane, and the boolean algebra over them.
//!
//! A **region** is a set of rings: outlines and the holes that pierce them, in board
//! nanometres, with containment — not winding — deciding which is which. Everything here
//! is a total function of its input, because two runs on the same board have to produce
//! the same program.
//!
//! This is deliberately thin. Clipper does the work; these names exist so the callers read
//! as geometry rather than as library calls, and so the one place that decides a fill rule
//! is this one. [`isolation`](crate::isolation) grew all of it and was its only caller
//! until [`clearing`](crate::clearing) needed the same algebra over the *complement* of
//! the copper — the same rings, asked the opposite question.

use clipper2_rust::{
    clipper::{difference_64, intersect_64, union_64},
    core::{FillRule, Path64, Paths64, Point64},
};

use crate::copper::Polygon;

/// One KiCad polygon as a properly wound region.
///
/// KiCad does not promise a winding, and a hole ring drawn the same way round as its
/// outline would fill rather than pierce under any winding rule. Differencing the holes
/// out asks Clipper to settle it, and its output is oriented the way everything downstream
/// assumes.
pub(crate) fn polygon_region(polygon: &Polygon) -> Vec<Ring> {
    if polygon.outline.len() < 3 {
        return Vec::new();
    }
    let outline = vec![polygon.outline.clone()];
    let holes: Vec<Ring> = polygon.holes.iter().filter(|h| h.len() >= 3).cloned().collect();
    if holes.is_empty() {
        union(&outline, &[])
    } else {
        difference(&outline, &holes)
    }
}

/// A closed ring of board-nanometre points. Whether it bounds copper or pierces it is a
/// question about what contains it, answered by [`components`].
pub(crate) type Ring = Vec<(i64, i64)>;

/// Splits a region into its connected pieces, each with the holes that belong to it.
///
/// Containment counting rather than winding: a ring inside an odd number of others is a
/// hole. That holds whatever orientation the rings arrive in, and the alternative — trust
/// the sign of the area — is one library convention away from silently pairing a hole with
/// the wrong island.
pub(crate) fn components(region: &[Ring]) -> Vec<Vec<Ring>> {
    let rings: Vec<&Ring> = region.iter().filter(|r| r.len() >= 3).collect();
    let depth: Vec<usize> = rings
        .iter()
        .map(|ring| {
            rings
                .iter()
                .filter(|other| !std::ptr::eq(*other, ring) && point_in_ring(ring[0], other))
                .count()
        })
        .collect();

    let mut out: Vec<Vec<Ring>> = Vec::new();
    let mut index_of: Vec<Option<usize>> = vec![None; rings.len()];
    for (i, ring) in rings.iter().enumerate() {
        if depth[i].is_multiple_of(2) {
            index_of[i] = Some(out.len());
            out.push(vec![(*ring).clone()]);
        }
    }
    for (i, ring) in rings.iter().enumerate() {
        if depth[i].is_multiple_of(2) {
            continue;
        }
        // The hole belongs to the smallest island that contains it — nesting means several
        // do, and only the innermost is its own.
        let owner = rings
            .iter()
            .enumerate()
            .filter(|(j, other)| {
                depth[*j].is_multiple_of(2) && *j != i && point_in_ring(ring[0], other)
            })
            .min_by_key(|(_, other)| area_nm2(other).unsigned_abs())
            .and_then(|(j, _)| index_of[j]);
        if let Some(owner) = owner {
            out[owner].push((*ring).clone());
        }
    }
    out
}

pub(crate) fn to_paths(rings: &[Ring]) -> Paths64 {
    rings
        .iter()
        .filter(|r| r.len() >= 3)
        .map(|r| r.iter().map(|&(x, y)| Point64 { x, y }).collect::<Path64>())
        .collect()
}

pub(crate) fn from_paths(paths: &Paths64) -> Vec<Ring> {
    paths
        .iter()
        .map(|p| p.iter().map(|pt| (pt.x, pt.y)).collect::<Ring>())
        .filter(|r: &Ring| r.len() >= 2)
        .collect()
}

pub(crate) fn union(a: &[Ring], b: &[Ring]) -> Vec<Ring> {
    let (a, b) = (to_paths(a), to_paths(b));
    if a.is_empty() && b.is_empty() {
        return Vec::new();
    }
    from_paths(&union_64(&a, &b, FillRule::NonZero))
}

pub(crate) fn difference(a: &[Ring], b: &[Ring]) -> Vec<Ring> {
    let (a, b) = (to_paths(a), to_paths(b));
    if a.is_empty() {
        return Vec::new();
    }
    from_paths(&difference_64(&a, &b, FillRule::NonZero))
}

pub(crate) fn intersect(a: &[Ring], b: &[Ring]) -> Vec<Ring> {
    let (a, b) = (to_paths(a), to_paths(b));
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    from_paths(&intersect_64(&a, &b, FillRule::NonZero))
}

pub(crate) fn area_nm2(ring: &[(i64, i64)]) -> i128 {
    let n = ring.len();
    let mut sum: i128 = 0;
    for i in 0..n {
        let (x0, y0) = ring[i];
        let (x1, y1) = ring[(i + 1) % n];
        sum += (x0 as i128) * (y1 as i128) - (x1 as i128) * (y0 as i128);
    }
    sum / 2
}

pub(crate) fn point_in_ring(point: (i64, i64), ring: &[(i64, i64)]) -> bool {
    let (px, py) = (point.0 as i128, point.1 as i128);
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let (x0, y0) = (ring[i].0 as i128, ring[i].1 as i128);
        let (x1, y1) = (ring[(i + 1) % n].0 as i128, ring[(i + 1) % n].1 as i128);
        if (y0 > py) != (y1 > py) {
            let cross = (x1 - x0) * (py - y0) - (px - x0) * (y1 - y0);
            if (cross > 0) == (y1 > y0) {
                inside = !inside;
            }
        }
    }
    inside
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BBox {
    pub(crate) x0: i64,
    pub(crate) y0: i64,
    pub(crate) x1: i64,
    pub(crate) y1: i64,
}

impl BBox {
    pub(crate) fn of(rings: &[Ring]) -> Option<BBox> {
        let mut bbox: Option<BBox> = None;
        for &(x, y) in rings.iter().flatten() {
            bbox = Some(match bbox {
                None => BBox { x0: x, y0: y, x1: x, y1: y },
                Some(b) => BBox {
                    x0: b.x0.min(x),
                    y0: b.y0.min(y),
                    x1: b.x1.max(x),
                    y1: b.y1.max(y),
                },
            });
        }
        bbox
    }

    pub(crate) fn expand(self, by: i64) -> BBox {
        BBox {
            x0: self.x0.saturating_sub(by),
            y0: self.y0.saturating_sub(by),
            x1: self.x1.saturating_add(by),
            y1: self.y1.saturating_add(by),
        }
    }

    pub(crate) fn overlaps(self, other: BBox) -> bool {
        self.x0 <= other.x1 && other.x0 <= self.x1 && self.y0 <= other.y1 && other.y0 <= self.y1
    }

    pub(crate) fn ring(self) -> Ring {
        vec![
            (self.x0, self.y0),
            (self.x1, self.y0),
            (self.x1, self.y1),
            (self.x0, self.y1),
        ]
    }
}
