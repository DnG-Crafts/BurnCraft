//! Windows metafiles: EMF, and WMF with or without a placeable header, written from Microsoft's
//! published format specifications ([MS-EMF] and [MS-WMF]).
//!
//! A metafile is a recording of drawing calls. The records are played on a small model of the
//! device context they were recorded from (pens, brushes, the current position, the coordinate
//! mapping, the path bracket and the clip), and whatever they fill or stroke comes out as an
//! outline in millimetres, coloured by its pen (or its brush, for a fill without an outline).
//! Clipped art is cut to its clip. Pictures and text aren't drawn: each is reported once.

use crate::{ImportResult, Imported};
use bc_geom::{Affine, BezPath, Point, Rect, Vec2};
use kurbo::Shape as _;
use std::collections::BTreeSet;
use std::f64::consts::TAU;

/// File extensions read as metafiles.
pub const EXTENSIONS: [&str; 2] = ["emf", "wmf"];

/// Most outlines one file makes, and most saved states (a hostile file can't exhaust memory).
const MAX_SHAPES: usize = 1 << 20;
const MAX_SAVED: usize = 1024;
const MAX_POINTS: usize = 1 << 22;
/// Coordinates further than this from the origin (mm) are damaged.
const LIMIT: f64 = 1e6;

const EMF_SIGNATURE: u32 = 0x464D_4520; // " EMF"
const WMF_PLACEABLE: u32 = 0x9AC6_CDD7;

const IMAGES: &str = "pictures in the file were left out (import them separately to engrave them)";
const TEXT: &str = "text in the file was left out (convert it to curves in the program that made it)";
const CUT_OUT_CLIPS: &str = "clip areas cut out of the clip were ignored";
const EMF_PLUS: &str = "the picture is drawn with EMF+ records, which aren't read";

/// Little-endian reading with bounds checks.
struct Bytes<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, at: 0 }
    }
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let v = self.b.get(self.at..self.at + N)?.try_into().ok()?;
        self.at += N;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        self.take().map(u16::from_le_bytes)
    }
    fn i16(&mut self) -> Option<f64> {
        self.take().map(|b| i16::from_le_bytes(b) as f64)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_le_bytes)
    }
    fn i32(&mut self) -> Option<f64> {
        self.take().map(|b| i32::from_le_bytes(b) as f64)
    }
    fn f32(&mut self) -> Option<f64> {
        self.take().map(|b| f32::from_le_bytes(b) as f64)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        (self.at + n <= self.b.len()).then(|| self.at += n)
    }
    /// A COLORREF: red, green, blue, then a byte that's ignored.
    fn color(&mut self) -> Option<[u8; 3]> {
        let [r, g, b, _] = self.take::<4>()?;
        Some([r, g, b])
    }
}

/// What a pen or brush draws with: a colour, or nothing.
type Ink = Option<[u8; 3]>;

#[derive(Clone)]
enum Object {
    Pen(Ink),
    Brush(Ink),
    /// Fonts, palettes, regions: they take a slot but don't change what's drawn.
    Other,
}

/// The device context's state, saved and restored as a whole.
#[derive(Clone)]
struct State {
    pen: Ink,
    brush: Ink,
    /// EMF world transform (logical → page).
    world: Affine,
    map_mode: u32,
    window_org: Vec2,
    window_ext: Vec2,
    viewport_org: Vec2,
    viewport_ext: Vec2,
    /// Arcs run counterclockwise (the default) or clockwise.
    clockwise: bool,
    /// Clip outlines in force (mm); what's drawn is cut to each.
    clips: Vec<BezPath>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            pen: Some([0; 3]),
            brush: Some([255; 3]),
            world: Affine::IDENTITY,
            map_mode: 1,
            window_org: Vec2::ZERO,
            window_ext: Vec2::new(1.0, 1.0),
            viewport_org: Vec2::ZERO,
            viewport_ext: Vec2::new(1.0, 1.0),
            clockwise: false,
            clips: Vec::new(),
        }
    }
}

/// How an arc is closed.
#[derive(Clone, Copy, PartialEq)]
enum ArcShape {
    Open,
    Chord,
    Pie,
}

/// Plays the records and collects the outlines.
struct Canvas {
    /// EMF: millimetres per device unit across and down. WMF: logical units → mm directly.
    emf: bool,
    mm_per_device: Vec2,
    wmf_to_mm: Affine,
    /// The picture's frame (mm): clips that hold all of it clip nothing.
    frame: Rect,
    state: State,
    saved: Vec<State>,
    objects: Vec<Option<Object>>,
    /// The current position (logical).
    position: Point,
    /// The path being recorded (mm), whether a figure is open in it and where it began.
    path: Option<BezPath>,
    figure_open: bool,
    figure_start: Point,
    /// Lines drawn one after another (mm), kept as one outline until something else is drawn.
    pending: BezPath,
    out: Vec<Imported>,
    notes: BTreeSet<&'static str>,
    unread: usize,
}

impl Canvas {
    fn new(emf: bool) -> Self {
        Self {
            emf,
            mm_per_device: Vec2::new(25.4 / 96.0, 25.4 / 96.0),
            wmf_to_mm: Affine::scale(25.4 / 96.0),
            frame: Rect::ZERO,
            state: State::default(),
            saved: Vec::new(),
            objects: Vec::new(),
            position: Point::ZERO,
            path: None,
            figure_open: false,
            figure_start: Point::ZERO,
            pending: BezPath::new(),
            out: Vec::new(),
            notes: BTreeSet::new(),
            unread: 0,
        }
    }

    /// Logical coordinates → millimetres.
    fn to_mm(&self) -> Affine {
        if !self.emf {
            return self.wmf_to_mm;
        }
        let s = &self.state;
        let page_to_device = match s.map_mode {
            // Fixed units, with y up: 0.1 mm, 0.01 mm, 0.01 in, 0.001 in, 1/1440 in.
            2..=6 => {
                let mm = [0.1, 0.01, 0.254, 0.0254, 25.4 / 1440.0][(s.map_mode - 2) as usize];
                let (sx, sy) = (mm / self.mm_per_device.x, mm / self.mm_per_device.y);
                Affine::translate(s.viewport_org) * Affine::scale_non_uniform(sx, -sy) * Affine::translate(-s.window_org)
            }
            // The window's extent maps onto the viewport's (isotropic: the same scale both ways).
            7 | 8 => {
                let ratio = |v: f64, w: f64| if w != 0.0 && v.is_finite() { v / w } else { 1.0 };
                let (mut sx, mut sy) = (ratio(s.viewport_ext.x, s.window_ext.x), ratio(s.viewport_ext.y, s.window_ext.y));
                if s.map_mode == 7 {
                    let m = sx.abs().min(sy.abs());
                    (sx, sy) = (m.copysign(sx), m.copysign(sy));
                }
                Affine::translate(s.viewport_org) * Affine::scale_non_uniform(sx, sy) * Affine::translate(-s.window_org)
            }
            // MM_TEXT: one unit a device pixel, y down.
            _ => Affine::translate(s.viewport_org - s.window_org),
        };
        Affine::scale_non_uniform(self.mm_per_device.x, self.mm_per_device.y) * page_to_device * s.world
    }

    fn note(&mut self, n: &'static str) {
        self.notes.insert(n);
    }

    // ----- objects

    /// Puts `obj` in slot `index` (EMF numbers its objects; slot 0 is reserved).
    fn put(&mut self, index: u32, obj: Object) {
        let i = index as usize;
        if i == 0 || i > u16::MAX as usize {
            return;
        }
        if self.objects.len() <= i {
            self.objects.resize(i + 1, None);
        }
        self.objects[i] = Some(obj);
    }

    /// Puts `obj` in the lowest free slot (WMF numbers them that way).
    fn add(&mut self, obj: Object) {
        match self.objects.iter().position(Option::is_none) {
            Some(i) => self.objects[i] = Some(obj),
            None if self.objects.len() <= u16::MAX as usize => self.objects.push(Some(obj)),
            None => {}
        }
    }

    fn delete(&mut self, index: u32) {
        if let Some(slot) = self.objects.get_mut(index as usize) {
            *slot = None;
        }
    }

    fn select(&mut self, index: u32) {
        self.flush();
        // EMF stock objects have the top bit set.
        if self.emf && index & 0x8000_0000 != 0 {
            let s = &mut self.state;
            match index & 0x7FFF_FFFF {
                0 | 18 => s.brush = Some([255; 3]),
                1 => s.brush = Some([192; 3]),
                2 => s.brush = Some([128; 3]),
                3 => s.brush = Some([64; 3]),
                4 => s.brush = Some([0; 3]),
                5 => s.brush = None,
                6 => s.pen = Some([255; 3]),
                7 | 19 => s.pen = Some([0; 3]),
                8 => s.pen = None,
                _ => {}
            }
            return;
        }
        match self.objects.get(index as usize).cloned().flatten() {
            Some(Object::Pen(p)) => self.state.pen = p,
            Some(Object::Brush(b)) => self.state.brush = b,
            _ => {}
        }
    }

    // ----- drawing

    /// An outline (mm), kept when it's stroked or filled, cut to the clip.
    fn draw(&mut self, outline: BezPath, fill: bool, stroke: bool) {
        if outline.elements().is_empty() || self.out.len() >= MAX_SHAPES {
            return;
        }
        let b = outline.bounding_box();
        if ![b.x0, b.y0, b.x1, b.y1].iter().all(|v| v.is_finite() && v.abs() < LIMIT) {
            return;
        }
        let ink = if stroke { self.state.pen } else { None }.or(if fill { self.state.brush } else { None });
        let Some(color) = ink else { return };
        let mut path = outline;
        for clip in &self.state.clips {
            let closed = bc_geom::curveops::closed_only(&path);
            path = if closed.elements().is_empty() {
                bc_geom::ops::clip_lines(&path, clip)
            } else {
                bc_geom::ops::boolean(&[closed, clip.clone()], bc_geom::ops::BooleanOp::Intersect).unwrap_or_default()
            };
            if path.elements().is_empty() {
                return;
            }
        }
        self.out.push(Imported { path, color: Some(color) });
    }

    /// Draws the lines drawn one after another so far.
    fn flush(&mut self) {
        if !self.pending.elements().is_empty() {
            let lines = std::mem::take(&mut self.pending);
            self.draw(lines, false, true);
        }
    }

    /// A figure in logical units: added to the path being recorded, or drawn (filled when it
    /// encloses an area).
    fn figure(&mut self, logical: BezPath, area: bool) {
        let mm = self.to_mm() * logical;
        match &mut self.path {
            Some(p) => {
                p.extend(mm);
                self.figure_open = false;
            }
            None => {
                self.flush();
                self.draw(mm, area, true);
            }
        }
    }

    /// A polygon, polyline or Bézier run through `pts` (logical).
    fn poly(&mut self, pts: &[Point], closed: bool, bezier: bool) {
        let Some((&first, rest)) = pts.split_first() else { return };
        let mut p = BezPath::new();
        p.move_to(first);
        if bezier {
            rest.as_chunks::<3>().0.iter().for_each(|c| p.curve_to(c[0], c[1], c[2]));
        } else {
            rest.iter().for_each(|q| p.line_to(*q));
        }
        if closed {
            p.close_path();
        }
        self.figure(p, closed);
    }

    /// Lines or Béziers on from the current position (logical), which moves to their end.
    fn on_from_position(&mut self, pts: &[Point], bezier: bool) {
        let Some(&last) = pts.last() else { return };
        let xf = self.to_mm();
        let start = xf * self.position;
        let target = match &mut self.path {
            Some(p) => {
                if !self.figure_open {
                    p.move_to(start);
                    self.figure_open = true;
                    self.figure_start = self.position;
                }
                p
            }
            None => {
                if self.pending.elements().last().and_then(|e| e.end_point()) != Some(start) {
                    self.pending.move_to(start);
                }
                &mut self.pending
            }
        };
        if bezier {
            pts.as_chunks::<3>().0.iter().for_each(|c| target.curve_to(xf * c[0], xf * c[1], xf * c[2]));
        } else {
            pts.iter().for_each(|q| target.line_to(xf * *q));
        }
        self.position = last;
    }

    /// The arc of the ellipse in `bounds` from the ray through `start` to the ray through `end`
    /// (logical), closed as `shape`; with `join`, a line from the current position to its start.
    fn arc(&mut self, bounds: Rect, start: Point, end: Point, shape: ArcShape, join: bool) {
        let r = bounds.abs();
        let (c, radii) = (r.center(), Vec2::new(r.width() / 2.0, r.height() / 2.0));
        if radii.x <= 0.0 || radii.y <= 0.0 {
            return;
        }
        let angle = |p: Point| ((p.y - c.y) / radii.y).atan2((p.x - c.x) / radii.x);
        let (a0, a1) = (angle(start), angle(end));
        // Counterclockwise as seen with y down runs towards smaller angles.
        let sweep = if self.state.clockwise { (a1 - a0).rem_euclid(TAU) } else { -(a0 - a1).rem_euclid(TAU) };
        let sweep = if sweep == 0.0 { if self.state.clockwise { TAU } else { -TAU } } else { sweep };
        let arc = kurbo::Arc { center: c, radii, start_angle: a0, sweep_angle: sweep, x_rotation: 0.0 };
        let first = c + Vec2::new(radii.x * a0.cos(), radii.y * a0.sin());
        let last = c + Vec2::new(radii.x * (a0 + sweep).cos(), radii.y * (a0 + sweep).sin());
        if join {
            self.on_from_position(&[first], false);
            let pts: Vec<Point> = arc.append_iter(1e-3 * radii.x.max(radii.y)).flat_map(|el| match el {
                kurbo::PathEl::CurveTo(a, b, p) => vec![a, b, p],
                _ => vec![],
            }).collect();
            self.on_from_position(&pts, true);
            self.position = last;
            return;
        }
        let mut p = BezPath::new();
        if shape == ArcShape::Pie {
            p.move_to(c);
            p.line_to(first);
        } else {
            p.move_to(first);
        }
        arc.append_iter(1e-3 * radii.x.max(radii.y)).for_each(|el| p.push(el));
        if shape != ArcShape::Open {
            p.close_path();
        }
        self.figure(p, shape != ArcShape::Open);
    }

    fn rounded(&mut self, bounds: Rect, corner: Vec2) {
        let r = bounds.abs();
        let (rx, ry) = ((corner.x / 2.0).abs().min(r.width() / 2.0), (corner.y / 2.0).abs().min(r.height() / 2.0));
        if rx <= 0.0 || ry <= 0.0 {
            return self.figure(r.to_path(0.01), true);
        }
        // A rounded rectangle with circular corners of radius `ry`, squeezed to elliptical ones.
        let k = ry / rx;
        let squeezed = Rect::new(r.x0 * k, r.y0, r.x1 * k, r.y1);
        let p = Affine::scale_non_uniform(1.0 / k, 1.0) * kurbo::RoundedRect::from_rect(squeezed, ry).to_path(0.01);
        self.figure(p, true);
    }

    // ----- paths and clips

    fn paint_path(&mut self, fill: bool, stroke: bool) {
        if let Some(mut p) = self.path.take() {
            if fill && self.figure_open {
                p.close_path();
            }
            self.draw(p, fill, stroke);
        }
    }

    fn close_figure(&mut self) {
        if let Some(p) = &mut self.path
            && self.figure_open
        {
            p.close_path();
            self.figure_open = false;
            self.position = self.figure_start;
        }
    }

    /// Adds a clip outline (mm): `replace` starts the clip afresh.
    fn clip(&mut self, outline: BezPath, replace: bool) {
        if replace {
            self.state.clips.clear();
        }
        // A rectangle round the whole picture (what writers often clip to first) clips nothing.
        let b = outline.bounding_box();
        if self.frame.area() > 0.0 && b.inflate(0.05, 0.05).contains(self.frame.origin()) && b.inflate(0.05, 0.05).contains(Point::new(self.frame.x1, self.frame.y1)) && outline.elements().len() <= 6 {
            return;
        }
        if self.state.clips.len() < 16 {
            self.state.clips.push(outline);
        }
    }

    fn save(&mut self) {
        if self.saved.len() < MAX_SAVED {
            self.saved.push(self.state.clone());
        }
    }

    /// `RestoreDC(n)`: negative counts back from the latest save; positive is a save's number.
    fn restore(&mut self, n: i32) {
        let len = self.saved.len();
        let keep = if n < 0 { len.checked_sub(n.unsigned_abs() as usize) } else { (n as usize).checked_sub(1).filter(|k| *k < len) };
        if let Some(k) = keep {
            self.saved.truncate(k + 1);
            if let Some(s) = self.saved.pop() {
                self.state = s;
            }
        }
    }

    fn finish(mut self) -> Result<ImportResult, String> {
        self.flush();
        let mut warnings: Vec<String> = self.notes.iter().map(|s| s.to_string()).collect();
        if self.unread > 0 {
            warnings.push(format!("{} record(s) of kinds BurnCraft doesn't read were skipped", self.unread));
        }
        if self.out.is_empty() {
            return Err(warnings.first().cloned().unwrap_or_else(|| "the picture has no outlines to import".into()));
        }
        Ok(ImportResult { shapes: self.out, warnings })
    }
}

// ---------------------------------------------------------------- EMF

fn rect32(r: &mut Bytes) -> Option<Rect> {
    Some(Rect::new(r.i32()?, r.i32()?, r.i32()?, r.i32()?))
}

fn point32(r: &mut Bytes) -> Option<Point> {
    Some(Point::new(r.i32()?, r.i32()?))
}

/// `count` points, 32- or 16-bit.
fn points(r: &mut Bytes, count: u32, small: bool) -> Option<Vec<Point>> {
    let n = count as usize;
    if n > MAX_POINTS || r.b.len().saturating_sub(r.at) < n * if small { 4 } else { 8 } {
        return None;
    }
    (0..n).map(|_| if small { Some(Point::new(r.i16()?, r.i16()?)) } else { point32(r) }).collect()
}

/// The polygons of a PolyPolygon / PolyPolyline record (after its bounds).
fn poly_polys(r: &mut Bytes, small: bool) -> Option<Vec<Vec<Point>>> {
    let n = r.u32()? as usize;
    let _total = r.u32()?;
    if n > MAX_POINTS {
        return None;
    }
    let counts: Vec<u32> = (0..n).map(|_| r.u32()).collect::<Option<_>>()?;
    counts.into_iter().map(|c| points(r, c, small)).collect()
}

fn xform(r: &mut Bytes) -> Option<Affine> {
    // eM11, eM12, eM21, eM22, eDx, eDy: x' = x·eM11 + y·eM21 + eDx, y' = x·eM12 + y·eM22 + eDy.
    let v = [r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?];
    v.iter().all(|x| x.is_finite()).then(|| Affine::new(v))
}

/// Plays one EMF record (`kind`, its data after the 8-byte header); `None` if it's damaged.
fn emf_record(c: &mut Canvas, kind: u32, data: &[u8]) -> Option<()> {
    let mut r = Bytes::new(data);
    match kind {
        // Polygons and lines: bounds, count, points (2–6 32-bit, 85–89 16-bit).
        2..=6 | 85..=89 => {
            let small = kind >= 85;
            let k = if small { kind - 83 } else { kind };
            r.skip(16)?;
            let n = r.u32()?;
            let pts = points(&mut r, n, small)?;
            match k {
                2 => c.poly(&pts, false, true),
                3 => c.poly(&pts, true, false),
                4 => c.poly(&pts, false, false),
                5 => c.on_from_position(&pts, true),
                _ => c.on_from_position(&pts, false),
            }
        }
        // PolyPolyline / PolyPolygon (7, 8; 90, 91 16-bit).
        7 | 8 | 90 | 91 => {
            r.skip(16)?;
            let polys = poly_polys(&mut r, kind >= 90)?;
            let closed = matches!(kind, 8 | 91);
            let mut p = BezPath::new();
            for poly in &polys {
                let Some((&first, rest)) = poly.split_first() else { continue };
                p.move_to(first);
                rest.iter().for_each(|q| p.line_to(*q));
                if closed {
                    p.close_path();
                }
            }
            c.figure(p, closed);
        }
        9 => c.state.window_ext = Vec2::new(r.i32()?, r.i32()?),
        10 => c.state.window_org = Vec2::new(r.i32()?, r.i32()?),
        11 => c.state.viewport_ext = Vec2::new(r.i32()?, r.i32()?),
        12 => c.state.viewport_org = Vec2::new(r.i32()?, r.i32()?),
        17 => c.state.map_mode = r.u32()?,
        27 => {
            c.position = point32(&mut r)?;
            c.figure_open = false;
        }
        30 => {
            let rect = rect32(&mut r)?;
            let outline = c.to_mm() * rect.abs().to_path(0.01);
            c.clip(outline, false);
        }
        // ScaleViewportExtEx / ScaleWindowExtEx: x num, x denom, y num, y denom.
        31 | 32 => {
            let [xn, xd, yn, yd] = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
            let ext = if kind == 31 { &mut c.state.viewport_ext } else { &mut c.state.window_ext };
            if xd != 0.0 && yd != 0.0 {
                *ext = Vec2::new(ext.x * xn / xd, ext.y * yn / yd);
            }
        }
        33 => c.save(),
        34 => c.restore(r.i32()? as i32),
        35 => c.state.world = xform(&mut r)?,
        36 => {
            let x = xform(&mut r)?;
            // 1 reset, 2 left-multiply (x applies first), 3 right-multiply, 4 set.
            c.state.world = match r.u32()? {
                1 => Affine::IDENTITY,
                2 => c.state.world * x,
                3 => x * c.state.world,
                _ => x,
            };
        }
        37 => c.select(r.u32()?),
        // CreatePen: slot, style, width (x, y), colour.
        38 => {
            let slot = r.u32()?;
            let style = r.u32()?;
            r.skip(8)?;
            let color = r.color()?;
            c.put(slot, Object::Pen((style & 0xF != 5).then_some(color)));
        }
        // CreateBrushIndirect: slot, style, colour, hatch.
        39 => {
            let slot = r.u32()?;
            let style = r.u32()?;
            let color = r.color()?;
            c.put(slot, Object::Brush(brush(style, color)));
        }
        40 => c.delete(r.u32()?),
        42 => {
            let b = rect32(&mut r)?.abs();
            c.figure(kurbo::Ellipse::from_rect(b).to_path(1e-3 * b.width().max(b.height()).max(1.0)), true);
        }
        43 => c.figure(rect32(&mut r)?.abs().to_path(0.01), true),
        44 => {
            let b = rect32(&mut r)?;
            let corner = Vec2::new(r.i32()?, r.i32()?);
            c.rounded(b, corner);
        }
        // Arc, Chord, Pie, ArcTo: box, start, end.
        45..=47 | 55 => {
            let b = rect32(&mut r)?;
            let (s, e) = (point32(&mut r)?, point32(&mut r)?);
            let shape = [ArcShape::Open, ArcShape::Chord, ArcShape::Pie].get((kind as usize).wrapping_sub(45)).copied().unwrap_or(ArcShape::Open);
            c.arc(b, s, e, shape, kind == 55);
        }
        54 => {
            let p = point32(&mut r)?;
            c.on_from_position(&[p], false);
        }
        57 => c.state.clockwise = r.u32()? == 2,
        59 => {
            c.flush();
            c.path = Some(BezPath::new());
            c.figure_open = false;
        }
        61 => c.close_figure(),
        62 => c.paint_path(true, false),
        63 => c.paint_path(true, true),
        64 => c.paint_path(false, true),
        67 => {
            if let Some(p) = c.path.take() {
                match r.u32()? {
                    1 => c.clip(p, false),
                    5 => c.clip(p, true),
                    _ => c.note(CUT_OUT_CLIPS),
                }
            }
        }
        68 => c.path = None,
        // GDI comment: EMF+ records ride in these.
        70 => {
            let _size = r.u32()?;
            if r.u32()? == 0x2B46_4D45 {
                c.note(EMF_PLUS);
            }
        }
        // Fonts, palettes and pattern brushes take an object slot.
        82 | 49 => c.put(r.u32()?, Object::Other),
        93 | 94 => c.put(r.u32()?, Object::Brush(Some([128; 3]))),
        // ExtCreatePen: slot, then (after the bitmap offsets) style, width, brush style, colour.
        95 => {
            let slot = r.u32()?;
            r.skip(16)?;
            let style = r.u32()?;
            let _width = r.u32()?;
            let brush_style = r.u32()?;
            let color = r.color()?;
            let visible = style & 0xF != 5 && brush_style != 1;
            c.put(slot, Object::Pen(visible.then_some(color)));
        }
        // Text.
        83 | 84 | 96 | 97 => c.note(TEXT),
        // Pictures and bitmap copies.
        76..=81 | 114 | 116 => c.note(IMAGES),
        // Settings that don't change outlines: background, text and blending modes, colour
        // management, palettes, comments, the brush origin, miter limit, layout.
        13 | 15 | 16 | 18..=26 | 28 | 29 | 48 | 50..=53 | 58 | 60 | 98..=113 | 115 | 118..=122 => {}
        _ => c.unread += 1,
    }
    Some(())
}

/// A LOGBRUSH's ink: solid and hatched draw in their colour, a null brush draws nothing, and
/// patterns are taken as grey.
fn brush(style: u32, color: [u8; 3]) -> Ink {
    match style {
        0 | 2 => Some(color),
        1 => None,
        _ => Some([128; 3]),
    }
}

fn read_emf(bytes: &[u8]) -> Result<ImportResult, String> {
    let mut c = Canvas::new(true);
    let mut h = Bytes::new(bytes);
    h.skip(8).ok_or("damaged header")?;
    let _bounds = rect32(&mut h).ok_or("damaged header")?;
    // The frame, in 0.01 mm.
    let frame = rect32(&mut h).ok_or("damaged header")?;
    c.frame = Rect::new(frame.x0 / 100.0, frame.y0 / 100.0, frame.x1 / 100.0, frame.y1 / 100.0);
    h.at = 72;
    // The reference device: its size in pixels, then in millimetres.
    if let (Some(px_x), Some(px_y), Some(mm_x), Some(mm_y)) = (h.i32(), h.i32(), h.i32(), h.i32())
        && px_x > 0.0
        && px_y > 0.0
        && mm_x > 0.0
        && mm_y > 0.0
    {
        c.mm_per_device = Vec2::new(mm_x / px_x, mm_y / px_y);
    }
    let mut at = 0usize;
    while at + 8 <= bytes.len() {
        let mut r = Bytes::new(&bytes[at..]);
        let (kind, size) = (r.u32().unwrap_or(0), r.u32().unwrap_or(0) as usize);
        if size < 8 || at + size > bytes.len() {
            break;
        }
        if kind == 14 {
            break;
        }
        if kind != 1 && emf_record(&mut c, kind, &bytes[at + 8..at + size]).is_none() {
            c.unread += 1;
        }
        at += size;
    }
    c.finish()
}

// ---------------------------------------------------------------- WMF

/// Plays one WMF record (`function`, its parameters); `None` if it's damaged.
fn wmf_record(c: &mut Canvas, function: u16, params: &[u8]) -> Option<()> {
    let mut r = Bytes::new(params);
    // WMF gives coordinates as 16-bit numbers, most with y before x.
    let yx = |r: &mut Bytes| -> Option<Point> {
        let y = r.i16()?;
        Some(Point::new(r.i16()?, y))
    };
    // Rectangles are given bottom, right, top, left.
    let rect = |r: &mut Bytes| -> Option<Rect> {
        let (b, rt, t, l) = (r.i16()?, r.i16()?, r.i16()?, r.i16()?);
        Some(Rect::new(l, t, rt, b))
    };
    match function {
        0x0214 => {
            c.position = yx(&mut r)?;
        }
        0x0213 => {
            let p = yx(&mut r)?;
            c.on_from_position(&[p], false);
        }
        // Polygon / Polyline: count, then x, y pairs.
        0x0324 | 0x0325 => {
            let n = r.u16()? as u32;
            let pts = points(&mut r, n, true)?;
            c.poly(&pts, function == 0x0324, false);
        }
        // PolyPolygon: number of polygons, their point counts, the points.
        0x0538 => {
            let n = r.u16()? as usize;
            let counts: Vec<u16> = (0..n).map(|_| r.u16()).collect::<Option<_>>()?;
            let mut p = BezPath::new();
            for k in counts {
                let pts = points(&mut r, k as u32, true)?;
                let Some((&first, rest)) = pts.split_first() else { continue };
                p.move_to(first);
                rest.iter().for_each(|q| p.line_to(*q));
                p.close_path();
            }
            c.figure(p, true);
        }
        0x041B => c.figure(rect(&mut r)?.abs().to_path(0.01), true),
        0x0418 => {
            let b = rect(&mut r)?.abs();
            c.figure(kurbo::Ellipse::from_rect(b).to_path(1e-3 * b.width().max(b.height()).max(1.0)), true);
        }
        // RoundRect: corner height, corner width, then the rectangle.
        0x061C => {
            let (h, w) = (r.i16()?, r.i16()?);
            let b = rect(&mut r)?;
            c.rounded(b, Vec2::new(w, h));
        }
        // Arc, Pie, Chord: end, start (each y, x), then the rectangle.
        0x0817 | 0x081A | 0x0830 => {
            let (e, s) = (yx(&mut r)?, yx(&mut r)?);
            let b = rect(&mut r)?;
            let shape = match function {
                0x081A => ArcShape::Pie,
                0x0830 => ArcShape::Chord,
                _ => ArcShape::Open,
            };
            c.arc(b, s, e, shape, false);
        }
        0x0416 => {
            let b = rect(&mut r)?;
            let outline = c.to_mm() * b.abs().to_path(0.01);
            c.clip(outline, false);
        }
        0x001E => c.save(),
        0x0127 => c.restore(r.i16()? as i32),
        // CreatePenIndirect: style, width (x, y), colour.
        0x02FA => {
            let style = r.u16()?;
            r.skip(4)?;
            let color = r.color()?;
            c.add(Object::Pen((style & 0xF != 5).then_some(color)));
        }
        // CreateBrushIndirect: style, colour, hatch.
        0x02FC => {
            let style = r.u16()? as u32;
            let color = r.color()?;
            c.add(Object::Brush(brush(style, color)));
        }
        // Pattern brushes, fonts, palettes and regions take an object slot.
        0x01F9 | 0x0142 => c.add(Object::Brush(Some([128; 3]))),
        0x02FB | 0x00F7 | 0x06FF => c.add(Object::Other),
        0x012D => c.select(r.u16()? as u32),
        0x01F0 => c.delete(r.u16()? as u32),
        // Text.
        0x0521 | 0x0A32 => c.note(TEXT),
        // Pictures and bitmap copies.
        0x0940 | 0x0B41 | 0x0F43 | 0x0922 | 0x0B23 | 0x0D33 => c.note(IMAGES),
        // Window and viewport settings (the placeable header gives the scale), modes, colours,
        // palettes and escapes: nothing to draw.
        0x020B | 0x020C | 0x020D | 0x020E | 0x0211 | 0x0410 | 0x0412 | 0x0103 | 0x0102 | 0x0201 | 0x0209 | 0x012E | 0x0106 | 0x0104 | 0x0107 | 0x0234 | 0x0035 | 0x0626 | 0x0231 | 0x0108 | 0x012C => {}
        _ => c.unread += 1,
    }
    Some(())
}

fn read_wmf(bytes: &[u8]) -> Result<ImportResult, String> {
    let mut c = Canvas::new(false);
    let mut h = Bytes::new(bytes);
    let mut start = 0;
    // The placeable header: key, handle, bounds (left, top, right, bottom), units per inch.
    if h.u32() == Some(WMF_PLACEABLE) {
        h.skip(2).ok_or("damaged header")?;
        let (l, t, r, b) = (h.i16().ok_or("damaged header")?, h.i16().ok_or("damaged header")?, h.i16().ok_or("damaged header")?, h.i16().ok_or("damaged header")?);
        let inch = h.u16().filter(|&u| u > 0).unwrap_or(1440) as f64;
        let k = 25.4 / inch;
        c.wmf_to_mm = Affine::scale(k) * Affine::translate((-l.min(r), -t.min(b)));
        c.frame = Rect::new(0.0, 0.0, (r - l).abs() * k, (b - t).abs() * k);
        start = 22;
    }
    // The metafile header: type, header size in 16-bit words (9), …
    let mut m = Bytes::new(&bytes[start.min(bytes.len())..]);
    let _type = m.u16().ok_or("damaged header")?;
    let words = m.u16().ok_or("damaged header")? as usize;
    let mut at = start + words * 2;
    while at + 6 <= bytes.len() {
        let mut r = Bytes::new(&bytes[at..]);
        let size = r.u32().unwrap_or(0) as usize * 2;
        let function = r.u16().unwrap_or(0);
        if function == 0 || size < 6 || at + size > bytes.len() {
            break;
        }
        if wmf_record(&mut c, function, &bytes[at + 6..at + size]).is_none() {
            c.unread += 1;
        }
        at += size;
    }
    c.finish()
}

/// Reads an EMF or WMF file into outlines (mm).
pub fn import(bytes: &[u8]) -> Result<ImportResult, String> {
    let u32_at = |at: usize| bytes.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    let u16_at = |at: usize| bytes.get(at..at + 2).and_then(|b| b.try_into().ok()).map(u16::from_le_bytes);
    if u32_at(0) == Some(1) && u32_at(40) == Some(EMF_SIGNATURE) {
        return read_emf(bytes);
    }
    // A WMF: placeable, or a bare header (type 1 or 2, 9 words, version 0x0100 or 0x0300).
    if u32_at(0) == Some(WMF_PLACEABLE) || (matches!(u16_at(0), Some(1 | 2)) && u16_at(2) == Some(9) && matches!(u16_at(4), Some(0x0100 | 0x0300))) {
        return read_wmf(bytes);
    }
    Err("not an EMF or WMF file".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32s(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
    fn i32s(v: &[i32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
    fn i16s(v: &[i16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// An EMF with a header (a 96 px-to-the-inch reference device), the records given and an end.
    fn emf(records: Vec<Vec<u8>>) -> Vec<u8> {
        let mut h = u32s(&[1, 108]);
        h.extend(i32s(&[0, 0, 96, 96, 0, 0, 2540, 2540]));
        // Signature, version, bytes, records, handles, descriptions (count, offset), palette.
        h.extend(u32s(&[EMF_SIGNATURE, 0x10000, 0, 0, 3, 0, 0, 0]));
        h.extend(i32s(&[1000, 1000, 264, 264]));
        h.resize(108, 0);
        let mut all = vec![h];
        all.extend(records);
        all.push(u32s(&[14, 20, 0, 16, 20]));
        all.concat()
    }

    #[test]
    fn an_emf_rectangle_comes_in_at_its_size_in_its_pen_colour() {
        let pen = [u32s(&[38, 28, 1, 0]), i32s(&[1, 0]), u32s(&[0x0000_00ff])].concat();
        let select = u32s(&[37, 12, 1]);
        let rect = [u32s(&[43, 24]), i32s(&[0, 0, 96, 48])].concat();
        let r = import(&emf(vec![pen, select, rect])).unwrap();
        assert_eq!(r.shapes.len(), 1, "{:?}", r.warnings);
        assert_eq!(r.shapes[0].color, Some([255, 0, 0]));
        let b = r.shapes[0].path.bounding_box();
        // 96 × 48 px on a 1000 px / 264 mm device: 25.34 × 12.67 mm.
        assert!((b.width() - 25.344).abs() < 0.01 && (b.height() - 12.672).abs() < 0.01, "{b:?}");
    }

    #[test]
    fn an_emf_path_is_filled_with_the_brush_and_cut_to_its_clip() {
        let null_pen = u32s(&[37, 12, 0x8000_0008]);
        let blue = [u32s(&[39, 24, 1, 0]), u32s(&[0x00ff_0000, 0])].concat();
        let select = u32s(&[37, 12, 1]);
        let clip = [u32s(&[30, 24]), i32s(&[0, 0, 48, 96])].concat();
        let begin = u32s(&[59, 8]);
        let poly = [u32s(&[86, 44]), i32s(&[0, 0, 0, 0]), u32s(&[4]), i16s(&[0, 0, 96, 0, 96, 96, 0, 96])].concat();
        let end = u32s(&[60, 8]);
        let fill = [u32s(&[62, 24]), i32s(&[0, 0, 0, 0])].concat();
        let r = import(&emf(vec![null_pen, blue, select, clip, begin, poly, end, fill])).unwrap();
        assert_eq!(r.shapes.len(), 1, "{:?}", r.warnings);
        assert_eq!(r.shapes[0].color, Some([0, 0, 255]));
        let b = r.shapes[0].path.bounding_box();
        assert!((b.width() - 12.672).abs() < 0.01 && (b.height() - 25.344).abs() < 0.01, "the left half: {b:?}");
    }

    #[test]
    fn a_placeable_wmf_polygon_and_lines_come_in_at_size() {
        // Placeable header: 1440 units to the inch, bounds 0, 0 – 1440, 720.
        let mut f = u32s(&[WMF_PLACEABLE]);
        f.extend(i16s(&[0, 0, 0, 1440, 720]));
        f.extend([0xA0, 0x05]); // 1440
        f.extend(u32s(&[0]));
        f.extend([0, 0]);
        // Metafile header: type 1, 9 words, version 0x0300, size, objects, max record, 0.
        f.extend(i16s(&[1, 9, 0x0300]));
        f.extend(u32s(&[0]));
        f.extend(i16s(&[2]));
        f.extend(u32s(&[0]));
        f.extend(i16s(&[0]));
        // A green pen, selected.
        f.extend([u32s(&[8]), i16s(&[0x02FA, 0, 1, 0]), u32s(&[0x0000_ff00])].concat());
        f.extend([u32s(&[4]), i16s(&[0x012D, 0])].concat());
        // A triangle, and a line drawn with MoveTo / LineTo.
        f.extend([u32s(&[10]), i16s(&[0x0324, 3, 0, 0, 1440, 0, 0, 720])].concat());
        f.extend([u32s(&[5]), i16s(&[0x0214, 0, 0])].concat());
        f.extend([u32s(&[5]), i16s(&[0x0213, 720, 1440])].concat());
        f.extend([u32s(&[3]), i16s(&[0])].concat());
        let r = import(&f).unwrap();
        assert_eq!(r.shapes.len(), 2, "{:?}", r.warnings);
        assert!(r.shapes.iter().all(|s| s.color == Some([0, 255, 0])));
        let b = r.shapes[0].path.bounding_box();
        assert!((b.width() - 25.4).abs() < 1e-6 && (b.height() - 12.7).abs() < 1e-6, "{b:?}");
    }

    #[test]
    fn other_files_are_refused() {
        assert!(import(b"not a metafile at all").is_err());
    }
}
