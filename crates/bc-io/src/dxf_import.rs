//! DXF import: lines, polylines (with arc bulges), circles, arcs, ellipses and splines.

use crate::{ImportResult, Imported};
use bc_geom::{Affine, BezPath, Point};
use dxf::entities::EntityType;
use dxf::enums::Units;
use kurbo::{Arc, Shape as _, SvgArc, Vec2};

const TOLERANCE: f64 = 0.01;

/// Imports a DXF from bytes. Coordinates are converted to mm and Y is flipped to point down.
pub fn import(data: &[u8]) -> Result<ImportResult, String> {
    let drawing = dxf::Drawing::load(&mut std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let scale = match drawing.header.default_drawing_units {
        Units::Inches => 25.4,
        Units::Feet => 304.8,
        Units::Centimeters => 10.0,
        Units::Meters => 1000.0,
        Units::Microns => 0.001,
        _ => 1.0, // millimetres, or unitless (assumed mm)
    };
    let to_doc = Affine::scale_non_uniform(scale, -scale);
    let mut result = ImportResult::default();
    let mut skipped = std::collections::BTreeMap::<String, usize>::new();
    for entity in drawing.entities() {
        let path = match &entity.specific {
            EntityType::Line(l) => {
                let mut p = BezPath::new();
                p.move_to(pt(&l.p1));
                p.line_to(pt(&l.p2));
                Some(p)
            }
            EntityType::LwPolyline(pl) => {
                let verts: Vec<(Point, f64)> = pl.vertices.iter().map(|v| (Point::new(v.x, v.y), v.bulge)).collect();
                Some(bulge_polyline(&verts, pl.flags & 1 != 0))
            }
            EntityType::Polyline(pl) => {
                let verts: Vec<(Point, f64)> = pl.vertices().map(|v| (pt(&v.location), v.bulge)).collect();
                Some(bulge_polyline(&verts, pl.flags & 1 != 0))
            }
            EntityType::Circle(c) => Some(kurbo::Circle::new(pt(&c.center), c.radius).to_path(TOLERANCE)),
            EntityType::Arc(a) => {
                let start = a.start_angle.to_radians();
                let mut sweep = a.end_angle.to_radians() - start;
                if sweep <= 0.0 {
                    sweep += std::f64::consts::TAU;
                }
                Some(arc_path(Arc::new(pt(&a.center), (a.radius, a.radius), start, sweep, 0.0)))
            }
            EntityType::Ellipse(e) => {
                let major = Vec2::new(e.major_axis.x, e.major_axis.y);
                let mut sweep = e.end_parameter - e.start_parameter;
                if sweep <= 0.0 {
                    sweep += std::f64::consts::TAU;
                }
                let arc = Arc::new(pt(&e.center), (major.hypot(), major.hypot() * e.minor_axis_ratio), e.start_parameter, sweep, major.atan2());
                Some(arc_path(arc))
            }
            EntityType::Spline(s) => spline_path(s),
            other => {
                let name = format!("{other:?}");
                let name = name.split(['(', ' ', '{']).next().unwrap_or("entity").to_string();
                *skipped.entry(name).or_default() += 1;
                None
            }
        };
        if let Some(p) = path.filter(|p| !p.elements().is_empty()) {
            result.shapes.push(Imported { path: to_doc * p, color: None });
        }
    }
    for (name, n) in skipped {
        result.warnings.push(format!("{n} {name} entit{} not imported", if n == 1 { "y" } else { "ies" }));
    }
    Ok(result)
}

fn pt(p: &dxf::Point) -> Point {
    Point::new(p.x, p.y)
}

fn arc_path(arc: Arc) -> BezPath {
    arc.to_path(TOLERANCE)
}

/// A polyline whose segments may be arcs (DXF "bulge" = tan(arc angle / 4), positive = counter-clockwise).
fn bulge_polyline(verts: &[(Point, f64)], closed: bool) -> BezPath {
    let mut path = BezPath::new();
    let Some(&(first, _)) = verts.first() else { return path };
    path.move_to(first);
    let n = verts.len();
    let segments = if closed { n } else { n.saturating_sub(1) };
    for i in 0..segments {
        let (from, bulge) = verts[i];
        let (to, _) = verts[(i + 1) % n];
        if bulge.abs() < 1e-9 {
            path.line_to(to);
            continue;
        }
        let chord = from.distance(to);
        let radius = chord * (1.0 + bulge * bulge) / (4.0 * bulge.abs());
        let svg_arc = SvgArc { from, to, radii: Vec2::new(radius, radius), x_rotation: 0.0, large_arc: bulge.abs() > 1.0, sweep: bulge > 0.0 };
        match Arc::from_svg_arc(&svg_arc) {
            Some(arc) => arc.to_cubic_beziers(TOLERANCE, |a, b, c| path.curve_to(a, b, c)),
            None => path.line_to(to),
        }
    }
    if closed {
        path.close_path();
    }
    path
}

/// Evaluates a (possibly rational) B-spline with de Boor's algorithm; falls back to fit points.
fn spline_path(s: &dxf::entities::Spline) -> Option<BezPath> {
    let p = s.degree_of_curve.max(1) as usize;
    let ctrl: Vec<Point> = s.control_points.iter().map(pt).collect();
    let knots = &s.knot_values;
    let points: Vec<Point> = if ctrl.len() > p && knots.len() == ctrl.len() + p + 1 {
        let weights: Vec<f64> = if s.weight_values.len() == ctrl.len() { s.weight_values.clone() } else { vec![1.0; ctrl.len()] };
        let (t0, t1) = (knots[p], knots[ctrl.len()]);
        let samples = (ctrl.len() * 16).max(32);
        (0..=samples).map(|i| de_boor(t0 + (t1 - t0) * i as f64 / samples as f64, p, knots, &ctrl, &weights)).collect()
    } else if s.fit_points.len() >= 2 {
        s.fit_points.iter().map(pt).collect()
    } else {
        return None;
    };
    Some(bc_geom::polyline_path(&points, s.flags & 1 != 0))
}

fn de_boor(t: f64, p: usize, knots: &[f64], ctrl: &[Point], weights: &[f64]) -> Point {
    let n = ctrl.len();
    // Knot span k with knots[k] <= t < knots[k+1], clamped to valid spans.
    let mut k = p;
    while k < n - 1 && t >= knots[k + 1] {
        k += 1;
    }
    // Homogeneous coordinates (x*w, y*w, w).
    let mut d: Vec<[f64; 3]> = (0..=p).map(|j| {
        let i = k + j - p;
        [ctrl[i].x * weights[i], ctrl[i].y * weights[i], weights[i]]
    }).collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = k + j - p;
            let denom = knots[i + p + 1 - r] - knots[i];
            let alpha = if denom.abs() < 1e-12 { 0.0 } else { (t - knots[i]) / denom };
            let prev = d[j - 1];
            for (cur, p) in d[j].iter_mut().zip(prev) {
                *cur = (1.0 - alpha) * p + alpha * *cur;
            }
        }
    }
    let [x, y, w] = d[p];
    Point::new(x / w, y / w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dxf_with(entities: &str, units: i32) -> Vec<u8> {
        format!("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n{units}\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n").into_bytes()
    }

    #[test]
    fn line_in_inches_is_scaled_and_flipped() {
        let r = import(&dxf_with("0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n1\n31\n0\n", 1)).unwrap();
        let b = bc_geom::bounds(&r.shapes[0].path).unwrap();
        assert!((b.width() - 25.4).abs() < 1e-6 && (b.y0 + 25.4).abs() < 1e-6, "{b:?}");
    }

    #[test]
    fn circle_and_unsupported_entities() {
        let r = import(&dxf_with("0\nCIRCLE\n8\n0\n10\n5\n20\n5\n30\n0\n40\n2\n0\nPOINT\n8\n0\n10\n1\n20\n1\n30\n0\n", 4)).unwrap();
        assert_eq!(r.shapes.len(), 1);
        let b = bc_geom::bounds(&r.shapes[0].path).unwrap();
        assert!((b.width() - 4.0).abs() < 1e-3);
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn bulge_semicircle() {
        // Bulge 1 = 180° arc; chord 10 → radius 5, so the arc reaches 5 mm off the chord.
        let p = bulge_polyline(&[(Point::new(0.0, 0.0), 1.0), (Point::new(10.0, 0.0), 0.0)], false);
        let b = bc_geom::bounds(&p).unwrap();
        assert!((b.height() - 5.0).abs() < 1e-3 && b.y0 < -4.99, "counter-clockwise from +X goes to -Y here: {b:?}");
    }

    #[test]
    fn spline_passes_through_clamped_ends() {
        let ctrl = [Point::new(0.0, 0.0), Point::new(5.0, 10.0), Point::new(10.0, 0.0)];
        let knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let w = [1.0; 3];
        assert_eq!(de_boor(0.0, 2, &knots, &ctrl, &w), Point::new(0.0, 0.0));
        let mid = de_boor(0.5, 2, &knots, &ctrl, &w);
        assert!((mid.x - 5.0).abs() < 1e-9 && (mid.y - 5.0).abs() < 1e-9);
        let end = de_boor(1.0, 2, &knots, &ctrl, &w);
        assert!(end.distance(Point::new(10.0, 0.0)) < 1e-9);
    }
}
