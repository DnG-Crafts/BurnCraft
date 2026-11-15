//! Rotated bounding rectangles, for finding how items or jig slots sit.

use crate::{Affine, Point, Vec2};

/// A rectangle turned by `angle` (radians) about its centre. `width` runs along the angle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrientedRect {
    pub center: Point,
    pub width: f64,
    pub height: f64,
    pub angle: f64,
}

impl OrientedRect {
    /// The same rectangle described with `width >= height` and the angle in (-90°, 90°].
    pub fn normalized(self) -> Self {
        use std::f64::consts::{FRAC_PI_2, PI};
        let (mut w, mut h, mut a) = (self.width, self.height, self.angle);
        if h > w {
            (w, h) = (h, w);
            a += FRAC_PI_2;
        }
        a = a.rem_euclid(PI);
        if a > FRAC_PI_2 + 1e-9 {
            a -= PI;
        }
        Self { center: self.center, width: w, height: h, angle: a }
    }

    /// Maps the axis-aligned rectangle of this size centred on the origin onto this one.
    pub fn placement(&self) -> Affine {
        Affine::translate(self.center.to_vec2()) * Affine::rotate(self.angle)
    }

    pub fn corners(&self) -> [Point; 4] {
        let t = self.placement();
        let (w, h) = (self.width / 2.0, self.height / 2.0);
        [t * Point::new(-w, -h), t * Point::new(w, -h), t * Point::new(w, h), t * Point::new(-w, h)]
    }
}

/// Convex hull (anticlockwise in y-up terms, by Andrew's monotone chain).
pub fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut p: Vec<Point> = points.to_vec();
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross = |o: Point, a: Point, b: Point| (a - o).cross(b - o);
    let mut hull: Vec<Point> = Vec::with_capacity(p.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &Point>> = if pass == 0 { Box::new(p.iter()) } else { Box::new(p.iter().rev()) };
        for &q in iter {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], q) <= 0.0 {
                hull.pop();
            }
            hull.push(q);
        }
        hull.pop();
    }
    hull
}

/// The smallest-area rectangle around `points` (one side lies along a hull edge). `None` for fewer
/// than two distinct points.
pub fn min_area_rect(points: &[Point]) -> Option<OrientedRect> {
    let hull = convex_hull(points);
    if hull.len() < 2 {
        return None;
    }
    let mut best: Option<(f64, OrientedRect)> = None;
    for i in 0..hull.len() {
        let e = hull[(i + 1) % hull.len()] - hull[i];
        if e.hypot() < 1e-12 {
            continue;
        }
        let u = e / e.hypot();
        let v = Vec2::new(-u.y, u.x);
        let (mut a0, mut a1, mut b0, mut b1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for p in &hull {
            let (a, b) = (p.to_vec2().dot(u), p.to_vec2().dot(v));
            (a0, a1, b0, b1) = (a0.min(a), a1.max(a), b0.min(b), b1.max(b));
        }
        let area = (a1 - a0) * (b1 - b0);
        if best.as_ref().is_none_or(|(ba, _)| area < *ba - 1e-9) {
            let c = u * ((a0 + a1) / 2.0) + v * ((b0 + b1) / 2.0);
            best = Some((area, OrientedRect { center: c.to_point(), width: a1 - a0, height: b1 - b0, angle: u.atan2() }));
        }
    }
    best.map(|(_, r)| r.normalized())
}
