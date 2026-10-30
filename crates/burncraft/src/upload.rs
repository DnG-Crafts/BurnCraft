//! The window shown when a picture is imported: how it will be used (engraved, engraved and cut
//! round, or just its outline cut), with optional background removal and a preview of the cut line.

use bc_geom::{Affine, BezPath};
use eframe::egui::{self, Color32, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseAs {
    /// Engrave the picture; nothing is cut.
    Engrave,
    /// Engrave the picture, then cut round it.
    EngraveAndCut,
    /// Cut just its outline from the material.
    Cut,
    /// Cut lines round the dark parts of the design (traced), e.g. round each letter.
    Trace,
}

/// A picture being imported.
pub struct Upload {
    pub name: String,
    /// The file as it came.
    original: Vec<u8>,
    /// The picture as it will be added (background removed, if chosen).
    pub png: Vec<u8>,
    /// Size it's placed at (mm).
    pub size: (f64, f64),
    pub use_as: UseAs,
    remove_background: bool,
    /// How different (%) from the backdrop a pixel may be and still be cleared.
    tolerance: f32,
    /// How far outside the picture the cut line runs (mm).
    pub margin: f64,
    /// Trace: how dark (0..1) a part must be to be traced, and whether the picture is added too.
    threshold: f32,
    pub keep_picture: bool,
    /// The cut line, in mm from the picture's top-left corner.
    pub outline: BezPath,
    texture: Option<egui::TextureHandle>,
    dirty: bool,
}

/// What the window asks the app to do.
pub enum UploadOutcome {
    Open,
    Cancel,
    Add,
}

impl Upload {
    pub fn new(name: String, bytes: Vec<u8>, size: (f64, f64)) -> Self {
        Self {
            name,
            png: bytes.clone(),
            original: bytes,
            size,
            use_as: UseAs::EngraveAndCut,
            remove_background: false,
            tolerance: 15.0,
            margin: 0.0,
            threshold: 0.5,
            keep_picture: false,
            outline: BezPath::new(),
            texture: None,
            dirty: true,
        }
    }

    /// Redoes the background removal and the cut line.
    fn refresh(&mut self) {
        let (w, h) = self.size;
        let mut png = self.original.clone();
        if self.remove_background {
            // Clear the backdrop inwards from each corner.
            for at in [(0.01, 0.01), (w - 0.01, 0.01), (w - 0.01, h - 0.01), (0.01, h - 0.01)] {
                if let Ok(p) = bc_raster::erase_colour(&png, at, w, h, self.tolerance / 100.0, false) {
                    png = p;
                }
            }
        }
        self.outline = if self.use_as == UseAs::Trace { trace_lines(&png, w, h, self.threshold) } else { cut_line(&png, w, h, self.margin) }.unwrap_or_default();
        self.png = png;
        self.texture = None;
        self.dirty = false;
    }

    pub fn ui(&mut self, ctx: &egui::Context) -> UploadOutcome {
        if self.dirty {
            self.refresh();
        }
        let mut outcome = UploadOutcome::Open;
        let mut open = true;
        let title = format!("Import picture - {}", self.name);
        crate::dialogs::centered(ctx, title).id(egui::Id::new("upload-window")).open(&mut open).resizable(false).auto_sized().collapsible(false).show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                self.preview(ui);
                ui.vertical(|ui| {
                    ui.set_width(270.0);
                    ui.label(RichText::new("How will you use it?").strong());
                    for (k, title, sub) in [
                        (UseAs::Engrave, "Engrave the picture", "Engrave the picture; nothing is cut"),
                        (UseAs::EngraveAndCut, "Engrave and cut round it", "Engrave the picture, then cut round it (red line)"),
                        (UseAs::Cut, "Cut the outline only", "Just the outer shape, cut from material"),
                        (UseAs::Trace, "Trace cut lines", "Cut lines round the dark parts (each letter, each shape)"),
                    ] {
                        if ui.radio(self.use_as == k, RichText::new(title).strong()).clicked() && self.use_as != k {
                            // Tracing and the outline are worked out differently.
                            self.dirty |= (self.use_as == UseAs::Trace) != (k == UseAs::Trace);
                            self.use_as = k;
                        }
                        ui.label(RichText::new(sub).small().weak());
                    }
                    ui.separator();
                    if ui.checkbox(&mut self.remove_background, "Remove background").on_hover_text("Clears the plain backdrop round the edges (white, or any one colour), so it isn't burned and the cut line follows the subject").changed() {
                        self.dirty = true;
                    }
                    if self.remove_background {
                        ui.horizontal(|ui| {
                            ui.label("Tolerance");
                            if ui.add(egui::Slider::new(&mut self.tolerance, 0.0..=60.0)).on_hover_text("Raise it if bits of the backdrop are left; lower it if parts of the subject disappear").drag_stopped() {
                                self.dirty = true;
                            }
                        });
                        ui.small("Use the image eraser (Brush or Colour) afterwards for finer clean-up.");
                    }
                    if self.use_as == UseAs::Trace {
                        ui.horizontal(|ui| {
                            ui.label("Threshold").on_hover_text("How dark a part must be to be traced");
                            if ui.add(egui::Slider::new(&mut self.threshold, 0.05..=0.95)).drag_stopped() {
                                self.dirty = true;
                            }
                            if ui.button("Auto").on_hover_text("The level that best splits the picture into dark and light").clicked() {
                                let shape = picture_shape(&self.png, self.size.0, self.size.1);
                                if let Ok(t) = bc_job::auto_threshold(&shape) {
                                    self.threshold = t;
                                    self.dirty = true;
                                }
                            }
                        });
                        ui.checkbox(&mut self.keep_picture, "Add the picture too").on_hover_text("Also add the picture, to engrave or as a guide");
                        if self.outline.elements().is_empty() {
                            ui.colored_label(ui.visuals().warn_fg_color, "Nothing dark enough to trace: raise the threshold.");
                        }
                    } else if self.use_as != UseAs::Engrave {
                        ui.horizontal(|ui| {
                            ui.label("Cut margin");
                            if ui.add(egui::DragValue::new(&mut self.margin).range(0.0..=20.0).speed(0.05).suffix(" mm")).on_hover_text("How far outside the picture the cut line runs").changed() {
                                self.dirty = true;
                            }
                        });
                        if self.outline.elements().is_empty() {
                            ui.colored_label(ui.visuals().warn_fg_color, "Nothing to cut round: try removing the background.");
                        }
                    }
                    ui.add_space(6.0);
                    ui.small(format!("{:.0} × {:.0} mm - resize after placing", self.size.0, self.size.1));
                    ui.add_space(6.0);
                    let can = self.use_as == UseAs::Engrave || !self.outline.elements().is_empty();
                    let add = egui::Button::new(RichText::new("Add to design").strong().color(Color32::WHITE))
                        .fill(Color32::from_rgb(47, 128, 237))
                        .min_size(egui::vec2(160.0, 30.0))
                        .corner_radius(egui::CornerRadius::same(15));
                    if ui.add_enabled(can, add).clicked() {
                        outcome = UploadOutcome::Add;
                    }
                });
            });
        });
        if !open {
            outcome = UploadOutcome::Cancel;
        }
        outcome
    }

    /// The picture on a checkerboard (see-through parts show), with the cut line in red.
    fn preview(&mut self, ui: &mut egui::Ui) {
        let (w, h) = self.size;
        let k = (360.0 / w.max(h)) as f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w as f32 * k + 20.0, h as f32 * k + 20.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, Color32::from_gray(235));
        let inner = egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 10.0), egui::vec2(w as f32 * k, h as f32 * k));
        let cell = 10.0;
        for j in 0..(inner.height() / cell).ceil() as i32 {
            for i in 0..(inner.width() / cell).ceil() as i32 {
                if (i + j) % 2 == 0 {
                    let c = egui::Rect::from_min_size(inner.min + egui::vec2(i as f32 * cell, j as f32 * cell), egui::vec2(cell, cell)).intersect(inner);
                    painter.rect_filled(c, 0.0, Color32::from_gray(212));
                }
            }
        }
        if self.use_as != UseAs::Cut && !(self.use_as == UseAs::Trace && !self.keep_picture) {
            if self.texture.is_none()
                && let Ok(img) = image::load_from_memory(&self.png)
            {
                // A preview of at most 800 px across is plenty.
                let img = img.thumbnail(800, 800).to_rgba8();
                let colour = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                self.texture = Some(ui.ctx().load_texture("upload-preview", colour, egui::TextureOptions::LINEAR));
            }
            if let Some(t) = &self.texture {
                painter.image(t.id(), inner, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
        if self.use_as != UseAs::Engrave {
            let stroke = egui::Stroke::new(2.0, Color32::from_rgb(220, 30, 30));
            for line in bc_geom::flatten(&self.outline, 0.2) {
                let mut pts: Vec<egui::Pos2> = line.points.iter().map(|p| inner.min + egui::vec2(p.x as f32 * k, p.y as f32 * k)).collect();
                if line.closed && let Some(&first) = pts.first() {
                    pts.push(first);
                }
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
    }
}

/// The picture as a shape whose top-left is at the origin (for tracing it in mm).
fn picture_shape(png: &[u8], w: f64, h: f64) -> bc_doc::Shape {
    bc_doc::Shape { id: 0, kind: bc_doc::ShapeKind::image(png, w, h), transform: Affine::translate((w / 2.0, h / 2.0)), layer: 0, locked: false, group: None, effects: Vec::new() }
}

/// Cut lines round the parts of the picture darker than `threshold`, as Trace Image makes them,
/// in mm from the picture's top-left corner.
fn trace_lines(png: &[u8], w: f64, h: f64, threshold: f32) -> Option<BezPath> {
    let settings = bc_job::TraceSettings { threshold, ..bc_job::TraceSettings::default() };
    bc_job::trace_image_with(&picture_shape(png, w, h), &settings).ok().filter(|p| !p.elements().is_empty())
}

/// The cut line round what's solid in the picture (`w` × `h` mm), `margin` mm outside it, in mm
/// from the picture's top-left corner: one outline per separate piece, no holes.
fn cut_line(png: &[u8], w: f64, h: f64, margin: f64) -> Option<BezPath> {
    let mask = bc_raster::opacity_mask(png).ok()?;
    let (pw, _) = bc_io::image_dimensions(png)?;
    // Pixel size (mm); tracing works at most 1500 px across, so a pixel is never finer than that.
    let px = (w / pw as f64).max(w.max(h) / 1500.0);
    let shape = picture_shape(&mask, w, h);
    let settings = bc_job::TraceSettings { threshold: 0.5, outer_only: true, tolerance: px, min_area: 64.0 * px * px, ..bc_job::TraceSettings::default() };
    let traced = bc_job::trace_image_with(&shape, &settings).ok()?;
    bc_geom::ops::contour(&[traced], margin.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo() -> Vec<u8> {
        // 100 × 100 px (10 × 10 mm at 0.1 mm/px): white, with a dark square in the middle.
        let mut img = image::RgbaImage::from_pixel(100, 100, image::Rgba([250, 250, 250, 255]));
        for y in 30..70 {
            for x in 30..70 {
                img.put_pixel(x, y, image::Rgba([30, 30, 30, 255]));
            }
        }
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        png.into_inner()
    }

    #[test]
    fn the_cut_line_follows_what_is_left_after_the_background_goes() {
        let mut u = Upload::new("test".into(), photo(), (10.0, 10.0));
        u.use_as = UseAs::EngraveAndCut;
        u.margin = 0.0;
        u.refresh();
        // Nothing removed: the picture is solid all over, so the line goes round its edge.
        let b = bc_geom::bounds(&u.outline).unwrap();
        assert!((b.width() - 10.0).abs() < 0.3, "{b:?}");
        // Background removed: just the square, 4 mm across, in the middle.
        u.remove_background = true;
        u.refresh();
        let b = bc_geom::bounds(&u.outline).unwrap();
        assert!((b.width() - 4.0).abs() < 0.3 && (b.x0 - 3.0).abs() < 0.3, "{b:?}");
        // A margin moves the line out.
        u.margin = 1.0;
        u.refresh();
        let b = bc_geom::bounds(&u.outline).unwrap();
        assert!((b.width() - 6.0).abs() < 0.3, "{b:?}");
    }
}

#[cfg(test)]
mod diagnostics {
    use super::*;

    /// Imports `UPLOAD_IMG` with the background removed, traces it as the Trace dialog does by
    /// default, and draws the lines (red) over the picture to `UPLOAD_OUT`.
    #[test]
    #[ignore = "diagnostic"]
    fn trace_an_upload() {
        let (Ok(src), Ok(dst)) = (std::env::var("UPLOAD_IMG"), std::env::var("UPLOAD_OUT")) else { return };
        let bytes = std::fs::read(src).unwrap();
        let mut u = Upload::new("x".into(), bytes, (200.0, 200.0));
        u.remove_background = true;
        u.refresh();
        let shape = bc_doc::Shape { id: 0, kind: bc_doc::ShapeKind::image(&u.png, 200.0, 200.0), transform: Affine::translate((100.0, 100.0)), layer: 0, locked: false, group: None, effects: Vec::new() };
        let traced = bc_job::trace_image_with(&shape, &bc_job::TraceSettings::default()).unwrap();
        let lines = bc_geom::flatten(&traced, 0.02);
        println!("{} outlines", lines.len());
        let k = 4.0;
        let mut out = image::RgbaImage::from_pixel(800, 800, image::Rgba([255, 255, 255, 255]));
        let pic = image::load_from_memory(&u.png).unwrap().resize_exact(800, 800, image::imageops::FilterType::Triangle).to_rgba8();
        for (o, p) in out.pixels_mut().zip(pic.pixels()) {
            let a = p.0[3] as u32;
            for c in 0..3 {
                o.0[c] = ((p.0[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
            }
        }
        for l in lines {
            let mut pts = l.points.clone();
            if l.closed && let Some(&f) = pts.first() {
                pts.push(f);
            }
            for w in pts.windows(2) {
                let n = (w[0].distance(w[1]) * k * 2.0).ceil().max(1.0) as usize;
                for s in 0..=n {
                    let q = w[0].lerp(w[1], s as f64 / n as f64);
                    let (x, y) = ((q.x * k) as i64, (q.y * k) as i64);
                    if (0..800).contains(&x) && (0..800).contains(&y) {
                        out.put_pixel(x as u32, y as u32, image::Rgba([230, 20, 20, 255]));
                    }
                }
            }
        }
        out.save(dst).unwrap();
    }
}
