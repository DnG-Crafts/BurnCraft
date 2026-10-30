//! Photos as line art: a spiral or rows of waves whose wiggle grows with darkness, or stippled
//! dots whose density follows it. The result is vector lines, which burn quickly (also on galvos)
//! and look hand-drawn.

use crate::Gray;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineStyle {
    /// One continuous spiral from the centre out.
    Spiral,
    /// Horizontal rows of waves.
    Waves,
    /// Dots, more of them where the picture is darker.
    Stipple,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineOptions {
    pub style: LineStyle,
    /// Distance between spiral turns, wave rows or dot grid cells (mm).
    pub spacing: f64,
    /// How far dark areas push the line out, 0..=1 (1: neighbouring lines nearly touch).
    pub strength: f64,
    /// Stipple dot diameter (mm).
    pub dot: f64,
    /// Seed for the stipple's random placement.
    pub seed: u64,
}

impl Default for LineOptions {
    fn default() -> Self {
        Self { style: LineStyle::Spiral, spacing: 1.5, strength: 1.0, dot: 0.4, seed: 1 }
    }
}

/// Darker than this counts as "something to draw"; lighter areas are left blank.
const BLANK: f64 = 0.04;
/// Step along the line (mm).
const STEP: f64 = 0.1;

/// Line art for `image` stretched over a `width` × `height` mm area with its top-left corner at
/// the origin. Returns polylines (closed ones repeat their first point).
pub fn photo_lines(image: &Gray, width: f64, height: f64, options: &LineOptions) -> Vec<Vec<(f64, f64)>> {
    if image.width == 0 || image.height == 0 || width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }
    let spacing = options.spacing.max(0.2);
    // Darkness at a point (mm), bilinear; outside the picture counts as blank.
    let dark = |x: f64, y: f64| -> f64 {
        if x < 0.0 || y < 0.0 || x > width || y > height {
            return 0.0;
        }
        let fx = (x / width * image.width as f64 - 0.5).clamp(0.0, (image.width - 1) as f64);
        let fy = (y / height * image.height as f64 - 0.5).clamp(0.0, (image.height - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(image.width - 1), (y0 + 1).min(image.height - 1));
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let g = |x, y| image.get(x, y) as f64;
        let top = g(x0, y0) * (1.0 - tx) + g(x1, y0) * tx;
        let bottom = g(x0, y1) * (1.0 - tx) + g(x1, y1) * tx;
        1.0 - (top * (1.0 - ty) + bottom * ty)
    };
    let amplitude = |d: f64| d * spacing * 0.48 * options.strength.clamp(0.0, 1.0);
    let mut out: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    // Adds a point of a running line, or ends the line where the picture is blank.
    let mut emit = |p: Option<(f64, f64)>, out: &mut Vec<Vec<(f64, f64)>>| match p {
        Some(p) => current.push(p),
        None => {
            if current.len() >= 2 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
        }
    };
    match options.style {
        LineStyle::Spiral => {
            let (cx, cy) = (width / 2.0, height / 2.0);
            let reach = cx.hypot(cy);
            let wavelength = spacing * 0.6;
            let (mut theta, mut phase) = (0.0f64, 0.0f64);
            loop {
                let r = spacing * theta / std::f64::consts::TAU;
                if r > reach {
                    break;
                }
                let (c, s) = (theta.cos(), theta.sin());
                let (x, y) = (cx + r * c, cy + r * s);
                let d = dark(x, y);
                let wiggle = phase.sin() * amplitude(d);
                emit((d > BLANK).then_some((x + wiggle * c, y + wiggle * s)), &mut out);
                // Equal steps along the curve: arc length per radian is about sqrt(r² + (dr/dθ)²).
                let ds_dtheta = r.hypot(spacing / std::f64::consts::TAU).max(1e-6);
                theta += STEP / ds_dtheta;
                phase += STEP / wavelength * std::f64::consts::TAU;
            }
            emit(None, &mut out);
        }
        LineStyle::Waves => {
            let rows = (height / spacing).ceil() as usize;
            for row in 0..rows {
                let y0 = (row as f64 + 0.5) * spacing;
                let mut phase = 0.0f64;
                let steps = (width / STEP).ceil() as usize;
                let xs: Vec<usize> = if row % 2 == 0 { (0..=steps).collect() } else { (0..=steps).rev().collect() };
                for i in xs {
                    let x = (i as f64 * STEP).min(width);
                    let d = dark(x, y0);
                    // Darker areas wiggle both wider and faster.
                    let wavelength = spacing * (1.4 - 0.9 * d);
                    phase += STEP / wavelength * std::f64::consts::TAU;
                    emit((d > BLANK).then_some((x, y0 + phase.sin() * amplitude(d))), &mut out);
                }
                emit(None, &mut out);
            }
        }
        LineStyle::Stipple => {
            let mut state = options.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let mut random = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 11) as f64 / (1u64 << 53) as f64
            };
            let r = (options.dot / 2.0).max(0.02);
            let (cols, rows) = ((width / spacing).ceil() as usize, (height / spacing).ceil() as usize);
            for row in 0..rows {
                let order: Vec<usize> = if row % 2 == 0 { (0..cols).collect() } else { (0..cols).rev().collect() };
                for col in order {
                    let x = (col as f64 + 0.5 + (random() - 0.5) * 0.7) * spacing;
                    let y = (row as f64 + 0.5 + (random() - 0.5) * 0.7) * spacing;
                    let d = dark(x, y);
                    if d > BLANK && random() < d {
                        let ring: Vec<(f64, f64)> = (0..=8).map(|k| {
                            let a = k as f64 / 8.0 * std::f64::consts::TAU;
                            (x + r * a.cos(), y + r * a.sin())
                        }).collect();
                        out.push(ring);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Left half black, right half white.
    fn half_dark() -> Gray {
        let mut g = Gray::new(100, 100, 1.0);
        for y in 0..100 {
            for x in 0..50 {
                g.data[y * 100 + x] = 0.0;
            }
        }
        g
    }

    fn all_points(lines: &[Vec<(f64, f64)>]) -> impl Iterator<Item = (f64, f64)> + '_ {
        lines.iter().flatten().copied()
    }

    /// Writes the picture at `BC_LINES_IN` as spiral, waves and stipple side by side (each 80 mm
    /// wide), as polylines, to `BC_LINES_OUT`.
    #[test]
    #[ignore = "diagnostic"]
    fn dump() {
        let g = Gray::decode(&std::fs::read(std::env::var("BC_LINES_IN").unwrap()).unwrap()).unwrap();
        let (w, h) = (80.0, 80.0 * g.height as f64 / g.width as f64);
        let mut out = String::new();
        for (k, style) in [LineStyle::Spiral, LineStyle::Waves, LineStyle::Stipple].into_iter().enumerate() {
            for l in photo_lines(&g, w, h, &LineOptions { style, spacing: 1.2, ..Default::default() }) {
                out += &l.iter().map(|(x, y)| format!("{:.2},{:.2}", x + 2.0 + k as f64 * 84.0, y + 2.0)).collect::<Vec<_>>().join(" ");
                out += "\n";
            }
        }
        std::fs::write(std::env::var("BC_LINES_OUT").unwrap(), out).unwrap();
    }

    #[test]
    fn lines_only_where_dark() {
        for style in [LineStyle::Spiral, LineStyle::Waves, LineStyle::Stipple] {
            let lines = photo_lines(&half_dark(), 50.0, 50.0, &LineOptions { style, ..Default::default() });
            assert!(!lines.is_empty(), "{style:?} draws something");
            let wiggle = 1.5 * 0.5 + 0.3;
            assert!(all_points(&lines).all(|(x, _)| x < 25.0 + wiggle), "{style:?} stays in the dark half");
        }
    }

    #[test]
    fn waves_wiggle_by_darkness() {
        let lines = photo_lines(&Gray::new(10, 10, 0.0), 30.0, 3.0, &LineOptions { style: LineStyle::Waves, spacing: 1.5, ..Default::default() });
        assert_eq!(lines.len(), 2, "one line per row");
        let (lo, hi) = lines[0].iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        assert!((hi - lo - 2.0 * 1.5 * 0.48).abs() < 0.05, "full darkness swings nearly a row each way: {lo}..{hi}");
        // Rows run in alternate directions so the head doesn't travel back.
        assert!(lines[0][0].0 < 1.0 && lines[1][0].0 > 29.0);
    }

    #[test]
    fn stipple_density_follows_darkness() {
        let options = LineOptions { style: LineStyle::Stipple, spacing: 1.0, ..Default::default() };
        let count = |v: f32| photo_lines(&Gray::new(10, 10, v), 40.0, 40.0, &options).len();
        let (dark, mid) = (count(0.0), count(0.5));
        assert!(dark > 1500 && (600..1000).contains(&mid), "{dark} {mid}");
        assert_eq!(count(1.0), 0);
    }
}
