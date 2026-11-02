//! Import of RDWorks project files (`.rld`, header `RDFILEVER8.0.01`).
//!
//! The format isn't published; this is how files saved by RDWorks V8.01.65 are laid out:
//!
//! - Little-endian; lengths in mm (f64), Y measured up from the bottom of the work area.
//! - A header with the work area size and one Pascal string (the text tool's default text);
//!   everything after that string moves with its length.
//! - Two fixed-size tables of palette-indexed defaults, then the **layer list**: one record per
//!   layer in the panel (1153 bytes for Laser Cut, 1011 for Laser Scan) with its colour, output
//!   switch, speed, powers and (scan layers) line interval.
//! - The **objects**: a list (a count, then the items, with a 195-byte gap ending in the next
//!   item's header between them) of paths (lines and cubic Béziers), bitmaps (an embedded BMP
//!   placed by its bounding box), text (its outline paths) and groups (a bounding box, then a list
//!   of their own). Each item names its layer by the layer's colour.

use crate::lightburn::{LbProject, LbShape};
use bc_doc::{Layer, LayerMode, ShapeKind};
use bc_geom::{Affine, BezPath, Point};

pub const EXTENSIONS: [&str; 1] = ["rld"];

const MAGIC: &[u8] = b"\x0fRDFILEVER8.0.01";
const OFF_WORK: usize = 20;
const OFF_TEXT: usize = 254;
/// End of the two fixed tables, for a 4-character header string.
const TAIL: usize = 554_249;
/// The layer list starts this far before the end of the tables.
const LAYER_LIST_BACK: usize = 833;
const LAYER_CUT_SIZE: usize = 1153;
const LAYER_SCAN_SIZE: usize = 1011;
/// Scan layers: the line interval (f64, mm) in the layer record.
const SCAN_INTERVAL_AT: usize = 855;
const SEGMENT_SIZE: usize = 74;
/// Dots come in as circles this size (mm).
const DOT_RADIUS: f64 = 0.1;
/// Between paths: 182 bytes not understood, then the next path's 13-byte header.
const PATH_GAP: usize = 195;

struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn get(&self, at: usize, n: usize) -> Result<&[u8], String> {
        self.0.get(at..at + n).ok_or_else(|| "the file ends early".to_string())
    }
    fn u8(&self, at: usize) -> Result<u8, String> {
        Ok(self.get(at, 1)?[0])
    }
    fn u32(&self, at: usize) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.get(at, 4)?.try_into().expect("4 bytes")))
    }
    fn f64(&self, at: usize) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.get(at, 8)?.try_into().expect("8 bytes")))
    }
}

/// A layer from the layer list.
struct RdLayer {
    scan: bool,
    rgb: [u8; 3],
    output: bool,
    speed: f64,
    min_power: f64,
    max_power: f64,
    interval: Option<f64>,
}

/// Item kinds, from the first two bytes of an item header.
const KIND_PATH: u16 = 1;
const KIND_BITMAP: u16 = 2;
/// A dot: a bounding box of no size, then the point. The next item follows it without a gap.
const KIND_DOT: u16 = 3;
/// A group: its bounding box, then a list of its members.
const KIND_GROUP: u16 = 4;
/// Text: the string and its properties, then a list of its outline paths.
const KIND_TEXT: u16 = 5;

/// Whether a 13-byte item header for a path, bitmap, dot or group starts at `at`:
/// `kind 00 kind 00 00 | u32 object number | R G B 00`.
fn is_item_header(b: &[u8], at: usize) -> bool {
    let Some(h) = b.get(at..at + 13) else { return false };
    let number = u32::from_le_bytes([h[5], h[6], h[7], h[8]]);
    matches!(h[0], 1..=4) && h[1] == 0 && h[2] == h[0] && h[3] == 0 && h[4] == 0 && (1..=1_000_000).contains(&number) && h[12] == 0
}

/// Whether an object list starts at `at`: a u32 count, then the first item's header.
fn is_list_start(b: &[u8], at: usize) -> bool {
    let Some(h) = b.get(at..at + 7) else { return false };
    let count = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
    if !(1..=10_000).contains(&count) {
        return false;
    }
    if u16::from_le_bytes([h[4], h[5]]) == KIND_TEXT { (1..=0x7f).contains(&h[6]) } else { is_item_header(b, at + 4) }
}

/// A shape read from the object stream: its kind and placement, layer colour and group.
struct Item {
    kind: ShapeKind,
    transform: Affine,
    rgb: [u8; 3],
    group: Option<usize>,
}

/// Reads the object stream: lists of items, where a group's members are a list of their own.
struct Stream<'a> {
    b: Bytes<'a>,
    /// Height of RDWorks' work area, to turn its Y-up positions into ours.
    height: f64,
    items: Vec<Item>,
    groups: usize,
    /// Dots, which come in as tiny circles.
    dots: usize,
}

impl Stream<'_> {
    fn point(&self, x: f64, y: f64) -> Point {
        Point::new(x, self.height - y)
    }

    /// A list at `at`; returns where it ends. Between items there's usually a 195-byte gap ending
    /// in the next item's header; after a dot the next header follows at once. After a picture or
    /// text the gap isn't known, so the next header is looked for.
    fn list(&mut self, at: usize, group: Option<usize>) -> Result<usize, String> {
        let n = self.b.u32(at)?;
        let mut head = at + 4;
        for k in 0..n {
            let end = self.item(head, group)?;
            if k + 1 == n {
                return Ok(end);
            }
            let next = end + PATH_GAP - 13;
            head = if is_item_header(self.b.0, next) {
                next
            } else if is_item_header(self.b.0, end) {
                end
            } else {
                (end..self.b.0.len()).find(|&i| is_item_header(self.b.0, i)).ok_or_else(|| format!("an object after byte {end} couldn't be found"))?
            };
        }
        Ok(head)
    }

    /// The item whose header starts at `head`; returns where it ends.
    fn item(&mut self, head: usize, group: Option<usize>) -> Result<usize, String> {
        let kind = u16::from_le_bytes([self.b.u8(head)?, self.b.u8(head + 1)?]);
        if kind == KIND_TEXT {
            let len = self.b.u8(head + 2)? as usize;
            let outlines = (head + 3 + len..self.b.0.len()).find(|&i| is_list_start(self.b.0, i)).ok_or("a text object without outlines")?;
            return self.list(outlines, group);
        }
        let h = self.b.get(head, 13)?;
        let rgb = [h[9], h[10], h[11]];
        let body = head + 13;
        match kind {
            KIND_PATH => {
                let (path, end) = self.path(body)?;
                if let Some(path) = path {
                    self.items.push(Item { kind: ShapeKind::Path(path), transform: Affine::IDENTITY, rgb, group });
                }
                Ok(end)
            }
            KIND_BITMAP => {
                let (x0, x1, y0, y1) = self.bbox(body)?;
                let bmp_at = body + 32;
                if self.b.get(bmp_at, 2)? != b"BM" {
                    return Err(format!("a picture at byte {bmp_at} couldn't be read"));
                }
                let size = self.b.u32(bmp_at + 2)? as usize;
                let bmp = self.b.get(bmp_at, size)?;
                let (w, h) = (x1 - x0, y1 - y0);
                if w > 0.0 && h > 0.0 {
                    let centre = self.point((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                    self.items.push(Item { kind: ShapeKind::image(bmp, w, h), transform: Affine::translate(centre.to_vec2()), rgb, group });
                }
                Ok(bmp_at + size)
            }
            KIND_DOT => {
                self.bbox(body)?;
                let at = self.point(self.b.f64(body + 32)?, self.b.f64(body + 40)?);
                // BurnCraft has no dot shape: a 0.2 mm circle marks the spot.
                self.items.push(Item { kind: ShapeKind::Ellipse { radius_x: DOT_RADIUS, radius_y: DOT_RADIUS }, transform: Affine::translate(at.to_vec2()), rgb, group });
                self.dots += 1;
                Ok(body + 48)
            }
            KIND_GROUP => {
                self.bbox(body)?;
                // Shapes are grouped by the outermost group they're in.
                let group = group.or_else(|| {
                    self.groups += 1;
                    Some(self.groups - 1)
                });
                self.list(body + 32, group)
            }
            other => Err(format!("an object of an unknown kind ({other}) at byte {head}")),
        }
    }

    /// A bounding box: x min, x max, y min, y max (mm).
    fn bbox(&self, at: usize) -> Result<(f64, f64, f64, f64), String> {
        let (x0, x1, y0, y1) = (self.b.f64(at)?, self.b.f64(at + 8)?, self.b.f64(at + 16)?, self.b.f64(at + 24)?);
        if [x0, x1, y0, y1].iter().all(|v| v.is_finite()) && x0 <= x1 && y0 <= y1 {
            Ok((x0, x1, y0, y1))
        } else {
            Err(format!("an object at byte {at} couldn't be read"))
        }
    }

    /// A path body: its bounding box, closed flag and segments (lines and cubic Béziers stored as
    /// P0, P3, C1, C2). Returns the path (None if it has no segments) and where it ends.
    fn path(&self, at: usize) -> Result<(Option<BezPath>, usize), String> {
        self.bbox(at)?;
        let mut at = at + 32;
        let closed = self.b.u32(at)? != 0;
        let segments = self.b.u32(at + 4)? as usize;
        at += 8;
        if segments > 1_000_000 {
            return Err(format!("a path at byte {at} couldn't be read"));
        }
        let mut path = BezPath::new();
        for s in 0..segments {
            let d = |i: usize| self.b.f64(at + 6 + 8 * i);
            let (p0, p3) = (self.point(d(0)?, d(1)?), self.point(d(2)?, d(3)?));
            if s == 0 {
                path.move_to(p0);
            }
            match self.b.u8(at)? {
                3 => path.curve_to(self.point(d(4)?, d(5)?), self.point(d(6)?, d(7)?), p3),
                _ => path.line_to(p3),
            }
            at += SEGMENT_SIZE;
        }
        if closed && segments > 0 {
            path.close_path();
        }
        Ok(((segments > 0).then_some(path), at + 6))
    }
}

/// Parses an RDWorks project: layer settings by layer index, and shapes in screen coordinates
/// (mm from the top-left of RDWorks' work area).
pub fn import(data: &[u8]) -> Result<LbProject, String> {
    if !data.starts_with(MAGIC) {
        return Err("not an RDWorks project of a version BurnCraft can read (RDFILEVER8.0.01)".into());
    }
    let b = Bytes(data);
    let height = b.f64(OFF_WORK + 8)?;
    if !(height.is_finite() && height > 0.0) {
        return Err("the work area size couldn't be read".into());
    }
    let shift = b.u8(OFF_TEXT)? as isize - 4;
    let list = (TAIL - LAYER_LIST_BACK) as isize + shift;
    let list = usize::try_from(list).map_err(|_| "the file is too short".to_string())?;

    // The layer list runs until the object list starts.
    let mut layers = Vec::new();
    let mut p = list;
    while !is_list_start(data, p) {
        let mode = b.u8(p)?;
        let size = match mode {
            0 => LAYER_CUT_SIZE,
            1 => LAYER_SCAN_SIZE,
            _ => return Err(format!("a layer of an unknown kind ({mode}) at byte {p}")),
        };
        let rgb = b.get(p + 4, 3)?;
        layers.push(RdLayer {
            scan: mode == 1,
            rgb: [rgb[0], rgb[1], rgb[2]],
            output: b.u8(p + 8)? != 0,
            speed: b.f64(p + 12)?,
            min_power: b.f64(p + 20)?,
            max_power: b.f64(p + 28)?,
            interval: if mode == 1 { Some(b.f64(p + SCAN_INTERVAL_AT)?) } else { None },
        });
        p += size;
    }

    let mut stream = Stream { b, height, items: Vec::new(), groups: 0, dots: 0 };
    stream.list(p, None)?;

    let mut out = LbProject::default();
    if stream.dots > 0 {
        out.warnings.push(format!("{} dot(s) came in as {} mm circles", stream.dots, 2.0 * DOT_RADIUS));
    }
    let mut unknown_colours = 0;
    let mut has_bitmap = vec![false; layers.len()];
    for item in stream.items {
        let layer = match layers.iter().position(|l| l.rgb == item.rgb) {
            Some(i) if i < bc_doc::CUT_LAYER_COUNT => i,
            _ => {
                unknown_colours += 1;
                0
            }
        };
        if matches!(item.kind, ShapeKind::Image { .. })
            && let Some(h) = has_bitmap.get_mut(layer)
        {
            *h = true;
        }
        out.shapes.push(LbShape { kind: item.kind, transform: item.transform, layer, group: item.group });
    }

    for (i, l) in layers.iter().enumerate().take(bc_doc::CUT_LAYER_COUNT) {
        let mut layer = Layer::new(i);
        layer.custom_color = Some(l.rgb).filter(|c| *c != Layer::color(i));
        layer.mode = match (l.scan, has_bitmap[i]) {
            (false, _) => LayerMode::Line,
            (true, true) => LayerMode::Image,
            (true, false) => LayerMode::Fill,
        };
        if l.speed.is_finite() && l.speed > 0.0 {
            layer.speed = l.speed;
        }
        if l.min_power.is_finite() && l.max_power.is_finite() {
            layer.min_power = l.min_power.clamp(0.0, 100.0);
            layer.max_power = l.max_power.clamp(0.0, 100.0);
        }
        if let Some(interval) = l.interval.filter(|v| v.is_finite() && *v > 0.0) {
            layer.interval = interval;
        }
        layer.output = l.output;
        layer.added = true;
        out.layers.push((i, layer));
    }
    if layers.len() > bc_doc::CUT_LAYER_COUNT {
        out.warnings.push(format!("only the first {} layers were read", bc_doc::CUT_LAYER_COUNT));
    }
    if unknown_colours > 0 {
        out.warnings.push(format!("{unknown_colours} shape(s) had a colour that isn't one of the layers, and went on layer 01"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 13-byte item header.
    fn header(kind: u8, number: u8, rgb: [u8; 3]) -> Vec<u8> {
        vec![kind, 0, kind, 0, 0, number, 0, 0, 0, rgb[0], rgb[1], rgb[2], 0]
    }

    /// A path body: a closed 20 × 10 rectangle with its corner at (x, y), Y up.
    fn rect(x: f64, y: f64) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [x, x + 20.0, y, y + 10.0] {
            b.extend(v.to_le_bytes());
        }
        b.extend(1u32.to_le_bytes());
        b.extend(4u32.to_le_bytes());
        let corners = [(x, y), (x + 20.0, y), (x + 20.0, y + 10.0), (x, y + 10.0)];
        for i in 0..4 {
            let (a, c) = (corners[i], corners[(i + 1) % 4]);
            let mut seg = vec![0u8; SEGMENT_SIZE];
            for (k, v) in [a.0, a.1, c.0, c.1].into_iter().enumerate() {
                seg[6 + 8 * k..14 + 8 * k].copy_from_slice(&v.to_le_bytes());
            }
            b.extend(seg);
        }
        b.extend([0u8; 6]);
        b
    }

    /// A project: the header, the tables (empty), one Laser Cut layer, then `objects`.
    fn file(rgb: [u8; 3], text: &str, objects: Vec<u8>) -> Vec<u8> {
        let shift = text.len() as isize - 4;
        let list = ((TAIL - LAYER_LIST_BACK) as isize + shift) as usize;
        let mut b = vec![0u8; list];
        b[..MAGIC.len()].copy_from_slice(MAGIC);
        b[OFF_WORK..OFF_WORK + 8].copy_from_slice(&600.0f64.to_le_bytes());
        b[OFF_WORK + 8..OFF_WORK + 16].copy_from_slice(&400.0f64.to_le_bytes());
        b[OFF_TEXT] = text.len() as u8;
        b[OFF_TEXT + 1..OFF_TEXT + 1 + text.len()].copy_from_slice(text.as_bytes());
        let mut layer = vec![0u8; LAYER_CUT_SIZE];
        layer[4..7].copy_from_slice(&rgb);
        layer[8] = 1;
        for (at, v) in [(12, 85.0f64), (20, 15.0), (28, 40.0)] {
            layer[at..at + 8].copy_from_slice(&v.to_le_bytes());
        }
        b.extend(layer);
        b.extend(objects);
        b.extend([0u8; 234]);
        b
    }

    /// One rectangle with its corner at (100, 50).
    fn sample(rgb: [u8; 3], text: &str) -> Vec<u8> {
        let mut objects = 1u32.to_le_bytes().to_vec();
        objects.extend(header(1, 1, rgb));
        objects.extend(rect(100.0, 50.0));
        file(rgb, text, objects)
    }

    #[test]
    fn dots_come_in_as_tiny_circles() {
        // A dot at (30, 40), then a rectangle straight after it (no gap after a dot).
        let rgb = [0, 0, 0];
        let mut objects = 2u32.to_le_bytes().to_vec();
        objects.extend(header(3, 1, rgb));
        for v in [30.0f64, 30.0, 40.0, 40.0, 30.0, 40.0] {
            objects.extend(v.to_le_bytes());
        }
        objects.extend(header(1, 2, rgb));
        objects.extend(rect(100.0, 50.0));
        let p = import(&file(rgb, "Text", objects)).unwrap();
        assert_eq!(p.shapes.len(), 2);
        assert!(matches!(p.shapes[0].kind, ShapeKind::Ellipse { radius_x, .. } if radius_x == DOT_RADIUS));
        assert_eq!(p.shapes[0].transform.translation(), bc_geom::Vec2::new(30.0, 360.0), "Y flips in a 400 mm area");
        assert!(matches!(p.shapes[1].kind, ShapeKind::Path(_)));
        assert_eq!(p.warnings.len(), 1);
    }

    #[test]
    fn groups_hold_their_members() {
        // Two items: a group of one rectangle, then a rectangle on its own.
        let rgb = [0, 0, 255];
        let mut objects = 2u32.to_le_bytes().to_vec();
        objects.extend(header(4, 1, [255; 3]));
        for v in [10.0f64, 30.0, 10.0, 20.0] {
            objects.extend(v.to_le_bytes());
        }
        objects.extend(1u32.to_le_bytes());
        objects.extend(header(1, 1, rgb));
        objects.extend(rect(10.0, 10.0));
        objects.extend([0u8; PATH_GAP - 13]);
        objects.extend(header(1, 2, rgb));
        objects.extend(rect(200.0, 100.0));
        let p = import(&file(rgb, "Text", objects)).unwrap();
        let groups: Vec<Option<usize>> = p.shapes.iter().map(|s| s.group).collect();
        assert_eq!(groups, [Some(0), None]);
        assert!(p.shapes.iter().all(|s| s.layer == 0));
    }

    #[test]
    fn reads_layers_and_a_rectangle() {
        for text in ["Text", "Longer text"] {
            let p = import(&sample([0, 0, 255], text)).unwrap();
            assert_eq!(p.layers.len(), 1);
            let l = &p.layers[0].1;
            assert_eq!((l.mode, l.speed, l.min_power, l.max_power, l.output), (LayerMode::Line, 85.0, 15.0, 40.0, true));
            assert_eq!(l.rgb(0), [0, 0, 255]);
            assert_eq!(p.shapes.len(), 1);
            let ShapeKind::Path(path) = &p.shapes[0].kind else { panic!("a path") };
            let bb = bc_geom::bounds(path).unwrap();
            // Y flips: 50..60 up from the bottom of a 400 mm area is 340..350 down from the top.
            assert_eq!((bb.x0, bb.y0, bb.x1, bb.y1), (100.0, 340.0, 120.0, 350.0));
            assert!(p.warnings.is_empty());
        }
    }

    #[test]
    fn other_files_are_refused() {
        assert!(import(b"\x0fRDFILEVER7.0.00 and so on").is_err());
        let mut short = sample([0, 0, 0], "Text");
        short.truncate(TAIL - 900);
        assert!(import(&short).is_err());
    }

    /// Reads the files listed in `BC_RLD_SAMPLES` (separated by `;`).
    #[test]
    #[ignore = "needs sample files"]
    fn sample_files() {
        for path in std::env::var("BC_RLD_SAMPLES").unwrap().split(';') {
            let p = import(&std::fs::read(path).unwrap()).unwrap();
            println!("{path}: {} layer(s), {} shape(s), warnings {:?}", p.layers.len(), p.shapes.len(), p.warnings);
            for (i, l) in &p.layers {
                println!("  layer {i}: {:?} rgb {:?} speed {} power {}/{} output {} interval {}", l.mode, l.rgb(*i), l.speed, l.min_power, l.max_power, l.output, l.interval);
            }
            for s in &p.shapes {
                let size = match &s.kind {
                    ShapeKind::Path(p) => bc_geom::bounds(p).map(|b| (b.width(), b.height(), b.x0, b.y0)),
                    ShapeKind::Image { width, height, .. } => Some((*width, *height, s.transform.translation().x, s.transform.translation().y)),
                    _ => None,
                };
                println!("  layer {} {} group {:?}: {:?}", s.layer, s.kind.type_name(), s.group, size);
            }
        }
    }
}
