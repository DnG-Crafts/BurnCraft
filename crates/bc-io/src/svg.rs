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
