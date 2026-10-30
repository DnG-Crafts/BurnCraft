//! Region operations on closed paths: booleans and offsetting.
//!
//! Paths are converted to filled regions with the even-odd rule (so holes in
//! compound paths stay holes), processed with the `geo` crate, and converted
//! back to paths made of closed polylines.

use crate::{flatten, BezPath, Point};
use geo::{BooleanOps, Buffer, Coord, LineString, MultiPolygon, Polygon};

/// Flattening tolerance for region operations, in mm.
const TOLERANCE: f64 = 0.01;

/// Filled region of a path (even-odd). Open subpaths are ignored.
pub fn region(path: &BezPath) -> MultiPolygon<f64> {
    let mut result = MultiPolygon::new(vec![]);
    for line in flatten(path, TOLERANCE).into_iter().filter(|l| l.closed && l.points.len() >= 3) {
        let ring: Vec<Coord<f64>> = line.points.iter().map(|p| Coord { x: p.x, y: p.y }).collect();
        let poly = MultiPolygon::new(vec![Polygon::new(LineString::new(ring), vec![])]);
        result = result.xor(&poly);
    }
    result
}

/// Whether a ring is a sliver: average width (2 × area / perimeter) below the flattening tolerance.
/// Booleans leave these where outlines nearly touch; they have no area to cut and, drawn with
/// mitred joins, show up as long spikes.
fn is_sliver(ring: &LineString<f64>) -> bool {
    use geo::{Area, Euclidean, Length};
    let perimeter = Euclidean.length(ring);
    let area = Polygon::new(ring.clone(), vec![]).unsigned_area();
    perimeter <= 0.0 || 2.0 * area / perimeter < TOLERANCE
}

/// Path made of the outer rings and holes of a region (slivers left out).
pub fn region_to_path(region: &MultiPolygon<f64>) -> BezPath {
    let mut path = BezPath::new();
    let mut add_ring = |ring: &LineString<f64>| {
        if is_sliver(ring) {
            return;
        }
        let pts: Vec<Point> = ring.coords().map(|c| Point::new(c.x, c.y)).collect();
        // geo rings repeat the first point at the end.
        let pts = if pts.len() > 1 && pts.first() == pts.last() { &pts[..pts.len() - 1] } else { &pts[..] };
        if pts.len() >= 3 {
            path.move_to(pts[0]);
            for p in &pts[1..] {
                path.line_to(*p);
            }
            path.close_path();
        }
    };
    for poly in region {
        add_ring(poly.exterior());
        for hole in poly.interiors() {
            add_ring(hole);
        }
    }
    path
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOp {
    Union,
    /// First minus the rest.
    Subtract,
    Intersect,
}

/// Combines the regions of `paths` (in order). Returns `None` if fewer than two closed paths are given.
/// Curves stay curves where possible (see [`crate::curveops`]); shapes that can't be combined
/// that way are combined as polygons.
pub fn boolean(paths: &[BezPath], op: BooleanOp) -> Option<BezPath> {
    crate::curveops::boolean(paths, op).or_else(|| boolean_polygons(paths, op))
}

/// [`boolean`] on the shapes flattened to polygons (always works, but curves become many short lines).
pub fn boolean_polygons(paths: &[BezPath], op: BooleanOp) -> Option<BezPath> {
    let regions: Vec<_> = paths.iter().map(region).filter(|r| !r.0.is_empty()).collect();
    let (first, rest) = regions.split_first()?;
    if rest.is_empty() {
        return None;
    }
    let result = rest.iter().fold(first.clone(), |acc, r| match op {
        BooleanOp::Union => acc.union(r),
        BooleanOp::Subtract => acc.difference(r),
        BooleanOp::Intersect => acc.intersection(r),
    });
    Some(region_to_path(&result))
}

/// Offsets the closed region of `path` outward (positive) or inward (negative) by `distance` mm, with round corners.
pub fn offset(path: &BezPath, distance: f64) -> Option<BezPath> {
    let r = region(path);
    if r.0.is_empty() {
        return None;
    }
    let out = region_to_path(&r.buffer(distance));
    (!out.elements().is_empty()).then_some(out)
}

/// Stencil bridges: wherever the region has a hole (the middle of an O, A or B), cuts a strip
/// `width` mm wide across the shape through the hole's centre, vertically or `horizontal`ly. Cut
/// out of a sheet, the island in the hole then stays attached by two bridges instead of falling
/// out. `None` if the region has no holes.
pub fn stencil_bridges(path: &BezPath, width: f64, horizontal: bool) -> Option<BezPath> {
    use geo::{BoundingRect, Centroid, Rect};
    let r = region(path);
    let half = width.max(0.01) / 2.0;
    let mut strips: Vec<Polygon<f64>> = Vec::new();
    for poly in &r {
        let Some(outer) = poly.exterior().bounding_rect() else { continue };
        for hole in poly.interiors() {
            let Some(c) = Polygon::new(hole.clone(), vec![]).centroid() else { continue };
            let strip = if horizontal {
                Rect::new(Coord { x: outer.min().x - 1.0, y: c.y() - half }, Coord { x: outer.max().x + 1.0, y: c.y() + half })
            } else {
                Rect::new(Coord { x: c.x() - half, y: outer.min().y - 1.0 }, Coord { x: c.x() + half, y: outer.max().y + 1.0 })
            };
            strips.push(strip.to_polygon());
        }
    }
    if strips.is_empty() {
        return None;
    }
    Some(region_to_path(&r.difference(&MultiPolygon::new(strips))))
}

/// A cut line around everything in `paths` (stickers, magnets, badges): the regions merged, grown
/// by `distance` mm with round corners, and with holes left out so it's one clean outline per
/// separate piece. `None` if nothing is closed.
pub fn contour(paths: &[BezPath], distance: f64) -> Option<BezPath> {
    let merged = paths.iter().map(region).filter(|r| !r.0.is_empty()).reduce(|a, b| a.union(&b))?;
    let grown = if distance > 0.0 { merged.buffer(distance) } else { merged };
    let outer = MultiPolygon::new(grown.0.into_iter().map(|p| Polygon::new(p.exterior().clone(), vec![])).collect());
    // Pieces nested inside another piece's outline are covered by it.
    let outer = outer.0.iter().fold(MultiPolygon::new(vec![]), |acc, p| acc.union(&MultiPolygon::new(vec![p.clone()])));
    let out = region_to_path(&outer);
    (!out.elements().is_empty()).then_some(out)
}

/// The parts of the open (and closed) lines of `path` that lie inside the region of `inside`, as
/// open polylines. For splitting engraved strokes between tiles.
pub fn clip_lines(path: &BezPath, inside: &BezPath) -> BezPath {
    use geo::MultiLineString;
    let lines: Vec<LineString<f64>> = flatten(path, TOLERANCE)
        .into_iter()
        .map(|l| {
            let mut pts: Vec<Coord<f64>> = l.points.iter().map(|p| Coord { x: p.x, y: p.y }).collect();
            if l.closed && let Some(&first) = pts.first() {
                pts.push(first);
            }
            LineString::new(pts)
        })
        .filter(|l| l.0.len() >= 2)
        .collect();
    let clipped = region(inside).clip(&MultiLineString::new(lines), false);
    let mut out = BezPath::new();
    for l in clipped {
        let pts: Vec<Point> = l.coords().map(|c| Point::new(c.x, c.y)).collect();
        if pts.len() >= 2 {
            out.extend(crate::polyline_path(&pts, false));
        }
    }
    out
}

/// Area of the region (mm²).
pub fn area(path: &BezPath) -> f64 {
    use geo::Area;
    region(path).unsigned_area()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bounds, rect_path, Affine};

    #[test]
    fn slivers_are_dropped_but_small_shapes_kept() {
        let ring = |pts: &[(f64, f64)]| LineString::from(pts.to_vec());
        let sliver = ring(&[(0.0, 0.0), (0.04, 0.0), (0.0, 0.002), (0.0, 0.0)]);
        let dot = ring(&[(0.0, 0.0), (0.1, 0.0), (0.1, 0.1), (0.0, 0.1), (0.0, 0.0)]);
        let region = MultiPolygon::new(vec![Polygon::new(sliver, vec![]), Polygon::new(dot, vec![])]);
        let rings = flatten(&region_to_path(&region), TOLERANCE);
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].points.len(), 4);
    }

    fn square_at(x: f64, y: f64, size: f64) -> BezPath {
        Affine::translate((x, y)) * rect_path(size, size, 0.0)
    }

    #[test]
    fn union_of_overlapping_squares() {
        let u = boolean(&[square_at(0.0, 0.0, 10.0), square_at(5.0, 0.0, 10.0)], BooleanOp::Union).unwrap();
        assert!((area(&u) - 150.0).abs() < 1e-6);
    }

    #[test]
    fn subtract_makes_a_hole() {
        let d = boolean(&[square_at(0.0, 0.0, 10.0), square_at(0.0, 0.0, 4.0)], BooleanOp::Subtract).unwrap();
        assert!((area(&d) - 84.0).abs() < 1e-6);
        // The result keeps its hole when converted back to a region.
        assert!((area(&region_to_path(&region(&d))) - 84.0).abs() < 1e-6);
    }

    #[test]
    fn intersect() {
        let i = boolean(&[square_at(0.0, 0.0, 10.0), square_at(5.0, 5.0, 10.0)], BooleanOp::Intersect).unwrap();
        assert!((area(&i) - 25.0).abs() < 1e-6);
    }

    #[test]
    fn single_path_is_not_a_boolean() {
        assert!(boolean(&[square_at(0.0, 0.0, 10.0)], BooleanOp::Union).is_none());
    }

    #[test]
    fn stencil_bridges_hold_the_middle_of_an_o() {
        // A square "O": 20 mm outside, 10 mm hole.
        let o = boolean(&[square_at(0.0, 0.0, 20.0), square_at(0.0, 0.0, 10.0)], BooleanOp::Subtract).unwrap();
        let s = stencil_bridges(&o, 2.0, false).unwrap();
        // The O splits into two halves with no holes, and loses the two 2 × 5 mm bridges.
        let r = region(&s);
        assert_eq!(r.0.len(), 2);
        assert!(r.0.iter().all(|p| p.interiors().is_empty()));
        assert!((area(&s) - (300.0 - 20.0)).abs() < 1e-6, "{}", area(&s));
        // Shapes without holes need no bridges.
        assert!(stencil_bridges(&square_at(0.0, 0.0, 10.0), 2.0, false).is_none());
    }

    #[test]
    fn contour_wraps_everything_without_holes() {
        // An "O" and a separate small square close by: within 3 mm of each other they merge.
        let o = boolean(&[square_at(0.0, 0.0, 20.0), square_at(0.0, 0.0, 10.0)], BooleanOp::Subtract).unwrap();
        let c = contour(&[o.clone(), square_at(14.0, 0.0, 4.0)], 3.0).unwrap();
        let r = region(&c);
        assert_eq!(r.0.len(), 1);
        assert!(r.0[0].interiors().is_empty(), "the O's hole is not cut");
        let b = bounds(&c).unwrap();
        assert!((b.x0 + 13.0).abs() < 0.01 && (b.x1 - 19.0).abs() < 0.01, "{b:?}");
        // Far apart: one outline each.
        assert_eq!(region(&contour(&[o, square_at(60.0, 0.0, 4.0)], 3.0).unwrap()).0.len(), 2);
    }

    #[test]
    fn lines_are_clipped_to_a_region() {
        let mut line = BezPath::new();
        line.move_to((-20.0, 0.0));
        line.line_to((20.0, 0.0));
        let c = clip_lines(&line, &square_at(0.0, 0.0, 10.0));
        let b = bounds(&c).unwrap();
        assert!((b.x0 + 5.0).abs() < 1e-9 && (b.x1 - 5.0).abs() < 1e-9, "{b:?}");
        assert!(clip_lines(&line, &square_at(0.0, 50.0, 10.0)).elements().is_empty());
    }

    #[test]
    fn offset_grows_and_shrinks() {
        let s = square_at(0.0, 0.0, 10.0);
        let grown = bounds(&offset(&s, 1.0).unwrap()).unwrap();
        assert!((grown.width() - 12.0).abs() < 1e-3);
        let shrunk = bounds(&offset(&s, -1.0).unwrap()).unwrap();
        assert!((shrunk.width() - 8.0).abs() < 1e-3);
        assert!(offset(&s, -6.0).is_none(), "shrinking past the centre leaves nothing");
    }
}
