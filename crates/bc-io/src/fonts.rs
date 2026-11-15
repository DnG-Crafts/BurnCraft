//! System fonts and text-to-outline conversion.

use crate::single_line;
use bc_doc::{TextAlign, TextStyle};
use bc_geom::{Affine, BezPath, Point};
use std::sync::Arc;
use usvg::fontdb::{Database, Family, Query, Stretch, Style, Weight};

/// The system font database, shared with the SVG importer.
pub struct FontLibrary {
    db: Arc<Database>,
    families: Vec<String>,
}

impl FontLibrary {
    /// Loads the system's installed fonts.
    pub fn load_system() -> Self {
        let mut db = Database::new();
        db.load_system_fonts();
        Self::from_database(db)
    }

    pub fn from_database(db: Database) -> Self {
        let mut families: Vec<String> = db.faces().flat_map(|f| f.families.iter().map(|(name, _)| name.clone())).collect();
        families.sort_by_key(|f| f.to_lowercase());
        families.dedup();
        // The built-in single-line font comes first.
        families.insert(0, single_line::FAMILY.to_string());
        Self { db: Arc::new(db), families }
    }

    pub fn database(&self) -> Arc<Database> {
        self.db.clone()
    }

    /// Installed family names, sorted.
    pub fn families(&self) -> &[String] {
        &self.families
    }

    /// A sensible default family that exists on this system.
    pub fn default_family(&self) -> String {
        ["Arial", "Segoe UI", "Liberation Sans", "DejaVu Sans", "Helvetica"]
            .iter()
            .find(|f| self.families.iter().any(|x| x == *f))
            .map(|f| f.to_string())
            .or_else(|| self.families.first().cloned())
            .unwrap_or_default()
    }

    /// Outlines `text` (multi-line allowed) so that capital letters are `height` mm tall, laid out
    /// with `style`. The result is centred on the origin. Returns `None` if the font is missing or the text is blank.
    pub fn outline(&self, text: &str, family: &str, height: f64, bold: bool, italic: bool, style: &TextStyle) -> Option<BezPath> {
        let (letters, single_line) = self.placed_letters(text, family, height, bold, italic, style)?;
        let mut path = BezPath::new();
        // Welding merges overlapping letters into one outline (strokes have no outline to merge).
        let welded = (style.weld && !single_line && letters.len() > 1)
            .then(|| bc_geom::ops::boolean(&letters.iter().map(|(_, g)| g.clone()).collect::<Vec<_>>(), bc_geom::ops::BooleanOp::Union))
            .flatten();
        match welded {
            Some(w) => path = w,
            None => letters.into_iter().for_each(|(_, g)| path.extend(g)),
        }
        bc_geom::bounds(&path)?;
        Some(path)
    }

    /// A font's capital height as a fraction of its em size (the size other programs give
    /// fonts in). `None` if the font isn't installed.
    pub fn cap_per_em(&self, family: &str, bold: bool, italic: bool) -> Option<f64> {
        if family == single_line::FAMILY {
            return Some(0.7);
        }
        let id = self.find(family, bold, italic)?;
        self.db.with_face_data(id, |data, index| {
            let face = ttf_parser::Face::parse(data, index).ok()?;
            let cap = face.capital_height().filter(|&c| c > 0).map(f64::from).unwrap_or(face.ascender() as f64 * 0.7);
            Some(cap / f64::from(face.units_per_em()))
        })?
    }

    /// Each letter of `text` as its own outline, placed exactly where [`Self::outline`] puts it
    /// (spaces and missing characters left out). For splitting text into letters.
    pub fn letters(&self, text: &str, family: &str, height: f64, bold: bool, italic: bool, style: &TextStyle) -> Option<Vec<(char, BezPath)>> {
        Some(self.placed_letters(text, family, height, bold, italic, style)?.0)
    }

    /// The letters laid out and centred as the whole text is; and whether it's the single-line
    /// font (strokes).
    fn placed_letters(&self, text: &str, family: &str, height: f64, bold: bool, italic: bool, style: &TextStyle) -> Option<(Vec<(char, BezPath)>, bool)> {
        let single_line = family == single_line::FAMILY;
        let mut letters = if single_line {
            let letters = layout(&Source::SingleLine, text, height, style);
            // Strokes have no weight; italic slants them.
            if italic { letters.into_iter().map(|(c, g)| (c, Affine::new([1.0, 0.0, -0.2, 1.0, 0.0, 0.0]) * g)).collect() } else { letters }
        } else {
            let id = self.find(family, bold, italic)?;
            self.db.with_face_data(id, |data, index| {
                let face = ttf_parser::Face::parse(data, index).ok()?;
                Some(layout(&Source::Font(&face), text, height, style))
            })??
        };
        let mut all = BezPath::new();
        letters.iter().for_each(|(_, g)| all.extend(g.clone()));
        let bounds = bc_geom::bounds(&all)?;
        // Curved text stays in its guide's coordinates (circle text: centred on the circle);
        // straight text is centred on the origin.
        if style.curve.is_none() {
            let shift = Affine::translate(-bounds.center().to_vec2());
            letters.iter_mut().for_each(|(_, g)| *g = shift * g.clone());
        }
        Some((letters, single_line))
    }

    /// Every character `family` has a glyph for (no control characters), in order.
    pub fn characters(&self, family: &str) -> Vec<char> {
        if family == single_line::FAMILY {
            return ('!'..='~').filter(|c| single_line::glyph(*c).is_some()).collect();
        }
        let Some(id) = self.find(family, false, false) else { return Vec::new() };
        let mut chars: Vec<char> = self
            .db
            .with_face_data(id, |data, index| {
                let face = ttf_parser::Face::parse(data, index).ok()?;
                let mut out = Vec::new();
                for sub in face.tables().cmap?.subtables {
                    if sub.is_unicode() {
                        sub.codepoints(|cp| {
                            if let Some(c) = char::from_u32(cp)
                                && !c.is_control()
                                && !c.is_whitespace()
                                && face.glyph_index(c).is_some()
                            {
                                out.push(c);
                            }
                        });
                    }
                }
                Some(out)
            })
            .flatten()
            .unwrap_or_default();
        chars.sort_unstable();
        chars.dedup();
        chars
    }
}

impl FontLibrary {
    /// The installed face for `family` in the given weight and slant.
    fn find(&self, family: &str, bold: bool, italic: bool) -> Option<usvg::fontdb::ID> {
        self.db.query(&Query {
            families: &[Family::Name(family)],
            weight: if bold { Weight::BOLD } else { Weight::NORMAL },
            stretch: Stretch::Normal,
            style: if italic { Style::Italic } else { Style::Normal },
        })
    }
}

/// Where glyphs come from: an installed font, or the built-in single-line font.
enum Source<'a> {
    Font(&'a ttf_parser::Face<'a>),
    SingleLine,
}

/// Lays the text out, one outline per letter (in order), before centring.
fn layout(source: &Source, text: &str, height: f64, style: &TextStyle) -> Vec<(char, BezPath)> {
    let (scale, line_height) = match source {
        Source::Font(face) => {
            let cap = face.capital_height().filter(|&c| c > 0).map(f64::from).unwrap_or(face.ascender() as f64 * 0.7);
            let scale = height / cap;
            (scale, (face.ascender() as f64 - face.descender() as f64 + face.line_gap() as f64) * scale)
        }
        Source::SingleLine => {
            let scale = height / single_line::CAP;
            (scale, single_line::LINE * scale)
        }
    };
    let line_height = line_height * style.line_spacing.max(0.1);
    let text = if style.upper_case { text.to_uppercase() } else { text.to_string() };
    // Pair kerning: extra space after the letter at each index of the whole text.
    let extra = |index: usize| style.kerning.iter().filter(|(i, _)| *i == index).map(|(_, d)| *d).sum::<f64>();
    // Lay out each line at x = 0 (one path per glyph), remembering its advance width for alignment.
    let mut lines: Vec<(Vec<(char, BezPath)>, f64)> = Vec::new();
    let mut index = 0;
    for (row, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let mut glyphs = Vec::new();
        let mut x = 0.0;
        let baseline = row as f64 * line_height;
        let mut prev: Option<ttf_parser::GlyphId> = None;
        for ch in line.chars() {
            let here = index;
            index += 1;
            match source {
                Source::Font(face) => {
                    let Some(gid) = face.glyph_index(ch) else { continue };
                    if let (Some(p), Some(kern)) = (prev, face.tables().kern)
                        && let Some(k) = kern.subtables.into_iter().filter(|s| s.horizontal && !s.variable).find_map(|s| s.glyphs_kerning(p, gid)) {
                            x += k as f64 * scale;
                        }
                    // Font units are Y-up; the document is Y-down.
                    let to_doc = Affine::new([scale, 0.0, 0.0, -scale, x, baseline]);
                    let mut builder = Builder(BezPath::new());
                    if face.outline_glyph(gid, &mut builder).is_some() {
                        glyphs.push((ch, to_doc * builder.0));
                    }
                    x += face.glyph_hor_advance(gid).unwrap_or(0) as f64 * scale + style.letter_spacing + extra(here);
                    prev = Some(gid);
                }
                Source::SingleLine => {
                    let Some((strokes, advance)) = single_line::glyph(ch) else { continue };
                    if !strokes.elements().is_empty() {
                        glyphs.push((ch, Affine::new([scale, 0.0, 0.0, scale, x, baseline]) * strokes));
                    }
                    x += advance * scale + style.letter_spacing + extra(here);
                }
            }
        }
        // The single-line font's advance includes the gap after the last letter.
        let trailing = if matches!(source, Source::SingleLine) { single_line::GAP * scale } else { 0.0 };
        lines.push((glyphs, (x - style.letter_spacing - trailing).max(0.0)));
        index += 1; // the line break
    }
    let widest = lines.iter().map(|(_, w)| *w).fold(0.0, f64::max);
    let mut placed: Vec<(char, BezPath)> = Vec::new();
    for (glyphs, w) in lines {
        let shift = match style.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => (widest - w) / 2.0,
            TextAlign::Right => widest - w,
        };
        placed.extend(glyphs.into_iter().map(|(c, g)| (c, Affine::translate((shift, 0.0)) * g)));
    }
    if let Some(curve) = &style.curve {
        // Distance along the guide of the text's anchor (its start, middle or end).
        let anchor = match style.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => widest / 2.0,
            TextAlign::Right => widest,
        };
        let guide = Guide::new(curve);
        placed = placed
            .into_iter()
            .map(|(c, g)| {
                let cx = bc_geom::bounds(&g).map(|b| b.center().x).unwrap_or(0.0);
                let (p, angle) = guide.at(cx - anchor);
                (c, Affine::translate(p.to_vec2()) * Affine::rotate(angle) * Affine::translate((-cx, 0.0)) * g)
            })
            .collect();
    }
    placed
}

/// Where curved text goes: a point and direction for each distance along the guide.
enum Guide {
    Circle { radius: f64, bottom: bool },
    /// A polyline with cumulative lengths; `start` is where the text anchor sits.
    Path { points: Vec<Point>, lengths: Vec<f64>, closed: bool, start: f64 },
}

impl Guide {
    fn new(curve: &bc_doc::TextCurve) -> Self {
        match curve {
            bc_doc::TextCurve::Circle { radius, bottom } => Guide::Circle { radius: radius.max(0.1), bottom: *bottom },
            bc_doc::TextCurve::Path { path, offset, flip } => {
                let line = bc_geom::flatten(path, 0.02).into_iter().next();
                let closed = line.as_ref().is_some_and(|l| l.closed);
                let mut points = line.map(|l| l.points).unwrap_or_default();
                if closed && let Some(&first) = points.first() {
                    points.push(first);
                }
                if *flip {
                    points.reverse();
                }
                let mut lengths = vec![0.0];
                for w in points.windows(2) {
                    lengths.push(lengths.last().unwrap() + w[0].distance(w[1]));
                }
                Guide::Path { points, lengths, closed, start: *offset }
            }
        }
    }

    /// Point and baseline direction (radians) `s` mm from the anchor. Letters stand on the left of
    /// the direction of travel (their bottoms face right), which on screen keeps them upright.
    fn at(&self, s: f64) -> (Point, f64) {
        match self {
            Guide::Circle { radius, bottom: false } => {
                // Over the top, clockwise: the anchor is at the top of the circle.
                let phi = -std::f64::consts::FRAC_PI_2 + s / radius;
                (Point::new(radius * phi.cos(), radius * phi.sin()), phi + std::f64::consts::FRAC_PI_2)
            }
            Guide::Circle { radius, bottom: true } => {
                // Along the bottom, left to right (anticlockwise): the anchor is at the bottom.
                let phi = std::f64::consts::FRAC_PI_2 - s / radius;
                (Point::new(radius * phi.cos(), radius * phi.sin()), phi - std::f64::consts::FRAC_PI_2)
            }
            Guide::Path { points, lengths, closed, start } => {
                let total = *lengths.last().unwrap_or(&0.0);
                if points.len() < 2 || total <= 0.0 {
                    return (points.first().copied().unwrap_or(Point::ORIGIN), 0.0);
                }
                let mut d = start + s;
                d = if *closed { d.rem_euclid(total) } else { d.clamp(0.0, total) };
                let i = lengths.partition_point(|&l| l <= d).clamp(1, points.len() - 1);
                let (a, b) = (points[i - 1], points[i]);
                let seg = (lengths[i] - lengths[i - 1]).max(1e-12);
                let p = a.lerp(b, (d - lengths[i - 1]) / seg);
                (p, (b - a).atan2())
            }
        }
    }
}

struct Builder(BezPath);

impl ttf_parser::OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(Point::new(x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(Point::new(x as f64, y as f64));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(Point::new(x1 as f64, y1 as f64), Point::new(x as f64, y as f64));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.curve_to(Point::new(x1 as f64, y1 as f64), Point::new(x2 as f64, y2 as f64), Point::new(x as f64, y as f64));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}
