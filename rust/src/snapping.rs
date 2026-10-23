//! Edge snapping while dragging a card and on-demand spatial grouping.
//!
//! Derived from DriftWM `layout/snap.rs` and `layout/cluster.rs` (GPL-3.0-or-later,
//! Copyright (C) 2026 Klim Kostiuk). Resize-only parts are dropped: cards have a
//! fixed size. Groups are never stored; a group is the connected component of
//! edge-adjacent cards at the current geometry, so nothing needs rebuilding when
//! a card moves, is removed or is restored from disk.

use std::collections::{HashSet, VecDeque};

use crate::workspace::{Rect, Vec2};

/// Bounding rectangle of a card in canvas coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapRect {
    pub x_low: f64,
    pub x_high: f64,
    pub y_low: f64,
    pub y_high: f64,
}

impl From<Rect> for SnapRect {
    fn from(r: Rect) -> Self {
        SnapRect {
            x_low: r.x,
            x_high: r.x_high(),
            y_low: r.y,
            y_high: r.y_high(),
        }
    }
}

pub struct SnapParams<'a> {
    pub extent: f64,
    pub perp_low: f64,
    pub perp_high: f64,
    pub horizontal: bool,
    pub others: &'a [SnapRect],
    pub gap: f64,
    pub threshold: f64,
    pub break_force: f64,
    pub same_edge: bool,
    pub edge_center: bool,
}

/// Snapped coordinate plus the natural position at the moment of engagement,
/// used for directional break detection.
#[derive(Clone, Copy, Debug)]
pub struct AxisSnap {
    pub snapped_pos: f64,
    pub natural_at_engage: f64,
}

#[derive(Default, Clone, Debug)]
pub struct SnapState {
    pub x: Option<AxisSnap>,
    pub y: Option<AxisSnap>,
    pub cooldown_x: Option<f64>,
    pub cooldown_y: Option<f64>,
}

fn try_candidate(best: &mut Option<(f64, f64)>, snap_pos: f64, dist: f64, threshold: f64) {
    if dist < threshold && best.is_none_or(|(_, bd)| dist < bd) {
        *best = Some((snap_pos, dist));
    }
}

fn perp_distance(lo1: f64, hi1: f64, lo2: f64, hi2: f64) -> f64 {
    if hi1 < lo2 {
        lo2 - hi1
    } else if hi2 < lo1 {
        lo1 - hi2
    } else {
        0.0
    }
}

/// Best snap candidate along one axis: opposite-edge docking (needs perpendicular
/// overlap), optional same-edge alignment and edge-center alignment (only for
/// perpendicular-adjacent cards). Returns `(snapped_origin, abs_distance)`.
pub fn find_snap_candidate(natural_edge_low: f64, p: &SnapParams<'_>) -> Option<(f64, f64)> {
    let natural_edge_high = natural_edge_low + p.extent;
    let mut best: Option<(f64, f64)> = None;

    for other in p.others {
        let (other_low, other_high, other_perp_low, other_perp_high) = if p.horizontal {
            (other.x_low, other.x_high, other.y_low, other.y_high)
        } else {
            (other.y_low, other.y_high, other.x_low, other.x_high)
        };

        let perp_overlap = p.perp_high > other_perp_low && other_perp_high > p.perp_low;
        let alignment_eligible = !perp_overlap
            && perp_distance(p.perp_low, p.perp_high, other_perp_low, other_perp_high)
                < p.gap + p.threshold;
        let same_edge_eligible = p.same_edge && alignment_eligible;
        let edge_center_eligible = p.edge_center && alignment_eligible;

        if !perp_overlap && !same_edge_eligible && !edge_center_eligible {
            continue;
        }

        if perp_overlap {
            try_candidate(
                &mut best,
                other_low - p.gap - p.extent,
                (natural_edge_high - other_low).abs(),
                p.threshold,
            );
            try_candidate(
                &mut best,
                other_high + p.gap,
                (natural_edge_low - other_high).abs(),
                p.threshold,
            );
        }

        if same_edge_eligible {
            try_candidate(
                &mut best,
                other_low,
                (natural_edge_low - other_low).abs(),
                p.threshold,
            );
            try_candidate(
                &mut best,
                other_high - p.extent,
                (natural_edge_high - other_high).abs(),
                p.threshold,
            );
        }

        if edge_center_eligible {
            let other_center = (other_low + other_high) / 2.0;
            try_candidate(
                &mut best,
                other_center - p.extent / 2.0,
                (natural_edge_low + p.extent / 2.0 - other_center).abs(),
                p.threshold,
            );
        }
    }

    best
}

/// Update snap state for one axis and return the final position on that axis.
pub fn update_axis(
    snap: &mut Option<AxisSnap>,
    cooldown: &mut Option<f64>,
    natural_pos: f64,
    p: &SnapParams<'_>,
) -> f64 {
    if let Some(s) = *snap {
        let (retreat, overshoot) = if s.snapped_pos > s.natural_at_engage {
            (s.natural_at_engage - natural_pos, natural_pos - s.snapped_pos)
        } else {
            (natural_pos - s.natural_at_engage, s.snapped_pos - natural_pos)
        };
        if retreat >= p.break_force || overshoot >= p.break_force {
            *cooldown = Some(s.snapped_pos);
            *snap = None;
            natural_pos
        } else {
            s.snapped_pos
        }
    } else {
        if let Some(cd) = *cooldown
            && (natural_pos - cd).abs() > p.threshold
        {
            *cooldown = None;
        }
        if cooldown.is_none()
            && let Some((snapped_pos, _)) = find_snap_candidate(natural_pos, p)
        {
            *snap = Some(AxisSnap {
                snapped_pos,
                natural_at_engage: natural_pos,
            });
            return snapped_pos;
        }
        natural_pos
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SnapConfig {
    pub gap: f64,
    pub distance: f64,
    pub break_force: f64,
    pub same_edge: bool,
    pub edge_center: bool,
}

/// Snap a dragged card's natural top-left against `others`. Thresholds are given
/// in screen px and divided by `zoom`, so the pull feels constant at any scale.
pub fn snap_move(
    natural: Vec2,
    size: Vec2,
    zoom: f64,
    others: &[SnapRect],
    cfg: &SnapConfig,
    state: &mut SnapState,
) -> Vec2 {
    let threshold = cfg.distance / zoom;
    let break_force = cfg.break_force / zoom;

    // Perpendicular range follows the visual card, not the raw finger: a held
    // snap lets the finger drift by up to break_force while the card stays put.
    let y_for_perp = state.y.map_or(natural.y, |s| s.snapped_pos);
    let px = SnapParams {
        extent: size.x,
        perp_low: y_for_perp,
        perp_high: y_for_perp + size.y,
        horizontal: true,
        others,
        gap: cfg.gap,
        threshold,
        break_force,
        same_edge: cfg.same_edge,
        edge_center: cfg.edge_center,
    };
    let x = update_axis(&mut state.x, &mut state.cooldown_x, natural.x, &px);

    let x_for_perp = state.x.map_or(natural.x, |s| s.snapped_pos);
    let py = SnapParams {
        extent: size.y,
        perp_low: x_for_perp,
        perp_high: x_for_perp + size.x,
        horizontal: false,
        others,
        gap: cfg.gap,
        threshold,
        break_force,
        same_edge: cfg.same_edge,
        edge_center: cfg.edge_center,
    };
    let y = update_axis(&mut state.y, &mut state.cooldown_y, natural.y, &py);
    Vec2::new(x, y)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// Which side of `a` is edge-adjacent to `b`: flush with `gap` on one axis and
/// strictly overlapping on the other. Corner touches don't count. `EPS` absorbs
/// whole-pixel drift.
pub fn adjacent_side(a: &SnapRect, b: &SnapRect, gap: f64) -> Option<Side> {
    const EPS: f64 = 1.0;
    let y_overlap = a.y_low < b.y_high && b.y_low < a.y_high;
    let x_overlap = a.x_low < b.x_high && b.x_low < a.x_high;

    if y_overlap {
        if ((a.x_high + gap) - b.x_low).abs() <= EPS {
            return Some(Side::Right);
        }
        if (a.x_low - (b.x_high + gap)).abs() <= EPS {
            return Some(Side::Left);
        }
    }
    if x_overlap {
        if ((a.y_high + gap) - b.y_low).abs() <= EPS {
            return Some(Side::Bottom);
        }
        if (a.y_low - (b.y_high + gap)).abs() <= EPS {
            return Some(Side::Top);
        }
    }
    None
}

/// Connected component of `root` over `adjacent_side`. Always contains `root`.
pub fn cluster_of(root: usize, rects: &[SnapRect], gap: f64) -> HashSet<usize> {
    let mut visited: HashSet<usize> = HashSet::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    if root >= rects.len() {
        return visited;
    }
    visited.insert(root);
    queue.push_back(root);
    while let Some(i) = queue.pop_front() {
        for (j, other) in rects.iter().enumerate() {
            if !visited.contains(&j) && adjacent_side(&rects[i], other, gap).is_some() {
                visited.insert(j);
                queue.push_back(j);
            }
        }
    }
    visited
}

/// All adjacent pairs `(i, j, side of i)` with `i < j`, and a group id per card
/// (cards sharing an id are in one group; singletons get unique ids).
pub fn adjacency(rects: &[SnapRect], gap: f64) -> (Vec<(usize, usize, Side)>, Vec<usize>) {
    let n = rects.len();
    let mut pairs = Vec::new();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..n {
        for j in (i + 1)..n {
            if let Some(side) = adjacent_side(&rects[i], &rects[j], gap) {
                pairs.push((i, j, side));
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    let groups = (0..n).map(|i| find(&mut parent, i)).collect();
    (pairs, groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> SnapRect {
        SnapRect {
            x_low: x,
            x_high: x + w,
            y_low: y,
            y_high: y + h,
        }
    }

    fn cfg() -> SnapConfig {
        SnapConfig {
            gap: 12.0,
            distance: 24.0,
            break_force: 32.0,
            same_edge: false,
            edge_center: false,
        }
    }

    #[test]
    fn adjacent_sides() {
        let a = rect(0.0, 0.0, 100.0, 100.0);
        assert_eq!(adjacent_side(&a, &rect(104.0, 20.0, 50.0, 50.0), 4.0), Some(Side::Right));
        assert_eq!(adjacent_side(&a, &rect(-54.0, 20.0, 50.0, 50.0), 4.0), Some(Side::Left));
        assert_eq!(adjacent_side(&a, &rect(20.0, 104.0, 60.0, 60.0), 4.0), Some(Side::Bottom));
        assert_eq!(adjacent_side(&a, &rect(20.0, -64.0, 60.0, 60.0), 4.0), Some(Side::Top));
    }

    #[test]
    fn corner_touch_is_not_adjacent() {
        let a = rect(0.0, 0.0, 100.0, 100.0);
        assert_eq!(adjacent_side(&a, &rect(104.0, 104.0, 50.0, 50.0), 4.0), None);
        assert_eq!(adjacent_side(&a, &rect(104.0, 100.0, 50.0, 50.0), 4.0), None);
    }

    #[test]
    fn wrong_gap_is_not_adjacent() {
        let a = rect(0.0, 0.0, 100.0, 100.0);
        assert_eq!(adjacent_side(&a, &rect(120.0, 0.0, 50.0, 50.0), 4.0), None);
        assert_eq!(adjacent_side(&a, &rect(103.5, 0.0, 50.0, 50.0), 4.0), Some(Side::Right));
    }

    #[test]
    fn cluster_follows_chain_not_islands() {
        let rects = vec![
            rect(0.0, 0.0, 100.0, 100.0),
            rect(104.0, 0.0, 100.0, 100.0),
            rect(208.0, 0.0, 100.0, 100.0),
            rect(900.0, 900.0, 100.0, 100.0),
        ];
        let c = cluster_of(0, &rects, 4.0);
        assert_eq!(c, HashSet::from([0, 1, 2]));
        assert_eq!(cluster_of(3, &rects, 4.0), HashSet::from([3]));
        assert!(cluster_of(9, &rects, 4.0).is_empty());
    }

    #[test]
    fn adjacency_groups() {
        let rects = vec![
            rect(0.0, 0.0, 100.0, 100.0),
            rect(104.0, 0.0, 100.0, 100.0),
            rect(500.0, 0.0, 100.0, 100.0),
        ];
        let (pairs, groups) = adjacency(&rects, 4.0);
        assert_eq!(pairs.len(), 1);
        assert_eq!(groups[0], groups[1]);
        assert_ne!(groups[0], groups[2]);
    }

    #[test]
    fn snaps_to_neighbor_edge_with_gap() {
        let other = [rect(0.0, 0.0, 100.0, 100.0)];
        let size = Vec2::new(100.0, 100.0);
        let mut st = SnapState::default();
        // Natural left edge 10 px from the gap position (112).
        let p = snap_move(Vec2::new(122.0, 5.0), size, 1.0, &other, &cfg(), &mut st);
        assert_eq!(p.x, 112.0);
        assert_eq!(p.y, 5.0);
        assert!(st.x.is_some());
    }

    #[test]
    fn no_snap_outside_threshold() {
        let other = [rect(0.0, 0.0, 100.0, 100.0)];
        let mut st = SnapState::default();
        let p = snap_move(Vec2::new(150.0, 5.0), Vec2::new(100.0, 100.0), 1.0, &other, &cfg(), &mut st);
        assert_eq!(p.x, 150.0);
        assert!(st.x.is_none());
    }

    #[test]
    fn no_snap_without_perpendicular_overlap() {
        let other = [rect(0.0, 0.0, 100.0, 100.0)];
        let mut st = SnapState::default();
        let p = snap_move(Vec2::new(115.0, 300.0), Vec2::new(100.0, 100.0), 1.0, &other, &cfg(), &mut st);
        assert_eq!(p.x, 115.0);
    }

    #[test]
    fn snap_holds_then_breaks() {
        let other = [rect(0.0, 0.0, 100.0, 100.0)];
        let size = Vec2::new(100.0, 100.0);
        let mut st = SnapState::default();
        snap_move(Vec2::new(120.0, 0.0), size, 1.0, &other, &cfg(), &mut st);
        let held = snap_move(Vec2::new(130.0, 0.0), size, 1.0, &other, &cfg(), &mut st);
        assert_eq!(held.x, 112.0);
        let broken = snap_move(Vec2::new(160.0, 0.0), size, 1.0, &other, &cfg(), &mut st);
        assert_eq!(broken.x, 160.0);
        assert!(st.x.is_none());
        // Cooldown prevents instant re-engagement next to the old snap point.
        let again = snap_move(Vec2::new(125.0, 0.0), size, 1.0, &other, &cfg(), &mut st);
        assert_eq!(again.x, 125.0);
    }

    #[test]
    fn break_by_retreating() {
        let other = [rect(0.0, 0.0, 100.0, 100.0)];
        let size = Vec2::new(100.0, 100.0);
        let mut st = SnapState::default();
        snap_move(Vec2::new(120.0, 0.0), size, 1.0, &other, &cfg(), &mut st);
        let away = snap_move(Vec2::new(120.0 + 35.0, 0.0), size, 1.0, &other, &cfg(), &mut st);
        assert_eq!(away.x, 155.0);
    }

    #[test]
    fn threshold_scales_with_zoom() {
        let other = [rect(0.0, 0.0, 100.0, 100.0)];
        let size = Vec2::new(100.0, 100.0);
        // Edge is 40 canvas px from the neighbor: outside 24 at zoom 1, inside 24/0.5 = 48 at zoom 0.5.
        let mut a = SnapState::default();
        let p1 = snap_move(Vec2::new(140.0, 0.0), size, 1.0, &other, &cfg(), &mut a);
        assert_eq!(p1.x, 140.0);
        let mut b = SnapState::default();
        let p2 = snap_move(Vec2::new(140.0, 0.0), size, 0.5, &other, &cfg(), &mut b);
        assert_eq!(p2.x, 112.0);
    }

    #[test]
    fn same_edge_alignment_only_when_enabled() {
        // Dragged card sits diagonal-adjacent: no perp overlap, small perp distance.
        let other = [rect(0.0, 0.0, 100.0, 100.0)];
        let size = Vec2::new(100.0, 100.0);
        let mut c = cfg();
        c.same_edge = true;
        let mut st = SnapState::default();
        let p = snap_move(Vec2::new(8.0, 108.0), size, 1.0, &other, &c, &mut st);
        assert_eq!(p.x, 0.0);
        let mut st2 = SnapState::default();
        let q = snap_move(Vec2::new(8.0, 108.0), size, 1.0, &other, &cfg(), &mut st2);
        assert_eq!(q.x, 8.0);
    }

    #[test]
    fn snapped_result_forms_a_group() {
        let others = [rect(0.0, 0.0, 100.0, 100.0)];
        let mut st = SnapState::default();
        let p = snap_move(Vec2::new(118.0, 10.0), Vec2::new(100.0, 100.0), 1.0, &others, &cfg(), &mut st);
        let rects = vec![others[0], rect(p.x, p.y, 100.0, 100.0)];
        assert_eq!(cluster_of(0, &rects, 12.0).len(), 2);
    }
}
