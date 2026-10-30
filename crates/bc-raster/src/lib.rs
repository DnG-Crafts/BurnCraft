//! Image processing for engraving: decoding, adjustments, resampling and dithering.
//!
//! Gray values are `f32` in 0.0 (black) ..= 1.0 (white). Dithered output is a
//! bitmap where `true` means "fire the laser" (a dark pixel).

pub mod lines;
pub mod outline;
pub mod pixscan;
pub mod trace;

use image::imageops::FilterType;

/// How opaque each pixel of an image file is (0 see-through, 255 solid), resized to `width` ×
/// `height` pixels, for showing it as it really is. `None` if it's solid all over.
pub fn opacity(bytes: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    if img.pixels().all(|p| p.0[3] == 255) {
        return None;
    }
    let alpha = image::GrayImage::from_fn(img.width(), img.height(), |x, y| image::Luma([img.get_pixel(x, y).0[3]]));
    let alpha = if (alpha.width() as usize, alpha.height() as usize) == (width, height) {
        alpha
    } else {
        image::imageops::resize(&alpha, width.max(1) as u32, height.max(1) as u32, FilterType::Triangle)
    };
    Some(alpha.into_raw())
}

/// A black-and-white PNG of where an image file is solid: black where it's opaque, white where
/// it's see-through (erased, or a cleared background). For tracing the shape of what's left.
pub fn opacity_mask(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8();
    let mask = image::GrayImage::from_fn(img.width(), img.height(), |x, y| image::Luma([if img.get_pixel(x, y).0[3] > 127 { 0 } else { 255 }]));
    let mut out = std::io::Cursor::new(Vec::new());
    mask.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// Clears one colour from an image file ("select & erase"): the area of colour around `at` (mm
/// from the picture's top-left, on a picture shown `width` × `height` mm), or with `everywhere`,
/// every pixel of that colour. `tolerance` (0..1) is how different a pixel may be and still
/// count. Cleared pixels become transparent, so they never burn. Returns the picture as PNG.
pub fn erase_colour(bytes: &[u8], at: (f64, f64), width: f64, height: f64, tolerance: f32, everywhere: bool) -> Result<Vec<u8>, String> {
    let mut img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (x, y) = ((at.0 / width.max(1e-9) * w as f64).floor(), (at.1 / height.max(1e-9) * h as f64).floor());
    if x < 0.0 || y < 0.0 || x >= w as f64 || y >= h as f64 {
        return Err("that's outside the picture".into());
    }
    let start = y as usize * w + x as usize;
    let pixels: Vec<[u8; 4]> = img.pixels().map(|p| p.0).collect();
    if pixels[start][3] == 0 {
        return Err("that part is already erased".into());
    }
    let target = pixels[start];
    // Colour distance 0..1 (RGB).
    let tol = tolerance.clamp(0.0, 1.0) * 255.0 * 3f32.sqrt();
    let near = |p: [u8; 4]| p[3] != 0 && (0..3).map(|c| (p[c] as f32 - target[c] as f32).powi(2)).sum::<f32>().sqrt() <= tol;
    let mut clear = vec![false; w * h];
    if everywhere {
        for (i, p) in pixels.iter().enumerate() {
            clear[i] = near(*p);
        }
    } else {
        let mut stack = vec![start];
        clear[start] = true;
        while let Some(i) = stack.pop() {
            let (px, py) = (i % w, i / w);
            let mut go = |j: usize| {
                if !clear[j] && near(pixels[j]) {
                    clear[j] = true;
                    stack.push(j);
                }
            };
            if px > 0 {
                go(i - 1);
            }
            if px + 1 < w {
                go(i + 1);
            }
            if py > 0 {
                go(i - w);
            }
            if py + 1 < h {
                go(i + w);
            }
        }
    }
    for (p, c) in img.pixels_mut().zip(&clear) {
        if *c {
            p.0 = [255, 255, 255, 0];
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// Rubs out parts of an image file: everything within `radius` of the brush path `spots` becomes
/// transparent (so it never burns). Positions are millimetres from the top-left of an image shown
/// `width` × `height` mm. Returns the edited picture as PNG.
pub fn erase(bytes: &[u8], spots: &[(f64, f64)], radius: f64, width: f64, height: f64) -> Result<Vec<u8>, String> {
    let mut img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (sx, sy) = (w as f64 / width.max(1e-9), h as f64 / height.max(1e-9));
    let px = |(x, y): (f64, f64)| (x * sx, y * sy);
    // Brush radius in pixels along each axis (pixels may not be square on a stretched image).
    let (rx, ry) = (radius * sx, radius * sy);
    let mut changed = false;
    let pairs: Vec<((f64, f64), (f64, f64))> = match spots {
        [] => Vec::new(),
        [one] => vec![(px(*one), px(*one))],
        _ => spots.windows(2).map(|w| (px(w[0]), px(w[1]))).collect(),
    };
    for (a, b) in pairs {
        let (x0, x1) = ((a.0.min(b.0) - rx).floor().max(0.0) as usize, ((a.0.max(b.0) + rx).ceil() as usize).min(w));
        let (y0, y1) = ((a.1.min(b.1) - ry).floor().max(0.0) as usize, ((a.1.max(b.1) + ry).ceil() as usize).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                // Distance to the segment, measured in brush radii (an ellipse in pixel space).
                let (cx, cy) = ((x as f64 + 0.5 - a.0) / rx, (y as f64 + 0.5 - a.1) / ry);
                let (dx, dy) = ((b.0 - a.0) / rx, (b.1 - a.1) / ry);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 { ((cx * dx + cy * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let (ex, ey) = (cx - t * dx, cy - t * dy);
                if ex * ex + ey * ey <= 1.0 {
                    let p = img.get_pixel_mut(x as u32, y as u32);
                    if p.0[3] != 0 {
                        p.0 = [255, 255, 255, 0];
                        changed = true;
                    }
                }
            }
        }
    }
    if !changed {
        return Err("nothing to erase there".into());
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// A grayscale image.
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Gray {
    pub fn new(width: usize, height: usize, value: f32) -> Self {
        Self { width, height, data: vec![value; width * height] }
    }

    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// Decodes PNG/JPEG/BMP/etc. Transparent pixels become white (not engraved).
    pub fn decode(bytes: &[u8]) -> Result<Self, image::ImageError> {
        let img = image::load_from_memory(bytes)?.to_rgba32f();
        let (w, h) = img.dimensions();
        let data = img
            .pixels()
            .map(|p| {
                let [r, g, b, a] = p.0;
                let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                (luma * a + (1.0 - a)).clamp(0.0, 1.0)
            })
            .collect();
        Ok(Self { width: w as usize, height: h as usize, data })
    }

    /// Resamples to `width` × `height` pixels (area-averaging when shrinking).
    pub fn resize(&self, width: usize, height: usize) -> Self {
        if width == self.width && height == self.height {
            return self.clone();
        }
        let buf = image::ImageBuffer::<image::Luma<f32>, Vec<f32>>::from_raw(self.width as u32, self.height as u32, self.data.clone())
            .expect("buffer size matches dimensions");
        let filter = if width < self.width { FilterType::Triangle } else { FilterType::CatmullRom };
        let out = image::imageops::resize(&buf, width.max(1) as u32, height.max(1) as u32, filter);
        Self { width: width.max(1), height: height.max(1), data: out.into_raw().into_iter().map(|v| v.clamp(0.0, 1.0)).collect() }
    }

    /// Applies the adjustments in place: background removal, auto levels and sharpening first,
    /// then brightness, contrast, gamma and invert.
    pub fn adjust(&mut self, a: &Adjustments) {
        if a.remove_background > 0.0 {
            self.remove_background(a.remove_background);
        }
        match a.effect {
            Effect::None => {}
            Effect::Sketch => self.sketch(),
            Effect::LineArt => {
                self.sketch();
                // Keep only the strong strokes.
                for v in &mut self.data {
                    *v = ((*v - 0.55) / 0.4).clamp(0.0, 1.0);
                }
            }
            Effect::Edges => self.edges(),
        }
        if a.auto_level {
            self.auto_level();
        }
        if a.sharpen > 0.0 {
            self.sharpen(a.sharpen);
        }
        let contrast = (a.contrast.clamp(-0.99, 0.99) + 1.0) / (1.0 - a.contrast.clamp(-0.99, 0.99));
        let gamma = a.gamma.clamp(0.1, 5.0);
        for v in &mut self.data {
            let mut x = *v + a.brightness;
            x = (x - 0.5) * contrast + 0.5;
            x = x.clamp(0.0, 1.0).powf(1.0 / gamma);
            if a.invert {
                x = 1.0 - x;
            }
            *v = x;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adjustments {
    /// Added to every value, -1.0..=1.0.
    pub brightness: f32,
    /// -1.0..=1.0; 0 leaves the image unchanged.
    pub contrast: f32,
    /// 1.0 leaves the image unchanged; above 1 lightens mid-tones.
    pub gamma: f32,
    pub invert: bool,
    /// Stretch the darkest and lightest 1 % to black and white.
    pub auto_level: bool,
    /// Unsharp-mask strength, 0 (off) to about 2.
    pub sharpen: f32,
    /// Turn the background white: pixels joined to the image edge within this much of the edge
    /// colour (0 = off, up to about 0.5).
    pub remove_background: f32,
    pub effect: Effect,
}

/// Artistic effects for engraving photos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effect {
    #[default]
    None,
    /// A pencil sketch.
    Sketch,
    /// Only the strong lines of a sketch.
    LineArt,
    /// Dark outlines where the brightness changes.
    Edges,
}

impl Default for Adjustments {
    fn default() -> Self {
        Self { brightness: 0.0, contrast: 0.0, gamma: 1.0, invert: false, auto_level: false, sharpen: 0.0, remove_background: 0.0, effect: Effect::None }
    }
}

impl Gray {
    /// A soft blur (three box passes ≈ Gaussian) with the given radius in pixels.
    pub fn blur(&self, radius: usize) -> Gray {
        let (w, h) = (self.width, self.height);
        let mut cur = self.data.clone();
        if radius == 0 || w == 0 || h == 0 {
            return self.clone();
        }
        let mut tmp = vec![0.0f32; w * h];
        for _ in 0..3 {
            // Horizontal then vertical running averages.
            for y in 0..h {
                let row = &cur[y * w..(y + 1) * w];
                let mut sum: f32 = (0..=radius).map(|i| row[i.min(w - 1)]).sum::<f32>() + radius as f32 * row[0];
                for x in 0..w {
                    tmp[y * w + x] = sum / (2 * radius + 1) as f32;
                    sum += row[(x + radius + 1).min(w - 1)] - row[x.saturating_sub(radius)];
                }
            }
            for x in 0..w {
                let col = |y: usize| tmp[y * w + x];
                let mut sum: f32 = (0..=radius).map(|i| col(i.min(h - 1))).sum::<f32>() + radius as f32 * col(0);
                for y in 0..h {
                    cur[y * w + x] = sum / (2 * radius + 1) as f32;
                    sum += col((y + radius + 1).min(h - 1)) - col(y.saturating_sub(radius));
                }
            }
        }
        Gray { width: w, height: h, data: cur }
    }

    /// Pencil sketch: the image "colour-dodged" with a blurred negative of itself.
    pub fn sketch(&mut self) {
        let radius = (self.width.min(self.height) / 120).max(2);
        let inverted = Gray { width: self.width, height: self.height, data: self.data.iter().map(|v| 1.0 - v).collect() };
        let blurred = inverted.blur(radius);
        for (v, b) in self.data.iter_mut().zip(&blurred.data) {
            *v = if *b >= 0.999 { 1.0 } else { (*v / (1.0 - b)).clamp(0.0, 1.0) };
        }
    }

    /// Dark lines where brightness changes (Sobel edge strength), on white.
    pub fn edges(&mut self) {
        let (w, h) = (self.width, self.height);
        if w < 3 || h < 3 {
            return;
        }
        let soft = self.blur((w.min(h) / 400).max(1));
        let at = |x: usize, y: usize| soft.data[y * w + x];
        let mut out = vec![1.0f32; w * h];
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)) - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
                let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)) - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
                out[y * w + x] = 1.0 - (gx.hypot(gy) * 1.5).clamp(0.0, 1.0);
            }
        }
        self.data = out;
    }

    /// Stretches values so the darkest and lightest 1 % become black and white.
    pub fn auto_level(&mut self) {
        if self.data.len() < 2 {
            return;
        }
        let mut sorted = self.data.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let lo = sorted[sorted.len() / 100];
        let hi = sorted[sorted.len() - 1 - sorted.len() / 100];
        if hi - lo < 1e-3 {
            return;
        }
        for v in &mut self.data {
            *v = ((*v - lo) / (hi - lo)).clamp(0.0, 1.0);
        }
    }

    /// Unsharp mask: adds `amount` × (pixel − 3×3 blur).
    pub fn sharpen(&mut self, amount: f32) {
        let (w, h) = (self.width, self.height);
        if w < 3 || h < 3 {
            return;
        }
        let src = self.data.clone();
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.0;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let xx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                        let yy = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                        sum += src[yy * w + xx];
                    }
                }
                let v = src[y * w + x];
                self.data[y * w + x] = (v + amount * (v - sum / 9.0)).clamp(0.0, 1.0);
            }
        }
    }

    /// Whitens the background: the region joined to the image edge whose value stays within
    /// `tolerance` of the typical edge value (a plain backdrop behind the subject).
    pub fn remove_background(&mut self, tolerance: f32) {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 {
            return;
        }
        let border: Vec<usize> = (0..w).flat_map(|x| [x, (h - 1) * w + x]).chain((0..h).flat_map(|y| [y * w, y * w + w - 1])).collect();
        let mut edge: Vec<f32> = border.iter().map(|&i| self.data[i]).collect();
        edge.sort_by(|a, b| a.total_cmp(b));
        let backdrop = edge[edge.len() / 2];
        let mut seen = vec![false; w * h];
        let mut stack: Vec<usize> = border.into_iter().filter(|&i| (self.data[i] - backdrop).abs() <= tolerance).collect();
        while let Some(i) = stack.pop() {
            if seen[i] {
                continue;
            }
            seen[i] = true;
            let (x, y) = (i % w, i / w);
            let mut push = |j: usize| {
                if !seen[j] && (self.data[j] - backdrop).abs() <= tolerance {
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        for (v, bg) in self.data.iter_mut().zip(seen) {
            if bg {
                *v = 1.0;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dither {
    Threshold,
    /// 8×8 Bayer ordered dither.
    Ordered,
    FloydSteinberg,
    Atkinson,
    JarvisJudiceNinke,
    Stucki,
}

/// Error-diffusion kernel: (dx, dy, weight), and the divisor.
fn kernel(d: Dither) -> (&'static [(i32, i32, f32)], f32) {
    match d {
        Dither::FloydSteinberg => (&[(1, 0, 7.0), (-1, 1, 3.0), (0, 1, 5.0), (1, 1, 1.0)], 16.0),
        // Atkinson diffuses only 6/8 of the error.
        Dither::Atkinson => (&[(1, 0, 1.0), (2, 0, 1.0), (-1, 1, 1.0), (0, 1, 1.0), (1, 1, 1.0), (0, 2, 1.0)], 8.0),
        Dither::JarvisJudiceNinke => (
            &[
                (1, 0, 7.0), (2, 0, 5.0),
                (-2, 1, 3.0), (-1, 1, 5.0), (0, 1, 7.0), (1, 1, 5.0), (2, 1, 3.0),
                (-2, 2, 1.0), (-1, 2, 3.0), (0, 2, 5.0), (1, 2, 3.0), (2, 2, 1.0),
            ],
            48.0,
        ),
        Dither::Stucki => (
            &[
                (1, 0, 8.0), (2, 0, 4.0),
                (-2, 1, 2.0), (-1, 1, 4.0), (0, 1, 8.0), (1, 1, 4.0), (2, 1, 2.0),
                (-2, 2, 1.0), (-1, 2, 2.0), (0, 2, 4.0), (1, 2, 2.0), (2, 2, 1.0),
            ],
            42.0,
        ),
        Dither::Threshold | Dither::Ordered => (&[], 1.0),
    }
}

const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// A 1-bit image; `true` = burn.
#[derive(Debug, Clone, PartialEq)]
pub struct Bits {
    pub width: usize,
    pub height: usize,
    pub data: Vec<bool>,
}

impl Bits {
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.data[y * self.width + x]
    }
}

/// Converts gray to burn/no-burn. `threshold` (0..1) is used by [`Dither::Threshold`].
/// Error diffusion scans in a serpentine (alternating direction) to avoid directional artefacts.
pub fn dither(img: &Gray, mode: Dither, threshold: f32) -> Bits {
    let (w, h) = (img.width, img.height);
    let mut out = vec![false; w * h];
    match mode {
        Dither::Threshold => {
            for (o, v) in out.iter_mut().zip(&img.data) {
                *o = *v < threshold;
            }
        }
        Dither::Ordered => {
            for y in 0..h {
                for x in 0..w {
                    let t = (BAYER8[y % 8][x % 8] as f32 + 0.5) / 64.0;
                    out[y * w + x] = img.get(x, y) < t;
                }
            }
        }
        _ => {
            let (k, div) = kernel(mode);
            let mut buf = img.data.clone();
            for y in 0..h {
                let reverse = y % 2 == 1;
                for i in 0..w {
                    let x = if reverse { w - 1 - i } else { i };
                    let old = buf[y * w + x];
                    let new = if old < 0.5 { 0.0 } else { 1.0 };
                    out[y * w + x] = new == 0.0;
                    let err = old - new;
                    for &(dx, dy, wgt) in k {
                        let nx = x as i32 + if reverse { -dx } else { dx };
                        let ny = y as i32 + dy;
                        if nx >= 0 && (nx as usize) < w && (ny as usize) < h {
                            buf[ny as usize * w + nx as usize] += err * wgt / div;
                        }
                    }
                }
            }
        }
    }
    Bits { width: w, height: h, data: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erase_colour_clears_the_touching_area_or_all_of_it() {
        // White, with a red square in the middle and a red dot in the corner.
        let mut img = image::RgbaImage::from_pixel(20, 20, image::Rgba([255, 255, 255, 255]));
        for y in 5..15 {
            for x in 5..15 {
                img.put_pixel(x, y, image::Rgba([200, 20, 20, 255]));
            }
        }
        img.put_pixel(0, 0, image::Rgba([200, 20, 20, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let png = png.into_inner();
        let clear = |bytes: &[u8]| image::load_from_memory(bytes).unwrap().to_rgba8().pixels().filter(|p| p.0[3] == 0).count();
        // Clicking the middle (20 × 20 mm picture): only the touching red square.
        assert_eq!(clear(&erase_colour(&png, (10.0, 10.0), 20.0, 20.0, 0.1, false).unwrap()), 100);
        // Everywhere: the dot too.
        assert_eq!(clear(&erase_colour(&png, (10.0, 10.0), 20.0, 20.0, 0.1, true).unwrap()), 101);
        // Clicking the white background clears the white around it, not the red.
        assert_eq!(clear(&erase_colour(&png, (2.0, 18.0), 20.0, 20.0, 0.1, false).unwrap()), 400 - 101);
        assert!(erase_colour(&png, (30.0, 10.0), 20.0, 20.0, 0.1, false).is_err());
    }

    #[test]
    fn opacity_is_read_and_resized() {
        let mut img = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        assert!(opacity(&png.into_inner(), 4, 4).is_none(), "solid pictures need no transparency");
        for x in 0..2 {
            for y in 0..4 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
            }
        }
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let a = opacity(&png.into_inner(), 2, 1).unwrap();
        assert_eq!(a.len(), 2);
        assert!(a[0] < 64 && a[1] > 191, "left half see-through: {a:?}");
    }

    #[test]
    fn the_opacity_mask_is_black_where_solid() {
        let mut img = image::RgbaImage::from_pixel(4, 1, image::Rgba([255, 255, 255, 255]));
        img.put_pixel(0, 0, image::Rgba([255, 255, 255, 0]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mask = image::load_from_memory(&opacity_mask(&png.into_inner()).unwrap()).unwrap().to_luma8();
        assert_eq!(mask.as_raw(), &vec![255, 0, 0, 0], "white (solid) pixels are ink; see-through ones aren't");
    }

    fn ramp(w: usize, h: usize) -> Gray {
        let data = (0..h).flat_map(|_| (0..w).map(move |x| x as f32 / (w - 1) as f32)).collect();
        Gray { width: w, height: h, data }
    }

    fn burn_fraction(b: &Bits) -> f32 {
        b.data.iter().filter(|&&v| v).count() as f32 / b.data.len() as f32
    }

    #[test]
    fn solid_tones() {
        for mode in [Dither::Threshold, Dither::Ordered, Dither::FloydSteinberg, Dither::Atkinson, Dither::JarvisJudiceNinke, Dither::Stucki] {
            assert_eq!(burn_fraction(&dither(&Gray::new(16, 16, 0.0), mode, 0.5)), 1.0, "{mode:?} black");
            assert_eq!(burn_fraction(&dither(&Gray::new(16, 16, 1.0), mode, 0.5)), 0.0, "{mode:?} white");
        }
    }

    #[test]
    fn diffusion_preserves_mean_tone() {
        for mode in [Dither::Ordered, Dither::FloydSteinberg, Dither::JarvisJudiceNinke, Dither::Stucki] {
            let f = burn_fraction(&dither(&Gray::new(64, 64, 0.25), mode, 0.5));
            assert!((f - 0.75).abs() < 0.03, "{mode:?}: {f}");
        }
        // A horizontal ramp from black to white burns about half the pixels.
        let f = burn_fraction(&dither(&ramp(128, 32), Dither::FloydSteinberg, 0.5));
        assert!((f - 0.5).abs() < 0.03, "{f}");
    }

    #[test]
    fn adjustments() {
        let mut g = Gray::new(1, 1, 0.5);
        g.adjust(&Adjustments { invert: true, ..Default::default() });
        assert!((g.data[0] - 0.5).abs() < 1e-6);
        let mut g = Gray::new(1, 1, 0.5);
        g.adjust(&Adjustments { brightness: 0.2, ..Default::default() });
        assert!((g.data[0] - 0.7).abs() < 1e-6);
    }

    #[test]
    fn decode_png_with_transparency() {
        let mut img = image::RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 0]));
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        let g = Gray::decode(&bytes).unwrap();
        assert_eq!((g.width, g.height), (2, 1));
        assert!(g.data[0] < 0.01 && g.data[1] > 0.99);
    }

    #[test]
    fn resize_keeps_tone() {
        let g = Gray::new(100, 50, 0.3).resize(20, 10);
        assert_eq!((g.width, g.height), (20, 10));
        assert!(g.data.iter().all(|v| (v - 0.3).abs() < 1e-3));
    }
}

#[cfg(test)]
mod photo_tests {
    use super::*;

    #[test]
    fn erasing_makes_pixels_white() {
        // A black 100 × 50 px picture shown 20 × 10 mm.
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(100, 50, image::Rgb([0, 0, 0])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let out = erase(png.get_ref(), &[(2.0, 5.0), (18.0, 5.0)], 1.0, 20.0, 10.0).unwrap();
        let g = Gray::decode(&out).unwrap();
        assert_eq!((g.width, g.height), (100, 50));
        assert!(g.get(50, 25) > 0.99 && g.get(10, 25) > 0.99, "along the stroke is white");
        assert!(g.get(50, 5) < 0.01 && g.get(2, 25) < 0.01, "off the stroke stays black");
        assert!(erase(&out, &[(10.0, 5.0)], 0.5, 20.0, 10.0).is_err(), "already erased");
    }

    #[test]
    fn auto_level_stretches_to_full_range() {
        let mut g = Gray { width: 100, height: 1, data: (0..100).map(|i| 0.4 + 0.2 * i as f32 / 99.0).collect() };
        g.auto_level();
        let (lo, hi) = g.data.iter().fold((1.0f32, 0.0f32), |(a, b), &v| (a.min(v), b.max(v)));
        assert!(lo < 0.02 && hi > 0.98, "{lo}..{hi}");
    }

    #[test]
    fn sharpen_increases_edge_contrast() {
        // Left half 0.4, right half 0.6.
        let mut g = Gray { width: 10, height: 3, data: (0..30).map(|i| if i % 10 < 5 { 0.4 } else { 0.6 }).collect() };
        g.sharpen(1.0);
        let (dark, light) = (g.get(4, 1), g.get(5, 1));
        assert!(dark < 0.4 && light > 0.6, "the edge gets crisper: {dark} {light}");
        assert!((g.get(0, 1) - 0.4).abs() < 1e-6, "flat areas don't change");
    }

    #[test]
    fn background_removal_keeps_the_subject() {
        // A light-grey (0.85) backdrop with a dark subject; inside the subject, a light-grey patch
        // that isn't joined to the edge must stay.
        let (w, h) = (20, 20);
        let mut data = vec![0.85; w * h];
        for y in 5..15 {
            for x in 5..15 {
                data[y * w + x] = 0.2;
            }
        }
        for y in 9..11 {
            for x in 9..11 {
                data[y * w + x] = 0.85;
            }
        }
        let mut g = Gray { width: w, height: h, data };
        g.remove_background(0.1);
        assert_eq!(g.get(0, 0), 1.0, "backdrop whitened");
        assert_eq!(g.get(7, 7), 0.2, "subject untouched");
        assert_eq!(g.get(9, 9), 0.85, "enclosed light patch untouched");
    }
}
#[cfg(test)]
mod effect_tests {
    use super::*;

    /// Writes the photo at `BC_EFFECT_IN` with each effect side by side to `BC_EFFECT_OUT`.
    #[test]
    #[ignore = "diagnostic"]
    fn effect_sheet() {
        let src = Gray::decode(&std::fs::read(std::env::var("BC_EFFECT_IN").unwrap()).unwrap()).unwrap();
        let effects = [Effect::None, Effect::Sketch, Effect::LineArt, Effect::Edges];
        let (w, h) = (src.width, src.height);
        let mut sheet = image::GrayImage::new((w * 2) as u32, (h * 2) as u32);
        for (k, e) in effects.iter().enumerate() {
            let mut g = src.clone();
            g.adjust(&Adjustments { effect: *e, ..Default::default() });
            let (ox, oy) = ((k % 2) * w, (k / 2) * h);
            for y in 0..h {
                for x in 0..w {
                    sheet.put_pixel((ox + x) as u32, (oy + y) as u32, image::Luma([(g.get(x, y) * 255.0) as u8]));
                }
            }
        }
        sheet.save(std::env::var("BC_EFFECT_OUT").unwrap()).unwrap();
    }

    #[test]
    fn effects_keep_size_and_range() {
        let mut g = Gray { width: 40, height: 30, data: (0..1200).map(|i| (i % 40) as f32 / 39.0).collect() };
        for e in [Effect::Sketch, Effect::LineArt, Effect::Edges] {
            let mut x = g.clone();
            x.adjust(&Adjustments { effect: e, ..Default::default() });
            assert_eq!((x.width, x.height), (40, 30));
            assert!(x.data.iter().all(|v| (0.0..=1.0).contains(v)), "{e:?}");
        }
        // A gentle gradient stays light; a hard black/white step becomes a dark line.
        g.edges();
        let gradient_min = g.data.iter().skip(41).take(30).cloned().fold(1.0f32, f32::min);
        let mut step = Gray { width: 40, height: 30, data: (0..1200).map(|i| if i % 40 < 20 { 0.0 } else { 1.0 }).collect() };
        step.edges();
        let step_min = step.data.iter().cloned().fold(1.0f32, f32::min);
        assert!(gradient_min > 0.5 && step_min < 0.2, "gradient {gradient_min}, step {step_min}");
    }
}