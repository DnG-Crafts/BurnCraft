//! SVG import (via usvg) and export.

use crate::{ImportResult, Imported};
use bc_doc::{Document, ShapeKind};
use bc_geom::{Affine, BezPath, Point};
use std::fmt::Write as _;
use std::sync::Arc;
use usvg::tiny_skia_path::PathSegment;

/// SVG user units are CSS pixels at 96 per inch.
const MM_PER_PX: f64 = 25.4 / 96.0;

/// Imports an SVG. Text is converted to outlines using `fonts` (system fonts).
pub fn import(data: &[u8], fonts: Arc<usvg::fontdb::Database>) -> Result<ImportResult, String> {
    let options = usvg::Options { fontdb: fonts, ..Default::default() };
    let tree = usvg::Tree::from_data(data, &options).map_err(|e| e.to_string())?;
    let mut result = ImportResult::default();
    walk(tree.root(), &mut result);
    Ok(result)
}

fn walk(group: &usvg::Group, out: &mut ImportResult) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk(g, out),
            usvg::Node::Path(p) => {
                if p.is_visible() {
                    out.shapes.push(convert_path(p));
                }
            }
            usvg::Node::Text(t) => walk(t.flattened(), out),
            usvg::Node::Image(_) => out.warnings.push("embedded images in SVG files are not imported yet".into()),
        }
    }
}

fn paint_color(paint: &usvg::Paint) -> Option<[u8; 3]> {
    match paint {
        usvg::Paint::Color(c) => Some([c.red, c.green, c.blue]),
        _ => None,
    }
}

fn convert_path(p: &usvg::Path) -> Imported {
    let t = p.abs_transform();
    let to_mm = Affine::scale(MM_PER_PX) * Affine::new([t.sx as f64, t.ky as f64, t.kx as f64, t.sy as f64, t.tx as f64, t.ty as f64]);
    let pt = |q: usvg::tiny_skia_path::Point| Point::new(q.x as f64, q.y as f64);
    let mut path = BezPath::new();
    for seg in p.data().segments() {
        match seg {
            PathSegment::MoveTo(a) => path.move_to(pt(a)),
            PathSegment::LineTo(a) => path.line_to(pt(a)),
            PathSegment::QuadTo(a, b) => path.quad_to(pt(a), pt(b)),
            PathSegment::CubicTo(a, b, c) => path.curve_to(pt(a), pt(b), pt(c)),
            PathSegment::Close => path.close_path(),
        }
    }
    let color = p.stroke().and_then(|s| paint_color(s.paint())).or_else(|| p.fill().and_then(|f| paint_color(f.paint())));
    Imported { path: to_mm * path, color }
}

/// Margin around the design on the exported page (mm).
const EXPORT_MARGIN: f64 = 5.0;

/// Exports all visible shapes as an SVG in millimetres. Images are embedded.
///
/// The page fits the design (plus a small margin), as other design programs do, so nothing is
/// lost off the edge of the page and viewers zoom to the design. Coordinates stay the same as on
/// the bed. Lines are drawn one screen pixel wide at any zoom.
pub fn export(doc: &Document) -> String {
    let visible: Vec<&bc_doc::Shape> = doc.shapes.iter().filter(|s| doc.layers[s.layer].visible).collect();
    let page = visible
        .iter()
        .filter_map(|s| s.bounds())
        .reduce(|a, b| a.union(b))
        .map(|b| b.inflate(EXPORT_MARGIN, EXPORT_MARGIN))
        .unwrap_or_else(|| bc_geom::Rect::new(0.0, 0.0, doc.bed_size.0, doc.bed_size.1));
    let (x, y, w, h) = (page.x0, page.y0, page.width(), page.height());
    let mut s = String::new();
    let _ = writeln!(s, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(s, r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{w}mm" height="{h}mm" viewBox="{x} {y} {w} {h}">"#);
    for shape in visible {
        let [r, g, b] = doc.layers[shape.layer].rgb(shape.layer);
        if let ShapeKind::Image { data, width, height, .. } = &shape.kind {
            let c = shape.transform.as_coeffs();
            let _ = writeln!(
                s,
                r#"  <image transform="matrix({} {} {} {} {} {})" x="{}" y="{}" width="{width}" height="{height}" xlink:href="data:image/png;base64,{data}"/>"#,
                c[0], c[1], c[2], c[3], c[4], c[5], -width / 2.0, -height / 2.0
            );
            continue;
        }
        let _ = writeln!(s, r##"  <path d="{}" fill="none" stroke="#{r:02x}{g:02x}{b:02x}" stroke-width="1" vector-effect="non-scaling-stroke"/>"##, shape.world_path().to_svg());
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import_str(svg: &str) -> ImportResult {
        import(svg.as_bytes(), Arc::new(usvg::fontdb::Database::new())).unwrap()
    }

    #[test]
    fn mm_document_imports_at_true_size() {
        let r = import_str(r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50"><rect x="10" y="10" width="20" height="5" stroke="#ff0000" fill="none"/></svg>"##);
        assert_eq!(r.shapes.len(), 1);
        let b = bc_geom::bounds(&r.shapes[0].path).unwrap();
        assert!((b.x0 - 10.0).abs() < 0.01 && (b.width() - 20.0).abs() < 0.01 && (b.height() - 5.0).abs() < 0.01, "{b:?}");
        assert_eq!(r.shapes[0].color, Some([255, 0, 0]));
    }

    #[test]
    fn transforms_are_applied() {
        let r = import_str(r#"<svg xmlns="http://www.w3.org/2000/svg" width="96px" height="96px"><g transform="translate(96 0)"><path d="M0 0 L96 0" stroke="black"/></g></svg>"#);
        let b = bc_geom::bounds(&r.shapes[0].path).unwrap();
        assert!((b.x0 - 25.4).abs() < 0.01 && (b.x1 - 50.8).abs() < 0.01, "{b:?}");
    }

    #[test]
    fn export_round_trips_geometry() {
        let mut doc = Document::default();
        doc.add_shape(ShapeKind::Rect { width: 20.0, height: 10.0, corner_radius: 0.0 }, Affine::translate((50.0, 40.0)), 2);
        let r = import_str(&export(&doc));
        assert_eq!(r.shapes.len(), 1);
        let b = bc_geom::bounds(&r.shapes[0].path).unwrap();
        // The page fits the design with a 5 mm margin, so the shape sits 5 mm in from its corner.
        assert!((b.x0 - EXPORT_MARGIN).abs() < 0.01 && (b.y0 - EXPORT_MARGIN).abs() < 0.01 && (b.width() - 20.0).abs() < 0.01, "{b:?}");
        assert_eq!(r.shapes[0].color, Some(bc_doc::Layer::color(2)));
    }

    /// Shapes off the bed (e.g. from a project made for a bigger machine) still land on the page.
    #[test]
    fn export_page_fits_the_design() {
        let mut doc = Document::default();
        doc.bed_size = (600.0, 400.0);
        doc.add_shape(ShapeKind::Rect { width: 100.0, height: 50.0, corner_radius: 0.0 }, Affine::translate((800.0, -30.0)), 3);
        let svg = export(&doc);
        assert!(svg.contains(r#"width="110mm" height="60mm" viewBox="745 -60 110 60""#), "{svg}");
        assert!(svg.contains(r#"vector-effect="non-scaling-stroke""#));
        // An empty design exports the bed.
        assert!(export(&Document::default()).contains(r#"viewBox="0 0 600 400""#));
    }
}
