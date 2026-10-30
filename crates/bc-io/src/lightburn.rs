//! Import of LightBurn project files (`.lbrn2`, `.lbrn`).
//!
//! The format is XML. Worked out from sample files:
//! - World coordinates are in mm with Y pointing up; we flip them using the bed height.
//! - Every shape has an `XForm` (Qt order: m11 m12 m21 m22 dx dy).
//! - Paths are a `VertList` (`V<x> <y>` with optional `c0x/c0y` and `c1x/c1y` Bézier handles)
//!   and a `PrimList` (`L<a> <b>` lines and `B<a> <b>` curves between vertex indices).
//! - Text keeps its font settings and a `BackupPath` with the exact outlines.
//! - `CutSetting` elements hold the layer settings.

use crate::fonts::FontLibrary;
use bc_doc::{DitherMode, Layer, LayerMode, ShapeKind, TextAlign, TextStyle, CUT_LAYER_COUNT, LAYER_COUNT};
use bc_geom::{Affine, BezPath, Point};
use roxmltree::Node;
use std::collections::{BTreeMap, HashMap};

/// A shape from the file, ready to add to a document.
#[derive(Debug, Clone)]
pub struct LbShape {
    pub kind: ShapeKind,
    pub transform: Affine,
    pub layer: usize,
    /// Shapes with the same value came from the same top-level group.
    pub group: Option<usize>,
}

#[derive(Debug, Default)]
pub struct LbProject {
    /// Layer settings by layer index.
    pub layers: Vec<(usize, Layer)>,
    pub shapes: Vec<LbShape>,
    pub warnings: Vec<String>,
}

pub const EXTENSIONS: [&str; 2] = ["lbrn2", "lbrn"];

/// Parses a project. `bed_height` (mm) is used to turn LightBurn's Y-up coordinates into ours.
pub fn import(data: &[u8], bed_height: f64, fonts: &FontLibrary) -> Result<LbProject, String> {
    let text = std::str::from_utf8(data).map_err(|_| "the file is not UTF-8 text".to_string())?;
    let xml = roxmltree::Document::parse(text).map_err(|e| format!("not a valid LightBurn file: {e}"))?;
    let root = xml.root_element();
    if root.tag_name().name() != "LightBurnProject" {
        return Err("not a LightBurn project".into());
    }
    let mut out = LbProject::default();
    // LightBurn can give one layer both vector settings and separate image settings.
    let mut vector = std::collections::BTreeMap::new();
    let mut image = std::collections::BTreeMap::new();
    for cs in root.children().filter(|n| n.tag_name().name().starts_with("CutSetting")) {
        if let Some((index, layer)) = cut_setting(cs) {
            if layer.mode == LayerMode::Image { image.insert(index, layer) } else { vector.insert(index, layer) };
        }
    }
    let flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, bed_height]);
    let mut ctx = Ctx { fonts, lists: HashMap::new(), out: &mut out, skipped: HashMap::new() };
    let mut group_count = 0;
    for shape in root.children().filter(|n| n.has_tag_name("Shape")) {
        let group = (shape.attribute("Type") == Some("Group")).then(|| {
            group_count += 1;
            group_count - 1
        });
        ctx.shape(shape, flip, group);
    }
    let mut skipped: Vec<String> = ctx.skipped.iter().map(|(t, n)| format!("{n} {t} shape(s) not supported")).collect();
    skipped.sort();
    out.warnings.extend(skipped);
    split_image_layers(&mut out, vector, image);
    Ok(out)
}

/// Our layers have one mode. Where a LightBurn layer holds both vectors and images with their own
/// image settings, the images move to a free layer that gets those settings.
fn split_image_layers(out: &mut LbProject, mut vector: BTreeMap<usize, Layer>, image: BTreeMap<usize, Layer>) {
    let is_image = |s: &LbShape| matches!(s.kind, ShapeKind::Image { .. });
    for (index, mut settings) in image {
        let has_vectors = out.shapes.iter().any(|s| s.layer == index && !is_image(s));
        if !(has_vectors && vector.contains_key(&index)) {
            vector.insert(index, settings);
            continue;
        }
        let used = |j: usize| vector.contains_key(&j) || out.shapes.iter().any(|s| s.layer == j);
        let Some(free) = (0..CUT_LAYER_COUNT).find(|&j| !used(j)) else {
            out.warnings.push(format!("no free layer for the images on {}; they use its vector settings", settings.name));
            continue;
        };
        out.warnings.push(format!("images on {} moved to layer {} with their image settings", settings.name, bc_doc::default_layer_name(free)));
        settings.name = Layer::new(free).name;
        for s in out.shapes.iter_mut().filter(|s| s.layer == index && is_image(s)) {
            s.layer = free;
        }
        vector.insert(free, settings);
    }
    out.layers = vector.into_iter().collect();
}

struct Ctx<'a> {
    fonts: &'a FontLibrary,
    /// Shared vertex/primitive lists by id (`VertID` / `PrimID`).
    lists: HashMap<(bool, String), String>,
    out: &'a mut LbProject,
    skipped: HashMap<String, usize>,
}

fn num(n: Node, attr: &str) -> Option<f64> {
    n.attribute(attr)?.trim().parse().ok()
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}

/// The shape's `XForm`, identity if missing.
fn xform(n: Node) -> Affine {
    let Some(text) = child(n, "XForm").and_then(|x| x.text()) else { return Affine::IDENTITY };
    let v: Vec<f64> = text.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    if v.len() == 6 { Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]) } else { Affine::IDENTITY }
}

/// Y flip in local coordinates (LightBurn glyphs and images are Y-up; ours are Y-down).
const LOCAL_FLIP: Affine = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);

impl Ctx<'_> {
    fn push(&mut self, kind: ShapeKind, transform: Affine, layer: usize, group: Option<usize>) {
        self.out.shapes.push(LbShape { kind, transform, layer: layer.min(LAYER_COUNT - 1), group });
    }

    fn skip(&mut self, what: &str) {
        *self.skipped.entry(what.to_string()).or_default() += 1;
    }

    /// `parent` maps LightBurn world coordinates (already including any enclosing groups) to ours.
    fn shape(&mut self, n: Node, parent: Affine, group: Option<usize>) {
        let m = parent * xform(n);
        // LightBurn projects have 30 cut layers then two tool layers (30, 31); ours come later.
        let layer = num(n, "CutIndex").and_then(|v| bc_doc::from_legacy_index(v as usize)).unwrap_or(0);
        let kind = n.attribute("Type").unwrap_or("");
        match kind {
            "Group" => {
                if let Some(children) = child(n, "Children") {
                    for c in children.children().filter(|c| c.has_tag_name("Shape")) {
                        self.shape(c, m, group);
                    }
                }
            }
            "Rect" => {
                let (w, h) = (num(n, "W").unwrap_or(0.0), num(n, "H").unwrap_or(0.0));
                let r = num(n, "Cr").unwrap_or(0.0).clamp(0.0, w.min(h) / 2.0);
                self.push(ShapeKind::Rect { width: w, height: h, corner_radius: r }, m, layer, group);
            }
            "Ellipse" => {
                let (rx, ry) = (num(n, "Rx").unwrap_or(0.0), num(n, "Ry").unwrap_or(0.0));
                self.push(ShapeKind::Ellipse { radius_x: rx, radius_y: ry }, m, layer, group);
            }
            "Polygon" => {
                // Same vertex layout as ours (first vertex at -90°); a different Ry stretches it.
                let rx = num(n, "Rx").unwrap_or(0.0);
                let ry = num(n, "Ry").unwrap_or(rx);
                let sides = num(n, "N").unwrap_or(5.0).max(3.0) as u32;
                let stretch = if rx > 0.0 { Affine::scale_non_uniform(1.0, ry / rx) } else { Affine::IDENTITY };
                self.push(ShapeKind::Polygon { sides, radius: rx }, m * stretch, layer, group);
            }
            "Path" => match self.path(n) {
                Some(p) if !p.elements().is_empty() => self.push(ShapeKind::Path(m * p), Affine::IDENTITY, layer, group),
                _ => self.skip("empty path"),
            },
            "Text" => self.text(n, parent, m, layer, group),
            "Bitmap" => match n.attribute("Data").map(str::trim).filter(|d| !d.is_empty()) {
                Some(data) => {
                    // Pixel rows run top-down in local space, like ours; the XForm carries the Y flip.
                    let (w, h) = (num(n, "W").unwrap_or(0.0), num(n, "H").unwrap_or(0.0));
                    let mut adjust = bc_doc::ImageAdjust::default();
                    if let Some(g) = num(n, "Gamma").filter(|g| *g > 0.0) {
                        adjust.gamma = g as f32;
                    }
                    self.push(ShapeKind::Image { data: data.into(), width: w, height: h, adjust, clip: None }, m, layer, group);
                }
                None => self.skip("image without embedded data"),
            },
            other => self.skip(other),
        }
    }

    /// Looks up (and remembers) a path's vertex and primitive lists, then builds it.
    fn path(&mut self, n: Node) -> Option<BezPath> {
        let mut list = |is_prims: bool, tag: &str, id_attr: &str| -> Option<String> {
            let id = n.attribute(id_attr).map(str::to_string);
            match child(n, tag).and_then(|c| c.text()) {
                Some(text) => {
                    if let Some(id) = id {
                        self.lists.insert((is_prims, id), text.to_string());
                    }
                    Some(text.to_string())
                }
                None => self.lists.get(&(is_prims, id?)).cloned(),
            }
        };
        let verts = list(false, "VertList", "VertID")?;
        let prims = list(true, "PrimList", "PrimID")?;
        Some(build_path(&parse_vertices(&verts), &prims))
    }

    fn text(&mut self, n: Node, parent: Affine, m: Affine, layer: usize, group: Option<usize>) {
        let backup = child(n, "BackupPath").and_then(|b| self.path(b).map(|p| (parent * xform(b)) * p));
        let font_spec = n.attribute("Font").unwrap_or("");
        let mut parts = font_spec.split(',');
        let family = parts.next().unwrap_or("").trim().to_string();
        let fields: Vec<f64> = parts.filter_map(|s| s.trim().parse().ok()).collect();
        // Qt font string: point size, pixel size, style hint, weight, style, …
        let weight = fields.get(3).copied().unwrap_or(400.0);
        let bold = (75.0..100.0).contains(&weight) || weight >= 600.0;
        let italic = fields.get(4).is_some_and(|s| *s > 0.0);
        let text = n.attribute("Str").unwrap_or("").to_string();
        let height = num(n, "H").unwrap_or(10.0).max(0.1);
        let style = TextStyle {
            align: match num(n, "Ah").unwrap_or(1.0) as i32 {
                0 => TextAlign::Left,
                2 => TextAlign::Right,
                _ => TextAlign::Center,
            },
            // LightBurn's vertical alignment isn't read yet; it only affects which edge stays put
            // when the text is edited later, not where it lands now.
            valign: bc_doc::TextVAlign::Middle,
            letter_spacing: num(n, "LS").unwrap_or(0.0),
            line_spacing: 1.0 + num(n, "LnS").unwrap_or(0.0) / height,
            upper_case: false,
            weld: num(n, "Weld").unwrap_or(0.0) != 0.0,
            curve: None,
            kerning: Vec::new(),
        };
        let transform = m * LOCAL_FLIP;
        let editable = self.fonts.outline(&text, &family, height, bold, italic, &style).map(|outline| {
            // Size and place our rendering over LightBurn's own outlines, as font metrics differ.
            let Some(target) = backup.as_ref().and_then(bc_geom::bounds) else { return (height, outline, transform) };
            let Some(ours) = bc_geom::bounds(&(transform * outline.clone())) else { return (height, outline, transform) };
            let k = target.height() / ours.height();
            let (height, outline) = match self.fonts.outline(&text, &family, height * k, bold, italic, &style) {
                Some(o) if k.is_finite() && (0.2..5.0).contains(&k) => (height * k, o),
                _ => (height, outline),
            };
            let Some(ours) = bc_geom::bounds(&(transform * outline.clone())) else { return (height, outline, transform) };
            (height, outline, Affine::translate(target.center() - ours.center()) * transform)
        });
        match (editable, backup) {
            (Some((height, outline, transform)), _) => {
                self.push(ShapeKind::Text { text, font: family, height, bold, italic, style, outline }, transform, layer, group);
            }
            (None, Some(path)) => {
                self.out.warnings.push(format!("font \"{family}\" not found; \"{text}\" was imported as outlines"));
                self.push(ShapeKind::Path(path), Affine::IDENTITY, layer, group);
            }
            (None, None) => self.skip("text with a missing font"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Vertex {
    p: Point,
    /// Outgoing handle.
    c0: Option<Point>,
    /// Incoming handle.
    c1: Option<Point>,
}

/// Reads a number at the start of `s`; returns it and the rest.
fn take_number(s: &str) -> Option<(f64, &str)> {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && (i == 0 || matches!(s.as_bytes()[i - 1], b'e' | b'E'))) || c == 'e' || c == 'E'))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    Some((s[..end].parse().ok()?, &s[end..]))
}

fn parse_vertices(list: &str) -> Vec<Vertex> {
    list.split('V')
        .filter(|chunk| !chunk.trim().is_empty())
        .filter_map(|chunk| {
            let (x, rest) = take_number(chunk)?;
            let (y, mut rest) = take_number(rest)?;
            let mut h = [[None::<f64>; 2]; 2];
            while let Some(i) = rest.find('c') {
                rest = &rest[i + 1..];
                let b = rest.as_bytes();
                if b.len() < 2 {
                    break;
                }
                let which = (b[0] == b'1') as usize;
                let axis = (b[1] == b'y') as usize;
                if let Some((v, r)) = take_number(&rest[2..]) {
                    h[which][axis] = Some(v);
                    rest = r;
                }
            }
            // A handle with only an x value (e.g. "c0x1") is a flag meaning "no handle".
            let handle = |k: usize| Some(Point::new(h[k][0]?, h[k][1]?));
            Some(Vertex { p: Point::new(x, y), c0: handle(0), c1: handle(1) })
        })
        .collect()
}

fn build_path(v: &[Vertex], prims: &str) -> BezPath {
    let mut path = BezPath::new();
    let prims = prims.trim();
    if prims.starts_with("Line") {
        // Compact form: all lines through the vertices in order.
        for (i, vert) in v.iter().enumerate() {
            if i == 0 { path.move_to(vert.p) } else { path.line_to(vert.p) }
        }
        if prims == "LineClosed" && v.len() > 2 {
            path.close_path();
        }
        return path;
    }
    let mut current: Option<usize> = None;
    let mut start = 0;
    let mut rest = prims;
    while let Some(pos) = rest.find(['L', 'B']) {
        let curve = rest.as_bytes()[pos] == b'B';
        rest = &rest[pos + 1..];
        let Some((a, r)) = take_number(rest) else { break };
        let Some((b, r)) = take_number(r) else { break };
        rest = r;
        let (a, b) = (a as usize, b as usize);
        let (Some(va), Some(vb)) = (v.get(a), v.get(b)) else { continue };
        if current != Some(a) {
            path.move_to(va.p);
            start = a;
        }
        if curve {
            path.curve_to(va.c0.unwrap_or(va.p), vb.c1.unwrap_or(vb.p), vb.p);
        } else {
            path.line_to(vb.p);
        }
        current = Some(b);
        if b == start {
            path.close_path();
            current = None;
        }
    }
    path
}

/// Reads a `CutSetting` element into a layer.
fn cut_setting(cs: Node) -> Option<(usize, Layer)> {
    let value = |name: &str| child(cs, name).and_then(|c| c.attribute("Value"));
    let number = |name: &str| value(name).and_then(|v| v.trim().parse::<f64>().ok());
    let flag = |name: &str| number(name).map(|v| v != 0.0);
    let index = bc_doc::from_legacy_index(number("index")? as usize)?;
    let mut l = Layer::new(index);
    if let Some(name) = value("name").filter(|n| !bc_doc::is_old_default_name(index, n)) {
        l.name = name.to_string();
    }
    l.mode = match cs.attribute("type").unwrap_or("Cut") {
        "Scan" | "Offset Fill" => LayerMode::Fill,
        "Scan+Cut" => LayerMode::FillThenLine,
        "Image" => LayerMode::Image,
        _ => LayerMode::Line,
    };
    if let Some(v) = number("speed") {
        l.speed = v;
    }
    if let Some(v) = number("maxPower") {
        l.max_power = v;
    }
    l.min_power = number("minPower").unwrap_or(l.min_power).min(l.max_power);
    if let Some(v) = number("numPasses") {
        l.passes = (v as u32).max(1);
    }
    if let Some(v) = number("interval").filter(|v| *v > 0.0) {
        l.interval = v;
    }
    if let Some(v) = flag("bidir") {
        l.bidirectional = v;
    }
    if let Some(v) = flag("crossHatch") {
        l.cross_hatch = v;
    }
    if let Some(v) = number("angle") {
        l.scan_angle = v;
    }
    if let Some(v) = number("kerf") {
        l.kerf = v;
    }
    if let Some(v) = number("overscan") {
        l.overscan = v;
    }
    if let Some(v) = flag("airAssist").or_else(|| flag("runBlower")) {
        l.air_assist = v;
    }
    if let Some(v) = number("frequency").filter(|v| *v > 0.0) {
        // Stored in Hz.
        l.frequency = if v > 1000.0 { v / 1000.0 } else { v };
    }
    // Perforation: cut perfLen, skip perfSkip.
    if let (Some(cut), Some(skip)) = (number("perfLen").filter(|v| *v > 0.0), number("perfSkip").filter(|v| *v > 0.0)) {
        l.perforate = flag("perforate").unwrap_or(true);
        l.perf_cut = cut;
        l.perf_skip = skip;
    }
    if let Some(v) = flag("doOutput") {
        l.output = v;
    }
    if let Some(v) = flag("hide") {
        l.visible = !v;
    }
    if let Some(d) = value("ditherMode") {
        l.dither = match d.to_ascii_lowercase().as_str() {
            "threshold" => DitherMode::Threshold,
            "ordered" => DitherMode::Ordered,
            "atkinson" => DitherMode::Atkinson,
            "jarvis" => DitherMode::Jarvis,
            "stucki" => DitherMode::Stucki,
            "grayscale" => DitherMode::Grayscale,
            _ => DitherMode::FloydSteinberg,
        };
    }
    Some((index, l))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Imports the project at `BC_LBRN_IN` onto a 600 × 400 bed, exports it as SVG, and prints
    /// shape counts and bounds (not the design itself).
    #[test]
    #[ignore = "diagnostic"]
    fn export_probe() {
        let bytes = std::fs::read(std::env::var("BC_LBRN_IN").unwrap()).unwrap();
        let fonts = FontLibrary::from_database(usvg::fontdb::Database::new());
        let p = import(&bytes, 400.0, &fonts).unwrap();
        println!("shapes {} warnings {:?}", p.shapes.len(), p.warnings);
        let mut doc = bc_doc::Document::default();
        doc.bed_size = (600.0, 400.0);
        for s in p.shapes {
            let id = doc.add_shape(s.kind, s.transform, s.layer);
            let sh = doc.shape(id).unwrap();
            println!("  {} layer {} visible {} bounds {:?} elements {}", sh.kind.type_name(), sh.layer, doc.layers[sh.layer].visible, sh.bounds(), sh.world_path().elements().len());
        }
        // As the app does on Open: slide the design onto the bed.
        let ids: Vec<u64> = doc.shapes.iter().map(|s| s.id).collect();
        let b = doc.bounds_of(ids.iter().copied()).unwrap();
        let shift = |lo: f64, hi: f64, max: f64| if hi - lo > max { 0.0 } else if lo < 0.0 { -lo } else if hi > max { max - hi } else { 0.0 };
        let t = bc_geom::Affine::translate((shift(b.x0, b.x1, 600.0), shift(b.y0, b.y1, 400.0)));
        for s in &mut doc.shapes {
            s.transform = t * s.transform;
        }
        println!("after placing: {:?}", doc.bounds_of(ids.iter().copied()));
        let svg = crate::svg::export(&doc);
        if let Ok(out) = std::env::var("BC_LBRN_SVG") {
            std::fs::write(out, &svg).unwrap();
        }
        println!("svg paths {} bytes {}", svg.matches("<path").count(), svg.len());
        for l in svg.lines().filter(|l| l.contains("<path")).take(2) {
            println!("  {}", &l[..l.len().min(160)]);
        }
    }

    const ONE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="2.1.04" DeviceName="NEJE" FormatVersion="1">
    <CutSetting type="Scan">
        <index Value="0"/><name Value="C00"/><minPower Value="0.2"/><maxPower Value="90"/><speed Value="1000"/>
    </CutSetting>
    <CutSetting type="Cut">
        <index Value="3"/><name Value="C03"/><maxPower Value="20"/><speed Value="100"/>
    </CutSetting>
    <Shape Type="Rect" CutIndex="0" W="88" H="69" Cr="0"><XForm>1 0 0 1 248 315.5</XForm></Shape>
    <Shape Type="Ellipse" CutIndex="3" Rx="37.5" Ry="25.5"><XForm>1 0 0 1 248.5 313.5</XForm></Shape>
    <Shape Type="Polygon" CutIndex="5" Rx="149.5" Ry="103.5" N="5"><XForm>1 0 0 -1 398.5 207.5</XForm></Shape>
</LightBurnProject>"#;

    fn fonts() -> FontLibrary {
        FontLibrary::from_database(usvg::fontdb::Database::new())
    }

    #[test]
    fn layers_and_primitives() {
        let p = import(ONE.as_bytes(), 400.0, &fonts()).unwrap();
        assert_eq!(p.layers.len(), 2);
        let (i, scan) = &p.layers[0];
        assert_eq!((*i, scan.mode, scan.speed, scan.max_power, scan.min_power), (0, LayerMode::Fill, 1000.0, 90.0, 0.2));
        assert_eq!(p.layers[1].1.mode, LayerMode::Line);
        assert_eq!(p.shapes.len(), 3);
        // Rect centre (248, 315.5) Y-up → (248, 84.5) on a 400 mm bed.
        let rect = &p.shapes[0];
        let b = bc_geom::bounds(&(rect.transform * bc_geom::rect_path(88.0, 69.0, 0.0))).unwrap();
        assert!((b.center().x - 248.0).abs() < 1e-9 && (b.center().y - 84.5).abs() < 1e-9);
        // The ellipse is 2 mm lower than the rect in LightBurn, so 2 mm further down here.
        let e = p.shapes[1].transform * Point::ORIGIN;
        assert!((e.y - 86.5).abs() < 1e-9);
        // The pentagon points up on screen (its first vertex has the smallest y).
        let poly = &p.shapes[2];
        let path = poly.transform * bc_geom::polygon_path(5, 149.5);
        let b = bc_geom::bounds(&path).unwrap();
        let first = match path.elements()[0] {
            kurbo::PathEl::MoveTo(p) => p,
            _ => unreachable!(),
        };
        assert!((first.y - b.y0).abs() < 1e-6, "apex at the top");
        assert!((b.height() - 103.5 * (1.0 + (36f64).to_radians().cos())).abs() < 1e-6);
    }

    #[test]
    fn vertex_lists_with_handles() {
        let v = parse_vertices("V28.375488 -2.7099609c0x1c1x1V30.9 -1.0c0x28.36c0y-0.5c1x1V1e-3 2c0x1c1x5c1y6");
        assert_eq!(v.len(), 3);
        assert_eq!((v[0].c0, v[0].c1), (None, None));
        assert_eq!(v[1].c0, Some(Point::new(28.36, -0.5)));
        assert_eq!(v[1].c1, None);
        assert_eq!(v[2].p, Point::new(0.001, 2.0));
        assert_eq!(v[2].c1, Some(Point::new(5.0, 6.0)));
    }

    #[test]
    fn primitives_build_closed_subpaths() {
        let v = parse_vertices("V0 0c0x1c1x1V10 0c0x1c1x1V10 10c0x1c1x1V5 5c0x1c1x1V6 5c0x6c0y6c1x1V5 6c0x1c1x5c1y7");
        let p = build_path(&v, "L0 1L1 2L2 0L3 4B4 5L5 3");
        let closes = p.elements().iter().filter(|e| matches!(e, kurbo::PathEl::ClosePath)).count();
        let moves = p.elements().iter().filter(|e| matches!(e, kurbo::PathEl::MoveTo(_))).count();
        assert_eq!((closes, moves), (2, 2));
        assert!(p.elements().iter().any(|e| matches!(e, kurbo::PathEl::CurveTo(a, b, _) if *a == Point::new(6.0, 6.0) && *b == Point::new(5.0, 7.0))));
    }

    #[test]
    fn text_without_font_uses_backup_outlines() {
        let xml = r#"<LightBurnProject>
            <Shape Type="Text" CutIndex="2" Font="NoSuchFont,-1,4096,5,400,0,0,0,0,0" Str="x" H="25">
                <BackupPath Type="Path" CutIndex="2"><XForm>1 0 0 1 100 100</XForm>
                    <VertList>V0 0c0x1c1x1V10 0c0x1c1x1V10 10c0x1c1x1</VertList><PrimList>L0 1L1 2L2 0</PrimList>
                </BackupPath>
                <XForm>1 0 0 1 100 100</XForm>
            </Shape></LightBurnProject>"#;
        let p = import(xml.as_bytes(), 300.0, &fonts()).unwrap();
        assert_eq!(p.shapes.len(), 1);
        assert!(matches!(p.shapes[0].kind, ShapeKind::Path(_)));
        assert_eq!(p.shapes[0].layer, 2);
        assert_eq!(p.warnings.len(), 1);
        let b = bc_geom::bounds(&p.shapes[0].kind_path()).unwrap();
        assert!((b.y0 - 190.0).abs() < 1e-9 && (b.y1 - 200.0).abs() < 1e-9, "Y flipped about the 300 mm bed");
    }

    #[test]
    fn shared_lists_and_groups() {
        let xml = r#"<LightBurnProject>
            <Shape Type="Group"><XForm>1 0 0 1 10 0</XForm><Children>
                <Shape Type="Path" CutIndex="1" VertID="0" PrimID="0"><XForm>1 0 0 1 0 0</XForm>
                    <VertList>V0 0c0x1c1x1V5 0c0x1c1x1</VertList><PrimList>L0 1</PrimList></Shape>
                <Shape Type="Path" CutIndex="1" VertID="0" PrimID="0"><XForm>1 0 0 1 0 5</XForm></Shape>
            </Children></Shape>
            <Shape Type="Star" CutIndex="0"/>
        </LightBurnProject>"#;
        let p = import(xml.as_bytes(), 100.0, &fonts()).unwrap();
        assert_eq!(p.shapes.len(), 2);
        assert!(p.shapes.iter().all(|s| s.group == Some(0)));
        let b = bc_geom::bounds(&p.shapes[1].kind_path()).unwrap();
        assert_eq!((b.x0, b.y0), (10.0, 95.0));
        assert_eq!(p.warnings, vec!["1 Star shape(s) not supported".to_string()]);
    }

    #[test]
    fn image_settings_on_a_vector_layer_get_their_own_layer() {
        let xml = r#"<LightBurnProject>
            <CutSetting type="Cut"><index Value="6"/><name Value="C06"/><speed Value="100"/></CutSetting>
            <CutSetting_Img type="Image"><index Value="6"/><name Value="C06"/><speed Value="300"/><ditherMode Value="stucki"/></CutSetting_Img>
            <Shape Type="Ellipse" CutIndex="6" Rx="5" Ry="5"><XForm>1 0 0 1 50 50</XForm></Shape>
            <Shape Type="Bitmap" CutIndex="6" W="20" H="10" Gamma="1.5" Data="AAAA"><XForm>0.5 0 0 -0.5 100 100</XForm></Shape>
        </LightBurnProject>"#;
        let p = import(xml.as_bytes(), 200.0, &fonts()).unwrap();
        let layers: Vec<(usize, LayerMode, f64)> = p.layers.iter().map(|(i, l)| (*i, l.mode, l.speed)).collect();
        assert_eq!(layers, vec![(0, LayerMode::Image, 300.0), (6, LayerMode::Line, 100.0)]);
        assert_eq!(p.layers[0].1.dither, DitherMode::Stucki);
        assert_eq!((p.shapes[0].layer, p.shapes[1].layer), (6, 0));
        assert_eq!(p.warnings.len(), 1);
        // Image rows run top-down: local top edge (y = -5) ends up on top (smaller y) on screen.
        let img = &p.shapes[1];
        let (top, bottom) = (img.transform * Point::new(0.0, -5.0), img.transform * Point::new(0.0, 5.0));
        assert!(top.y < bottom.y);
        assert!(matches!(img.kind, ShapeKind::Image { adjust, .. } if adjust.gamma == 1.5));
    }

    #[test]
    fn lightburn_tool_layers_land_on_t1_and_t2() {
        let xml = r#"<LightBurnProject>
            <CutSetting type="Tool"><index Value="30"/><name Value="T1"/></CutSetting>
            <Shape Type="Ellipse" CutIndex="30" Rx="5" Ry="5"><XForm>1 0 0 1 50 50</XForm></Shape>
            <Shape Type="Ellipse" CutIndex="31" Rx="5" Ry="5"><XForm>1 0 0 1 50 50</XForm></Shape>
        </LightBurnProject>"#;
        let p = import(xml.as_bytes(), 200.0, &fonts()).unwrap();
        assert_eq!((p.shapes[0].layer, p.shapes[1].layer), (CUT_LAYER_COUNT, CUT_LAYER_COUNT + 1));
        assert!(p.layers.iter().all(|(i, _)| *i == CUT_LAYER_COUNT));
    }

    #[test]
    fn smooth_vertex_markers_are_ignored() {
        let v = parse_vertices("V411 394c0x464.39511c0y365.76025c1x357.60489c1y422.23975SV500 396c0x1c1x500c1y396");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].c1, Some(Point::new(357.60489, 422.23975)));
        assert_eq!(v[1].p, Point::new(500.0, 396.0));
    }

    /// Imports the files listed in `BC_LBRN_SAMPLES` (separated by `;`) with system fonts.
    #[test]
    #[ignore = "needs sample files"]
    fn sample_files() {
        let fonts = FontLibrary::load_system();
        for path in std::env::var("BC_LBRN_SAMPLES").unwrap().split(';') {
            let p = import(&std::fs::read(path).unwrap(), 400.0, &fonts).unwrap();
            println!("{path}: warnings {:?}", p.warnings);
            for (i, l) in &p.layers {
                println!("  layer {i} {} {:?} {} mm/s {}-{}%", l.name, l.mode, l.speed, l.min_power, l.max_power);
            }
            for s in &p.shapes {
                let shape = bc_doc::Shape { id: 0, kind: s.kind.clone(), transform: s.transform, layer: s.layer, locked: false, group: None, effects: Vec::new() };
                let b = shape.bounds().unwrap();
                println!("  {} on {}: x {:.2}..{:.2} y {:.2}..{:.2}", s.kind.type_name(), s.layer, b.x0, b.x1, b.y0, b.y1);
            }
        }
    }

    impl LbShape {
        fn kind_path(&self) -> BezPath {
            match &self.kind {
                ShapeKind::Path(p) => self.transform * p.clone(),
                _ => panic!("not a path"),
            }
        }
    }
}
