//! Curve-preserving path operations, on BezPaths: booleans that keep curves as curves (so a
//! welded circle is still a handful of arcs, not hundreds of tiny lines), and the areas the
//! Shape Builder works on.
//!
//! The `linesweeper` crate sweeps the input outlines, splitting them wherever they cross, and
//! tells us how many times each input winds round each side of every piece. From that we pick
//! the outlines of any combination of the inputs. The sweep also splits curves where they turn
//! up or down, so afterwards the pieces of each input curve are joined back into one curve.

use crate::ops::BooleanOp;
use crate::{BezPath, PathEl, Point, Rect};
use kurbo::{CubicBez, ParamCurve, ParamCurveNearest, PathSeg, Shape as _};
use linesweeper::topology::{Topology, WindingNumber};
use std::collections::BTreeSet;

/// Most curve and line pieces swept at once; past that the caller combines polygons instead.
const MAX_SEGMENTS: usize = 40_000;
/// Outlines narrower than this on average (mm) are slivers left where edges nearly meet.
const SLIVER: f64 = 0.005;

/// Only the closed subpaths of `path` (open lines have no area to combine), each ending with a
/// close.
pub fn closed_only(path: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    let mut sub: Vec<PathEl> = Vec::new();
    let flush = |sub: &mut Vec<PathEl>, out: &mut BezPath| {
        let start = match sub.first() {
            Some(PathEl::MoveTo(p)) => Some(*p),
            _ => None,
        };
        let explicit = matches!(sub.last(), Some(PathEl::ClosePath));
        let meets = sub.iter().rev().find_map(|el| el.end_point()).zip(start).is_some_and(|(e, s)| e.distance(s) < 1e-6);
        if start.is_some() && sub.len() > 2 && (explicit || meets) {
            sub.iter().for_each(|el| out.push(*el));
            if !explicit {
                out.close_path();
            }
        }
        sub.clear();
    };
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) {
            flush(&mut sub, &mut out);
        }
        sub.push(*el);
    }
    flush(&mut sub, &mut out);
    out
}

/// Coordinates are snapped to this grid (mm) before sweeping, so edges that should meet do meet
/// exactly: rounding (such as `sin(π)` not being quite 0) otherwise leaves them a hair apart, and
/// the sweep keeps them as separate edges.
const GRID: f64 = 1e-6;

/// `path` with every point on the [`GRID`].
fn snapped(path: &BezPath) -> BezPath {
    let s = |p: Point| Point::new((p.x / GRID).round() * GRID, (p.y / GRID).round() * GRID);
    path.elements()
        .iter()
        .map(|el| match *el {
            PathEl::MoveTo(a) => PathEl::MoveTo(s(a)),
            PathEl::LineTo(a) => PathEl::LineTo(s(a)),
            PathEl::QuadTo(a, b) => PathEl::QuadTo(s(a), s(b)),
            PathEl::CurveTo(a, b, c) => PathEl::CurveTo(s(a), s(b), s(c)),
            PathEl::ClosePath => PathEl::ClosePath,
        })
        .collect()
}

/// The closed subpaths of `path`, snapped, ready to sweep.
fn prepared(path: &BezPath) -> BezPath {
    snapped(&closed_only(path))
}

/// How many times each input winds round a point, by input (missing ones are 0; kept without
/// trailing zeros so equal coverings compare equal).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Winding(Vec<i32>);

impl Winding {
    fn trimmed(mut self) -> Self {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
        self
    }
    /// Is the point inside input `i` (even-odd, as BurnCraft fills shapes)?
    fn inside(&self, i: usize) -> bool {
        self.0.get(i).is_some_and(|w| w % 2 != 0)
    }
    /// Which of `n` inputs cover the point.
    fn cover(&self, n: usize) -> Vec<bool> {
        (0..n).map(|i| self.inside(i)).collect()
    }
}

impl std::ops::AddAssign for Winding {
    fn add_assign(&mut self, other: Self) {
        if self.0.len() < other.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a += b;
        }
        *self = std::mem::take(self).trimmed();
    }
}

impl std::ops::Add for Winding {
    type Output = Self;
    fn add(mut self, other: Self) -> Self {
        self += other;
        self
    }
}

impl WindingNumber for Winding {
    type Tag = usize;
    fn single(tag: usize, positive: bool) -> Self {
        let mut v = vec![0; tag + 1];
        v[tag] = if positive { 1 } else { -1 };
        Winding(v)
    }
    fn of_tag(&self, tag: usize) -> Self {
        let mut v = vec![0; tag + 1];
        v[tag] = self.0.get(tag).copied().unwrap_or(0);
        Winding(v).trimmed()
    }
}

/// Sweeps the (closed) inputs together; `None` when they're too big, not finite, or the sweep
/// fails on near-degenerate input (it can panic; that must never take the app down).
fn sweep(inputs: &[BezPath]) -> Option<Topology<Winding>> {
    let segments: usize = inputs.iter().map(|p| p.segments().count()).sum();
    if segments > MAX_SEGMENTS {
        return None;
    }
    let mut reach: f64 = 0.0;
    for p in inputs {
        let b = p.bounding_box();
        for v in [b.x0, b.y0, b.x1, b.y1] {
            if !v.is_finite() {
                return None;
            }
            reach = reach.max(v.abs());
        }
    }
    let eps = (reach * f64::EPSILON * 64.0).max(1e-6);
    let swept = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Topology::<Winding>::from_paths(inputs.iter().enumerate().map(|(i, p)| (p, i)), eps)));
    swept.ok()?.ok()
}

/// The input curves, as cubics with their bounds, to join the swept pieces back onto.
struct Sources(Vec<(Rect, CubicBez)>);

impl Sources {
    fn of(inputs: &[BezPath]) -> Self {
        let curves = inputs
            .iter()
            .flat_map(|p| p.segments())
            .filter_map(|s| match s {
                PathSeg::Line(_) => None,
                PathSeg::Quad(q) => Some(q.raise()),
                PathSeg::Cubic(c) => Some(c),
            })
            .map(|c| (c.bounding_box().inflate(1e-4, 1e-4), c))
            .collect();
        Sources(curves)
    }

    /// The input curve `seg` lies on, and where along it (parameters at its two ends).
    fn locate(&self, seg: &CubicBez) -> Option<(usize, f64, f64)> {
        let tol = 1e-5 * (1.0 + seg.p0.x.abs().max(seg.p0.y.abs()));
        let (a, b, mid) = (seg.p0, seg.p3, seg.eval(0.5));
        self.0.iter().enumerate().find_map(|(i, (bounds, c))| {
            if !bounds.contains(a) || !bounds.contains(b) {
                return None;
            }
            let (na, nb) = (c.nearest(a, 1e-10), c.nearest(b, 1e-10));
            let on = na.distance_sq.sqrt() < tol && nb.distance_sq.sqrt() < tol && c.nearest(mid, 1e-10).distance_sq.sqrt() < tol * 20.0;
            (on && (na.t - nb.t).abs() > 1e-9).then_some((i, na.t, nb.t))
        })
    }
}

/// One piece of a swept outline: a straight line, a piece of input curve `src` from `t0` to
/// `t1`, or a curve we can't place.
#[derive(Clone, Copy)]
enum Piece {
    Line(Point, Point),
    OnSource { src: usize, t0: f64, t1: f64, from: Point, to: Point },
    Free(CubicBez),
}

/// A swept outline with the pieces split from one input curve joined back into it, and runs of
/// straight pieces in line joined into one line.
fn rejoin(path: &BezPath, sources: &Sources) -> BezPath {
    let mut pieces: Vec<Piece> = Vec::new();
    for seg in path.segments() {
        let piece = match seg {
            PathSeg::Line(l) => Piece::Line(l.p0, l.p1),
            PathSeg::Quad(q) => place(q.raise(), sources),
            PathSeg::Cubic(c) => place(c, sources),
        };
        // Join onto the previous piece where it continues it.
        let joined = match (pieces.last_mut(), piece) {
            (Some(Piece::Line(a, b)), Piece::Line(_, c)) => {
                let (u, v) = (*b - *a, c - *b);
                let straight = u.cross(v).abs() <= 1e-9 * u.hypot() * v.hypot() + 1e-12 && u.dot(v) > 0.0;
                if straight {
                    *b = c;
                }
                straight
            }
            (Some(Piece::OnSource { src, t1, to, .. }), Piece::OnSource { src: s2, t0: u0, t1: u1, to: e, .. }) if *src == s2 && (*t1 - u0).abs() < 1e-6 => {
                *t1 = u1;
                *to = e;
                true
            }
            _ => false,
        };
        if !joined {
            pieces.push(piece);
        }
    }
    let mut out = BezPath::new();
    let Some(start) = path.elements().first().and_then(|el| el.end_point()) else { return out };
    out.move_to(start);
    for p in pieces {
        match p {
            Piece::Line(_, b) => out.line_to(b),
            Piece::Free(c) => out.curve_to(c.p1, c.p2, c.p3),
            Piece::OnSource { src, t0, t1, from, to } => {
                let c = sources.0[src].1.subsegment(t0.min(t1)..t0.max(t1));
                let c = if t0 <= t1 { c } else { CubicBez::new(c.p3, c.p2, c.p1, c.p0) };
                // The ends exactly where the sweep put them, so pieces still meet.
                let _ = from;
                out.curve_to(c.p1, c.p2, to);
            }
        }
    }
    out.close_path();
    out
}

fn place(c: CubicBez, sources: &Sources) -> Piece {
    match sources.locate(&c) {
        Some((src, t0, t1)) => Piece::OnSource { src, t0, t1, from: c.p0, to: c.p3 },
        None => Piece::Free(c),
    }
}

/// Whether an outline is a sliver: average width (2 × area / perimeter) below [`SLIVER`].
fn is_sliver(p: &BezPath) -> bool {
    let perimeter = p.perimeter(1e-4);
    perimeter <= 0.0 || 2.0 * p.area().abs() / perimeter < SLIVER
}

/// The outlines of the swept area where `keep` holds, joined back onto `sources`.
fn outlines(top: &Topology<Winding>, sources: &Sources, keep: impl Fn(&Winding) -> bool) -> BezPath {
    let mut out = BezPath::new();
    for c in top.contours(keep).contours() {
        let p = rejoin(&c.path, sources);
        if !is_sliver(&p) {
            out.extend(p);
        }
    }
    out
}

/// Combines the filled areas of `paths` (in order: the first minus the rest for Subtract),
/// keeping curves. `None` with fewer than two closed paths, or when the shapes can't be combined
/// this way (too complex, or degenerate), so the caller can fall back.
pub fn boolean(paths: &[BezPath], op: BooleanOp) -> Option<BezPath> {
    let inputs: Vec<BezPath> = paths.iter().map(prepared).filter(|p| !p.elements().is_empty()).collect();
    let n = inputs.len();
    if n < 2 {
        return None;
    }
    let top = sweep(&inputs)?;
    let sources = Sources::of(&inputs);
    Some(outlines(&top, &sources, |w| match op {
        BooleanOp::Union => (0..n).any(|i| w.inside(i)),
        BooleanOp::Subtract => w.inside(0) && !(1..n).any(|i| w.inside(i)),
        BooleanOp::Intersect => (0..n).all(|i| w.inside(i)),
    }))
}

/// One area of the Shape Builder: its outline, and which inputs cover it.
struct Area {
    outline: BezPath,
    cover: Vec<bool>,
}

impl Area {
    /// The front-most input covering it (later inputs are in front).
    fn top(&self) -> Option<usize> {
        self.cover.iter().rposition(|&c| c)
    }
}

/// The Shape Builder's view of a set of shapes: every area their outlines enclose (overlaps
/// split apart), which can be merged into one shape or deleted.
pub struct Builder {
    areas: Vec<Area>,
    outlines: Vec<BezPath>,
    sources: Sources,
    count: usize,
}

/// What a Shape Builder step leaves: the new merged shape (with the input it takes its layer
/// from), and what remains of each input.
pub struct Built {
    pub merged: Option<(BezPath, usize)>,
    pub rest: Vec<(usize, BezPath)>,
}

impl Builder {
    /// `None` when there is nothing closed to build from.
    pub fn new(paths: &[BezPath]) -> Option<Self> {
        let inputs: Vec<BezPath> = paths.iter().map(prepared).collect();
        let n = inputs.len();
        if inputs.iter().all(|p| p.elements().is_empty()) {
            return None;
        }
        let top = sweep(&inputs)?;
        let sources = Sources::of(&inputs);
        // Every distinct covering found beside a swept edge; each is one or more separate areas.
        let mut covers: BTreeSet<Vec<bool>> = BTreeSet::new();
        for s in top.segment_indices() {
            let half = s.first_half();
            for w in [top.winding_clockwise(half), top.winding_counter_clockwise(half)] {
                let cover = w.cover(n);
                if cover.iter().any(|&c| c) {
                    covers.insert(cover);
                }
            }
        }
        let mut areas = Vec::new();
        for cover in covers {
            let contours = top.contours(|w| w.cover(n) == cover);
            for group in contours.grouped() {
                let mut outline = BezPath::new();
                for idx in group {
                    let all: Vec<_> = contours.contours().collect();
                    outline.extend(rejoin(&all[idx.0].path, &sources));
                }
                if !is_sliver(&outline) {
                    areas.push(Area { outline, cover: cover.clone() });
                }
            }
        }
        if areas.is_empty() {
            return None;
        }
        let outlines = areas.iter().map(|a| a.outline.clone()).collect();
        Some(Self { areas, outlines, sources, count: n })
    }

    /// Each area's outline, to draw.
    pub fn outlines(&self) -> &[BezPath] {
        &self.outlines
    }

    /// The area under `p`.
    pub fn region_at(&self, p: Point) -> Option<usize> {
        self.areas.iter().position(|a| crate::contains(&a.outline, p))
    }

    /// The union of some areas: they share their edges exactly, so they sweep together cleanly.
    fn join(&self, areas: &[&Area]) -> BezPath {
        match areas {
            [] => BezPath::new(),
            [one] => one.outline.clone(),
            _ => {
                let paths: Vec<BezPath> = areas.iter().map(|a| a.outline.clone()).collect();
                match sweep(&paths) {
                    Some(top) => outlines(&top, &self.sources, |w| (0..paths.len()).any(|i| w.inside(i))),
                    None => paths.into_iter().flat_map(|p| p.into_elements()).collect(),
                }
            }
        }
    }

    /// Merges the `picked` areas into one shape (or with `delete`, removes them), and works out
    /// what's left of each input: the areas it's the front-most shape over, joined up again.
    pub fn apply(&self, picked: &[usize], delete: bool) -> Built {
        let chosen: Vec<&Area> = picked.iter().filter_map(|&i| self.areas.get(i)).collect();
        let merged = (!delete && !chosen.is_empty()).then(|| {
            let owner = chosen.iter().filter_map(|a| a.top()).max().unwrap_or(0);
            (self.join(&chosen), owner)
        });
        let rest = (0..self.count)
            .filter_map(|i| {
                let left: Vec<&Area> = self.areas.iter().enumerate().filter(|(k, a)| !picked.contains(k) && a.top() == Some(i)).map(|(_, a)| a).collect();
                let path = self.join(&left);
                (!path.elements().is_empty()).then_some((i, path))
            })
            .collect();
        Built { merged, rest }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ellipse_path, rect_path, Affine};

    fn curves(p: &BezPath) -> usize {
        p.elements().iter().filter(|e| matches!(e, PathEl::CurveTo(..) | PathEl::QuadTo(..))).count()
    }

    #[test]
    fn welding_circles_keeps_them_round_and_light() {
        let a = ellipse_path(20.0, 20.0);
        let b = Affine::translate((25.0, 0.0)) * ellipse_path(20.0, 20.0);
        let u = boolean(&[a.clone(), b.clone()], BooleanOp::Union).unwrap();
        // Two circles of 4 curves each: the weld is a few curves, not a polygon of hundreds of lines.
        assert!(u.elements().len() < 20, "{} elements", u.elements().len());
        assert!(curves(&u) >= 4);
        let expected = crate::ops::area(&crate::ops::boolean_polygons(&[a, b], BooleanOp::Union).unwrap());
        assert!((crate::ops::area(&u) - expected).abs() < 1.0, "{} vs {expected}", crate::ops::area(&u));
    }

    #[test]
    fn subtract_and_intersect_match_the_polygon_results() {
        let a = rect_path(40.0, 40.0, 0.0);
        let b = ellipse_path(15.0, 15.0);
        for op in [BooleanOp::Subtract, BooleanOp::Intersect] {
            let c = boolean(&[a.clone(), b.clone()], op).unwrap();
            let p = crate::ops::boolean_polygons(&[a.clone(), b.clone()], op).unwrap();
            assert!((crate::ops::area(&c) - crate::ops::area(&p)).abs() < 1.0, "{op:?}");
        }
        assert!(boolean(&[a], BooleanOp::Union).is_none());
    }

    #[test]
    fn open_lines_are_left_out() {
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((50.0, 50.0));
        assert!(closed_only(&line).elements().is_empty());
        assert!(boolean(&[rect_path(10.0, 10.0, 0.0), line], BooleanOp::Union).is_none());
    }

    #[test]
    fn a_square_minus_a_square_keeps_four_straight_sides_per_outline() {
        let d = boolean(&[rect_path(40.0, 40.0, 0.0), rect_path(10.0, 10.0, 0.0)], BooleanOp::Subtract).unwrap();
        let lines = d.elements().iter().filter(|e| matches!(e, PathEl::LineTo(_))).count();
        assert!(lines <= 8, "{lines} lines: collinear pieces are joined");
        assert!((crate::ops::area(&d) - 1500.0).abs() < 1e-6);
    }

    #[test]
    fn shape_builder_merges_and_deletes_regions() {
        // Two overlapping squares: three areas (left only, overlap, right only).
        let a = rect_path(20.0, 20.0, 0.0);
        let b = Affine::translate((10.0, 0.0)) * rect_path(20.0, 20.0, 0.0);
        let sb = Builder::new(&[a, b]).unwrap();
        assert_eq!(sb.outlines().len(), 3);
        let left = sb.region_at(Point::new(-8.0, 0.0)).unwrap();
        let mid = sb.region_at(Point::new(5.0, 0.0)).unwrap();
        assert!(sb.region_at(Point::new(18.0, 0.0)).is_some());
        // Merge the left and the overlap: a 20 × 20 square, and the right 10 × 20 left over.
        let r = sb.apply(&[left, mid], false);
        let (m, _) = r.merged.unwrap();
        assert!((crate::ops::area(&m) - 400.0).abs() < 0.5);
        let left_over: f64 = r.rest.iter().map(|(_, p)| crate::ops::area(p)).sum();
        assert!((left_over - 200.0).abs() < 0.5, "{left_over}");
        // Deleting the overlap leaves two 10 × 20 pieces and nothing merged.
        let r = sb.apply(&[mid], true);
        assert!(r.merged.is_none());
        let total: f64 = r.rest.iter().map(|(_, p)| crate::ops::area(p)).sum();
        assert!((total - 400.0).abs() < 0.5, "{total}");
    }

    #[test]
    fn edges_a_rounding_error_apart_still_join() {
        // A cap whose flat edge is 1e-13 off the square's top edge welds on as one outline.
        let square = rect_path(20.0, 20.0, 0.0);
        let mut cap = BezPath::new();
        cap.move_to((-10.0, -10.0));
        cap.line_to((0.0, -20.0));
        cap.line_to((10.0, -10.0 + 1e-13));
        cap.close_path();
        let u = boolean(&[square, cap], BooleanOp::Union).unwrap();
        assert_eq!(u.elements().iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count(), 1);
    }

    #[test]
    fn a_ring_is_one_area_with_its_hole() {
        let ring = BezPath::from_iter(ellipse_path(20.0, 20.0).into_iter().chain(ellipse_path(10.0, 10.0)));
        let sb = Builder::new(&[ring]).unwrap();
        assert_eq!(sb.outlines().len(), 1);
        assert!(sb.region_at(Point::new(15.0, 0.0)).is_some());
        assert!(sb.region_at(Point::ZERO).is_none(), "the hole isn't part of it");
    }
}
