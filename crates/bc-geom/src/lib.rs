//! Geometry primitives for BurnCraft.
//!
//! A thin layer over `kurbo`. All coordinates are in millimetres, with the
//! document's Y axis pointing down (screen convention). Mapping to a machine's
//! origin happens later, in the job builder.

pub mod boxgen;
pub mod curvefit;
pub mod curveops;
pub mod cut;
pub mod dieline;
pub mod fit;
pub mod makers;
pub mod nest;
pub mod nodes;
pub mod ops;
pub mod pathtools;
pub mod shapelib;
pub mod tiles;
pub mod warp;

pub use kurbo::{Affine, BezPath, PathEl, Point, Rect, Vec2};
use kurbo::Shape as _;
use kurbo::{ParamCurve as _, ParamCurveArclen as _};

/// Default curve flattening tolerance, in mm.
pub const TOLERANCE: f64 = 0.01;

/// A rectangle centred on the origin, optionally with rounded corners.
pub fn rect_path(width: f64, height: f64, corner_radius: f64) -> BezPath {
    let rect = Rect::new(-width / 2.0, -height / 2.0, width / 2.0, height / 2.0);
    let radius = corner_radius.clamp(0.0, width.min(height) / 2.0);
    if radius > 0.0 {
        kurbo::RoundedRect::from_rect(rect, radius).to_path(TOLERANCE)
    } else {
        rect.to_path(TOLERANCE)
    }
}

/// An ellipse centred on the origin.
pub fn ellipse_path(radius_x: f64, radius_y: f64) -> BezPath {
    kurbo::Ellipse::new(Point::ORIGIN, (radius_x, radius_y), 0.0).to_path(TOLERANCE)
}

/// A regular polygon centred on the origin with its first corner pointing up.
pub fn polygon_path(sides: u32, radius: f64) -> BezPath {
    let n = sides.max(3);
    let points: Vec<Point> = (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64 - std::f64::consts::FRAC_PI_2;
            Point::new(radius * a.cos(), radius * a.sin())
        })
        .collect();
    polyline_path(&points, true)
}

/// A star centred on the origin: `points` tips at `outer` radius, valleys at `inner` radius.
pub fn star_path(points: u32, outer: f64, inner: f64) -> BezPath {
    let n = points.max(3) * 2;
    let pts: Vec<Point> = (0..n)
        .map(|i| {
            let r = if i % 2 == 0 { outer } else { inner };
            let a = std::f64::consts::TAU * i as f64 / n as f64 - std::f64::consts::FRAC_PI_2;
            Point::new(r * a.cos(), r * a.sin())
        })
        .collect();
    polyline_path(&pts, true)
}

/// Rounds every corner of `path` where two straight edges meet, with circular arcs of `radius` mm.
/// Curved segments are left as they are; radii are reduced where edges are too short.
pub fn fillet(path: &BezPath, radius: f64) -> BezPath {
    fillet_where(path, radius, |_| true)
}

/// Corners of `path` that can be rounded: points where two straight edges meet at an angle.
pub fn corner_points(path: &BezPath) -> Vec<Point> {
    subpaths(path).iter().flat_map(|s| (0..s.segs.len()).filter_map(|i| s.corner(i).map(|(_, v, _)| v))).collect()
}

/// How a corner is cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CornerStyle {
    /// A circular arc of the given radius.
    #[default]
    Round,
    /// A straight bevel starting the given distance back from the corner along each edge.
    Chamfer,
}

/// Rounds the corners for which `pick(corner point)` is true (see [`fillet`]).
pub fn fillet_where(path: &BezPath, radius: f64, pick: impl Fn(Point) -> bool) -> BezPath {
    corner_where(path, radius, CornerStyle::Round, pick)
}

/// Rounds or chamfers the corners for which `pick(corner point)` is true.
pub fn corner_where(path: &BezPath, radius: f64, style: CornerStyle, pick: impl Fn(Point) -> bool) -> BezPath {
    if radius <= 0.0 {
        return path.clone();
    }
    let mut out = BezPath::new();
    for sub in subpaths(path) {
        let n = sub.segs.len();
        // Corner after segment i: tangent distance t, keyed by i.
        let rounded: Vec<Option<(Vec2, Point, Vec2, f64)>> = (0..n)
            .map(|i| {
                let (d_in, v, d_out) = sub.corner(i)?;
                if !pick(v) {
                    return None;
                }
                let angle = (-d_in).dot(d_out).clamp(-1.0, 1.0).acos();
                // Limited to half of each neighbouring edge, so two rounded ends of one edge never overlap.
                let len = |seg: &kurbo::PathSeg| seg.arclen(1e-6);
                let max_t = (len(&sub.segs[i]) / 2.0).min(len(&sub.segs[(i + 1) % n]) / 2.0);
                let t = match style {
                    CornerStyle::Round => radius / (angle / 2.0).tan(),
                    CornerStyle::Chamfer => radius,
                }
                .min(max_t);
                Some((d_in, v, d_out, t))
            })
            .collect();
        // Trimmed start of segment j (after a rounded corner) and end of segment i (before one).
        let start_of = |j: usize| -> Point {
            let prev = if j == 0 { if sub.closed { Some(n - 1) } else { None } } else { Some(j - 1) };
            match prev.and_then(|p| rounded[p]) {
                Some((_, v, d_out, t)) => v + d_out * t,
                None => sub.segs[j].start(),
            }
        };
        let end_of = |i: usize| -> Point {
            match rounded[i] {
                Some((d_in, v, _, t)) => v - d_in * t,
                None => sub.segs[i].end(),
            }
        };
        out.move_to(start_of(0));
        for (i, (seg, corner)) in sub.segs.iter().zip(&rounded).enumerate() {
            match seg {
                kurbo::PathSeg::Line(_) => out.line_to(end_of(i)),
                kurbo::PathSeg::Quad(q) => out.quad_to(q.p1, q.p2),
                kurbo::PathSeg::Cubic(c) => out.curve_to(c.p1, c.p2, c.p3),
            }
            if let Some((_, v, _, _)) = *corner {
                match style {
                    // A quadratic through the corner approximates the arc closely and keeps tangents.
                    CornerStyle::Round => out.quad_to(v, start_of((i + 1) % n)),
                    CornerStyle::Chamfer => out.line_to(start_of((i + 1) % n)),
                }
            }
        }
        if sub.closed {
            out.close_path();
        }
    }
    out
}

/// A corner the corner tool can work on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corner {
    /// Where the two straight edges meet (for a rounded corner, where they would meet).
    pub vertex: Point,
    /// Where to show and click it: the vertex, or the middle of the rounding.
    pub handle: Point,
    /// The current radius of a rounded corner; `None` when sharp.
    pub radius: Option<f64>,
}

/// Sharp corners and corners rounded by [`fillet`] (a quadratic between two straight edges,
/// tangent to both), so rounded corners can be changed again.
pub fn corners(path: &BezPath) -> Vec<Corner> {
    let mut out = Vec::new();
    for sub in subpaths(path) {
        let n = sub.segs.len();
        for i in 0..n {
            if let Some((_, v, _)) = sub.corner(i) {
                out.push(Corner { vertex: v, handle: v, radius: None });
            }
            if let Some((k, r)) = sub.rounded_at(i) {
                let kurbo::PathSeg::Quad(q) = sub.segs[k] else { continue };
                out.push(Corner { vertex: q.p1, handle: q.eval(0.5), radius: Some(r) });
            }
        }
    }
    out
}

/// Sets the corner at `vertex` (sharp or rounded) to `radius`; 0 makes it sharp again.
pub fn set_corner_radius(path: &BezPath, vertex: Point, radius: f64) -> BezPath {
    set_corner(path, vertex, radius, CornerStyle::Round)
}

/// Sets the corner at `vertex` (sharp or rounded) to a rounding or chamfer of `size`; 0 makes it sharp.
/// (A chamfer turns one corner into two sharp ones, so it can't be found again as a single corner.)
pub fn set_corner(path: &BezPath, vertex: Point, size: f64, style: CornerStyle) -> BezPath {
    let is_it = |p: Point| p.distance(vertex) < 1e-6;
    let mut subs = subpaths(path);
    // Undo any existing rounding at this corner: extend both edges back to the vertex.
    for sub in &mut subs {
        let n = sub.segs.len();
        let Some(k) = (0..n).find_map(|i| sub.rounded_at(i).map(|(k, _)| k).filter(|&k| matches!(sub.segs[k], kurbo::PathSeg::Quad(q) if is_it(q.p1)))) else { continue };
        let (prev, next) = ((k + n - 1) % n, (k + 1) % n);
        let (kurbo::PathSeg::Line(a), kurbo::PathSeg::Line(b)) = (sub.segs[prev], sub.segs[next]) else { continue };
        sub.segs[prev] = kurbo::PathSeg::Line(kurbo::Line::new(a.p0, vertex));
        sub.segs[next] = kurbo::PathSeg::Line(kurbo::Line::new(vertex, b.p1));
        sub.segs.remove(k);
    }
    let sharp = build(&subs);
    corner_where(&sharp, size, style, is_it)
}

fn build(subs: &[Subpath]) -> BezPath {
    let mut out = BezPath::new();
    for sub in subs {
        let Some(first) = sub.segs.first() else { continue };
        out.move_to(first.start());
        for seg in &sub.segs {
            match seg {
                kurbo::PathSeg::Line(l) => out.line_to(l.p1),
                kurbo::PathSeg::Quad(q) => out.quad_to(q.p1, q.p2),
                kurbo::PathSeg::Cubic(c) => out.curve_to(c.p1, c.p2, c.p3),
            }
        }
        if sub.closed {
            out.close_path();
        }
    }
    out
}
/// One subpath as segments (a closing edge becomes a line).
struct Subpath {
    segs: Vec<kurbo::PathSeg>,
    closed: bool,
}

impl Subpath {
    /// A rounded corner whose quadratic is segment `i + 1` (between straight segments `i` and `i + 2`,
    /// tangent to both): (index of the quadratic, its radius).
    fn rounded_at(&self, i: usize) -> Option<(usize, f64)> {
        let n = self.segs.len();
        if n < 3 || (!self.closed && i + 2 >= n) {
            return None;
        }
        let k = (i + 1) % n;
        let (kurbo::PathSeg::Line(a), kurbo::PathSeg::Quad(q), kurbo::PathSeg::Line(b)) = (self.segs[i], self.segs[k], self.segs[(i + 2) % n]) else { return None };
        let dir = |v: Vec2| (v.hypot() > 1e-9).then(|| v.normalize());
        let (Some(la), Some(qa), Some(qb), Some(lb)) = (dir(a.p1 - a.p0), dir(q.p1 - q.p0), dir(q.p2 - q.p1), dir(b.p1 - b.p0)) else { return None };
        let tangent = |x: Vec2, y: Vec2| x.dot(y) > 1.0 - 1e-6;
        if !(tangent(la, qa) && tangent(qb, lb)) {
            return None;
        }
        let angle = (-qa).dot(qb).clamp(-1.0, 1.0).acos();
        if !(1e-3..std::f64::consts::PI - 1e-3).contains(&angle) {
            return None;
        }
        Some((k, (q.p1 - q.p0).hypot() * (angle / 2.0).tan()))
    }
    /// The corner after segment `i`, if both neighbours are straight and meet at an angle:
    /// (incoming direction, corner point, outgoing direction).
    fn corner(&self, i: usize) -> Option<(Vec2, Point, Vec2)> {
        let n = self.segs.len();
        if !self.closed && i + 1 >= n {
            return None;
        }
        let (kurbo::PathSeg::Line(a), kurbo::PathSeg::Line(b)) = (self.segs[i], self.segs[(i + 1) % n]) else { return None };
        let (la, lb) = (a.p1 - a.p0, b.p1 - b.p0);
        if la.hypot() < 1e-9 || lb.hypot() < 1e-9 {
            return None;
        }
        let (d_in, d_out) = (la.normalize(), lb.normalize());
        let angle = (-d_in).dot(d_out).clamp(-1.0, 1.0).acos();
        (1e-3..std::f64::consts::PI - 1e-3).contains(&angle).then_some((d_in, a.p1, d_out))
    }
}

fn subpaths(path: &BezPath) -> Vec<Subpath> {
    let mut subs: Vec<Subpath> = Vec::new();
    let (mut start, mut cur) = (Point::ORIGIN, Point::ORIGIN);
    let mut segs: Vec<kurbo::PathSeg> = Vec::new();
    let mut flush = |segs: &mut Vec<kurbo::PathSeg>, closed: bool| {
        if !segs.is_empty() {
            subs.push(Subpath { segs: std::mem::take(segs), closed });
        }
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                flush(&mut segs, false);
                (start, cur) = (p, p);
            }
            PathEl::LineTo(p) => {
                segs.push(kurbo::PathSeg::Line(kurbo::Line::new(cur, p)));
                cur = p;
            }
            PathEl::QuadTo(a, p) => {
                segs.push(kurbo::PathSeg::Quad(kurbo::QuadBez::new(cur, a, p)));
                cur = p;
            }
            PathEl::CurveTo(a, b, p) => {
                segs.push(kurbo::PathSeg::Cubic(kurbo::CubicBez::new(cur, a, b, p)));
                cur = p;
            }
            PathEl::ClosePath => {
                if cur.distance(start) > 1e-9 {
                    segs.push(kurbo::PathSeg::Line(kurbo::Line::new(cur, start)));
                }
                flush(&mut segs, true);
                cur = start;
            }
        }
    }
    flush(&mut segs, false);
    subs
}
/// Which cells of a `cols` × `rows` grid covering `area` lie inside `path` (even-odd), row by row from
/// the top. Cell centres are tested, by scanlines, so it's fast even for large images.
pub fn scanline_mask(path: &BezPath, area: Rect, cols: usize, rows: usize) -> Vec<bool> {
    let mut mask = vec![false; cols * rows];
    if cols == 0 || rows == 0 {
        return mask;
    }
    let edges: Vec<(Point, Point)> = flatten(path, 0.01)
        .into_iter()
        .flat_map(|l| {
            let mut pts = l.points;
            if let (Some(&first), Some(&last)) = (pts.first(), pts.last())
                && first != last
            {
                pts.push(first); // every outline counts as closed for a mask
            }
            pts.windows(2).map(|w| (w[0], w[1])).collect::<Vec<_>>()
        })
        .collect();
    let (cw, ch) = (area.width() / cols as f64, area.height() / rows as f64);
    let mut xs: Vec<f64> = Vec::new();
    for r in 0..rows {
        let y = area.y0 + (r as f64 + 0.5) * ch;
        xs.clear();
        for &(a, b) in &edges {
            if (a.y <= y) != (b.y <= y) {
                xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        for span in xs.as_chunks::<2>().0 {
            // Columns whose centres fall between the two crossings.
            let c0 = ((span[0] - area.x0) / cw - 0.5).ceil().max(0.0) as usize;
            let c1 = ((span[1] - area.x0) / cw - 0.5).floor();
            if c1 < 0.0 {
                continue;
            }
            for c in c0..=(c1 as usize).min(cols - 1) {
                mask[r * cols + c] = true;
            }
        }
    }
    mask
}
/// An open or closed polyline through `points`.
pub fn polyline_path(points: &[Point], closed: bool) -> BezPath {
    let mut path = BezPath::new();
    if let Some((first, rest)) = points.split_first() {
        path.move_to(*first);
        for p in rest {
            path.line_to(*p);
        }
        if closed {
            path.close_path();
        }
    }
    path
}

/// One flattened subpath: its points, and whether it was closed.
#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub points: Vec<Point>,
    pub closed: bool,
}

/// Flattens a path into polylines, one per subpath.
pub fn flatten(path: &BezPath, tolerance: f64) -> Vec<Polyline> {
    let mut out: Vec<Polyline> = Vec::new();
    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) => out.push(Polyline { points: vec![p], closed: false }),
        PathEl::LineTo(p) => {
            if let Some(last) = out.last_mut() {
                last.points.push(p);
            }
        }
        PathEl::ClosePath => {
            if let Some(last) = out.last_mut() {
                last.closed = true;
            }
        }
        // `kurbo::flatten` only emits MoveTo, LineTo and ClosePath.
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
    });
    out.retain(|p| p.points.len() > 1);
    // A subpath that ends where it started is closed, even without an explicit ClosePath.
    for line in &mut out {
        if line.points.len() > 2 && line.points[0].distance(*line.points.last().unwrap()) < 1e-9 {
            line.points.pop();
            line.closed = true;
        }
    }
    out
}

/// Distance from `p` to the segment `a`–`b`.
pub fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 == 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Whether any part of the path's lines passes through `rect` (edges included).
pub fn outline_touches(path: &BezPath, rect: Rect) -> bool {
    // A segment touches the box if it survives clipping to it (Liang–Barsky).
    let touches = |a: Point, b: Point| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for (p, q) in [(-dx, a.x - rect.x0), (dx, rect.x1 - a.x), (-dy, a.y - rect.y0), (dy, rect.y1 - a.y)] {
            if p.abs() < 1e-12 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    t0 = t0.max(r);
                } else {
                    t1 = t1.min(r);
                }
                if t0 > t1 {
                    return false;
                }
            }
        }
        true
    };
    flatten(path, TOLERANCE * 10.0).iter().any(|line| {
        let pts = &line.points;
        if pts.len() == 1 {
            return rect.contains(pts[0]);
        }
        let closing = (line.closed && pts.len() > 2).then(|| (pts[pts.len() - 1], pts[0]));
        pts.windows(2).map(|w| (w[0], w[1])).chain(closing).any(|(a, b)| touches(a, b))
    })
}

/// Distance from `p` to the nearest point on the path's outline.
pub fn distance_to_outline(path: &BezPath, p: Point) -> f64 {
    let mut best = f64::INFINITY;
    for line in flatten(path, TOLERANCE * 10.0) {
        let pts = &line.points;
        for w in pts.windows(2) {
            best = best.min(distance_to_segment(p, w[0], w[1]));
        }
        if line.closed && pts.len() > 2 {
            best = best.min(distance_to_segment(p, pts[pts.len() - 1], pts[0]));
        }
    }
    best
}

/// Whether `p` is inside the path (non-zero winding). Open subpaths count as closed.
pub fn contains(path: &BezPath, p: Point) -> bool {
    path.contains(p)
}

/// Bounding box of a path, or `None` for an empty path.
pub fn bounds(path: &BezPath) -> Option<Rect> {
    if path.elements().is_empty() { None } else { Some(path.bounding_box()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_is_centred() {
        let b = bounds(&rect_path(20.0, 10.0, 0.0)).unwrap();
        assert_eq!(b, Rect::new(-10.0, -5.0, 10.0, 5.0));
    }

    #[test]
    fn corner_radius_is_clamped() {
        let b = bounds(&rect_path(10.0, 4.0, 50.0)).unwrap();
        assert!((b.width() - 10.0).abs() < 1e-9 && (b.height() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn flatten_ellipse_is_one_closed_polyline() {
        let lines = flatten(&ellipse_path(5.0, 3.0), TOLERANCE);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
    }

    #[test]
    fn outline_distance() {
        let square = rect_path(10.0, 10.0, 0.0);
        assert!((distance_to_outline(&square, Point::new(0.0, 0.0)) - 5.0).abs() < 1e-9);
        assert!(distance_to_outline(&square, Point::new(5.0, 0.0)) < 1e-9);
    }

    #[test]
    fn fillet_rounds_square_corners() {
        let sq = Affine::translate((5.0, 5.0)) * rect_path(10.0, 10.0, 0.0);
        let f = fillet(&sq, 2.0);
        let b = bounds(&f).unwrap();
        assert!((b.width() - 10.0).abs() < 1e-9, "edges stay put");
        assert!(!contains(&f, Point::new(0.2, 0.2)), "the corner is cut off");
        assert!(contains(&f, Point::new(5.0, 5.0)));
        assert_eq!(f.elements().iter().filter(|e| matches!(e, PathEl::QuadTo(..))).count(), 4);
        // Curves are left alone.
        let e = ellipse_path(5.0, 5.0);
        assert_eq!(fillet(&e, 1.0).elements(), e.elements());
    }

    #[test]
    fn round_one_corner_then_another() {
        let sq = Affine::translate((5.0, 5.0)) * rect_path(10.0, 10.0, 0.0);
        assert_eq!(corner_points(&sq).len(), 4);
        let quads = |p: &BezPath| p.elements().iter().filter(|e| matches!(e, PathEl::QuadTo(..))).count();
        // Top-left only.
        let one = fillet_where(&sq, 3.0, |v| v.distance(Point::ORIGIN) < 1e-6);
        assert_eq!(quads(&one), 1);
        assert!(!contains(&one, Point::new(0.3, 0.3)), "top-left is rounded");
        assert!(contains(&one, Point::new(9.8, 0.2)) && contains(&one, Point::new(9.8, 9.8)), "other corners stay sharp");
        assert_eq!(corner_points(&one).len(), 3, "the rounded corner is no longer a corner");
        // A different radius on the bottom-right, even though the path now has a curve in it.
        let two = fillet_where(&one, 1.0, |v| v.distance(Point::new(10.0, 10.0)) < 1e-6);
        assert_eq!(quads(&two), 2);
        assert!(!contains(&two, Point::new(9.9, 9.9)) && contains(&two, Point::new(9.5, 9.5)), "small radius");
        assert!(contains(&two, Point::new(9.8, 0.2)), "top-right still sharp");
        let b = bounds(&two).unwrap();
        assert!((b.width() - 10.0).abs() < 1e-9 && (b.height() - 10.0).abs() < 1e-9, "edges stay put");
    }

    #[test]
    fn rounded_corners_can_be_changed_and_undone() {
        let sq = Affine::translate((5.0, 5.0)) * rect_path(10.0, 10.0, 0.0);
        let tl = Point::ORIGIN;
        let one = set_corner_radius(&sq, tl, 3.0);
        let found = corners(&one);
        assert_eq!(found.len(), 4, "3 sharp + 1 rounded: {found:?}");
        let rounded = found.iter().find(|c| c.radius.is_some()).unwrap();
        assert_eq!(rounded.vertex, tl);
        assert!((rounded.radius.unwrap() - 3.0).abs() < 1e-9, "reads back the radius");
        assert!(rounded.handle.distance(tl) > 0.5 && rounded.handle.x < 2.0, "handle sits on the curve");
        // Change it to 1 mm: still one rounded corner, now smaller.
        let smaller = set_corner_radius(&one, tl, 1.0);
        let r: Vec<f64> = corners(&smaller).iter().filter_map(|c| c.radius).collect();
        assert_eq!(r.len(), 1);
        assert!((r[0] - 1.0).abs() < 1e-9);
        assert!(contains(&smaller, Point::new(0.5, 0.5)) && !contains(&one, Point::new(0.5, 0.5)));
        // 0 makes it sharp again: back to the original square.
        let sharp = set_corner_radius(&smaller, tl, 0.0);
        assert!(corners(&sharp).iter().all(|c| c.radius.is_none()));
        assert_eq!(corners(&sharp).len(), 4);
        assert!((crate::ops::area(&sharp) - 100.0).abs() < 1e-6);
    }

    #[test]
    fn chamfer_cuts_a_straight_bevel() {
        let sq = Affine::translate((5.0, 5.0)) * rect_path(10.0, 10.0, 0.0);
        let c = set_corner(&sq, Point::ORIGIN, 2.0, CornerStyle::Chamfer);
        assert!((crate::ops::area(&c) - 98.0).abs() < 1e-6, "a 2 × 2 triangle is cut off");
        assert!(c.elements().iter().all(|e| !matches!(e, PathEl::QuadTo(..) | PathEl::CurveTo(..))), "straight lines only");
        // The bevel's two ends become ordinary corners.
        let pts = corner_points(&c);
        assert!(pts.contains(&Point::new(2.0, 0.0)) && pts.contains(&Point::new(0.0, 2.0)));
    }

    #[test]
    fn scanline_mask_of_a_circle() {
        let circle = ellipse_path(5.0, 5.0);
        let mask = scanline_mask(&circle, Rect::new(-5.0, -5.0, 5.0, 5.0), 100, 100);
        let inside = mask.iter().filter(|&&m| m).count() as f64;
        assert!((inside / 10_000.0 - std::f64::consts::PI / 4.0).abs() < 0.01, "about π/4 of the square: {inside}");
        assert!(mask[50 * 100 + 50] && !mask[0], "centre in, corner out");
    }
    #[test]
    fn open_paths_keep_their_ends() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        assert_eq!(corner_points(&p), vec![Point::new(10.0, 0.0)]);
        let f = fillet(&p, 2.0);
        assert_eq!(f.elements().first(), Some(&PathEl::MoveTo(Point::new(0.0, 0.0))));
        assert_eq!(f.elements().last().and_then(|e| e.end_point()), Some(Point::new(10.0, 10.0)));
    }

    #[test]
    fn polygon_and_star() {
        let hex = polygon_path(6, 10.0);
        assert_eq!(nodes::handles(&hex).len(), 6);
        let b = bounds(&hex).unwrap();
        assert!((b.y0 + 10.0).abs() < 1e-9, "first corner points up");
        let star = star_path(5, 10.0, 4.0);
        assert_eq!(nodes::handles(&star).len(), 10);
    }

    #[test]
    fn segment_distance_clamps_to_endpoints() {
        let d = distance_to_segment(Point::new(-3.0, 4.0), Point::ORIGIN, Point::new(10.0, 0.0));
        assert!((d - 5.0).abs() < 1e-9);
    }
}
