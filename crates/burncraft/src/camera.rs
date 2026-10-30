//! Camera panel, 4-point alignment, and the overlay drawn under the design.

use bc_camera::{CameraStream, Frame, Homography};
use bc_doc::Layer;
use bc_geom::Point;
use bc_job::{Job, JobLayer, JobPath};
use eframe::egui::{self, Color32, Stroke};
use serde::{Deserialize, Serialize};

/// Remembered between runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraSettings {
    pub index: Option<u32>,
    /// Camera pixels → bed millimetres.
    pub alignment: Option<Homography>,
    pub opacity: f32,
    /// Distance of the alignment target's corners from the bed edges (mm).
    pub target_inset: f64,
    /// Power (%) and speed (mm/s) used to mark the alignment target.
    pub target_power: f64,
    pub target_speed: f64,
    /// Item detection: brightness difference from the bed (0–255) and smallest item (mm²).
    pub detect_threshold: u8,
    pub detect_min_area: f64,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self { index: None, alignment: None, opacity: 0.8, target_inset: 30.0, target_power: 15.0, target_speed: 100.0, detect_threshold: 40, detect_min_area: 100.0 }
    }
}

/// What the overlay needs from the canvas's point of view.
pub struct Overlay {
    pub texture: egui::TextureId,
    /// Bed millimetres → camera pixels.
    pub bed_to_image: Homography,
    pub image_size: (f64, f64),
    pub opacity: f32,
}

pub enum CameraAction {
    /// Mark the alignment target on the material.
    MarkTarget(Job),
    /// Items found on the bed, to become jig slots.
    Detected(Vec<bc_geom::fit::OrientedRect>),
}

struct Aligning {
    frame: Frame,
    texture: egui::TextureHandle,
    clicks: Vec<(f64, f64)>,
}

pub struct CameraUi {
    pub settings: CameraSettings,
    cameras: Vec<(u32, String)>,
    stream: Option<CameraStream>,
    texture: Option<(u64, egui::TextureHandle, (usize, usize))>,
    pub show_overlay: bool,
    aligning: Option<Aligning>,
}

const CORNER_NAMES: [&str; 4] = ["top-left", "top-right", "bottom-right", "bottom-left"];

impl CameraUi {
    pub fn new(settings: CameraSettings) -> Self {
        Self { settings, cameras: Vec::new(), stream: None, texture: None, show_overlay: true, aligning: None }
    }

    /// Bed corners of the alignment target, in click order.
    fn target_corners(&self, bed: (f64, f64)) -> [(f64, f64); 4] {
        let i = self.settings.target_inset.clamp(0.0, bed.0.min(bed.1) / 3.0);
        [(i, i), (bed.0 - i, i), (bed.0 - i, bed.1 - i), (i, bed.1 - i)]
    }

    /// A job that marks the target: a rectangle with small crosses at the corners.
    fn target_job(&self, bed: (f64, f64)) -> Job {
        let c = self.target_corners(bed);
        let mut paths: Vec<JobPath> = Vec::new();
        let p = |(x, y): (f64, f64)| Point::new(x, y);
        paths.push(JobPath::burn(vec![p(c[0]), p(c[1]), p(c[2]), p(c[3]), p(c[0])]));
        for &(x, y) in &c {
            paths.push(JobPath::burn(vec![Point::new(x - 5.0, y), Point::new(x + 5.0, y)]));
            paths.push(JobPath::burn(vec![Point::new(x, y - 5.0), Point::new(x, y + 5.0)]));
        }
        let mut settings = Layer::new(0);
        settings.name = "Camera target".into();
        settings.speed = self.settings.target_speed;
        settings.min_power = self.settings.target_power;
        settings.max_power = self.settings.target_power;
        Job { layers: vec![JobLayer { index: 0, settings, paths }], skipped: Vec::new() }
    }

    /// Uploads the newest frame as a texture, when there is one.
    pub fn update(&mut self, ctx: &egui::Context) {
        let Some(stream) = &self.stream else { return };
        if stream.latest_sequence() != self.texture.as_ref().map(|t| t.0)
            && let Some(f) = stream.latest() {
                let image = egui::ColorImage::from_rgb([f.width, f.height], &f.rgb);
                match &mut self.texture {
                    Some((seq, tex, size)) => {
                        tex.set(image, egui::TextureOptions::LINEAR);
                        *seq = f.sequence;
                        *size = (f.width, f.height);
                    }
                    None => self.texture = Some((f.sequence, ctx.load_texture("camera", image, egui::TextureOptions::LINEAR), (f.width, f.height))),
                }
            }
        // Keep frames flowing while the camera runs.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// The newest camera frame, while the camera runs.
    pub fn latest_frame(&self) -> Option<Frame> {
        self.stream.as_ref()?.latest()
    }

    pub fn overlay(&self) -> Option<Overlay> {
        if !self.show_overlay || self.stream.is_none() {
            return None;
        }
        let (_, tex, (w, h)) = self.texture.as_ref()?;
        Some(Overlay { texture: tex.id(), bed_to_image: self.settings.alignment?.inverse()?, image_size: (*w as f64, *h as f64), opacity: self.settings.opacity })
    }

    pub fn panel(&mut self, ui: &mut egui::Ui, bed: (f64, f64), connected: bool, speed: crate::units::Speed) -> Option<CameraAction> {
        let mut action = None;
        egui::CollapsingHeader::new("Camera").id_salt("camera").show(ui, |ui| {
            ui.horizontal(|ui| {
                let name = self.settings.index.and_then(|i| self.cameras.iter().find(|c| c.0 == i).map(|c| c.1.clone())).unwrap_or_else(|| match self.settings.index {
                    Some(i) => format!("Camera {i}"),
                    None => "(choose)".into(),
                });
                egui::ComboBox::from_id_salt("camera-pick").selected_text(name).show_ui(ui, |ui| {
                    for (i, n) in &self.cameras {
                        ui.selectable_value(&mut self.settings.index, Some(*i), n);
                    }
                    if self.cameras.is_empty() {
                        ui.weak(if bc_camera::available() { "Press ⟳ to look for cameras" } else { bc_camera::UNAVAILABLE });
                    }
                });
                if ui.small_button("⟳").on_hover_text("Look for cameras").clicked() {
                    self.cameras = bc_camera::list_cameras();
                }
                if self.stream.is_some() {
                    if ui.button("Stop").clicked() {
                        self.stream = None;
                        self.texture = None;
                    }
                } else if ui.add_enabled(self.settings.index.is_some(), egui::Button::new("Start")).clicked() {
                    self.stream = self.settings.index.map(CameraStream::start);
                }
            });
            if let Some(e) = self.stream.as_ref().and_then(CameraStream::error) {
                ui.colored_label(Color32::from_rgb(200, 60, 40), format!("Camera error: {e}"));
            }
            // Live preview while the camera runs, aligned or not.
            if self.stream.is_some() {
                match &self.texture {
                    Some((_, tex, (w, h))) => {
                        let width = ui.available_width().min(480.0);
                        let height = (width * *h as f32 / (*w).max(1) as f32).min(360.0);
                        let width = height * *w as f32 / (*h).max(1) as f32;
                        ui.add(egui::Image::new((tex.id(), egui::vec2(width, height))));
                        ui.weak(format!("{w} × {h}"));
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.weak("Waiting for the first picture…");
                        });
                    }
                }
            }
            ui.horizontal(|ui| {
                ui.add_enabled(self.settings.alignment.is_some(), egui::Checkbox::new(&mut self.show_overlay, "Show on bed"))
                    .on_disabled_hover_text("Align the camera first, so the picture can be laid over the bed in the right place");
                ui.add(egui::Slider::new(&mut self.settings.opacity, 0.1..=1.0).text("opacity"));
            });
            match self.settings.alignment {
                Some(_) => ui.weak("Aligned: with \"Show on bed\" on, the picture is laid over the design area."),
                None => ui.weak("Not aligned yet, so the picture only shows above. To lay it over the bed: 1. mark the target, 2. align."),
            };
            ui.horizontal(|ui| {
                ui.label("Target inset");
                ui.add(egui::DragValue::new(&mut self.settings.target_inset).range(5.0..=200.0).suffix(" mm"));
                ui.add(egui::DragValue::new(&mut self.settings.target_power).range(1.0..=100.0).suffix(" %"));
                ui.add(speed.edit(&mut self.settings.target_speed, 1.0..=1000.0));
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(connected, egui::Button::new("1. Mark target")).on_hover_text("Burns a rectangle with corner crosses onto scrap material covering the bed").clicked() {
                    action = Some(CameraAction::MarkTarget(self.target_job(bed)));
                }
                let frame = self.stream.as_ref().and_then(CameraStream::latest);
                if ui.add_enabled(frame.is_some(), egui::Button::new("2. Align…")).on_hover_text("Click the four target corners in the camera image").clicked() {
                    let f = frame.expect("enabled only with a frame");
                    let texture = ui.ctx().load_texture("camera-align", egui::ColorImage::from_rgb([f.width, f.height], &f.rgb), egui::TextureOptions::LINEAR);
                    self.aligning = Some(Aligning { frame: f, texture, clicks: Vec::new() });
                }
                if self.settings.alignment.is_some() && ui.small_button("Forget").clicked() {
                    self.settings.alignment = None;
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let frame = self.stream.as_ref().and_then(CameraStream::latest);
                let ready = frame.is_some() && self.settings.alignment.is_some();
                let hint = "Finds items laid on the bed and outlines each on tool layer Tool1, as slots for Arrange > Place on Each Slot";
                if ui.add_enabled(ready, egui::Button::new("Detect items")).on_hover_text(hint).on_disabled_hover_text("Start the camera and align it first").clicked()
                    && let (Some(f), Some(to_image)) = (frame, self.settings.alignment.and_then(|a| a.inverse()))
                {
                    let options = bc_camera::detect::DetectOptions { resolution: 1.0, threshold: self.settings.detect_threshold, min_area: self.settings.detect_min_area };
                    action = Some(CameraAction::Detected(bc_camera::detect::detect_items(&f, &to_image, bed, &options)));
                }
                ui.add(egui::DragValue::new(&mut self.settings.detect_threshold).range(5..=200).prefix("contrast ")).on_hover_text("How different from the bed an item must look; raise it if shadows or bed marks are picked up");
                ui.add(egui::DragValue::new(&mut self.settings.detect_min_area).range(4.0..=100_000.0).speed(5.0).prefix("min ").suffix(" mm²")).on_hover_text("Ignore anything smaller than this");
            });
        });
        action
    }

    /// The alignment window: click the four target corners in order.
    pub fn alignment_window(&mut self, ctx: &egui::Context, bed: (f64, f64)) {
        let corners = self.target_corners(bed);
        let Some(a) = &mut self.aligning else { return };
        let mut open = true;
        let mut done = false;
        crate::dialogs::centered(ctx, "Align camera").open(&mut open).default_size([900.0, 650.0]).show(ctx, |ui| {
            let next = a.clicks.len();
            if next < 4 {
                ui.label(format!("Click the {} corner of the marked rectangle (cross centre).", CORNER_NAMES[next]));
            } else {
                ui.label("All four corners set.");
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(!a.clicks.is_empty(), egui::Button::new("Undo last click")).clicked() {
                    a.clicks.pop();
                }
                if ui.add_enabled(a.clicks.len() == 4, egui::Button::new("Save alignment")).clicked() {
                    done = true;
                }
            });
            let (fw, fh) = (a.frame.width as f32, a.frame.height as f32);
            let avail = ui.available_size();
            let scale = (avail.x / fw).min(avail.y / fh).max(0.05);
            let (rect, response) = ui.allocate_exact_size(egui::vec2(fw * scale, fh * scale), egui::Sense::click());
            let painter = ui.painter_at(rect);
            painter.image(a.texture.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            if response.clicked() && a.clicks.len() < 4
                && let Some(p) = response.interact_pointer_pos() {
                    let px = ((p.x - rect.min.x) / scale) as f64;
                    let py = ((p.y - rect.min.y) / scale) as f64;
                    a.clicks.push((px, py));
                }
            let to_screen = |(x, y): (f64, f64)| egui::pos2(rect.min.x + x as f32 * scale, rect.min.y + y as f32 * scale);
            for (i, c) in a.clicks.iter().enumerate() {
                let s = to_screen(*c);
                painter.circle_stroke(s, 8.0, Stroke::new(2.0, Color32::RED));
                painter.text(s + egui::vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, CORNER_NAMES[i], egui::FontId::proportional(14.0), Color32::RED);
            }
            if a.clicks.len() > 1 {
                let mut pts: Vec<egui::Pos2> = a.clicks.iter().map(|c| to_screen(*c)).collect();
                if a.clicks.len() == 4 {
                    pts.push(pts[0]);
                }
                painter.add(egui::Shape::line(pts, Stroke::new(1.0, Color32::RED)));
            }
        });
        if done {
            let clicks: [(f64, f64); 4] = a.clicks.clone().try_into().expect("four clicks");
            self.settings.alignment = Homography::from_points(clicks, corners);
            self.aligning = None;
        } else if !open {
            self.aligning = None;
        }
    }
}

/// Draws the camera image warped onto the bed, as a grid of textured quads.
pub fn draw_overlay(painter: &egui::Painter, overlay: &Overlay, bed: (f64, f64), to_screen: impl Fn(Point) -> egui::Pos2) {
    const N: usize = 24;
    let tint = Color32::from_white_alpha((overlay.opacity * 255.0) as u8);
    let mut mesh = egui::Mesh::with_texture(overlay.texture);
    let (w, h) = overlay.image_size;
    let mut index = vec![None; (N + 1) * (N + 1)];
    for j in 0..=N {
        for i in 0..=N {
            let p = Point::new(bed.0 * i as f64 / N as f64, bed.1 * j as f64 / N as f64);
            let (u, v) = overlay.bed_to_image.apply((p.x, p.y));
            let uv = egui::pos2((u / w) as f32, (v / h) as f32);
            if (-0.02..=1.02).contains(&uv.x) && (-0.02..=1.02).contains(&uv.y) {
                index[j * (N + 1) + i] = Some(mesh.vertices.len() as u32);
                mesh.vertices.push(egui::epaint::Vertex { pos: to_screen(p), uv, color: tint });
            }
        }
    }
    for j in 0..N {
        for i in 0..N {
            let at = |di: usize, dj: usize| index[(j + dj) * (N + 1) + i + di];
            if let (Some(a), Some(b), Some(c), Some(d)) = (at(0, 0), at(1, 0), at(1, 1), at(0, 1)) {
                mesh.add_triangle(a, b, c);
                mesh.add_triangle(a, c, d);
            }
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}
