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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_font_outline_has_requested_cap_height() {
        let fonts = FontLibrary::load_system();
        let family = fonts.default_family();
        if family.is_empty() {
            return; // no fonts on this machine (e.g. minimal CI)
        }
        let style = TextStyle::default();
        let path = fonts.outline("H", &family, 10.0, false, false, &style).expect("outline");
        let b = bc_geom::bounds(&path).unwrap();
        assert!((b.height() - 10.0).abs() < 0.3, "cap height {}", b.height());
        assert!(b.center().distance(Point::ORIGIN) < 1e-6);
        assert!(fonts.outline("   ", &family, 10.0, false, false, &style).is_none());
    }

    /// Prints each contour's bounds for `BC_FONT_PROBE` ("family|text"), welded and not.
    #[test]
    #[ignore = "diagnostic"]
    fn probe() {
        let fonts = FontLibrary::load_system();
        let spec = std::env::var("BC_FONT_PROBE").unwrap();
        let (family, text) = spec.split_once('|').unwrap();
        for weld in [false, true] {
            let p = fonts.outline(text, family, 25.0, false, false, &TextStyle { weld, ..Default::default() }).unwrap();
            println!("weld {weld}: total {:?}", bc_geom::bounds(&p).unwrap());
            for l in bc_geom::flatten(&p, 0.01) {
                let (x0, x1) = l.points.iter().fold((f64::MAX, f64::MIN), |(a, b), q| (a.min(q.x), b.max(q.x)));
                let (y0, y1) = l.points.iter().fold((f64::MAX, f64::MIN), |(a, b), q| (a.min(q.y), b.max(q.y)));
                println!("  {} pts closed {} x {x0:.2}..{x1:.2} y {y0:.2}..{y1:.2}", l.points.len(), l.closed);
            }
        }
    }

    /// Welding "abcdefg" in Berlin Sans FB Demi used to leave sliver triangles on the "d".
    #[test]
    fn weld_leaves_no_slivers() {
        let fonts = FontLibrary::load_system();
        let family = "Berlin Sans FB Demi";
        if !fonts.families().iter().any(|f| f == family) {
            return;
        }
        let plain = fonts.outline("abcdefg", family, 25.0, false, false, &TextStyle::default()).unwrap();
        let welded = fonts.outline("abcdefg", family, 25.0, false, false, &TextStyle { weld: true, ..Default::default() }).unwrap();
        let rings = |p: &BezPath| bc_geom::flatten(p, 0.01).len();
        assert_eq!(rings(&welded), rings(&plain), "no letters overlap, so welding adds no rings");
    }

    /// Writes stencilled "BOAT 80", a rubber stamp of "AB", and single-line text as polylines to `BC_TOOLS_DUMP`.
    #[test]
    #[ignore = "diagnostic"]
    fn dump_tools() {
        use bc_doc::{edit, Document, ShapeKind};
        let fonts = FontLibrary::load_system();
        let family = fonts.default_family();
        let text = |doc: &mut Document, s: &str, family: &str, at: (f64, f64)| {
            let outline = fonts.outline(s, family, 20.0, true, false, &TextStyle::default()).unwrap();
            doc.add_shape(ShapeKind::Text { text: s.into(), font: family.into(), height: 20.0, bold: true, italic: false, style: TextStyle::default(), outline }, Affine::translate(at), 0)
        };
        let mut doc = Document::default();
        let a = text(&mut doc, "BOAT 80", &family, (60.0, 20.0));
        edit::stencil(&mut doc, &[a].into(), 1.5, false);
        let b = text(&mut doc, "AB", &family, (30.0, 60.0));
        let stamp = edit::rubber_stamp(&doc, &[b].into(), 4.0, 0.8).unwrap();
        let c = text(&mut doc, "Single line 123", single_line::FAMILY, (70.0, 100.0));
        let mut paths: Vec<BezPath> = vec![doc.shape(a).unwrap().world_path(), Affine::translate((60.0, 0.0)) * stamp.background, Affine::translate((60.0, 0.0)) * stamp.outline];
        if let Some(s) = stamp.shoulder {
            paths.push(Affine::translate((60.0, 0.0)) * s);
        }
        paths.push(doc.shape(c).unwrap().world_path());
        let mut out = String::new();
        for p in paths {
            for l in bc_geom::flatten(&p, 0.02) {
                out += &l.points.iter().map(|q| format!("{:.2},{:.2}", q.x, q.y)).collect::<Vec<_>>().join(" ");
                out += if l.closed { " Z\n" } else { "\n" };
            }
        }
        std::fs::write(std::env::var("BC_TOOLS_DUMP").unwrap(), out).unwrap();
    }

    #[test]
    fn single_line_text_is_open_strokes() {
        let fonts = FontLibrary::from_database(Database::new());
        assert_eq!(fonts.families()[0], single_line::FAMILY);
        let p = fonts.outline("Hello 42", single_line::FAMILY, 10.0, false, false, &TextStyle::default()).unwrap();
        let lines = bc_geom::flatten(&p, 0.01);
        assert!(!lines.is_empty() && lines.iter().filter(|l| l.closed).count() < lines.len() / 2, "mostly open strokes");
        let b = bc_geom::bounds(&p).unwrap();
        assert!(b.center().distance(Point::ORIGIN) < 1e-6);
        // "H" is exactly the cap height.
        let h = bc_geom::bounds(&fonts.outline("H", single_line::FAMILY, 10.0, false, false, &TextStyle::default()).unwrap()).unwrap();
        assert!((h.height() - 10.0).abs() < 1e-9 && h.width() > 6.0);
        // Letter spacing adds exactly one gap per pair.
        let w = |s: f64| bc_geom::bounds(&fonts.outline("HH", single_line::FAMILY, 10.0, false, false, &TextStyle { letter_spacing: s, ..Default::default() }).unwrap()).unwrap().width();
        assert!((w(3.0) - w(0.0) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn pair_kerning_letters_and_characters() {
        let fonts = FontLibrary::from_database(Database::new());
        let f = single_line::FAMILY;
        let plain = TextStyle::default();
        let width = |style: &TextStyle| bc_geom::bounds(&fonts.outline("HHH", f, 10.0, false, false, style).unwrap()).unwrap().width();
        // 2 mm more after the first letter only.
        let kerned = TextStyle { kerning: vec![(0, 2.0)], ..Default::default() };
        assert!((width(&kerned) - width(&plain) - 2.0).abs() < 1e-9);
        // Kerning after the last letter of a line, or past the end, changes nothing inside it.
        assert!((width(&TextStyle { kerning: vec![(2, 5.0), (40, 5.0)], ..Default::default() }) - width(&plain)).abs() < 1e-9);
        // Letters land exactly where the whole text puts them.
        let letters = fonts.letters("A B", f, 10.0, false, false, &plain).unwrap();
        assert_eq!(letters.iter().map(|(c, _)| *c).collect::<String>(), "AB", "the space has no outline");
        let whole = bc_geom::bounds(&fonts.outline("A B", f, 10.0, false, false, &plain).unwrap()).unwrap();
        let (a, b) = (bc_geom::bounds(&letters[0].1).unwrap(), bc_geom::bounds(&letters[1].1).unwrap());
        assert!((a.x0 - whole.x0).abs() < 1e-9 && (b.x1 - whole.x1).abs() < 1e-9);
        // The single-line font's characters are its printable ASCII.
        let chars = fonts.characters(f);
        assert!(chars.contains(&'A') && chars.contains(&'~') && !chars.contains(&' '));
    }

    #[test]
    fn text_around_a_circle() {
        let fonts = FontLibrary::load_system();
        let family = fonts.default_family();
        if family.is_empty() {
            return;
        }
        let r = 30.0;
        let curve = |bottom| TextStyle { curve: Some(bc_doc::TextCurve::Circle { radius: r, bottom }), ..Default::default() };
        let radii = |p: &BezPath| {
            let pts: Vec<Point> = bc_geom::flatten(p, 0.05).into_iter().flat_map(|l| l.points).collect();
            let d: Vec<f64> = pts.iter().map(|q| q.distance(Point::ORIGIN)).collect();
            (d.iter().cloned().fold(f64::MAX, f64::min), d.iter().cloned().fold(f64::MIN, f64::max))
        };
        let top = fonts.outline("HELLO", &family, 5.0, false, false, &curve(false)).unwrap();
        let (lo, hi) = radii(&top);
        assert!(lo > r - 0.2 && hi < r + 5.5, "letters stand outside the circle: {lo:.2}..{hi:.2}");
        let b = bc_geom::bounds(&top).unwrap();
        assert!(b.center().x.abs() < 1.0 && b.y1 < 0.0, "centred over the top: {b:?}");

        let bottom = fonts.outline("HELLO", &family, 5.0, false, false, &curve(true)).unwrap();
        let (lo, hi) = radii(&bottom);
        assert!(lo > r - 5.5 && hi < r + 0.2, "letters stand inside, upright: {lo:.2}..{hi:.2}");
        let b = bc_geom::bounds(&bottom).unwrap();
        assert!(b.center().x.abs() < 1.0 && b.y0 > 0.0, "centred under the bottom: {b:?}");
    }

    #[test]
    fn text_along_a_path() {
        let fonts = FontLibrary::load_system();
        let family = fonts.default_family();
        if family.is_empty() {
            return;
        }
        // A straight horizontal guide: same letters as straight text, starting at the offset.
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((200.0, 0.0));
        let style = |offset, flip| TextStyle { align: TextAlign::Left, curve: Some(bc_doc::TextCurve::Path { path: line.clone(), offset, flip }), ..Default::default() };
        let on = fonts.outline("Path", &family, 8.0, false, false, &style(20.0, false)).unwrap();
        let straight = fonts.outline("Path", &family, 8.0, false, false, &TextStyle { align: TextAlign::Left, ..Default::default() }).unwrap();
        let (a, b) = (bc_geom::bounds(&on).unwrap(), bc_geom::bounds(&straight).unwrap());
        assert!((a.width() - b.width()).abs() < 0.05 && (a.height() - b.height()).abs() < 0.05, "same size: {a:?} vs {b:?}");
        assert!((a.x0 - 20.0).abs() < 1.5, "starts at the offset: {a:?}");
        assert!(a.y1 <= 0.3 && a.y0 < -5.0, "stands above the guide (round letters overshoot it slightly): {a:?}");
        // Flipped: runs the other way, below the line.
        let flipped = bc_geom::bounds(&fonts.outline("Path", &family, 8.0, false, false, &style(20.0, true)).unwrap()).unwrap();
        assert!(flipped.y0 >= -0.3 && flipped.y1 > 5.0 && flipped.x1 > 175.0, "flipped: {flipped:?}");
    }

    #[test]
    fn alignment_and_spacing() {
        let fonts = FontLibrary::load_system();
        let family = fonts.default_family();
        if family.is_empty() {
            return;
        }
        let width_of = |text: &str, style: TextStyle| bc_geom::bounds(&fonts.outline(text, &family, 10.0, false, false, &style).unwrap()).unwrap();
        // "I" on the second line sits at the left, centre or right of the wider first line.
        let x_of_i = |align| {
            let p = fonts.outline("WWWW\nI", &family, 10.0, false, false, &TextStyle { align, ..Default::default() }).unwrap();
            let lines = bc_geom::flatten(&p, 0.01);
            
            lines.iter().filter(|l| l.points.iter().all(|q| q.y > 2.0)).flat_map(|l| &l.points).map(|q| q.x).fold(f64::MAX, f64::min)
        };
        assert!(x_of_i(TextAlign::Left) < x_of_i(TextAlign::Center) && x_of_i(TextAlign::Center) < x_of_i(TextAlign::Right));
        let tight = width_of("ABC", TextStyle::default()).width();
        let loose = width_of("ABC", TextStyle { letter_spacing: 2.0, ..Default::default() }).width();
        assert!((loose - tight - 4.0).abs() < 1e-6, "two gaps of 2 mm: {tight} → {loose}");
        let one = width_of("A\nA", TextStyle::default()).height();
        let double = width_of("A\nA", TextStyle { line_spacing: 2.0, ..Default::default() }).height();
        assert!(double > one * 1.4);
    }
}
