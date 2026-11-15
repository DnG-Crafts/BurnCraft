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
