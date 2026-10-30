//! Warps (envelope distortions) and path simplification/smoothing.
//!
//! Warped outlines are flattened finely first, so curves bend smoothly; the result is a polyline.

use crate::{flatten, polyline_path, BezPath, Point, Rect};

/// Envelope shapes. `amount` runs from -1 to 1 (as a fraction of the area's size).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Warp {
    /// Bend into an arch (positive arches up, negative sags down).
    Arch,
    /// Swell the middle sideways (negative pinches it).
    Bulge,
    /// Make the top wider than the bottom (negative: narrower): perspective, or tapered tumblers.
    Taper,
    /// A wave along the width.
    Wave,
    /// The bottom edge bows up (negative: down); the top stays straight.
    Bridge,
    /// A wave that grows towards the right, like a flag.
    Flag,
    /// Swell from the centre like a fisheye lens (negative pinches in).
    Barrel,
    /// Wrapped round a can lying on its side: the sides squeeze in as they turn away.
    TunaCan,
    /// Recede towards the top, like film titles rolling away.
    TitleCrawl,
    /// Fan out from a point above, like sun rays.
    Rays,
    /// Turned about the upright axis, in perspective.
    Rotate3d,
}

impl Warp {
    pub const ALL: [Warp; 11] =
        [Warp::Arch, Warp::Bulge, Warp::Taper, Warp::Wave, Warp::Bridge, Warp::Flag, Warp::Barrel, Warp::TunaCan, Warp::TitleCrawl, Warp::Rays, Warp::Rotate3d];

    pub fn label(self) -> &'static str {
        match self {
            Warp::Arch => "Arch",
            Warp::Bulge => "Bulge",
            Warp::Taper => "Taper / perspective",
            Warp::Wave => "Wave",
            Warp::Bridge => "Bridge",
            Warp::Flag => "Flag",
            Warp::Barrel => "Barrel (fisheye)",
            Warp::TunaCan => "Tuna can",
            Warp::TitleCrawl => "Title crawl",
            Warp::Rays => "Rays",
            Warp::Rotate3d => "3D rotate",
        }
    }

    /// Where `p` (inside `area`) goes.
    pub fn map(self, p: Point, area: Rect, amount: f64) -> Point {
        use std::f64::consts::PI;
        let (w, h) = (area.width().max(1e-9), area.height().max(1e-9));
        let (u, v) = ((p.x - area.x0) / w, (p.y - area.y0) / h);
        let c = area.center();
        match self {
            // A parabola: the middle rises by `amount` × height, the ends stay.
            Warp::Arch => Point::new(p.x, p.y - amount * h * (1.0 - (2.0 * u - 1.0).powi(2))),
            Warp::Bulge => Point::new(c.x + (p.x - c.x) * (1.0 + amount * (1.0 - (2.0 * v - 1.0).powi(2))), p.y),
            // Width scales linearly from (1 + amount) at the top to (1 - amount) at the bottom.
            Warp::Taper => Point::new(c.x + (p.x - c.x) * (1.0 + amount * (1.0 - 2.0 * v)), p.y),
            Warp::Wave => Point::new(p.x, p.y - amount * h * 0.5 * (std::f64::consts::TAU * u).sin()),
            Warp::Bridge => Point::new(p.x, p.y - amount * h * (PI * u).sin() * v),
            Warp::Flag => Point::new(p.x, p.y + amount * h * 0.5 * u * (2.0 * PI * 1.5 * u).sin()),
            Warp::Barrel => {
                let half_diag = w.hypot(h) / 2.0;
                let d = p - c;
                let r = d.hypot() / half_diag;
                c + d * (1.0 + amount * (r * r - 0.33))
            }
            Warp::TunaCan => {
                // `amount` is how far round the can the artwork wraps: up to almost all the way.
                let wrap = (amount.abs() * PI * 1.6).clamp(0.05, PI * 1.6);
                let r = (w / 2.0) / (wrap / 2.0).sin();
                let phi = (u - 0.5) * wrap;
                Point::new(c.x + r * phi.sin(), p.y + amount.signum() * (r - r * phi.cos()) * 0.15)
            }
            Warp::TitleCrawl => {
                let top = (1.0 - amount.abs() * 0.9).clamp(0.05, 1.0);
                let scale = top + (1.0 - top) * v.clamp(0.0, 1.0);
                let nv = v.clamp(0.0, 1.0).powf(1.0 + amount.abs());
                Point::new(c.x + (p.x - c.x) * scale, area.y1 - (1.0 - nv) * h)
            }
            Warp::Rays => {
                let spread = (amount.abs() * PI).max(1e-3);
                let r0 = (w / 2.0) / (spread / 2.0).sin();
                let phi = (u - 0.5) * spread * amount.signum();
                let radius = r0 + (p.y - area.y0);
                // Centred above the top edge, so the top keeps its width and the rest fans out.
                Point::new(c.x + radius * phi.sin(), area.y0 - r0 + radius * phi.cos())
            }
            Warp::Rotate3d => {
                let theta = (amount.clamp(-1.0, 1.0) * 80f64.to_radians()).clamp(-1.3, 1.3);
                let focal = 2.0 * w;
                let lx = p.x - c.x;
                let scale = focal / (focal + lx * theta.sin()).max(1e-3);
                Point::new(c.x + lx * theta.cos() * scale, c.y + (p.y - c.y) * scale)
            }
        }
    }
}

/// Warps `path` within `area` (usually the selection's bounds).
pub fn warp(path: &BezPath, area: Rect, kind: Warp, amount: f64) -> BezPath {
    let mut out = BezPath::new();
    // Fine enough that bends look smooth at laser scale.
    for line in flatten(path, 0.02) {
        let pts: Vec<Point> = densify(&line.points, area.width().max(area.height()) / 200.0).into_iter().map(|p| kind.map(p, area, amount)).collect();
        out.extend(polyline_path(&pts, line.closed));
    }
    out
}

/// Moves every point of `path` by `map`, finely enough that straight edges bend smoothly within
/// `area` (the region the mapping works over).
pub fn map_points(path: &BezPath, area: Rect, map: impl Fn(Point) -> Point) -> BezPath {
    let mut out = BezPath::new();
    for line in flatten(path, 0.02) {
        let pts: Vec<Point> = densify(&line.points, area.width().max(area.height()) / 200.0).into_iter().map(&map).collect();
        out.extend(polyline_path(&pts, line.closed));
    }
    out
}

/// Adds points so no segment is longer than `max` (straight edges must bend too).
fn densify(points: &[Point], max: f64) -> Vec<Point> {
    let max = max.max(0.05);
    let mut out = Vec::with_capacity(points.len());
    for w in points.windows(2) {
        out.push(w[0]);
        let n = (w[0].distance(w[1]) / max).ceil() as usize;
        for i in 1..n {
            out.push(w[0].lerp(w[1], i as f64 / n as f64));
        }
    }
    out.extend(points.last());
    out
}

/// Removes points that deviate less than `tolerance` mm from a straight line (Ramer–Douglas–Peucker),
/// then optionally rebuilds the outline as smooth curves through the remaining points.
pub fn simplify(path: &BezPath, tolerance: f64, smooth: bool) -> BezPath {
    let mut out = BezPath::new();
    for line in flatten(path, (tolerance / 4.0).max(0.005)) {
        let mut pts = line.points.clone();
        if line.closed && pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        let kept = if line.closed && pts.len() > 3 {
            // Split the loop at its farthest point so both halves simplify cleanly.
            let far = (1..pts.len()).max_by(|&a, &b| pts[0].distance(pts[a]).total_cmp(&pts[0].distance(pts[b]))).unwrap_or(1);
            let mut a = rdp(&pts[..=far], tolerance);
            let mut tail: Vec<Point> = pts[far..].to_vec();
            tail.push(pts[0]);
            let b = rdp(&tail, tolerance);
            a.pop();
            a.extend(b);
            a.pop();
            a
        } else {
            rdp(&pts, tolerance)
        };
        if smooth && kept.len() >= 3 {
            out.extend(catmull_rom(&kept, line.closed));
        } else {
            out.extend(polyline_path(&kept, line.closed));
        }
    }
    out
}

/// Fewer points along a run, staying within `tol` of it (Ramer-Douglas-Peucker).
pub fn simplify_points(pts: &[Point], tol: f64) -> Vec<Point> {
    rdp(pts, tol)
}

pub(crate) fn rdp(pts: &[Point], tol: f64) -> Vec<Point> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], *pts.last().unwrap());
    let (idx, dist) = pts[1..pts.len() - 1]
        .iter()
        .enumerate()
        .map(|(i, p)| (i + 1, crate::distance_to_segment(*p, a, b)))
        .max_by(|x, y| x.1.total_cmp(&y.1))
        .unwrap();
    if dist <= tol {
        return vec![a, b];
    }
    let mut left = rdp(&pts[..=idx], tol);
    left.pop();
    left.extend(rdp(&pts[idx..], tol));
    left
}

/// Smooth cubic curves through `pts` (Catmull–Rom, converted to Béziers).
fn catmull_rom(pts: &[Point], closed: bool) -> BezPath {
    let n = pts.len();
    let get = |i: isize| -> Point {
        if closed {
            pts[i.rem_euclid(n as isize) as usize]
        } else {
            pts[i.clamp(0, n as isize - 1) as usize]
        }
    };
    let mut path = BezPath::new();
    path.move_to(pts[0]);
    let segments = if closed { n } else { n - 1 };
    for i in 0..segments as isize {
        let (p0, p1, p2, p3) = (get(i - 1), get(i), get(i + 1), get(i + 2));
        let c1 = p1 + (p2 - p0) / 6.0;
        let c2 = p2 - (p3 - p1) / 6.0;
        path.curve_to(c1, c2, p2);
    }
    if closed {
        path.close_path();
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bounds, rect_path, Affine};

    #[test]
    fn every_warp_bends_and_none_leaves_nothing() {
        let strip = Affine::translate((50.0, 10.0)) * rect_path(100.0, 20.0, 0.0);
        let area = bounds(&strip).unwrap();
        for kind in Warp::ALL {
            let bent = warp(&strip, area, kind, 0.4);
            let b = bounds(&bent).unwrap_or_else(|| panic!("{kind:?} lost the shape"));
            assert!(b.width() > 10.0 && b.height() > 5.0, "{kind:?}: {b:?}");
            // Some keep their corners (the bridge bows the bottom up between them), so compare
            // what's enclosed, not just the extent.
            assert!(b != area || (crate::ops::area(&bent) - 2000.0).abs() > 20.0, "{kind:?} changed nothing");
            // No amount: no change.
            let same = bounds(&warp(&strip, area, kind, 0.0)).unwrap();
            if !matches!(kind, Warp::TunaCan | Warp::Rays | Warp::Barrel) {
                assert!((same.x0 - area.x0).abs() < 1e-6 && (same.y1 - area.y1).abs() < 1e-6, "{kind:?} at 0: {same:?}");
            }
        }
    }

    #[test]
    fn rays_fan_out_and_rotate3d_shrinks_the_far_side() {
        let strip = rect_path(100.0, 20.0, 0.0);
        let area = bounds(&strip).unwrap();
        let fan = bounds(&warp(&strip, area, Warp::Rays, 0.5)).unwrap();
        assert!(fan.width() > 100.0, "the bottom fans wider: {fan:?}");
        let turned = bounds(&warp(&strip, area, Warp::Rotate3d, 0.5)).unwrap();
        assert!(turned.width() < 100.0, "turned away, it looks narrower: {turned:?}");
    }

    #[test]
    fn arch_lifts_the_middle_and_keeps_the_ends() {
        let r = Affine::translate((50.0, 10.0)) * rect_path(100.0, 20.0, 0.0);
        let area = bounds(&r).unwrap();
        let w = warp(&r, area, Warp::Arch, 0.5);
        let b = bounds(&w).unwrap();
        assert!((b.y0 - (0.0 - 10.0)).abs() < 0.1, "the top middle rises by half the height: {b:?}");
        assert!((b.x0 - 0.0).abs() < 1e-9 && (b.x1 - 100.0).abs() < 1e-9, "width unchanged");
        // The corners stay put.
        let corner = Warp::Arch.map(Point::new(0.0, 0.0), area, 0.5);
        assert!(corner.distance(Point::new(0.0, 0.0)) < 1e-9);
    }

    #[test]
    fn taper_widens_the_top() {
        let r = Affine::translate((50.0, 50.0)) * rect_path(100.0, 100.0, 0.0);
        let area = bounds(&r).unwrap();
        let w = warp(&r, area, Warp::Taper, 0.2);
        let top: Vec<f64> = flatten(&w, 0.01).iter().flat_map(|l| l.points.clone()).filter(|p| p.y.abs() < 1e-9).map(|p| p.x).collect();
        let (lo, hi) = (top.iter().cloned().fold(f64::MAX, f64::min), top.iter().cloned().fold(f64::MIN, f64::max));
        assert!((hi - lo - 120.0).abs() < 1e-6, "top is 20% wider: {lo}..{hi}");
    }

    #[test]
    fn simplify_drops_needless_points_and_smooths() {
        // A wobbly line: 1000 points with ±0.01 mm noise on a straight 100 mm run.
        let pts: Vec<Point> = (0..=1000).map(|i| Point::new(i as f64 * 0.1, if i % 2 == 0 { 0.01 } else { -0.01 })).collect();
        let noisy = polyline_path(&pts, false);
        let s = simplify(&noisy, 0.05, false);
        assert_eq!(flatten(&s, 0.01)[0].points.len(), 2, "a straight line needs 2 points");
        // A circle drawn with many points becomes a handful of smooth curves that stay close to it.
        let circle = crate::ellipse_path(20.0, 20.0);
        let smooth = simplify(&circle, 0.2, true);
        let curves = smooth.elements().iter().filter(|e| matches!(e, crate::PathEl::CurveTo(..))).count();
        assert!((4..40).contains(&curves), "{curves} curves");
        let b = bounds(&smooth).unwrap();
        assert!((b.width() - 40.0).abs() < 0.6, "{b:?}");
    }
}
