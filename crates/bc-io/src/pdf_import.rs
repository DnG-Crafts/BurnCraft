//! PDF and Adobe Illustrator (.ai) import.
//!
//! Illustrator files since CS are PDF files inside, so both go through the same reader. The first
//! page's drawing operators are interpreted: paths (lines, curves, rectangles) under the current
//! transform, with nested form XObjects, and the stroke/fill colour of each path (used to pick a
//! layer). PDF units are points (1/72 inch) with Y up; shapes come out in mm with Y down.
//! Text and embedded images are reported, not imported.

use crate::{ImportResult, Imported};
use bc_geom::{Affine, BezPath, Point};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId};

const MM_PER_PT: f64 = 25.4 / 72.0;
/// Deepest nesting of form XObjects followed (guards against loops).
const MAX_DEPTH: usize = 12;

#[derive(Clone)]
struct GraphicsState {
    ctm: Affine,
    stroke: [u8; 3],
    fill: [u8; 3],
}

#[derive(Default)]
struct Stats {
    text: usize,
    images: usize,
}

pub fn import(data: &[u8]) -> Result<ImportResult, String> {
    if !data.starts_with(b"%PDF") && !data.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return Err(if data.starts_with(b"%!PS") {
            "this is an old PostScript-based Illustrator file; open it in Illustrator and save it again as a newer .ai or PDF".into()
        } else {
            "not a PDF (or PDF-compatible Illustrator) file".into()
        });
    }
    let doc = Document::load_mem(data).map_err(|e| format!("couldn't read the PDF: {e}"))?;
    let pages = doc.get_pages();
    let (&first, &page_id) = pages.iter().next().ok_or("the file has no pages")?;
    let _ = first;
    let media = media_box(&doc, page_id).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    // Page space (points, Y up from the bottom of the page) → document mm, Y down.
    let to_doc = Affine::scale(MM_PER_PT) * Affine::new([1.0, 0.0, 0.0, -1.0, -media[0], media[3]]);
    let content = doc.get_and_decode_page_content(page_id).map_err(|e| format!("couldn't read the page drawing: {e}"))?;
    let (resources, _) = doc.get_page_resources(page_id).unwrap_or((None, Vec::new()));
    let mut out = ImportResult::default();
    let mut stats = Stats::default();
    let state = GraphicsState { ctm: Affine::IDENTITY, stroke: [0, 0, 0], fill: [0, 0, 0] };
    run(&doc, &content.operations, resources, state, to_doc, &mut out, &mut stats, 0);
    if pages.len() > 1 {
        out.warnings.push(format!("only the first of {} pages was imported", pages.len()));
    }
    if stats.text > 0 {
        out.warnings.push("text in the PDF wasn't imported (convert it to outlines in your design program first)".into());
    }
    if stats.images > 0 {
        out.warnings.push(format!("{} embedded image(s) weren't imported", stats.images));
    }
    Ok(out)
}

/// The page's MediaBox (inherited from parent page-tree nodes if needed).
fn media_box(doc: &Document, page_id: ObjectId) -> Option<[f64; 4]> {
    let mut dict = doc.get_object(page_id).ok()?.as_dict().ok()?;
    for _ in 0..32 {
        if let Ok(b) = dict.get(b"MediaBox").and_then(|o| doc.dereference(o).map(|(_, o)| o)).and_then(|o| o.as_array()) {
            let v: Vec<f64> = b.iter().filter_map(number).collect();
            if v.len() == 4 {
                return Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])]);
            }
        }
        let parent = dict.get(b"Parent").ok()?.as_reference().ok()?;
        dict = doc.get_object(parent).ok()?.as_dict().ok()?;
    }
    None
}

fn number(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

fn rgb_from(ops: &[Object], space: usize) -> Option<[u8; 3]> {
    let v: Vec<f64> = ops.iter().filter_map(number).collect();
    let to = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    match (space, v.as_slice()) {
        (1, [g, ..]) => Some([to(*g); 3]),
        (3, [r, g, b, ..]) => Some([to(*r), to(*g), to(*b)]),
        (4, [c, m, y, k, ..]) => Some([to((1.0 - c) * (1.0 - k)), to((1.0 - m) * (1.0 - k)), to((1.0 - y) * (1.0 - k))]),
        _ => None,
    }
}

/// Interprets drawing operators. `to_doc` maps page space to document mm.
#[allow(clippy::too_many_arguments)]
fn run(doc: &Document, ops: &[Operation], resources: Option<&Dictionary>, mut gs: GraphicsState, to_doc: Affine, out: &mut ImportResult, stats: &mut Stats, depth: usize) {
    let mut stack: Vec<GraphicsState> = Vec::new();
    let mut path = BezPath::new();
    let mut current = Point::ORIGIN;
    let mut start = Point::ORIGIN;
    let n = |o: &Operation, i: usize| o.operands.get(i).and_then(number).unwrap_or(0.0);
    for op in ops {
        let p = |i: usize| Point::new(n(op, i), n(op, i + 1));
        match op.operator.as_str() {
            "q" => stack.push(gs.clone()),
            "Q" => {
                if let Some(s) = stack.pop() {
                    gs = s;
                }
            }
            "cm" => gs.ctm *= Affine::new([n(op, 0), n(op, 1), n(op, 2), n(op, 3), n(op, 4), n(op, 5)]),
            "m" => {
                current = p(0);
                start = current;
                path.move_to(current);
            }
            "l" => {
                current = p(0);
                path.line_to(current);
            }
            "c" => {
                path.curve_to(p(0), p(2), p(4));
                current = p(4);
            }
            // First control point is the current point.
            "v" => {
                path.curve_to(current, p(0), p(2));
                current = p(2);
            }
            // Second control point is the end point.
            "y" => {
                path.curve_to(p(0), p(2), p(2));
                current = p(2);
            }
            "h" => {
                path.close_path();
                current = start;
            }
            "re" => {
                let (x, y, w, h) = (n(op, 0), n(op, 1), n(op, 2), n(op, 3));
                path.move_to((x, y));
                path.line_to((x + w, y));
                path.line_to((x + w, y + h));
                path.line_to((x, y + h));
                path.close_path();
                current = Point::new(x, y);
                start = current;
            }
            // Painting: stroke and/or fill end the path.
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                if matches!(op.operator.as_str(), "s" | "b" | "b*") {
                    path.close_path();
                }
                let stroked = matches!(op.operator.as_str(), "S" | "s" | "B" | "B*" | "b" | "b*");
                if !path.elements().is_empty() {
                    let color = if stroked { gs.stroke } else { gs.fill };
                    out.shapes.push(Imported { path: (to_doc * gs.ctm) * std::mem::take(&mut path), color: Some(color) });
                }
                path = BezPath::new();
            }
            // End path without painting (used after setting a clip).
            "n" => path = BezPath::new(),
            "RG" => gs.stroke = rgb_from(&op.operands, 3).unwrap_or(gs.stroke),
            "rg" => gs.fill = rgb_from(&op.operands, 3).unwrap_or(gs.fill),
            "G" => gs.stroke = rgb_from(&op.operands, 1).unwrap_or(gs.stroke),
            "g" => gs.fill = rgb_from(&op.operands, 1).unwrap_or(gs.fill),
            "K" => gs.stroke = rgb_from(&op.operands, 4).unwrap_or(gs.stroke),
            "k" => gs.fill = rgb_from(&op.operands, 4).unwrap_or(gs.fill),
            // Colours set in a named colour space: guess from the number of components.
            "SC" | "SCN" => gs.stroke = rgb_from(&op.operands, op.operands.iter().filter(|o| number(o).is_some()).count()).unwrap_or(gs.stroke),
            "sc" | "scn" => gs.fill = rgb_from(&op.operands, op.operands.iter().filter(|o| number(o).is_some()).count()).unwrap_or(gs.fill),
            "BT" => stats.text += 1,
            "BI" => stats.images += 1,
            "Do" => {
                let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else { continue };
                let Some(xobject) = resources
                    .and_then(|r| r.get(b"XObject").ok())
                    .and_then(|x| doc.dereference(x).ok())
                    .and_then(|(_, x)| x.as_dict().ok())
                    .and_then(|x| x.get(name).ok())
                    .and_then(|o| doc.dereference(o).ok())
                    .and_then(|(_, o)| o.as_stream().ok())
                else {
                    continue;
                };
                match xobject.dict.get(b"Subtype").and_then(|s| s.as_name()) {
                    Ok(b"Form") if depth < MAX_DEPTH => {
                        let matrix: Vec<f64> = xobject.dict.get(b"Matrix").and_then(|m| m.as_array()).map(|a| a.iter().filter_map(number).collect()).unwrap_or_default();
                        let m = if matrix.len() == 6 { Affine::new([matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5]]) } else { Affine::IDENTITY };
                        let own = xobject.dict.get(b"Resources").ok().and_then(|r| doc.dereference(r).ok()).and_then(|(_, r)| r.as_dict().ok()).or(resources);
                        let data = xobject.decompressed_content().unwrap_or_else(|_| xobject.content.clone());
                        if let Ok(content) = Content::decode(&data) {
                            let inner = GraphicsState { ctm: gs.ctm * m, ..gs.clone() };
                            run(doc, &content.operations, own, inner, to_doc, out, stats, depth + 1);
                        }
                    }
                    Ok(b"Image") => stats.images += 1,
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
