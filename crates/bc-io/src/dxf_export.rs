//! DXF export: every visible vector shape (text as outlines) as polylines in millimetres, one DXF
//! layer per BurnCraft layer (named 01, 02 … Tool1, Tool2) in its colour. The origin is the bed's
//! bottom-left corner with Y up, as CAD programs expect. Images aren't exported.

use bc_doc::{Document, Layer, ShapeKind};
use dxf::entities::{Entity, EntityType, LwPolyline};
use dxf::LwPolylineVertex;
use dxf::enums::{AcadVersion, Units};
use dxf::tables::Layer as DxfLayer;
use dxf::{Color, Drawing};

/// Curves are written as polylines this close to the true shape (mm).
const TOLERANCE: f64 = 0.01;

/// The DXF layer name: the layer's own name (01 … by default), without characters DXF forbids.
fn layer_name(layer: &Layer, index: usize) -> String {
    let clean: String = layer.name.chars().map(|c| if "<>/\\\":;?*|=`,".contains(c) { '_' } else { c }).collect();
    if clean.trim().is_empty() { bc_doc::default_layer_name(index) } else { clean.trim().to_string() }
}

/// Returns the DXF file and notes about anything left out.
pub fn export(doc: &Document) -> Result<(Vec<u8>, Vec<String>), String> {
    let mut drawing = Drawing::new();
    drawing.header.version = AcadVersion::R2004;
    drawing.header.default_drawing_units = Units::Millimeters;
    let bed_height = doc.bed_size.1;
    let mut notes = Vec::new();
    let mut images = 0;
    let mut used = std::collections::BTreeSet::new();
    for shape in doc.shapes.iter().filter(|s| doc.layers[s.layer].visible) {
        if matches!(shape.kind, ShapeKind::Image { .. }) {
            images += 1;
            continue;
        }
        let name = layer_name(&doc.layers[shape.layer], shape.layer);
        if used.insert(shape.layer) {
            drawing.add_layer(DxfLayer { name: name.clone(), color: Color::from_index(7), ..Default::default() });
        }
        let [r, g, b] = doc.layers[shape.layer].rgb(shape.layer);
        for line in bc_geom::flatten(&shape.world_path(), TOLERANCE).into_iter().filter(|l| l.points.len() >= 2) {
            let mut points = line.points;
            if line.closed && points.len() > 2 && points.first() == points.last() {
                points.pop();
            }
            let mut pl = LwPolyline { vertices: points.iter().map(|p| LwPolylineVertex { x: p.x, y: bed_height - p.y, ..Default::default() }).collect(), ..Default::default() };
            pl.set_is_closed(line.closed);
            let mut entity = Entity::new(EntityType::LwPolyline(pl));
            entity.common.layer = name.clone();
            entity.common.color_24_bit = ((r as i32) << 16) | ((g as i32) << 8) | b as i32;
            drawing.add_entity(entity);
        }
    }
    if images > 0 {
        notes.push(format!("{images} image(s) weren't exported (DXF holds vector shapes only)"));
    }
    let mut bytes = Vec::new();
    drawing.save(&mut bytes).map_err(|e| e.to_string())?;
    Ok((bytes, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bc_geom::Affine;

    #[test]
    fn round_trips_through_the_importer() {
        let mut doc = Document::default();
        doc.add_shape(ShapeKind::Rect { width: 40.0, height: 20.0, corner_radius: 0.0 }, Affine::translate((50.0, 30.0)), 0);
        doc.add_shape(ShapeKind::Ellipse { radius_x: 10.0, radius_y: 10.0 }, Affine::translate((100.0, 100.0)), 3);
        doc.add_shape(ShapeKind::image(&[1, 2, 3], 10.0, 10.0), Affine::IDENTITY, 0);
        let (bytes, notes) = export(&doc).unwrap();
        assert_eq!(notes.len(), 1, "the image is reported");
        let text = String::from_utf8_lossy(&bytes);
        let has_line = |s: &str| text.lines().any(|l| l.trim() == s);
        assert!(text.contains("LWPOLYLINE") && has_line("01") && has_line("04"), "layers named by number");
        // The importer reads it back: same sizes, and the rectangle still sits 50 mm right of the circle.
        let back = crate::dxf_import::import(&bytes).unwrap();
        assert_eq!(back.shapes.len(), 2);
        let boxes: Vec<_> = back.shapes.iter().map(|s| bc_geom::bounds(&s.path).unwrap()).collect();
        let rect = boxes.iter().find(|b| (b.width() - 40.0).abs() < 1e-6).expect("rectangle");
        let circle = boxes.iter().find(|b| (b.width() - 20.0).abs() < 0.05).expect("circle");
        assert!((rect.height() - 20.0).abs() < 1e-6);
        assert!((circle.center().x - rect.center().x - 50.0).abs() < 0.05);
        assert!((circle.center().y - rect.center().y - 70.0).abs() < 0.05, "the circle is still 70 mm below the rectangle");
    }
}
