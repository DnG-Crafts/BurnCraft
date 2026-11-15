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
