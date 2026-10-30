//! Turns a document into an ordered, machine-independent job.
//!
//! - **Line**: shapes are flattened to polylines, optionally kerf-compensated,
//!   ordered (holes before the shapes that contain them, then nearest next
//!   start point), split around tabs, and given lead-ins.
//! - **Fill**: the union of the layer's closed shapes is scanned with lines at
//!   the layer interval and angle (even-odd, so holes stay empty), optionally
//!   cross-hatched, with laser-off overscan at each end.
//! - **Image**: each image is resampled so one pixel = one interval, adjusted,
//!   and dithered (or mapped to power levels in grayscale mode).
//!
//! Output is repeated for the layer's pass count, and mapped to machine
//! coordinates for the selected origin corner by [`MachineConfig`].

mod line;
mod raster;
pub mod render;

pub use raster::{adjustments, auto_threshold, burn_preview, dither_image, photo_lines, scanlines, trace_image, trace_image_with, TraceKind, TraceSettings};

use bc_doc::{DitherMode, Document, Layer, LayerMode, Shape, ShapeKind};
use bc_geom::Point;
use std::collections::BTreeSet;

/// Curve flattening tolerance for output, in mm.
pub const OUTPUT_TOLERANCE: f64 = 0.02;

/// Corner of the bed where the machine's (0, 0) is, as seen from the front.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum OriginCorner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

impl OriginCorner {
    pub const ALL: [OriginCorner; 4] = [OriginCorner::TopLeft, OriginCorner::TopRight, OriginCorner::BottomLeft, OriginCorner::BottomRight];

    pub fn label(self) -> &'static str {
        match self {
            OriginCorner::TopLeft => "Top left",
            OriginCorner::TopRight => "Top right",
            OriginCorner::BottomLeft => "Bottom left",
            OriginCorner::BottomRight => "Bottom right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MachineConfig {
    /// Work area in mm.
    pub bed: (f64, f64),
    pub origin: OriginCorner,
    /// Rapid (travel) speed in mm/s, used for time estimates.
    pub rapid_speed: f64,
}

impl MachineConfig {
    /// Maps a design point (mm, origin top-left, Y down) to machine mm.
    pub fn to_machine(&self, p: Point) -> (f64, f64) {
        let (w, h) = self.bed;
        match self.origin {
            OriginCorner::TopLeft => (p.x, p.y),
            OriginCorner::TopRight => (w - p.x, p.y),
            OriginCorner::BottomLeft => (p.x, h - p.y),
            OriginCorner::BottomRight => (w - p.x, h - p.y),
        }
    }

    /// Inverse of [`MachineConfig::to_machine`].
    pub fn to_design(&self, (x, y): (f64, f64)) -> Point {
        // Each mapping is its own inverse.
        let p = self.to_machine(Point::new(x, y));
        Point::new(p.0, p.1)
    }
}

/// A polyline moved along at the layer's speed.
#[derive(Debug, Clone, PartialEq)]
pub struct JobPath {
    pub points: Vec<Point>,
    /// Fraction of the layer's power range: 1.0 = max power, 0.0 = laser off (overscan run-up).
    /// Values in between are grayscale levels: power = min + fraction × (max − min).
    pub power: f32,
}

impl JobPath {
    pub fn burn(points: Vec<Point>) -> Self {
        Self { points, power: 1.0 }
    }

    pub fn off(points: Vec<Point>) -> Self {
        Self { points, power: 0.0 }
    }

    pub fn is_off(&self) -> bool {
        self.power <= 0.0
    }

    pub fn length(&self) -> f64 {
        self.points.windows(2).map(|w| w[0].distance(w[1])).sum()
    }
}

/// One output layer: its settings and ordered paths (design mm). Closed paths end at their first point.
#[derive(Debug, Clone, PartialEq)]
pub struct JobLayer {
    pub index: usize,
    pub settings: Layer,
    pub paths: Vec<JobPath>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Job {
    pub layers: Vec<JobLayer>,
    /// Layers left out or changed, with the reason.
    pub skipped: Vec<(usize, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct JobStats {
    /// Length moved with the laser on.
    pub cut_mm: f64,
    /// Rapid moves between paths.
    pub travel_mm: f64,
    /// Rough estimate in seconds (ignores acceleration).
    pub seconds: f64,
}

/// What the target controller can do, which changes how some layers are built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildOptions {
    /// The controller can vary power along a line (grayscale images).
    pub grayscale: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self { grayscale: true }
    }
}

/// Where an oversized job's sections are, for running it in pieces on a pass-through machine.
/// Sections butt up against each other, so every part of the design burns exactly once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sections {
    /// Top of the design (mm).
    pub top: f64,
    /// Height of each section, which is also how far the material moves between sections.
    pub height: f64,
    pub count: usize,
}

impl Sections {
    /// Bands `height` tall covering the job.
    pub fn plan(job: &Job, height: f64) -> Option<Self> {
        let (a, b) = job.bounds()?;
        let height = height.max(1.0);
        let count = ((b.y - a.y) / height - 1e-9).ceil().max(1.0) as usize;
        Some(Self { top: a.y, height, count })
    }

    /// The design-space band of section `k`: (top, bottom).
    pub fn band(&self, k: usize) -> (f64, f64) {
        let y0 = self.top + k as f64 * self.height;
        // Bands are half-open (lines along an edge burn in the lower band), so the last one reaches
        // a hair further to keep the design's bottom edge.
        let extra = if k + 1 == self.count { 1e-6 } else { 0.0 };
        (y0, y0 + self.height + extra)
    }
}

impl Job {
    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| l.paths.iter().all(JobPath::is_off))
    }

    /// The part of the job between `y0` and `y1` (design mm), clipped and moved up so `y0` lands at
    /// `top` (e.g. 0, the top of the bed). Paths crossing the band edges are cut there.
    pub fn section(&self, y0: f64, y1: f64, top: f64) -> Job {
        let shift = bc_geom::Vec2::new(0.0, top - y0);
        let clip = |path: &JobPath| -> Vec<JobPath> {
            let mut out = Vec::new();
            let mut current: Vec<Point> = Vec::new();
            for w in path.points.windows(2) {
                let (a, b) = (w[0], w[1]);
                // The part of segment a→b inside the band, as parameters t0..t1.
                let (mut t0, mut t1) = (0.0f64, 1.0f64);
                let dy = b.y - a.y;
                if dy.abs() < 1e-12 {
                    if a.y < y0 || a.y >= y1 {
                        (t0, t1) = (1.0, 0.0);
                    }
                } else {
                    let (ta, tb) = ((y0 - a.y) / dy, (y1 - a.y) / dy);
                    t0 = t0.max(ta.min(tb));
                    t1 = t1.min(ta.max(tb));
                }
                if t0 > t1 {
                    if current.len() >= 2 {
                        out.push(JobPath { points: std::mem::take(&mut current), power: path.power });
                    }
                    current.clear();
                    continue;
                }
                let (p, q) = (a.lerp(b, t0) + shift, a.lerp(b, t1) + shift);
                if current.last().is_none_or(|l| l.distance(p) > 1e-9) {
                    if current.len() >= 2 {
                        out.push(JobPath { points: std::mem::take(&mut current), power: path.power });
                    }
                    current = vec![p];
                }
                current.push(q);
                if t1 < 1.0 {
                    // Left the band mid-segment.
                    out.push(JobPath { points: std::mem::take(&mut current), power: path.power });
                }
            }
            if current.len() >= 2 {
                out.push(JobPath { points: current, power: path.power });
            }
            out
        };
        let layers = self
            .layers
            .iter()
            .map(|l| JobLayer { index: l.index, settings: l.settings.clone(), paths: l.paths.iter().flat_map(clip).filter(|p| p.length() > 1e-9).collect() })
            .filter(|l| !l.paths.is_empty())
            .collect();
        Job { layers, skipped: self.skipped.clone() }
    }

    /// Bounding box of all motion, including overscan (design mm): (min, max).
    pub fn bounds(&self) -> Option<(Point, Point)> {
        self.layers.iter().flat_map(|l| l.paths.iter().flat_map(|p| &p.points)).fold(None, |acc, p| match acc {
            None => Some((*p, *p)),
            Some((a, b)) => Some((Point::new(a.x.min(p.x), a.y.min(p.y)), Point::new(b.x.max(p.x), b.y.max(p.y)))),
        })
    }

    /// Path lengths and a rough duration, starting from `start` (design mm).
    pub fn stats(&self, start: Point, rapid_speed: f64) -> JobStats {
        let mut s = JobStats::default();
        let mut pos = start;
        for layer in &self.layers {
            let speed = layer.settings.speed.max(0.1);
            for path in &layer.paths {
                s.travel_mm += pos.distance(path.points[0]);
                let len = path.length();
                if !path.is_off() {
                    s.cut_mm += len;
                }
                s.seconds += len / speed;
                pos = *path.points.last().unwrap();
            }
        }
        s.seconds += s.travel_mm / rapid_speed.max(1.0);
        s
    }
}

/// Builds the job. With `only` set, just those shapes are output ("cut selected").
pub fn build(doc: &Document, only: Option<&BTreeSet<u64>>, options: BuildOptions) -> Job {
    let mut job = Job::default();
    for (index, settings) in doc.layers.iter().enumerate() {
        let shapes: Vec<_> = doc.shapes.iter().filter(|s| s.layer == index && only.is_none_or(|sel| sel.contains(&s.id))).collect();
        // Tool layers (Tool1, Tool2) are guides only and never cut.
        if shapes.is_empty() || bc_doc::is_tool_layer(index) {
            continue;
        }
        if !settings.output {
            job.skipped.push((index, "output is turned off".into()));
            continue;
        }
        let (images, vectors): (Vec<&Shape>, Vec<&Shape>) = shapes.into_iter().partition(|s| matches!(s.kind, ShapeKind::Image { .. }));
        let mut paths: Vec<JobPath> = Vec::new();
        match settings.mode {
            LayerMode::Line => paths = line::perforate(line::line_paths(&vectors, settings), settings),
            LayerMode::Fill => {
                paths = raster::fill_paths(&vectors, settings);
                // Single-line text has nothing to fill: engrave its strokes instead.
                let strokes: Vec<&Shape> = vectors.iter().copied().filter(|s| matches!(s.kind, ShapeKind::Text { .. }) && !s.is_closed()).collect();
                if !strokes.is_empty() {
                    paths.extend(line::line_paths(&strokes, settings));
                }
            }
            LayerMode::FillThenLine => {
                paths = raster::fill_paths(&vectors, settings);
                paths.extend(line::perforate(line::line_paths(&vectors, settings), settings));
            }
            LayerMode::Image => {
                let mut settings = settings.clone();
                if settings.dither == DitherMode::Grayscale && !options.grayscale {
                    settings.dither = DitherMode::FloydSteinberg;
                    job.skipped.push((index, "this controller can't vary power along a line; grayscale was replaced by Floyd–Steinberg dithering".into()));
                }
                for img in &images {
                    match raster::image_paths(img, &settings) {
                        Ok(p) => paths.extend(p),
                        Err(e) => job.skipped.push((index, format!("an image could not be processed: {e}"))),
                    }
                }
                if !vectors.is_empty() {
                    job.skipped.push((index, "vector shapes on an Image layer are not output".into()));
                }
            }
        }
        if !images.is_empty() && settings.mode != LayerMode::Image {
            job.skipped.push((index, "images are only engraved on Image-mode layers".into()));
        }
        if paths.iter().all(JobPath::is_off) {
            continue;
        }
        let passes = settings.passes.max(1) as usize;
        let paths = (0..passes).flat_map(|_| paths.iter().cloned()).collect();
        job.layers.push(JobLayer { index, settings: settings.clone(), paths });
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use bc_doc::ShapeKind;
    use bc_geom::{Affine, BezPath};

    pub(crate) fn doc_with(shapes: &[(ShapeKind, (f64, f64), usize)]) -> Document {
        let mut doc = Document::default();
        for (kind, at, layer) in shapes {
            doc.add_shape(kind.clone(), Affine::translate(*at), *layer);
        }
        doc
    }

    pub(crate) fn rect(w: f64, h: f64) -> ShapeKind {
        ShapeKind::Rect { width: w, height: h, corner_radius: 0.0 }
    }

    fn build_all(doc: &Document) -> Job {
        build(doc, None, BuildOptions::default())
    }

    fn burns(job: &Job) -> Vec<&JobPath> {
        job.layers[0].paths.iter().filter(|p| !p.is_off()).collect()
    }

    #[test]
    fn inner_shapes_are_cut_first() {
        let doc = doc_with(&[(rect(100.0, 100.0), (100.0, 100.0), 0), (rect(10.0, 10.0), (100.0, 100.0), 0)]);
        let job = build_all(&doc);
        let paths = &job.layers[0].paths;
        assert_eq!(paths.len(), 2);
        let xs = || paths[0].points.iter().map(|p| p.x);
        let first_w = xs().fold(f64::MIN, f64::max) - xs().fold(f64::MAX, f64::min);
        assert!((first_w - 10.0).abs() < 1e-6, "the small inner square comes first");
    }

    #[test]
    fn passes_repeat_paths_and_disabled_layers_are_reported() {
        let mut doc = doc_with(&[(rect(10.0, 10.0), (50.0, 50.0), 0), (rect(10.0, 10.0), (80.0, 50.0), 1)]);
        doc.layers[0].passes = 3;
        doc.layers[1].output = false;
        let job = build_all(&doc);
        assert_eq!(job.layers.len(), 1);
        assert_eq!(job.layers[0].paths.len(), 3);
        assert_eq!(job.skipped.len(), 1);
    }

    #[test]
    fn fill_scans_the_interior_bidirectionally() {
        let mut doc = doc_with(&[(rect(10.0, 10.0), (50.0, 50.0), 0)]);
        doc.layers[0].mode = LayerMode::Fill;
        doc.layers[0].interval = 1.0;
        let job = build_all(&doc);
        let paths = burns(&job);
        assert_eq!(paths.len(), 10);
        assert!(paths.iter().all(|p| (p.length() - 10.0).abs() < 1e-6));
        assert!(paths[0].points[0].x < paths[0].points[1].x && paths[1].points[0].x > paths[1].points[1].x, "alternate rows reverse");
    }

    #[test]
    fn fill_leaves_holes_empty() {
        let mut doc = Document::default();
        let mut outer = rect_path_world(0.0, 0.0, 10.0);
        outer.extend(rect_path_world(3.0, 3.0, 4.0)); // square hole in the middle
        doc.add_shape(ShapeKind::Path(outer), Affine::translate((20.0, 20.0)), 0);
        doc.layers[0].mode = LayerMode::Fill;
        doc.layers[0].interval = 1.0;
        let job = build_all(&doc);
        let burned: f64 = burns(&job).iter().map(|p| p.length()).sum();
        assert!((burned - (100.0 - 16.0)).abs() < 1e-6, "burned length {burned}");
    }

    #[test]
    fn fill_angle_and_cross_hatch() {
        let mut doc = doc_with(&[(rect(10.0, 10.0), (50.0, 50.0), 0)]);
        doc.layers[0].mode = LayerMode::Fill;
        doc.layers[0].interval = 1.0;
        doc.layers[0].scan_angle = 90.0;
        let job = build_all(&doc);
        assert!(burns(&job).iter().all(|p| (p.points[0].x - p.points[1].x).abs() < 1e-6), "90° scans are vertical");
        doc.layers[0].cross_hatch = true;
        let job = build_all(&doc);
        assert_eq!(burns(&job).len(), 20);
    }

    #[test]
    fn overscan_adds_laser_off_run_ups() {
        let mut doc = doc_with(&[(rect(10.0, 10.0), (50.0, 50.0), 0)]);
        doc.layers[0].mode = LayerMode::Fill;
        doc.layers[0].interval = 1.0;
        doc.layers[0].overscan = 2.0;
        let job = build_all(&doc);
        let paths = &job.layers[0].paths;
        assert_eq!(paths.len(), 30, "off + burn + off per line");
        assert!(paths[0].is_off() && !paths[1].is_off() && paths[2].is_off());
        assert!((paths[0].length() - 2.0).abs() < 1e-9 && paths[0].points[1] == paths[1].points[0]);
        let (min, max) = job.bounds().unwrap();
        assert!((max.x - min.x - 14.0).abs() < 1e-9, "bounds include overscan");
        assert!((job.stats(Point::ORIGIN, 100.0).cut_mm - 100.0).abs() < 1e-6, "overscan is not counted as burning");
    }

    pub(crate) fn rect_path_world(x: f64, y: f64, size: f64) -> BezPath {
        bc_geom::polyline_path(&[Point::new(x, y), Point::new(x + size, y), Point::new(x + size, y + size), Point::new(x, y + size)], true)
    }

    fn test_png(pixels: &[u8], w: u32, h: u32) -> Vec<u8> {
        let img = image::GrayImage::from_raw(w, h, pixels.to_vec()).unwrap();
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        bytes
    }

    #[test]
    fn image_engraves_dark_pixels() {
        // 4×2 image: left half black, right half white.
        let bytes = test_png(&[0, 0, 255, 255, 0, 0, 255, 255], 4, 2);
        let mut doc = Document::default();
        doc.add_shape(ShapeKind::image(&bytes, 4.0, 2.0), Affine::translate((10.0, 10.0)), 0);
        doc.layers[0].mode = LayerMode::Image;
        doc.layers[0].interval = 1.0;
        doc.layers[0].dither = DitherMode::Threshold;
        let job = build_all(&doc);
        let paths = burns(&job);
        assert_eq!(paths.len(), 2, "one dark run per row");
        let xs: Vec<f64> = paths.iter().flat_map(|p| &p.points).map(|p| p.x).collect();
        assert!(xs.iter().all(|&x| (8.0..=10.0).contains(&x)), "runs cover the left half: {xs:?}");
    }

    /// Renders a sample job on three materials to `BC_RENDER_OUT` (a PNG), for a visual check.
    #[test]
    #[ignore = "diagnostic"]
    fn render_sample() {
        let mut doc = Document::default();
        // A filled rectangle, a cut circle, and a dithered gradient photo.
        doc.add_shape(ShapeKind::Rect { width: 30.0, height: 20.0, corner_radius: 4.0 }, Affine::translate((25.0, 20.0)), 0);
        doc.add_shape(ShapeKind::Ellipse { radius_x: 12.0, radius_y: 12.0 }, Affine::translate((25.0, 20.0)), 1);
        let gradient: Vec<u8> = (0..64 * 64).map(|i| ((i % 64) * 4) as u8).collect();
        doc.add_shape(ShapeKind::image(&test_png(&gradient, 64, 64), 25.0, 25.0), Affine::translate((62.0, 20.0)), 2);
        doc.layers[0].mode = LayerMode::Fill;
        doc.layers[0].max_power = 40.0;
        doc.layers[0].speed = 150.0;
        doc.layers[1].max_power = 90.0;
        doc.layers[1].speed = 10.0;
        doc.layers[2].mode = LayerMode::Image;
        doc.layers[2].interval = 0.15;
        doc.layers[2].max_power = 50.0;
        doc.layers[2].speed = 150.0;
        let job = build_all(&doc);
        let area = bc_geom::Rect::new(5.0, 3.0, 80.0, 37.0);
        let mats = [render::Material::Plywood, render::Material::Slate, render::Material::BlackAnodised];
        let pics: Vec<_> = mats.iter().map(|m| render::render(&job, area, 8.0, *m, 1.0)).collect();
        let (w, h) = (pics[0].width as u32, pics[0].height as u32);
        let mut out = image::RgbImage::new(w, h * 3);
        for (k, p) in pics.iter().enumerate() {
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 3) as usize;
                    out.put_pixel(x, y + k as u32 * h, image::Rgb([p.rgb[i], p.rgb[i + 1], p.rgb[i + 2]]));
                }
            }
        }
        out.save(std::env::var("BC_RENDER_OUT").unwrap()).unwrap();
    }
    /// Times building and previewing a 12-megapixel photo job (run with --release).
    #[test]
    #[ignore = "timing"]
    fn big_photo_timing() {
        let (w, h) = (4032u32, 3024u32);
        let pixels: Vec<u8> = (0..w * h).map(|i| ((i % w) * 255 / w) as u8 ^ (((i / w) % 7) as u8 * 9)).collect();
        let bytes = test_png(&pixels, w, h);
        let mut doc = Document::default();
        doc.add_shape(ShapeKind::image(&bytes, 403.2, 302.4), Affine::translate((210.0, 160.0)), 0);
        doc.layers[0].mode = LayerMode::Image;
        doc.layers[0].interval = 0.1;
        let t = std::time::Instant::now();
        let job = build_all(&doc);
        let build = t.elapsed();
        let paths: usize = job.layers.iter().map(|l| l.paths.len()).sum();
        let t = std::time::Instant::now();
        let stats = job.stats(Point::ORIGIN, 300.0);
        let st = t.elapsed();
        let t = std::time::Instant::now();
        let _ = render::render_lines(&job, bc_geom::Rect::new(0.0, 0.0, 600.0, 400.0), 2.0, 1.0, true);
        let lines = t.elapsed();
        println!("TIMING {paths} runs: build {build:?}, stats {st:?}, lines view {lines:?}, est {:.0} s", stats.seconds);
    }
    #[test]
    fn burn_preview_matches_the_layer() {
        // A 20 × 10 mm mid-grey image on a 0.5 mm Floyd–Steinberg layer: 40 × 20 dots, about half burnt.
        let bytes = test_png(&[128; 64], 8, 8);
        let mut doc = Document::default();
        let id = doc.add_shape(ShapeKind::image(&bytes, 20.0, 10.0), Affine::translate((50.0, 50.0)), 0);
        let mut layer = Layer::new(0);
        layer.interval = 0.5;
        layer.dither = DitherMode::FloydSteinberg;
        let g = burn_preview(doc.shape(id).unwrap(), &layer, 1_000_000).unwrap();
        assert_eq!((g.width, g.height), (40, 20));
        assert!(g.data.iter().all(|&v| v == 0.0 || v == 1.0), "dots only");
        let burnt = g.data.iter().filter(|&&v| v == 0.0).count() as f64 / g.data.len() as f64;
        assert!((burnt - 0.5).abs() < 0.1, "{burnt}");
        // Very fine intervals are coarsened to the pixel budget.
        layer.interval = 0.001;
        let g = burn_preview(doc.shape(id).unwrap(), &layer, 10_000).unwrap();
        assert!(g.width * g.height <= 10_500);
    }
    #[test]
    fn oversized_jobs_split_into_sections() {
        // A vertical line from y = 10 to 250 on a 100 mm-tall bed: 3 sections of 100 mm, no overlap.
        let mut settings = Layer::new(0);
        settings.speed = 10.0;
        let job = Job { layers: vec![JobLayer { index: 0, settings, paths: vec![JobPath::burn(vec![Point::new(50.0, 10.0), Point::new(50.0, 250.0)])] }], skipped: Vec::new() };
        let plan = Sections::plan(&job, 100.0).unwrap();
        assert_eq!(plan.count, 3);
        let mut total = 0.0;
        for k in 0..plan.count {
            let (y0, y1) = plan.band(k);
            let s = job.section(y0, y1, 0.0);
            let (a, b) = s.bounds().unwrap();
            assert!(a.y >= -1e-9 && b.y <= 100.0 + 1e-9, "section {k} fits the bed: {a:?} {b:?}");
            total += s.layers[0].paths.iter().map(JobPath::length).sum::<f64>();
        }
        assert!((total - 240.0).abs() < 1e-9, "every millimetre burned exactly once: {total}");
        // A square whose edges lie on band boundaries: each horizontal edge burns once.
        let square = vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 20.0), Point::new(0.0, 20.0), Point::new(0.0, 0.0)];
        let job = Job { layers: vec![JobLayer { index: 0, settings: Layer::new(0), paths: vec![JobPath::burn(square)] }], skipped: Vec::new() };
        let plan = Sections::plan(&job, 10.0).unwrap();
        assert_eq!(plan.count, 2);
        let total: f64 = (0..2).map(|k| {
            let (y0, y1) = plan.band(k);
            job.section(y0, y1, 0.0).layers.iter().flat_map(|l| &l.paths).map(JobPath::length).sum::<f64>()
        }).sum();
        assert!((total - 60.0).abs() < 1e-9, "perimeter burned once: {total}");
    }

    #[test]
    fn offset_fill_traces_rings_inward() {
        // A 10 mm square at 1 mm: rings inset 0.5, 1.5 … 4.5 mm, outermost first.
        let mut doc = doc_with(&[(rect(10.0, 10.0), (20.0, 20.0), 0)]);
        doc.layers[0].mode = LayerMode::Fill;
        doc.layers[0].interval = 1.0;
        doc.layers[0].offset_fill = true;
        let job = build_all(&doc);
        let paths = burns(&job);
        assert_eq!(paths.len(), 5, "{} rings", paths.len());
        let first = bc_geom::bounds(&bc_geom::polyline_path(&paths[0].points, true)).unwrap();
        let last = bc_geom::bounds(&bc_geom::polyline_path(&paths[4].points, true)).unwrap();
        assert!((first.width() - 9.0).abs() < 0.05 && (last.width() - 1.0).abs() < 0.05, "{first:?} {last:?}");
        assert!(paths.iter().all(|p| p.points.first() == p.points.last()), "closed rings");
    }
    #[test]
    fn perforation_cuts_dashes() {
        let mut layer = Layer::new(0);
        layer.perforate = true;
        layer.perf_cut = 3.0;
        layer.perf_skip = 1.0;
        let line = JobPath::burn(vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)]);
        let dashes = line::perforate(vec![line], &layer);
        let spans: Vec<(f64, f64)> = dashes.iter().map(|p| (p.points[0].x, p.points.last().unwrap().x)).collect();
        assert_eq!(spans.len(), 3, "{spans:?}");
        for (got, want) in spans.iter().zip([(0.0, 3.0), (4.0, 7.0), (8.0, 10.0)]) {
            assert!((got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9, "{spans:?}");
        }
        // Off by default.
        layer.perforate = false;
        assert_eq!(line::perforate(vec![JobPath::burn(vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)])], &layer).len(), 1);
    }

    #[test]
    fn clipped_images_only_burn_inside_the_clip() {
        // A black 10 × 10 mm image, clipped to its left 4 mm by a rectangle.
        let bytes = test_png(&[0; 100], 10, 10);
        let mut doc = Document::default();
        let img = doc.add_shape(ShapeKind::image(&bytes, 10.0, 10.0), Affine::translate((50.0, 50.0)), 0);
        let mask = doc.add_shape(ShapeKind::Rect { width: 4.0, height: 10.0, corner_radius: 0.0 }, Affine::translate((47.0, 50.0)), 1);
        assert_eq!(bc_doc::edit::clip_image(&mut doc, &[img, mask].into_iter().collect()), Some(img));
        assert!(doc.shape(mask).is_none(), "the mask shape is used up");
        let b = doc.shape(img).unwrap().bounds().unwrap();
        assert!((b.width() - 4.0).abs() < 1e-9, "the image's outline is now the clip: {b:?}");
        doc.layers[0].mode = LayerMode::Image;
        doc.layers[0].interval = 1.0;
        doc.layers[0].dither = DitherMode::Threshold;
        let xs: Vec<f64> = burns(&build_all(&doc)).iter().flat_map(|p| p.points.iter().map(|q| q.x)).collect();
        assert!(!xs.is_empty());
        assert!(xs.iter().all(|&x| (45.0 - 1e-9..=49.0 + 1e-9).contains(&x)), "only x 45..49 burns: {xs:?}");
        // Removing the clip restores the whole image.
        assert!(bc_doc::edit::unclip_images(&mut doc, &[img].into_iter().collect()));
        let xs: Vec<f64> = burns(&build_all(&doc)).iter().flat_map(|p| p.points.iter().map(|q| q.x)).collect();
        assert!(xs.iter().any(|&x| x > 54.0));
    }

    #[test]
    fn grayscale_power_levels_and_fallback() {
        // One row: black, mid-gray, white.
        let bytes = test_png(&[0, 128, 255], 3, 1);
        let mut doc = Document::default();
        doc.add_shape(ShapeKind::image(&bytes, 3.0, 1.0), Affine::translate((10.0, 10.0)), 0);
        doc.layers[0].mode = LayerMode::Image;
        doc.layers[0].interval = 1.0;
        doc.layers[0].dither = DitherMode::Grayscale;
        let job = build_all(&doc);
        let powers: Vec<f32> = burns(&job).iter().map(|p| p.power).collect();
        assert_eq!(powers.len(), 2, "white is skipped: {powers:?}");
        assert!(powers[0] > 0.95 && (powers[1] - 0.5).abs() < 0.1, "{powers:?}");
        let job = build(&doc, None, BuildOptions { grayscale: false });
        assert!(burns(&job).iter().all(|p| p.power == 1.0));
        assert_eq!(job.skipped.len(), 1);
    }

    #[test]
    fn trace_finds_the_dark_square_at_true_size() {
        // 10×10 white image with a dark 4×4 square in the middle, shown 20 mm wide at (100, 100).
        let mut px = vec![255u8; 100];
        for y in 3..7 {
            for x in 3..7 {
                px[y * 10 + x] = 0;
            }
        }
        let mut doc = Document::default();
        let id = doc.add_shape(ShapeKind::image(&test_png(&px, 10, 10), 20.0, 20.0), Affine::translate((100.0, 100.0)), 0);
        let shape = doc.shape(id).unwrap();
        // Smooth curves: the square's edges land where the pixels' edges are, within a little.
        let b = bc_geom::bounds(&trace_image(shape, 0.5, 0.1, 0.5).unwrap()).unwrap();
        assert!((b.width() - 8.0).abs() < 0.5 && (b.x0 - 96.0).abs() < 0.3, "{b:?}");
        // Straight lines along the pixels: exact.
        let straight = TraceSettings { smooth: false, ..TraceSettings::default() };
        let b = bc_geom::bounds(&trace_image_with(shape, &straight).unwrap()).unwrap();
        assert!((b.width() - 8.0).abs() < 1e-6 && (b.x0 - 96.0).abs() < 1e-6, "{b:?}");
        // A thick bar's centre line is one open stroke along its middle.
        let mut bar = vec![255u8; 100];
        for y in 4..7 {
            for x in 1..9 {
                bar[y * 10 + x] = 0;
            }
        }
        let id = doc.add_shape(ShapeKind::image(&test_png(&bar, 10, 10), 20.0, 20.0), Affine::translate((100.0, 100.0)), 0);
        let line = trace_image_with(doc.shape(id).unwrap(), &TraceSettings { kind: TraceKind::CentreLine, ..TraceSettings::default() }).unwrap();
        assert!(!line.elements().iter().any(|e| matches!(e, bc_geom::PathEl::ClosePath)));
        let b = bc_geom::bounds(&line).unwrap();
        assert!(b.width() > 6.0 && b.height() < 2.5 && (b.y0 - 101.0).abs() < 1.0, "a horizontal stroke through the middle: {b:?}");
    }

    #[test]
    fn images_on_vector_layers_are_reported() {
        let mut doc = Document::default();
        doc.add_shape(ShapeKind::image(&[1, 2, 3], 4.0, 2.0), Affine::IDENTITY, 0);
        let job = build_all(&doc);
        assert!(job.is_empty());
        assert_eq!(job.skipped.len(), 1);
    }

    #[test]
    fn tool_layers_are_never_cut() {
        let mut doc = doc_with(&[(rect(10.0, 10.0), (50.0, 50.0), bc_doc::CUT_LAYER_COUNT)]);
        doc.layers[bc_doc::CUT_LAYER_COUNT].output = true;
        assert!(build_all(&doc).is_empty());
    }

    #[test]
    fn cut_selected_limits_output() {
        let doc = doc_with(&[(rect(10.0, 10.0), (50.0, 50.0), 0), (rect(10.0, 10.0), (80.0, 50.0), 0)]);
        let job = build(&doc, Some(&BTreeSet::from([2])), BuildOptions::default());
        assert_eq!(job.layers[0].paths.len(), 1);
    }

    #[test]
    fn origin_corner_mapping_round_trips() {
        for origin in OriginCorner::ALL {
            let cfg = MachineConfig { bed: (600.0, 400.0), origin, rapid_speed: 300.0 };
            let p = Point::new(12.0, 34.0);
            assert_eq!(cfg.to_design(cfg.to_machine(p)), p);
        }
        let cfg = MachineConfig { bed: (600.0, 400.0), origin: OriginCorner::TopRight, rapid_speed: 300.0 };
        assert_eq!(cfg.to_machine(Point::new(0.0, 0.0)), (600.0, 0.0));
    }

    #[test]
    fn stats_measure_cut_length() {
        let doc = doc_with(&[(rect(10.0, 20.0), (50.0, 50.0), 0)]);
        let s = build_all(&doc).stats(Point::ORIGIN, 300.0);
        assert!((s.cut_mm - 60.0).abs() < 1e-6);
    }
}
