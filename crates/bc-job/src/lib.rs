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

    /// The first `along` mm of the path, and where it ends.
    pub fn partial(&self, along: f64) -> (Vec<Point>, Point) {
        let mut out = vec![self.points[0]];
        let mut left = along.max(0.0);
        for w in self.points.windows(2) {
            let d = w[0].distance(w[1]);
            if d >= left {
                let end = if d > 0.0 { w[0].lerp(w[1], left / d) } else { w[1] };
                out.push(end);
                return (out, end);
            }
            left -= d;
            out.push(w[1]);
        }
        let end = *self.points.last().expect("paths have points");
        (out, end)
    }
}

/// Where a job is at some moment of its run (see [`Job::moment`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moment {
    /// Paths finished, counting through the layers in order.
    pub done: usize,
    /// How far (mm) along the next path the head has got, or None while it travels to its start.
    pub along: Option<f64>,
    /// Where the head is (design mm).
    pub head: Point,
    /// The layer being worked on (an index into the job's layers), until the job ends.
    pub layer: Option<usize>,
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

    /// Where the job is `fraction` of the way through its running time (the same rough timing as
    /// [`Job::stats`]), starting from `start`.
    pub fn moment(&self, start: Point, rapid_speed: f64, fraction: f64) -> Moment {
        if fraction >= 1.0 {
            let paths = self.layers.iter().flat_map(|l| &l.paths);
            let head = paths.clone().last().and_then(|p| p.points.last().copied()).unwrap_or(start);
            return Moment { done: paths.count(), along: None, head, layer: None };
        }
        let total = self.stats(start, rapid_speed).seconds;
        let mut left = fraction.clamp(0.0, 1.0) * total;
        let rapid = rapid_speed.max(1.0);
        let mut pos = start;
        let mut done = 0;
        for (li, layer) in self.layers.iter().enumerate() {
            let speed = layer.settings.speed.max(0.1);
            for path in &layer.paths {
                let first = path.points[0];
                let travel = pos.distance(first) / rapid;
                if left < travel {
                    let head = pos.lerp(first, left / travel);
                    return Moment { done, along: None, head, layer: Some(li) };
                }
                left -= travel;
                let cut = path.length() / speed;
                if left < cut {
                    let along = left * speed;
                    return Moment { done, along: Some(along), head: path.partial(along).1, layer: Some(li) };
                }
                left -= cut;
                pos = *path.points.last().expect("paths have points");
                done += 1;
            }
        }
        Moment { done, along: None, head: pos, layer: None }
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

/// The design's cutting lines (Line and Fill + Line layers with Output on) in the order the job
/// cuts them, each starting where and running the way it will be cut: one pass of each layer.
pub fn cut_order(doc: &Document) -> Vec<Vec<Point>> {
    let mut out = Vec::new();
    for (index, settings) in doc.layers.iter().enumerate() {
        if bc_doc::is_tool_layer(index) || !settings.output || !matches!(settings.mode, LayerMode::Line | LayerMode::FillThenLine) {
            continue;
        }
        let vectors: Vec<&Shape> = doc.shapes.iter().filter(|s| s.layer == index && !matches!(s.kind, ShapeKind::Image { .. })).collect();
        if vectors.is_empty() {
            continue;
        }
        out.extend(line::line_paths(&vectors, settings).into_iter().filter(|p| !p.is_off() && p.points.len() >= 2).map(|p| p.points));
    }
    out
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
