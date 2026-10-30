//! Job files: a 64-byte header, then G-code text (line work) or one byte per pixel (engraving).
//!
//! Header: kind (0x20 G-code, 0x10 image), width and height, the job id, then the top-left
//! corner, all big-endian, with the job's name from byte 34. G-code files give the size in 0.1 mm;
//! image files give it in pixels, which are 0.1 mm square, stored row by row from the top, 0 for
//! full burn and 255 for none. Coordinates are mm from the field's top-left corner.

/// Pixels (and header units) per mm.
const PER_MM: f64 = 10.0;
const HEADER_LEN: usize = 64;
const NAME_AT: usize = 34;
/// Images larger than this (pixels a side) are refused rather than sent.
const MAX_PIXELS: usize = 4000;

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

/// mm → 0.1 mm units, rounded down (corners) or up (sizes).
fn units_down(mm: f64) -> Result<u16, String> {
    u16::try_from((mm * PER_MM + 1e-6).floor() as i64).map_err(|_| "the job is outside the marking field".to_string())
}

fn units_up(mm: f64) -> Result<u16, String> {
    u16::try_from(((mm * PER_MM - 1e-6).ceil() as i64).max(1)).map_err(|_| "the job is too large".to_string())
}

fn header(kind: u8, (w, h): (u16, u16), job: u32, mode: &[u8], (x, y): (u16, u16), name: &str) -> Vec<u8> {
    let mut b = vec![kind];
    for v in [w, h] {
        b.extend_from_slice(&v.to_be_bytes());
    }
    b.extend_from_slice(&job.to_be_bytes());
    b.extend_from_slice(mode);
    b.extend_from_slice(&x.to_be_bytes());
    b.extend_from_slice(&y.to_be_bytes());
    b.extend_from_slice(&[0x00, 0xFE]);
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
    let mut bytes = header(0x20, (units_up(max.0 - min.0)?, units_up(max.1 - min.1)?), job, &[0x00, 0x00, 0x00, 0x0F], (units_down(min.0)?, units_down(min.1)?), name);
    bytes.extend_from_slice(g.as_bytes());
    Ok(File { kind: Kind::Gcode, job, bytes, at_mm: corner_mm(min) })
}

/// Engraving as a bitmap: the burning paths (raster scan lines) are drawn onto 0.1 mm pixels.
pub fn image_file(paths: &Paths, job: u32, name: &str) -> Result<File, String> {
    let (on, min, max) = burning(paths)?;
    let (x0, y0) = (units_down(min.0)?, units_down(min.1)?);
    let w = (units_up(max.0)?.saturating_sub(x0)).max(1) as usize;
    let h = (units_up(max.1)?.saturating_sub(y0)).max(1) as usize;
    if w > MAX_PIXELS || h > MAX_PIXELS {
        return Err("the image is too large".into());
    }
    let mut pixels = vec![255u8; w * h];
    for (points, power) in on {
        let shade = 255 - (*power as f64 * 255.0).round().clamp(0.0, 255.0) as u8;
        for seg in points.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            let steps = (((b.0 - a.0).hypot(b.1 - a.1)) * PER_MM * 2.0).ceil().max(1.0) as usize;
            for i in 0..steps {
                // Sample midpoints, so a line ending on a pixel edge doesn't spill into the next one.
                let t = (i as f64 + 0.5) / steps as f64;
                let px = ((a.0 + (b.0 - a.0) * t) * PER_MM).floor() as i64 - x0 as i64;
                let py = ((a.1 + (b.1 - a.1) * t) * PER_MM).floor() as i64 - y0 as i64;
                if (0..w as i64).contains(&px) && (0..h as i64).contains(&py) {
                    let p = &mut pixels[py as usize * w + px as usize];
                    *p = (*p).min(shade);
                }
            }
        }
    }
    let mut bytes = header(0x10, (w as u16, h as u16), job, &[0x04], (x0, y0), name);
    bytes.extend_from_slice(&pixels);
    Ok(File { kind: Kind::Image, job, bytes, at_mm: corner_mm(min) })
}

/// The preview command for a rectangle (mm).
pub fn preview(min: (f64, f64), max: (f64, f64)) -> Result<Vec<u8>, String> {
    let (min, max) = ((min.0.min(max.0), min.1.min(max.1)), (min.0.max(max.0), min.1.max(max.1)));
    if min.0 < -1e-6 || min.1 < -1e-6 {
        return Err("the frame is outside the marking field".into());
    }
    let size = |d: f64| units_down(d).map(|v| v.max(1));
    Ok(crate::protocol::preview(size(max.0 - min.0)?, size(max.1 - min.1)?, units_down(min.0.max(0.0))?, units_down(min.1.max(0.0))?))
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
        let f = image_file(&rows, 0xA756D19E, "Untitled").unwrap();
        assert_eq!(&f.bytes[..16], &hex("10 00 95 00 7d a7 56 d1 9e 04 01 c8 01 c1 00 fe")[..]);
        assert_eq!(&f.bytes[NAME_AT..NAME_AT + 8], b"Untitled");
        assert_eq!(f.bytes.len(), 0x4901, "149 × 125 pixels after the header, as captured");
        assert!(f.bytes[HEADER_LEN..].iter().all(|&p| p == 0));
        assert_eq!(f.at_mm, (45, 44));
    }

    #[test]
    fn image_pixels_follow_the_lines() {
        // Two short lines: the top row half burnt, a gap row, then a full row.
        let paths = vec![(vec![(1.0, 1.05), (1.2, 1.05)], 1.0), (vec![(1.0, 1.25), (1.4, 1.25)], 1.0), (vec![(1.0, 1.15), (5.0, 1.15)], 0.0)];
        let f = image_file(&paths, 1, "t").unwrap();
        let (w, h) = (u16::from_be_bytes([f.bytes[1], f.bytes[2]]), u16::from_be_bytes([f.bytes[3], f.bytes[4]]));
        assert_eq!((w, h), (4, 3));
        assert_eq!(&f.bytes[HEADER_LEN..], &[0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn bad_jobs_are_refused() {
        assert!(gcode_file(&[(vec![(0.0, 0.0), (1.0, 1.0)], 0.0)], 1, "").is_err(), "nothing burns");
        assert!(gcode_file(&[(vec![(-5.0, 0.0), (1.0, 1.0)], 1.0)], 1, "").is_err(), "off the field");
        assert_eq!(preview((10.763, 14.576), (48.386, 41.639)).unwrap(), crate::protocol::preview(0x178, 0x10E, 0x6B, 0x91));
    }
}
