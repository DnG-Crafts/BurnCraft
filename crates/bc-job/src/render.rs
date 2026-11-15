//! A realistic picture of how a job will look burned into a material.
//!
//! The burn darkness of each path comes from its layer's power, speed and passes (a rough
//! "energy per mm" model, good enough to compare settings and spot problems), drawn at the laser's
//! line width for cuts and at the line interval for fills and images, then coloured for the material.

use crate::{Job, JobPath, Moment};
use bc_doc::LayerMode;
use bc_geom::{Point, Rect};

/// Materials the preview can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    Plywood,
    Walnut,
    Slate,
    /// Black anodised aluminium or black acrylic: the laser exposes a light layer.
    BlackAnodised,
    ClearAcrylic,
    Leather,
    Cardboard,
}

impl Material {
    pub const ALL: [Material; 7] = [Material::Plywood, Material::Walnut, Material::Slate, Material::BlackAnodised, Material::ClearAcrylic, Material::Leather, Material::Cardboard];

    pub fn label(self) -> &'static str {
        match self {
            Material::Plywood => "Birch plywood",
            Material::Walnut => "Walnut",
            Material::Slate => "Slate",
            Material::BlackAnodised => "Black anodised / acrylic",
            Material::ClearAcrylic => "Clear acrylic",
            Material::Leather => "Leather",
            Material::Cardboard => "Cardboard",
        }
    }

    /// (surface colour, fully burned colour, grain strength).
    fn colours(self) -> ([f32; 3], [f32; 3], f32) {
        match self {
            Material::Plywood => ([224.0, 192.0, 145.0], [40.0, 26.0, 14.0], 0.07),
            Material::Walnut => ([118.0, 80.0, 52.0], [28.0, 18.0, 10.0], 0.10),
            Material::Slate => ([68.0, 72.0, 78.0], [205.0, 205.0, 205.0], 0.05),
            Material::BlackAnodised => ([28.0, 28.0, 31.0], [222.0, 222.0, 225.0], 0.0),
            Material::ClearAcrylic => ([185.0, 205.0, 220.0], [246.0, 248.0, 250.0], 0.0),
            Material::Leather => ([150.0, 96.0, 58.0], [48.0, 30.0, 20.0], 0.05),
            Material::Cardboard => ([192.0, 160.0, 116.0], [70.0, 44.0, 24.0], 0.04),
        }
    }

    fn grainy(self) -> bool {
        matches!(self, Material::Plywood | Material::Walnut)
    }
}

/// An RGB picture of `area` at `px_per_mm`.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
    pub area: Rect,
}

/// Darkness (0..1) a path burns to, from its layer's settings.
fn intensity(path: &JobPath, settings: &bc_doc::Layer) -> f32 {
    if path.power <= 0.0 {
        return 0.0;
    }
    let pct = if path.power >= 1.0 { settings.max_power } else { settings.min_power + path.power as f64 * (settings.max_power - settings.min_power) };
    // More power or slower speed burns darker; each extra pass adds less.
    let energy = pct / 100.0 * (60.0 / settings.speed.max(0.1)).powf(0.5) * (settings.passes.max(1) as f64).powf(0.6);
    (1.0 - (-1.6 * energy).exp()).clamp(0.0, 1.0) as f32
}

/// Renders `job` as far as `upto` on `material` over `area`.
pub fn render(job: &Job, area: Rect, px_per_mm: f64, material: Material, upto: &Moment) -> Picture {
    let width = ((area.width() * px_per_mm).ceil() as usize).max(1);
    let height = ((area.height() * px_per_mm).ceil() as usize).max(1);
    let mut burn = vec![0.0f32; width * height];
    let mut drawn = 0;
    'layers: for layer in &job.layers {
        let s = &layer.settings;
        // Cuts and scores are a fine line; fills and images cover their line spacing.
        let line_mm = if s.mode == LayerMode::Line { 0.15 } else { s.interval.max(0.05) };
        let radius = (line_mm * px_per_mm / 2.0).max(0.5);
        for path in &layer.paths {
            // Whole paths that are done, then the part of the next one cut so far.
            let partial;
            let points = if drawn < upto.done {
                &path.points
            } else if let Some(along) = upto.along {
                partial = path.partial(along).0;
                &partial
            } else {
                break 'layers;
            };
            let i = intensity(path, s);
            if i > 0.0 {
                for w in points.windows(2) {
                    stamp_line(&mut burn, width, height, area, px_per_mm, w[0], w[1], radius, i);
                }
            }
            if drawn >= upto.done {
                break 'layers;
            }
            drawn += 1;
        }
    }
    let (base, burned, grain) = material.colours();
    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            // Wood grain: wavy stripes along the sheet; other materials get a faint mottle.
            let (mx, my) = (area.x0 + x as f64 / px_per_mm, area.y0 + y as f64 / px_per_mm);
            let tone = if material.grainy() {
                let wave = (my * 0.9 + (mx * 0.031).sin() * 3.0 + (mx * 0.007).sin() * 6.0).sin();
                1.0 + grain * wave as f32 + grain * 0.6 * noise(mx * 0.08, my * 2.5)
            } else {
                1.0 + grain * noise(mx * 3.0, my * 3.0)
            };
            let b = burn[y * width + x];
            for c in 0..3 {
                let surface = (base[c] * tone).clamp(0.0, 255.0);
                rgb.push((surface + (burned[c] - surface) * b).round() as u8);
            }
        }
    }
    Picture { width, height, rgb, area }
}

/// The Lines view of a job, as far as `upto`, as a picture of `area` at `px_per_mm`: burns in
/// their layer colour (fainter for lower grayscale power), laser-off runs and travel moves faint
/// when `show_travel`. Drawing into a picture keeps huge jobs (photos are millions of short runs)
/// as cheap to show as small ones.
pub fn render_lines(job: &Job, area: Rect, px_per_mm: f64, upto: &Moment, show_travel: bool) -> Picture {
    let width = ((area.width() * px_per_mm).ceil() as usize).max(1);
    let height = ((area.height() * px_per_mm).ceil() as usize).max(1);
    let mut rgb = vec![250u8; width * height * 3];
    let to_px = |p: Point| ((p.x - area.x0) * px_per_mm, (p.y - area.y0) * px_per_mm);
    let mut pos = Point::ORIGIN;
    let mut drawn = 0;
    'layers: for layer in &job.layers {
        let [r, g, b] = layer.settings.rgb(layer.index);
        for path in &layer.paths {
            let current = drawn >= upto.done;
            if current && upto.along.is_none() {
                // Travelling to this path: the move so far.
                if show_travel {
                    draw_line(&mut rgb, width, height, to_px(pos), to_px(upto.head), [120, 120, 255], 0.5);
                }
                break 'layers;
            }
            if show_travel && path.points.first().is_some_and(|p| p.distance(pos) > 1e-9) {
                draw_line(&mut rgb, width, height, to_px(pos), to_px(path.points[0]), [120, 120, 255], 0.5);
            }
            let partial;
            let points = if current {
                partial = path.partial(upto.along.unwrap_or(0.0)).0;
                &partial
            } else {
                &path.points
            };
            let alpha = if path.power <= 0.0 { 0.15 } else { (80.0 + 175.0 * path.power) / 255.0 };
            if show_travel || path.power > 0.0 {
                for w in points.windows(2) {
                    draw_line(&mut rgb, width, height, to_px(w[0]), to_px(w[1]), [r, g, b], alpha);
                }
            }
            if current {
                break 'layers;
            }
            pos = path.points.last().copied().unwrap_or(pos);
            drawn += 1;
        }
    }
    Picture { width, height, rgb, area }
}

/// A 1-pixel line, blended over the picture.
fn draw_line(rgb: &mut [u8], width: usize, height: usize, a: (f64, f64), b: (f64, f64), color: [u8; 3], alpha: f32) {
    let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil().max(1.0) as usize;
    let mut last = usize::MAX;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        if x < 0.0 || y < 0.0 || x >= width as f64 || y >= height as f64 {
            continue;
        }
        let idx = (y as usize * width + x as usize) * 3;
        if idx == last {
            continue; // don't darken a pixel twice within one line
        }
        last = idx;
        for c in 0..3 {
            let v = rgb[idx + c] as f32;
            rgb[idx + c] = (v + (color[c] as f32 - v) * alpha).round() as u8;
        }
    }
}

/// Burns a thick line into the buffer (keeping the darkest value where lines overlap).
#[allow(clippy::too_many_arguments)]
fn stamp_line(burn: &mut [f32], width: usize, height: usize, area: Rect, k: f64, a: Point, b: Point, radius: f64, intensity: f32) {
    let to_px = |p: Point| ((p.x - area.x0) * k, (p.y - area.y0) * k);
    let (a, b) = (to_px(a), to_px(b));
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let (x0, x1) = ((a.0.min(b.0) - radius - 1.0).floor().max(0.0) as usize, (a.0.max(b.0) + radius + 1.0).ceil().min(width as f64) as usize);
    let (y0, y1) = ((a.1.min(b.1) - radius - 1.0).floor().max(0.0) as usize, (a.1.max(b.1) + radius + 1.0).ceil().min(height as f64) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let t = if len2 > 0.0 { (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let d = ((px - (a.0 + t * dx)).powi(2) + (py - (a.1 + t * dy)).powi(2)).sqrt();
            // Soft edge over one pixel.
            let cover = (radius + 0.5 - d).clamp(0.0, 1.0) as f32;
            if cover > 0.0 {
                let v = &mut burn[y * width + x];
                *v = v.max(intensity * cover);
            }
        }
    }
}

/// Smooth value noise in -1..1.
fn noise(x: f64, y: f64) -> f32 {
    let hash = |ix: i64, iy: i64| -> f64 {
        let mut h = (ix.wrapping_mul(374_761_393) ^ iy.wrapping_mul(668_265_263)) as u64;
        h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        ((h ^ (h >> 16)) & 0xFFFF) as f64 / 32767.5 - 1.0
    };
    let (ix, iy) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let top = hash(ix, iy) + (hash(ix + 1, iy) - hash(ix, iy)) * s(fx);
    let bottom = hash(ix, iy + 1) + (hash(ix + 1, iy + 1) - hash(ix, iy + 1)) * s(fx);
    (top + (bottom - top) * s(fy)) as f32
}
