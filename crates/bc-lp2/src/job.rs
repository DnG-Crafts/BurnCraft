//! Job files: a 64-byte header, then G-code text (line work) or one byte per pixel (engraving).
//!
//! Header: kind (0x20 G-code, 0x10 image), width and height, the job id, then the top-left
//! corner and the resolution in dots per inch, all big-endian, with the job's name from byte 34.
//! G-code files give the size and corner in 0.1 mm (254 dpi). Image files give them in pixels at
//! the chosen [`Resolution`], with the pixels stored row by row from the top, 0 for full burn and
//! 255 for none. Coordinates are mm from the field's top-left corner.

/// Header units per mm in G-code files.
const PER_MM: f64 = 10.0;
const HEADER_LEN: usize = 64;
const NAME_AT: usize = 34;
/// Images larger than this (pixels a side) are refused rather than sent.
const MAX_PIXELS: usize = 4000;

/// Image resolutions, as the LP2 app names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// 1K: 10 pixels per mm.
    K1,
    /// 1.3K: 13⅓ pixels per mm.
    K1_3,
    /// 2K: 20 pixels per mm.
    K2,
}

impl Resolution {
    pub const ALL: [Resolution; 3] = [Resolution::K1, Resolution::K1_3, Resolution::K2];

    pub fn label(self) -> &'static str {
        match self {
            Resolution::K1 => "1K",
            Resolution::K1_3 => "1.3K",
            Resolution::K2 => "2K",
        }
    }

    pub fn pixels_per_mm(self) -> f64 {
        match self {
            Resolution::K1 => 10.0,
            Resolution::K1_3 => 40.0 / 3.0,
            Resolution::K2 => 20.0,
        }
    }

    /// The header's resolution code.
    fn code(self) -> u8 {
        match self {
            Resolution::K1 => 0x04,
            Resolution::K1_3 => 0x03,
            Resolution::K2 => 0x02,
        }
    }

    /// Dots per inch, as the header gives it (254, 338, 508).
    fn dpi(self) -> u16 {
        (self.pixels_per_mm() * 25.4 + 1e-6).floor() as u16
    }

    /// The resolution whose pixel size is nearest a raster line interval (mm).
    pub fn for_interval(interval: f64) -> Self {
        Self::ALL.into_iter().min_by(|a, b| (1.0 / a.pixels_per_mm() - interval).abs().total_cmp(&(1.0 / b.pixels_per_mm() - interval).abs())).expect("not empty")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Gcode,
    Image,
}

/// A job ready to upload.
#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub kind: Kind,
    pub job: u32,
    pub bytes: Vec<u8>,
    /// The top-left corner in whole mm, for the start command.
    pub at_mm: (u16, u16),
}

/// A polyline with a power fraction (0 = laser off), in mm.
pub type Path = (Vec<(f64, f64)>, f32);
pub type Paths = [Path];

/// The laser-on paths and their bounds (min, max).
type Burning<'a> = (Vec<&'a Path>, (f64, f64), (f64, f64));

fn burning(paths: &Paths) -> Result<Burning<'_>, String> {
    let on: Vec<_> = paths.iter().filter(|(p, power)| *power > 0.0 && p.len() >= 2).collect();
    let mut min = (f64::MAX, f64::MAX);
    let mut max = (f64::MIN, f64::MIN);
    for &(x, y) in on.iter().flat_map(|(p, _)| p) {
        min = (min.0.min(x), min.1.min(y));
        max = (max.0.max(x), max.1.max(y));
    }
    if on.is_empty() {
        return Err("there's nothing to engrave".into());
    }
    if min.0 < -1e-6 || min.1 < -1e-6 {
        return Err("the job is outside the marking field".into());
    }
    Ok((on, (min.0.max(0.0), min.1.max(0.0)), max))
}

/// mm → header units (`per_mm` of them to a mm), rounded down (corners) or up (sizes).
fn units_down(mm: f64, per_mm: f64) -> Result<u16, String> {
    u16::try_from((mm * per_mm + 1e-6).floor() as i64).map_err(|_| "the job is outside the marking field".to_string())
}

fn units_up(mm: f64, per_mm: f64) -> Result<u16, String> {
    u16::try_from(((mm * per_mm - 1e-6).ceil() as i64).max(1)).map_err(|_| "the job is too large".to_string())
}

#[allow(clippy::too_many_arguments)]
fn header(kind: u8, (w, h): (u16, u16), job: u32, mode: &[u8], (x, y): (u16, u16), dpi: u16, name: &str) -> Vec<u8> {
    let mut b = vec![kind];
    for v in [w, h] {
        b.extend_from_slice(&v.to_be_bytes());
    }
    b.extend_from_slice(&job.to_be_bytes());
    b.extend_from_slice(mode);
    b.extend_from_slice(&x.to_be_bytes());
    b.extend_from_slice(&y.to_be_bytes());
    b.extend_from_slice(&dpi.to_be_bytes());
    b.resize(HEADER_LEN, 0);
    let name = name.as_bytes();
    let n = name.len().min(HEADER_LEN - NAME_AT - 1);
    b[NAME_AT..NAME_AT + n].copy_from_slice(&name[..n]);
    b
}

fn corner_mm(min: (f64, f64)) -> (u16, u16) {
    ((min.0 + 1e-9).floor() as u16, (min.1 + 1e-9).floor() as u16)
}

/// Line work as G-code: each path at full on/off (the job's power and speed are set when it's
/// started), as the LP2 app writes it.
pub fn gcode_file(paths: &Paths, job: u32, name: &str) -> Result<File, String> {
    let (on, min, max) = burning(paths)?;
    let mut g = String::from("G90\nG21\nM8F11760M5S0\nG1S255\nF12000\nM05S0\n");
    for (points, power) in on {
        let s = (*power as f64 * 255.0).round().clamp(1.0, 255.0);
        let (x, y) = points[0];
        g += &format!("G0 X{x:.3} Y{y:.3}\nM03S{s}\n");
        for &(x, y) in &points[1..] {
            g += &format!("G1 X{x:.3} Y{y:.3}\n");
        }
        g += "M05S0\n";
    }
    g += "M05\n";
    let size = (units_up(max.0 - min.0, PER_MM)?, units_up(max.1 - min.1, PER_MM)?);
    let corner = (units_down(min.0, PER_MM)?, units_down(min.1, PER_MM)?);
    // The header counts the G-code (one more than its lines: 15 for the app's 14-line rectangle).
    let count = g.matches('\n').count() as u32 + 1;
    let mut bytes = header(0x20, size, job, &count.to_be_bytes(), corner, 254, name);
    bytes.extend_from_slice(g.as_bytes());
    Ok(File { kind: Kind::Gcode, job, bytes, at_mm: corner_mm(min) })
}

/// Engraving as a bitmap: the burning paths (raster scan lines) are drawn onto pixels at `res`.
/// `roller` marks the file for the electric roller, as the LP2 app does (header byte 21).
pub fn image_file(paths: &Paths, job: u32, name: &str, res: Resolution, roller: bool) -> Result<File, String> {
    let (on, min, max) = burning(paths)?;
    let per_mm = res.pixels_per_mm();
    let (x0, y0) = (units_down(min.0, per_mm)?, units_down(min.1, per_mm)?);
    let w = (units_up(max.0, per_mm)?.saturating_sub(x0)).max(1) as usize;
    let h = (units_up(max.1, per_mm)?.saturating_sub(y0)).max(1) as usize;
    if w > MAX_PIXELS || h > MAX_PIXELS {
        return Err("the image is too large".into());
    }
    let mut pixels = vec![255u8; w * h];
    for (points, power) in on {
        let shade = 255 - (*power as f64 * 255.0).round().clamp(0.0, 255.0) as u8;
        for seg in points.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            let steps = (((b.0 - a.0).hypot(b.1 - a.1)) * per_mm * 2.0).ceil().max(1.0) as usize;
            for i in 0..steps {
                // Sample midpoints, so a line ending on a pixel edge doesn't spill into the next one.
                let t = (i as f64 + 0.5) / steps as f64;
                let px = ((a.0 + (b.0 - a.0) * t) * per_mm).floor() as i64 - x0 as i64;
                let py = ((a.1 + (b.1 - a.1) * t) * per_mm).floor() as i64 - y0 as i64;
                if (0..w as i64).contains(&px) && (0..h as i64).contains(&py) {
                    let p = &mut pixels[py as usize * w + px as usize];
                    *p = (*p).min(shade);
                }
            }
        }
    }
    let mut bytes = header(0x10, (w as u16, h as u16), job, &[res.code()], (x0, y0), res.dpi(), name);
    // The work mode: 0 flat, 1 the electric roller.
    bytes[21] = u8::from(roller);
    bytes.extend_from_slice(&pixels);
    Ok(File { kind: Kind::Image, job, bytes, at_mm: corner_mm(min) })
}

/// The preview command for a rectangle (mm), with the light at `power` on an object `object_mm`
/// across (the roller).
pub fn preview(kind: crate::protocol::PreviewKind, min: (f64, f64), max: (f64, f64), power: u8, object_mm: f64) -> Result<Vec<u8>, String> {
    let (min, max) = ((min.0.min(max.0), min.1.min(max.1)), (min.0.max(max.0), min.1.max(max.1)));
    if min.0 < -1e-6 || min.1 < -1e-6 {
        return Err("the frame is outside the marking field".into());
    }
    let size = |d: f64| units_down(d, PER_MM).map(|v| v.max(1));
    Ok(crate::protocol::preview(kind, size(max.0 - min.0)?, size(max.1 - min.1)?, units_down(min.0.max(0.0), PER_MM)?, units_down(min.1.max(0.0), PER_MM)?, power, object_mm))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn rectangle_matches_the_capture() {
        let ring = vec![(10.763, 15.576), (48.386, 15.576), (48.386, 42.639), (10.763, 42.639), (10.763, 15.576)];
        let f = gcode_file(&[(ring, 1.0)], 0x547F8A85, "Untitled").unwrap();
        let mut expected = hex("20 01 79 01 0f 54 7f 8a 85 00 00 00 0f 00 6b 00 9b 00 fe");
        expected.resize(NAME_AT, 0);
        expected.extend_from_slice(b"Untitled");
        expected.resize(HEADER_LEN, 0);
        expected.extend_from_slice(b"G90\nG21\nM8F11760M5S0\nG1S255\nF12000\nM05S0\nG0 X10.763 Y15.576\nM03S255\nG1 X48.386 Y15.576\nG1 X48.386 Y42.639\nG1 X10.763 Y42.639\nG1 X10.763 Y15.576\nM05S0\nM05\n");
        assert_eq!(f.bytes.len(), 0xDA);
        assert_eq!(f.bytes, expected);
        assert_eq!(f.at_mm, (10, 15));
    }

    #[test]
    fn image_header_matches_the_capture() {
        // A solid 14.9 × 12.5 mm block at (45.6, 44.9), drawn as 0.1 mm scan lines.
        let rows: Vec<(Vec<(f64, f64)>, f32)> = (0..125).map(|r| (vec![(45.6, 44.9 + (r as f64 + 0.5) / 10.0), (60.5, 44.9 + (r as f64 + 0.5) / 10.0)], 1.0)).collect();
        let f = image_file(&rows, 0xA756D19E, "Untitled", Resolution::K1, false).unwrap();
        assert_eq!(&f.bytes[..16], &hex("10 00 95 00 7d a7 56 d1 9e 04 01 c8 01 c1 00 fe")[..]);
        assert_eq!(&f.bytes[NAME_AT..NAME_AT + 8], b"Untitled");
        assert_eq!(f.bytes.len(), 0x4901, "149 × 125 pixels after the header, as captured");
        assert!(f.bytes[HEADER_LEN..].iter().all(|&p| p == 0));
        assert_eq!(f.at_mm, (45, 44));
    }

    #[test]
    fn finer_resolutions_match_the_captures() {
        // The same 14.85 × 12.45 mm block at (45.65, 44.95), as 2K scan lines 0.05 mm apart.
        let rows: Vec<(Vec<(f64, f64)>, f32)> = (0..249).map(|r| (vec![(45.65, 44.95 + (r as f64 + 0.5) / 20.0), (60.5, 44.95 + (r as f64 + 0.5) / 20.0)], 1.0)).collect();
        let f = image_file(&rows, 0x79364E17, "Untitled", Resolution::K2, false).unwrap();
        assert_eq!(image_file(&rows, 1, "", Resolution::K2, true).unwrap().bytes[21], 1, "marked for the roller");
        assert_eq!(&f.bytes[..16], &hex("10 01 29 00 f9 79 36 4e 17 02 03 91 03 83 01 fc")[..]);
        assert_eq!(f.bytes.len(), 74017, "297 × 249 pixels after the header, as captured");
        assert_eq!(f.at_mm, (45, 44));
        assert_eq!((Resolution::K1_3.code(), Resolution::K1_3.dpi()), (0x03, 0x152));
        assert_eq!((Resolution::K1.dpi(), Resolution::K2.dpi()), (0xFE, 0x1FC));
        assert_eq!(Resolution::for_interval(0.1), Resolution::K1);
        assert_eq!(Resolution::for_interval(0.07), Resolution::K1_3);
        assert_eq!(Resolution::for_interval(0.03), Resolution::K2);
    }

    #[test]
    fn image_pixels_follow_the_lines() {
        // Two short lines: the top row half burnt, a gap row, then a full row.
        let paths = vec![(vec![(1.0, 1.05), (1.2, 1.05)], 1.0), (vec![(1.0, 1.25), (1.4, 1.25)], 1.0), (vec![(1.0, 1.15), (5.0, 1.15)], 0.0)];
        let f = image_file(&paths, 1, "t", Resolution::K1, false).unwrap();
        let (w, h) = (u16::from_be_bytes([f.bytes[1], f.bytes[2]]), u16::from_be_bytes([f.bytes[3], f.bytes[4]]));
        assert_eq!((w, h), (4, 3));
        assert_eq!(&f.bytes[HEADER_LEN..], &[0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn bad_jobs_are_refused() {
        assert!(gcode_file(&[(vec![(0.0, 0.0), (1.0, 1.0)], 0.0)], 1, "").is_err(), "nothing burns");
        assert!(gcode_file(&[(vec![(-5.0, 0.0), (1.0, 1.0)], 1.0)], 1, "").is_err(), "off the field");
        use crate::protocol::PreviewKind::Rectangle;
        assert_eq!(preview(Rectangle, (10.763, 14.576), (48.386, 41.639), 1, 20.0).unwrap(), crate::protocol::preview(Rectangle, 0x178, 0x10E, 0x6B, 0x91, 1, 20.0));
    }
}
