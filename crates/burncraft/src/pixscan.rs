//! The PixScan window: a photo of an item on the printed marker sheet, placed on the bed at true
//! size (see `bc_raster::pixscan`).

use bc_raster::pixscan::{self, MarkerSheet, Pt};
use eframe::egui::{self, Color32};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Paper {
    A4,
    Letter,
}

impl Paper {
    fn size(self) -> (f64, f64) {
        match self {
            Paper::A4 => (210.0, 297.0),
            Paper::Letter => (215.9, 279.4),
        }
    }
}

struct Photo {
    image: image::RgbaImage,
    texture: egui::TextureHandle,
    /// Marker centres in photo pixels: top-left, top-right, bottom-right, bottom-left.
    corners: [Pt; 4],
}

pub struct PixScan {
    pub open: bool,
    paper: Paper,
    sheet: MarkerSheet,
    photo: Option<Photo>,
    /// Pixels per mm in the placed picture.
    detail: f64,
    dragging: Option<usize>,
    status: String,
}

impl Default for PixScan {
    fn default() -> Self {
        Self { open: false, paper: Paper::A4, sheet: MarkerSheet::default(), photo: None, detail: 8.0, dragging: None, status: String::new() }
    }
}

/// A straightened photo to add to the design: PNG bytes and its size in mm.
pub struct Placed {
    pub png: Vec<u8>,
    pub width: f64,
    pub height: f64,
}

impl PixScan {
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Placed> {
        if !self.open {
            return None;
        }
        let mut placed = None;
        let mut open = true;
        crate::dialogs::centered(ctx, "PixScan: Photo to True Size").open(&mut open).resizable(false).auto_sized().show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label("1. Print the marker sheet at 100% (actual size).");
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.paper, Paper::A4, "A4");
                ui.radio_value(&mut self.paper, Paper::Letter, "Letter");
                if ui.button("Save Marker Sheet…").clicked() {
                    self.save_sheet();
                }
            });
            ui.label("2. Put the item on the sheet, inside the four squares, and photograph it from straight above.");
            ui.horizontal(|ui| {
                if ui.button("Open Photo…").clicked() {
                    self.open_photo(ctx);
                }
                ui.label("Detail");
                ui.add(egui::DragValue::new(&mut self.detail).range(2.0..=20.0).speed(0.1).suffix(" px/mm")).on_hover_text("Pixels per mm in the placed picture");
            });
            if let Some(photo) = &mut self.photo {
                ui.label("3. Check each corner sits on the middle of its square (drag to adjust), then place it.");
                corner_editor(ui, photo, &mut self.dragging);
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new("Place on the Bed").strong()).clicked() {
                        let (w, h) = self.sheet.span(self.paper.size().0, self.paper.size().1);
                        placed = match pixscan::rectify(&photo.image, &photo.corners, w, h, self.detail) {
                            Some(img) => {
                                let mut png = Vec::new();
                                match img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png) {
                                    Ok(()) => Some(Placed { png, width: w, height: h }),
                                    Err(e) => {
                                        self.status = format!("Couldn't make the picture: {e}");
                                        None
                                    }
                                }
                            }
                            None => {
                                self.status = "The corners don't make a usable shape: drag them onto the squares.".into();
                                None
                            }
                        };
                    }
                });
            }
            if !self.status.is_empty() {
                ui.add(egui::Label::new(&self.status).wrap());
            }
        });
        self.open = open;
        if placed.is_some() {
            self.open = false;
        }
        placed
    }

    fn save_sheet(&mut self) {
        let (w, h) = self.paper.size();
        let name = format!("PixScan sheet {}.svg", if self.paper == Paper::A4 { "A4" } else { "Letter" });
        let Some(path) = rfd::FileDialog::new().add_filter("SVG", &["svg"]).set_file_name(name).save_file() else { return };
        self.status = match std::fs::write(&path, self.sheet.svg(w, h)) {
            Ok(()) => format!("Saved {}. Print it at 100% (\"actual size\"), not \"fit to page\".", path.display()),
            Err(e) => format!("Couldn't save: {e}"),
        };
    }

    fn open_photo(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new().add_filter("Photo", &["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff"]).pick_file() else { return };
        let image = match image::open(&path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.status = format!("Couldn't open {}: {e}", path.display());
                return;
            }
        };
        let (w, h) = (image.width() as f64, image.height() as f64);
        let (corners, found) = match pixscan::find_markers(&image) {
            Some(c) => (c, true),
            // Not found: start with the corners in from the photo's edges, to drag into place.
            None => ([(w * 0.1, h * 0.1), (w * 0.9, h * 0.1), (w * 0.9, h * 0.9), (w * 0.1, h * 0.9)], false),
        };
        self.status = if found { "Found the four squares.".into() } else { "Couldn't find the four squares: drag the corners onto them.".into() };
        // A preview texture at most 1600 px across.
        let k = (1600.0 / w.max(h)).min(1.0);
        let preview = image::imageops::resize(&image, ((w * k) as u32).max(1), ((h * k) as u32).max(1), image::imageops::FilterType::Triangle);
        let colour = egui::ColorImage::from_rgba_unmultiplied([preview.width() as usize, preview.height() as usize], preview.as_raw());
        let texture = ctx.load_texture("pixscan-photo", colour, egui::TextureOptions::LINEAR);
        self.photo = Some(Photo { image, texture, corners });
    }
}

/// The photo with its four corners, which can be dragged.
fn corner_editor(ui: &mut egui::Ui, photo: &mut Photo, dragging: &mut Option<usize>) {
    let (pw, ph) = (photo.image.width() as f32, photo.image.height() as f32);
    let scale = (560.0 / pw).min(400.0 / ph);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(pw * scale, ph * scale), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.image(photo.texture.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    let to_screen = |p: Pt| rect.min + egui::vec2(p.0 as f32 * scale, p.1 as f32 * scale);
    let to_photo = |s: egui::Pos2| (((s.x - rect.min.x) / scale) as f64, ((s.y - rect.min.y) / scale) as f64);
    if response.drag_started()
        && let Some(at) = response.interact_pointer_pos()
    {
        *dragging = (0..4).min_by(|&a, &b| to_screen(photo.corners[a]).distance(at).total_cmp(&to_screen(photo.corners[b]).distance(at))).filter(|&i| to_screen(photo.corners[i]).distance(at) < 30.0);
    }
    if let (Some(i), Some(at)) = (*dragging, response.interact_pointer_pos())
        && response.dragged()
    {
        let at = at.clamp(rect.min, rect.max);
        photo.corners[i] = to_photo(at);
    }
    if response.drag_stopped() {
        *dragging = None;
    }
    let pts: Vec<egui::Pos2> = photo.corners.iter().map(|c| to_screen(*c)).collect();
    let line = egui::Stroke::new(2.0, Color32::from_rgb(255, 80, 40));
    for i in 0..4 {
        painter.line_segment([pts[i], pts[(i + 1) % 4]], line);
    }
    for (i, p) in pts.iter().enumerate() {
        painter.circle(*p, 7.0, Color32::from_rgba_unmultiplied(255, 80, 40, 90), line);
        painter.text(*p + egui::vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, ["TL", "TR", "BR", "BL"][i], egui::FontId::proportional(12.0), Color32::WHITE);
    }
}
