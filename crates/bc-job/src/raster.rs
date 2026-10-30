//! Fill and image output: scanlines, angles, cross-hatching, overscan, dithering and grayscale.

use crate::{JobPath, OUTPUT_TOLERANCE};
use bc_doc::{DitherMode, Layer, Shape, ShapeKind};
use bc_geom::{Affine, BezPath, Point};

/// Even-odd scanline fill of closed rings (first point not repeated), horizontal lines.
pub fn scanlines(rings: &[Vec<Point>], interval: f64, bidirectional: bool) -> Vec<Vec<Point>> {
    let (y0, y1) = rings.iter().flatten().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.y), b.max(p.y)));
    if !y0.is_finite() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut row = 0usize;
    let mut y = y0 + interval / 2.0;
    while y < y1 {
        let mut xs: Vec<f64> = Vec::new();
        for ring in rings {
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                if (a.y > y) != (b.y > y) {
                    xs.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
                }
            }
        }
        xs.sort_by(f64::total_cmp);
        let mut spans: Vec<Vec<Point>> = xs.as_chunks::<2>().0.iter().filter(|c| c[1] - c[0] > 1e-6).map(|c| vec![Point::new(c[0], y), Point::new(c[1], y)]).collect();
        if !spans.is_empty() {
            if bidirectional && row % 2 == 1 {
                spans.reverse();
                spans.iter_mut().for_each(|s| s.reverse());
            }
            out.extend(spans);
            row += 1;
        }
        y += interval;
    }
    out
}

/// Scanlines at `angle` degrees: rotate the rings so the scan direction is horizontal, scan, rotate back.
fn angled_scanlines(rings: &[Vec<Point>], interval: f64, bidirectional: bool, angle: f64) -> Vec<Vec<Point>> {
    if angle.abs() < 1e-9 {
        return scanlines(rings, interval, bidirectional);
    }
    let to_scan = Affine::rotate(-angle.to_radians());
    let back = to_scan.inverse();
    let rotated: Vec<Vec<Point>> = rings.iter().map(|r| r.iter().map(|p| to_scan * *p).collect()).collect();
    scanlines(&rotated, interval, bidirectional).into_iter().map(|s| s.into_iter().map(|p| back * p).collect()).collect()
}

/// Wraps burn segments with laser-off run-ups of `overscan` mm before and after each.
fn with_overscan(segments: Vec<JobPath>, overscan: f64) -> Vec<JobPath> {
    if overscan <= 0.0 {
        return segments;
    }
    let mut out = Vec::with_capacity(segments.len() * 3);
    for seg in segments {
        let (a, b) = (seg.points[0], *seg.points.last().unwrap());
        let len = a.distance(b);
        if len < 1e-9 {
            out.push(seg);
            continue;
        }
        let dir = (b - a) / len;
        out.push(JobPath::off(vec![a - dir * overscan, a]));
        out.push(seg);
        out.push(JobPath::off(vec![b, b + dir * overscan]));
    }
    out
}

/// Scan segments covering the union of the closed shapes.
pub(crate) fn fill_paths(shapes: &[&Shape], settings: &Layer) -> Vec<JobPath> {
    let closed: Vec<BezPath> = shapes.iter().filter(|s| s.is_closed()).map(|s| s.world_path()).collect();
    let region = match closed.len() {
        0 => return Vec::new(),
        1 => closed[0].clone(),
        _ => bc_geom::ops::boolean(&closed, bc_geom::ops::BooleanOp::Union).unwrap_or_else(|| closed[0].clone()),
    };
    let interval = settings.interval.max(0.01);
    if settings.offset_fill {
        return offset_fill(&region, interval);
    }
    let rings: Vec<Vec<Point>> = bc_geom::flatten(&region, OUTPUT_TOLERANCE).into_iter().filter(|l| l.closed).map(|l| l.points).collect();
    let mut segments: Vec<JobPath> = angled_scanlines(&rings, interval, settings.bidirectional, settings.scan_angle).into_iter().map(JobPath::burn).collect();
    if settings.cross_hatch {
        segments.extend(angled_scanlines(&rings, interval, settings.bidirectional, settings.scan_angle + 90.0).into_iter().map(JobPath::burn));
    }
    with_overscan(segments, settings.overscan)
}

/// Offset (contour) fill: rings following the outline, stepping inward by `interval` until the
/// shape is used up, outermost first. Each ring is offset from the original outline, so rounding
/// doesn't build up.
fn offset_fill(region: &BezPath, interval: f64) -> Vec<JobPath> {
    const MAX_RINGS: usize = 20_000;
    let mut out = Vec::new();
    for k in 0..MAX_RINGS {
        let Some(ring) = bc_geom::ops::offset(region, -(k as f64 + 0.5) * interval) else { break };
        let lines: Vec<Vec<Point>> = bc_geom::flatten(&ring, OUTPUT_TOLERANCE).into_iter().filter(|l| l.closed && l.points.len() >= 3).map(|l| l.points).collect();
        if lines.is_empty() {
            break;
        }
        for mut pts in lines {
            if pts.first() != pts.last() {
                pts.push(pts[0]);
            }
            out.push(JobPath::burn(pts));
        }
    }
    out
}
fn raster_dither(mode: DitherMode) -> bc_raster::Dither {
    match mode {
        DitherMode::Threshold => bc_raster::Dither::Threshold,
        DitherMode::Ordered => bc_raster::Dither::Ordered,
        DitherMode::FloydSteinberg | DitherMode::Grayscale => bc_raster::Dither::FloydSteinberg,
        DitherMode::Atkinson => bc_raster::Dither::Atkinson,
        DitherMode::Jarvis => bc_raster::Dither::JarvisJudiceNinke,
        DitherMode::Stucki => bc_raster::Dither::Stucki,
    }
}

/// The image at the layer's resolution, with adjustments applied.
fn prepared_gray(shape: &Shape, settings: &Layer) -> Result<bc_raster::Gray, String> {
    let ShapeKind::Image { width, height, adjust, .. } = &shape.kind else {
        return Err("not an image".into());
    };
    let bytes = shape.image_bytes().ok_or("image data is corrupt")?;
    let mut gray = bc_raster::Gray::decode(&bytes).map_err(|e| e.to_string())?;
    let px = settings.interval.max(0.01);
    let (cols, rows) = (((width / px).round() as usize).max(1), ((height / px).round() as usize).max(1));
    if cols * rows > 60_000_000 {
        return Err(format!("{cols}×{rows} pixels is too large; increase the layer interval"));
    }
    gray = gray.resize(cols, rows);
    gray.adjust(&adjustments(adjust));
    apply_clip(shape, &mut gray);
    Ok(gray)
}

/// The raster crate's adjustments for an image's settings.
pub fn adjustments(a: &bc_doc::ImageAdjust) -> bc_raster::Adjustments {
    bc_raster::Adjustments {
        brightness: a.brightness,
        contrast: a.contrast,
        gamma: a.gamma,
        invert: a.invert,
        auto_level: a.auto_level,
        sharpen: a.sharpen,
        remove_background: a.remove_background,
        effect: match a.effect {
            bc_doc::ImageEffect::None => bc_raster::Effect::None,
            bc_doc::ImageEffect::Sketch => bc_raster::Effect::Sketch,
            bc_doc::ImageEffect::LineArt => bc_raster::Effect::LineArt,
            bc_doc::ImageEffect::Edges => bc_raster::Effect::Edges,
        },
    }
}

/// How the laser will burn an image on `layer`: 8-bit pixels (0 = burn) at the layer's line interval
/// (coarsened if that would be more than `max_pixels`), dithered or in grayscale levels like the job.
pub fn burn_preview(shape: &Shape, layer: &bc_doc::Layer, max_pixels: usize) -> Result<bc_raster::Gray, String> {
    let ShapeKind::Image { width, height, .. } = &shape.kind else { return Err("not an image".into()) };
    let mut settings = layer.clone();
    let px = settings.interval.max(0.01);
    let count = (width / px) * (height / px);
    if count > max_pixels as f64 {
        settings.interval = px * (count / max_pixels as f64).sqrt();
    }
    if settings.dither == DitherMode::Grayscale {
        let mut gray = prepared_gray(shape, &settings)?;
        for v in &mut gray.data {
            *v = 1.0 - ((1.0 - *v) * GRAY_LEVELS).round() / GRAY_LEVELS;
        }
        return Ok(gray);
    }
    let bits = dither_image(shape, &settings)?;
    Ok(bc_raster::Gray { width: bits.width, height: bits.height, data: bits.data.iter().map(|&b| if b { 0.0 } else { 1.0 }).collect() })
}
/// Whitens (never burns) the pixels outside an image's clip outline.
fn apply_clip(shape: &Shape, gray: &mut bc_raster::Gray) {
    let ShapeKind::Image { width, height, clip: Some(clip), .. } = &shape.kind else { return };
    let area = bc_geom::Rect::new(-width / 2.0, -height / 2.0, width / 2.0, height / 2.0);
    let inside = bc_geom::scanline_mask(clip, area, gray.width, gray.height);
    for (v, keep) in gray.data.iter_mut().zip(inside) {
        if !keep {
            *v = 1.0;
        }
    }
}

/// Dithers an image shape at the layer's resolution.
pub fn dither_image(shape: &Shape, settings: &Layer) -> Result<bc_raster::Bits, String> {
    Ok(bc_raster::dither(&prepared_gray(shape, settings)?, raster_dither(settings.dither), 0.5))
}

/// What tracing makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TraceKind {
    /// Closed outlines round the ink: for cutting, or filling.
    #[default]
    Outline,
    /// Single strokes along the middle of each line: line art engraved once, not outlined.
    CentreLine,
}

/// How an image is traced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceSettings {
    /// Pixels darker than this (0..1, after the image's adjustments) are traced; with `subject`,
    /// how different from the background (0..1) the object must be.
    pub threshold: f32,
    /// Find the object against a plain background (flooding in from the picture's edges)
    /// instead of tracing dark areas: for photos of real things.
    pub subject: bool,
    pub kind: TraceKind,
    /// Smooth curves (corners kept sharp) rather than straight lines along the pixels.
    pub smooth: bool,
    /// How far (mm) the result may stray from the picture.
    pub tolerance: f64,
    /// Regions (and gaps) smaller than this (mm²) are ignored.
    pub min_area: f64,
    /// Only the outside edges: no holes.
    pub outer_only: bool,
}

impl Default for TraceSettings {
    fn default() -> Self {
        Self { threshold: 0.5, subject: false, kind: TraceKind::Outline, smooth: true, tolerance: 0.1, min_area: 0.5, outer_only: false }
    }
}

/// The image shape's pixels as traced (adjustments and clip applied, at most 1500 px long),
/// and the size of a pixel in mm.
fn trace_pixels(shape: &Shape) -> Result<(bc_raster::Gray, f64, f64), String> {
    let ShapeKind::Image { width, height, adjust, .. } = &shape.kind else {
        return Err("not an image".into());
    };
    let bytes = shape.image_bytes().ok_or("image data is corrupt")?;
    let mut gray = bc_raster::Gray::decode(&bytes).map_err(|e| e.to_string())?;
    // Up to 1500 px on the long side: plenty for tracing, and fast.
    let longest = gray.width.max(gray.height);
    if longest > 1500 {
        let s = 1500.0 / longest as f64;
        gray = gray.resize(((gray.width as f64 * s).round() as usize).max(1), ((gray.height as f64 * s).round() as usize).max(1));
    }
    gray.adjust(&adjustments(adjust));
    apply_clip(shape, &mut gray);
    let (px, py) = (width / gray.width as f64, height / gray.height as f64);
    Ok((gray, px, py))
}

/// The threshold that best splits the image into dark and light, for a one-click start.
pub fn auto_threshold(shape: &Shape) -> Result<f32, String> {
    Ok(bc_raster::outline::auto_threshold(&trace_pixels(shape)?.0))
}

/// Traces an image shape into closed outlines (document coordinates), as smooth curves. Pixels
/// darker than `threshold` (0..1, after the shape's adjustments) are traced; `tolerance` is the
/// allowed deviation and `min_area` the smallest kept region, in mm (mm²).
pub fn trace_image(shape: &Shape, threshold: f32, tolerance: f64, min_area: f64) -> Result<BezPath, String> {
    trace_image_with(shape, &TraceSettings { threshold, tolerance, min_area, ..TraceSettings::default() })
}

/// Traces an image shape (document coordinates).
pub fn trace_image_with(shape: &Shape, s: &TraceSettings) -> Result<BezPath, String> {
    use bc_raster::outline::{self, Ink};
    let ShapeKind::Image { width, height, .. } = &shape.kind else {
        return Err("not an image".into());
    };
    let (gray, px, py) = trace_pixels(shape)?;
    // Pixel corners -> the shape's own coordinates -> the document.
    let to_doc = shape.transform * Affine::translate((-width / 2.0, -height / 2.0)) * Affine::scale_non_uniform(px, py);
    let tol = (s.tolerance / px.min(py)).max(0.3);
    let min_pixels = (s.min_area / (px * py)).round() as usize;
    let ink = if s.subject { Ink::Subject(s.threshold) } else { Ink::Darker(s.threshold) };
    let m = outline::mask(&gray, ink, min_pixels);
    let point = |p: &(f64, f64)| Point::new(p.0, p.1);
    let mut out = BezPath::new();
    match s.kind {
        TraceKind::CentreLine => {
            for line in outline::centre_lines(&m, 3) {
                let pts: Vec<Point> = line.iter().map(point).collect();
                if s.smooth {
                    out.extend(bc_geom::curvefit::fit_stroke(&pts, tol));
                } else {
                    out.extend(bc_geom::polyline_path(&bc_geom::warp::simplify_points(&pts, tol), false));
                }
            }
        }
        TraceKind::Outline if s.smooth => {
            for o in outline::outlines(&gray, ink, &m) {
                if (s.outer_only && !o.outer) || area(&o.points).abs() < min_pixels as f64 {
                    continue;
                }
                let pts: Vec<Point> = o.points.iter().map(point).collect();
                // Even out wobble from noise in the picture, but not across real corners.
                let even = bc_geom::curvefit::resample_closed(&pts, 0.5);
                let corners = bc_geom::curvefit::corners(&even, 4, 55.0);
                let calm = bc_geom::curvefit::smooth_closed(&even, &corners, 1.5 + tol);
                out.extend(bc_geom::curvefit::fit_closed(&calm, tol * 0.8, 55.0, 4));
            }
        }
        TraceKind::Outline => {
            // Straight lines along the pixel edges, simplified.
            let bits = bc_raster::Bits { width: m.width, height: m.height, data: m.px };
            let options = bc_raster::trace::TraceOptions { tolerance: tol, min_area: min_pixels.max(1) as f64 };
            for poly in bc_raster::trace::trace(&bits, options) {
                // The pixel-edge tracer winds outside edges one way and holes the other.
                if s.outer_only && area(&poly) > 0.0 {
                    continue;
                }
                let pts: Vec<Point> = poly.iter().map(point).collect();
                out.extend(bc_geom::polyline_path(&pts, true));
            }
        }
    }
    Ok(to_doc * out)
}

/// Signed area of a closed loop in pixel coordinates.
fn area(p: &[(f64, f64)]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1).sum::<f64>() / 2.0
}

/// An image shape as line art (spiral, waves or stipple; see [`bc_raster::lines`]), in document
/// coordinates over the image. The shape's adjustments and clip apply.
pub fn photo_lines(shape: &Shape, options: &bc_raster::lines::LineOptions) -> Result<BezPath, String> {
    let ShapeKind::Image { width, height, adjust, .. } = &shape.kind else {
        return Err("not an image".into());
    };
    let bytes = shape.image_bytes().ok_or("image data is corrupt")?;
    let mut gray = bc_raster::Gray::decode(&bytes).map_err(|e| e.to_string())?;
    // A pixel every 0.2 mm is finer than any line spacing, up to 1500 px on the long side.
    let s = (width.max(*height) / 0.2).min(1500.0) / gray.width.max(gray.height) as f64;
    if s < 1.0 {
        gray = gray.resize(((gray.width as f64 * s).round() as usize).max(1), ((gray.height as f64 * s).round() as usize).max(1));
    }
    gray.adjust(&adjustments(adjust));
    apply_clip(shape, &mut gray);
    let mut out = BezPath::new();
    for line in bc_raster::lines::photo_lines(&gray, *width, *height, options) {
        let pts: Vec<Point> = line.iter().map(|&(x, y)| shape.transform * Point::new(x - width / 2.0, y - height / 2.0)).collect();
        out.extend(bc_geom::polyline_path(&pts, false));
    }
    if out.elements().is_empty() {
        return Err("the picture is too light to draw anything; darken it or raise the contrast".into());
    }
    Ok(out)
}

/// Grayscale power levels used for runs (more levels = more, shorter segments).
const GRAY_LEVELS: f32 = 32.0;

/// Burn segments for an image, one row per interval, mapped through the shape's transform.
/// Dithered modes burn dark runs at full power; grayscale merges runs of equal power level.
pub(crate) fn image_paths(shape: &Shape, settings: &Layer) -> Result<Vec<JobPath>, String> {
    let ShapeKind::Image { width, height, .. } = &shape.kind else { return Ok(Vec::new()) };
    // Per-pixel power fraction (0 = skip).
    let (cols, rows, power): (usize, usize, Vec<f32>) = if settings.dither == DitherMode::Grayscale {
        let gray = prepared_gray(shape, settings)?;
        let levels = gray.data.iter().map(|v| ((1.0 - v) * GRAY_LEVELS).round() / GRAY_LEVELS).collect();
        (gray.width, gray.height, levels)
    } else {
        let bits = dither_image(shape, settings)?;
        (bits.width, bits.height, bits.data.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect())
    };
    let (px, py) = (width / cols as f64, height / rows as f64);
    let mut segments = Vec::new();
    let mut row_index = 0usize;
    for r in 0..rows {
        let y = -height / 2.0 + (r as f64 + 0.5) * py;
        let row = &power[r * cols..(r + 1) * cols];
        let mut spans = Vec::new();
        let mut c = 0;
        while c < cols {
            let level = row[c];
            let start = c;
            while c < cols && row[c] == level {
                c += 1;
            }
            if level > 0.0 {
                let (x0, x1) = (-width / 2.0 + start as f64 * px, -width / 2.0 + c as f64 * px);
                spans.push(JobPath { points: vec![shape.transform * Point::new(x0, y), shape.transform * Point::new(x1, y)], power: level });
            }
        }
        if !spans.is_empty() {
            if settings.bidirectional && row_index % 2 == 1 {
                spans.reverse();
                spans.iter_mut().for_each(|s| s.points.reverse());
            }
            segments.extend(spans);
            row_index += 1;
        }
    }
    Ok(with_overscan(segments, settings.overscan))
}
