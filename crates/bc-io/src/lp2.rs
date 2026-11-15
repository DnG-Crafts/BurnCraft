//! Import of LaserPecker Design Space projects (`.lp2`).
//!
//! An `.lp2` file is a zip: `.lpproject` (JSON), the pictures it uses under `res/`, and a
//! `preview.png`. The JSON's `data` is the canvas as a list of objects, each placed by its
//! top-left corner (`left`, `top`, mm on the LP2's work area), its size before scaling (`width`,
//! `height`), `scaleX`/`scaleY`, `angle` (degrees, about that corner) and `flipX`/`flipY`. Shapes
//! carry their geometry as SVG path data (`path`), a point list (`points`), a line's ends
//! (`x1`..`y2`) or, for rectangles and ellipses, their size; pictures name their file under `res/`.
//! Text is kept as text: its `text`, `fontFamily`, `fontSize` (mm, the em size), `fontWeight`,
//! `fontStyle`, `textAlign`, `lineHeight` and `charSpacing` (thousandths of an em), in a box sized
//! as the app lays it out. `paintStyle` 1 puts a shape on the fill layer, otherwise it's line
//! work; `visible` false hides it. `laserOptions` holds each layer's settings (depth, power,
//! passes, resolution).

use crate::fonts::FontLibrary;
use crate::lightburn::{LbProject, LbShape};
use bc_doc::{Layer, LayerMode, ShapeKind, TextAlign, TextStyle};
use bc_geom::{Affine, BezPath, Point};
use kurbo::Shape as _;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;

pub const EXTENSIONS: [&str; 1] = ["lp2"];

const IMAGE: i64 = 10010;
const RECTANGLE: i64 = 10005;
const ELLIPSE: i64 = 10006;
const TEXTS: [i64; 3] = [10001, 10013, 10030];

/// The layers the app's own layers come in on: fill, picture, line, cut.
const FILL_LAYER: usize = 0;
const PICTURE_LAYER: usize = 1;
const LINE_LAYER: usize = 2;
const CUT_LAYER: usize = 3;
/// Hidden objects go on a copy of their layer this many further on, switched off.
const HIDDEN: usize = 4;

struct Reader<'a> {
    fonts: &'a FontLibrary,
    files: HashMap<String, Vec<u8>>,
    shapes: Vec<LbShape>,
    groups: HashMap<String, usize>,
    missing_fonts: std::collections::BTreeSet<String>,
    curved: usize,
    skipped_text: usize,
    hidden: usize,
    skipped_other: usize,
    skewed: usize,
}

fn num(o: &Value, key: &str, default: f64) -> f64 {
    o.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(default)
}

fn flag(o: &Value, key: &str) -> bool {
    o.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// An object's placement: from its own coordinates, centred on the middle of its box, to its
/// parent's (the canvas, or a group's centre).
fn placement(o: &Value) -> Affine {
    let (w, h) = (num(o, "width", 0.0) + num(o, "strokeWidth", 0.0), num(o, "height", 0.0) + num(o, "strokeWidth", 0.0));
    let (sx, sy) = (num(o, "scaleX", 1.0), num(o, "scaleY", 1.0));
    let angle = num(o, "angle", 0.0).to_radians();
    let rotate = Affine::rotate(angle);
    // `left`/`top` is the box's top-left corner; the object turns about it.
    let centre = Point::new(num(o, "left", 0.0), num(o, "top", 0.0)) + (rotate * Point::new(w * sx / 2.0, h * sy / 2.0)).to_vec2();
    let flip = |f: bool| if f { -1.0 } else { 1.0 };
    Affine::translate(centre.to_vec2()) * rotate * Affine::scale_non_uniform(sx * flip(flag(o, "flipX")), sy * flip(flag(o, "flipY")))
}

/// Line work given as points (a polygon or polyline) or a line's two ends, in its own coordinates.
fn points_path(o: &Value) -> Option<BezPath> {
    let pts: Vec<Point> = if let Some(list) = o.get("points").and_then(Value::as_array) {
        list.iter().filter_map(|p| Some(Point::new(p.get("x")?.as_f64()?, p.get("y")?.as_f64()?))).collect()
    } else if o.get("x1").is_some() {
        vec![Point::new(num(o, "x1", 0.0), num(o, "y1", 0.0)), Point::new(num(o, "x2", 0.0), num(o, "y2", 0.0))]
    } else {
        return None;
    };
    let mut path = BezPath::new();
    path.move_to(*pts.first()?);
    for p in &pts[1..] {
        path.line_to(*p);
    }
    if pts.len() > 2 {
        path.close_path();
    }
    Some(path)
}

impl Reader<'_> {
    fn object(&mut self, o: &Value, parent: Affine, group: Option<usize>, hidden: bool) -> Result<(), String> {
        let hidden = hidden || o.get("visible").and_then(Value::as_bool) == Some(false);
        let before = self.shapes.len();
        self.place(o, parent, group, hidden)?;
        if hidden {
            for s in &mut self.shapes[before..] {
                if s.layer < HIDDEN {
                    s.layer += HIDDEN;
                    self.hidden += 1;
                }
            }
        }
        Ok(())
    }

    fn place(&mut self, o: &Value, parent: Affine, group: Option<usize>, hidden: bool) -> Result<(), String> {
        if num(o, "skewX", 0.0) != 0.0 || num(o, "skewY", 0.0) != 0.0 {
            self.skewed += 1;
        }
        let mtype = o.get("mtype").and_then(Value::as_i64).unwrap_or(0);
        let at = parent * placement(o);
        // The app's own grouping, shared by objects that were grouped together.
        let group = group.or_else(|| {
            let id = o.get("groupId").and_then(Value::as_str).filter(|s| !s.is_empty())?;
            let next = self.groups.len();
            Some(*self.groups.entry(id.to_string()).or_insert(next))
        });
        if let Some(children) = o.get("objects").and_then(Value::as_array) {
            let group = group.or_else(|| {
                let next = self.groups.len();
                Some(*self.groups.entry(format!("#{next}")).or_insert(next))
            });
            // Children are placed about the group's centre, in its (scaled, turned) coordinates.
            for child in children {
                self.object(child, at, group, hidden)?;
            }
            return Ok(());
        }
        let layer = if flag(o, "isCut") {
            CUT_LAYER
        } else if num(o, "paintStyle", 0.0) == 1.0 {
            FILL_LAYER
        } else {
            LINE_LAYER
        };
        let picture = o.get("imageOriginalUri").or_else(|| o.get("srcUri")).and_then(Value::as_str);
        if mtype == IMAGE || (picture.is_some() && o.get("path").is_none()) {
            let Some(uri) = picture else {
                self.skipped_other += 1;
                return Ok(());
            };
            let bytes = self.files.get(uri.trim_start_matches('/')).ok_or_else(|| format!("the picture {uri} is missing from the file"))?;
            let (w, h) = (num(o, "width", 0.0), num(o, "height", 0.0));
            // Sized in the object's own units; the placement scales it to mm.
            let kind = ShapeKind::image(bytes, w, h);
            let (sx, sy) = (num(o, "scaleX", 1.0).abs(), num(o, "scaleY", 1.0).abs());
            let kind = match kind {
                ShapeKind::Image { data, adjust, clip, .. } => ShapeKind::Image { data, width: w * sx, height: h * sy, adjust: bc_doc::ImageAdjust { invert: flag(o, "inverse"), ..adjust }, clip },
                k => k,
            };
            let transform = at * Affine::scale_non_uniform(1.0 / sx.max(1e-9), 1.0 / sy.max(1e-9));
            self.shapes.push(LbShape { kind, transform, layer: PICTURE_LAYER, group });
            return Ok(());
        }
        if TEXTS.contains(&mtype) && o.get("path").is_none() {
            return self.text(o, at, layer, group);
        }
        let local = if mtype == RECTANGLE && o.get("path").is_none() {
            let (w, h) = (num(o, "width", 0.0), num(o, "height", 0.0));
            Some(bc_geom::rect_path(w, h, num(o, "rx", 0.0).min(w / 2.0).min(h / 2.0)))
        } else if mtype == ELLIPSE && o.get("path").is_none() {
            Some(bc_geom::ellipse_path(num(o, "rx", num(o, "width", 0.0) / 2.0), num(o, "ry", num(o, "height", 0.0) / 2.0)))
        } else if let Some(d) = o.get("path").and_then(Value::as_str) {
            let path = BezPath::from_svg(d).map_err(|e| format!("a shape's path couldn't be read: {e}"))?;
            // Path data is centred on the middle of its bounds.
            let c = path.bounding_box().center();
            Some(Affine::translate(-c.to_vec2()) * path)
        } else {
            points_path(o).map(|p| {
                let c = p.bounding_box().center();
                Affine::translate(-c.to_vec2()) * p
            })
        };
        let Some(local) = local else {
            self.skipped_other += 1;
            return Ok(());
        };
        if local.elements().is_empty() {
            return Ok(());
        }
        self.shapes.push(LbShape { kind: ShapeKind::Path(at * local), transform: Affine::IDENTITY, layer, group });
        Ok(())
    }
}

impl Reader<'_> {
    /// Text as editable text in the same font, size and place (laid out by BurnCraft, so it can
    /// sit slightly differently from the app's).
    fn text(&mut self, o: &Value, at: Affine, layer: usize, group: Option<usize>) -> Result<(), String> {
        let text = o.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        if text.trim().is_empty() {
            return Ok(());
        }
        let wanted = o.get("fontFamily").and_then(Value::as_str).unwrap_or("").to_string();
        let bold = o.get("fontWeight").and_then(Value::as_str).is_some_and(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
        let italic = o.get("fontStyle").and_then(Value::as_str).is_some_and(|s| s.contains("italic") || s.contains("oblique"));
        let family = if self.fonts.cap_per_em(&wanted, bold, italic).is_some() {
            wanted
        } else {
            self.missing_fonts.insert(wanted);
            self.fonts.default_family()
        };
        let Some(cap) = self.fonts.cap_per_em(&family, bold, italic) else {
            self.skipped_text += 1;
            return Ok(());
        };
        if num(o, "curvature", 0.0) != 0.0 {
            self.curved += 1;
        }
        // The em size, scaled with the box (the placement then only places and turns it).
        let sy = num(o, "scaleY", 1.0).abs().max(1e-9);
        let em = num(o, "fontSize", 10.0).max(0.1) * sy;
        let style = TextStyle {
            align: match o.get("textAlign").and_then(Value::as_str) {
                Some("center") => TextAlign::Center,
                Some("right") => TextAlign::Right,
                _ => TextAlign::Left,
            },
            letter_spacing: num(o, "charSpacing", 0.0) / 1000.0 * em,
            line_spacing: num(o, "lineHeight", 1.16),
            ..TextStyle::default()
        };
        let height = em * cap;
        let Some(outline) = self.fonts.outline(&text, &family, height, bold, italic, &style) else {
            self.skipped_text += 1;
            return Ok(());
        };
        // The outline is centred on itself; centre it in the app's box. Its size already has the
        // box's height scale, so only a stretch (wider than tall) is left in the transform.
        let transform = at * Affine::scale(1.0 / sy);
        self.shapes.push(LbShape { kind: ShapeKind::Text { text, font: family, height, bold, italic, style, outline }, transform, layer, group });
        Ok(())
    }
}

/// The settings of the app's layers that are used: depth (as speed), power, passes, resolution.
fn layers(options: &Value, used: &[usize]) -> Vec<(usize, Layer)> {
    let list = options.as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for (id, index, mode, name) in [
        ("layerFill", FILL_LAYER, LayerMode::Fill, "Fill"),
        ("layerPicture", PICTURE_LAYER, LayerMode::Image, "Picture"),
        ("layerLine", LINE_LAYER, LayerMode::Line, "Line"),
        ("layerCut", CUT_LAYER, LayerMode::Line, "Cut"),
    ] {
        for (index, hidden) in [(index, false), (index + HIDDEN, true)] {
            if used.contains(&index) {
                out.push((index, layer(&list, id, index, mode, name, hidden)));
            }
        }
    }
    out
}

/// One of the app's layers; `hidden` is the copy for hidden objects, not shown or engraved.
fn layer(list: &[Value], id: &str, index: usize, mode: LayerMode, name: &str, hidden: bool) -> Layer {
    {
        let mut layer = Layer::new(index);
        layer.name = if hidden { format!("{name} (hidden)") } else { name.into() };
        layer.mode = mode;
        layer.visible = !hidden;
        layer.output = !hidden;
        if let Some(o) = list.iter().find(|o| o.get("layerId").and_then(Value::as_str) == Some(id)) {
            layer.speed = bc_lp2::speed_for_depth(num(o, "printDepth", 10.0));
            layer.max_power = num(o, "printPower", 100.0).clamp(1.0, 100.0);
            layer.min_power = 0.0;
            layer.passes = num(o, "printCount", 1.0).round().clamp(1.0, 255.0) as u32;
            let dpi = num(o, "dpi", 254.0);
            if dpi > 0.0 {
                layer.interval = 25.4 / dpi;
            }
        }
        layer.added = true;
        layer
    }
}

/// Parses a LaserPecker Design Space project: shapes in mm on the LP2's work area, on a Fill,
/// Picture, Line or Cut layer, with those layers' settings.
pub fn import(data: &[u8], fonts: &FontLibrary) -> Result<LbProject, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|_| "not a LaserPecker project (it isn't a zip file)".to_string())?;
    let mut files = HashMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        if f.is_dir() {
            continue;
        }
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        files.insert(f.name().to_string(), bytes);
    }
    let project = files.remove(".lpproject").ok_or("not a LaserPecker project (no .lpproject inside)")?;
    let project: Value = serde_json::from_slice(&project).map_err(|e| format!("the project couldn't be read: {e}"))?;
    // The canvas and layer settings are JSON stored as strings.
    let inner = |key: &str| -> Result<Value, String> {
        match project.get(key) {
            Some(Value::String(s)) => serde_json::from_str(s).map_err(|e| format!("the project's {key} couldn't be read: {e}")),
            Some(v) => Ok(v.clone()),
            None => Ok(Value::Null),
        }
    };
    let canvas = inner("data")?;
    let objects = canvas.as_array().or_else(|| canvas.get("objects").and_then(Value::as_array)).ok_or("the project has no objects")?;
    let mut r = Reader { fonts, files, shapes: Vec::new(), groups: HashMap::new(), missing_fonts: Default::default(), curved: 0, skipped_text: 0, hidden: 0, skipped_other: 0, skewed: 0 };
    for o in objects {
        r.object(o, Affine::IDENTITY, None, false)?;
    }
    let mut used: Vec<usize> = r.shapes.iter().map(|s| s.layer).collect();
    used.sort_unstable();
    used.dedup();
    let mut out = LbProject { layers: layers(&inner("laserOptions")?, &used), shapes: r.shapes, warnings: Vec::new() };
    if r.skipped_text > 0 {
        out.warnings.push(format!("{} text object(s) couldn't be laid out (no font found)", r.skipped_text));
    }
    for f in r.missing_fonts.iter().filter(|f| !f.is_empty()) {
        out.warnings.push(format!("font \"{f}\" isn't installed; its text uses {} instead", fonts.default_family()));
    }
    if r.curved > 0 {
        out.warnings.push(format!("{} curved text object(s) came in straight", r.curved));
    }
    if r.hidden > 0 {
        out.warnings.push(format!("{} hidden object(s) came in on hidden layers that aren't engraved", r.hidden));
    }
    if r.skipped_other > 0 {
        out.warnings.push(format!("{} object(s) of a kind BurnCraft can't read were left out", r.skipped_other));
    }
    if r.skewed > 0 {
        out.warnings.push(format!("{} skewed object(s) came in without their skew", r.skewed));
    }
    Ok(out)
}
