//! The Art tab: a shape library, borders, ready-made blanks (coasters, ornaments, tags…), product
//! templates to size a design on, and a browsable folder of your own art.

use crate::app::BurnCraftApp;
use bc_doc::ShapeKind;
use bc_geom::shapelib;
use bc_geom::{Affine, BezPath, Point};
use eframe::egui::{self, Stroke};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// File types the art folder shows.
const ART_EXTENSIONS: [&str; 15] = ["bcproj", "svg", "dxf", "pdf", "ai", "eps", "emf", "wmf", "lbrn2", "lbrn", "png", "jpg", "jpeg", "bmp", "psd"];
/// Most files listed from a folder (and its subfolders).
const MAX_FILES: usize = 1000;
const THUMB: f32 = 76.0;

/// A blank: a name and its shapes, centred on the origin.
pub struct Template {
    pub name: &'static str,
    pub shapes: fn() -> Vec<(ShapeKind, Affine)>,
}

fn rect(w: f64, h: f64, r: f64) -> ShapeKind {
    ShapeKind::Rect { width: w, height: h, corner_radius: r }
}

fn circle(r: f64) -> ShapeKind {
    ShapeKind::Ellipse { radius_x: r, radius_y: r }
}

fn at(x: f64, y: f64) -> Affine {
    Affine::translate((x, y))
}

pub const TEMPLATES: [Template; 9] = [
    Template { name: "Round coaster Ø90", shapes: || vec![(circle(45.0), Affine::IDENTITY)] },
    Template { name: "Square coaster 90", shapes: || vec![(rect(90.0, 90.0, 8.0), Affine::IDENTITY)] },
    Template { name: "Hexagon coaster 95", shapes: || vec![(ShapeKind::Polygon { sides: 6, radius: 47.5 }, Affine::rotate(std::f64::consts::FRAC_PI_6))] },
    Template { name: "Ornament Ø75", shapes: || vec![(circle(37.5), Affine::IDENTITY), (circle(2.0), at(0.0, -31.5))] },
    Template { name: "Keychain 50×25", shapes: || vec![(rect(50.0, 25.0, 5.0), Affine::IDENTITY), (circle(2.5), at(-19.0, 0.0))] },
    Template { name: "Dog tag 50×28", shapes: || vec![(rect(50.0, 28.0, 14.0), Affine::IDENTITY), (circle(2.0), at(-18.0, 0.0))] },
    Template { name: "Luggage tag 100×55", shapes: || vec![(rect(100.0, 55.0, 5.0), Affine::IDENTITY), (rect(4.0, 14.0, 2.0), at(-40.0, 0.0))] },
    Template { name: "Bookmark 50×150", shapes: || vec![(rect(50.0, 150.0, 5.0), Affine::IDENTITY), (circle(2.5), at(0.0, -65.0))] },
    Template { name: "Name plate 200×60", shapes: || vec![(rect(200.0, 60.0, 6.0), Affine::IDENTITY)] },
];

/// A product blank to size and place a design on: its outline (mm, top-left at the origin) and
/// the area where a design usually goes. Added on a tool layer, so it's never sent to the laser.
pub struct Product {
    pub name: &'static str,
    /// Size choices: (label, width, height) in mm.
    pub sizes: &'static [(&'static str, f64, f64)],
    pub build: fn(f64, f64) -> (BezPath, bc_geom::Rect),
}

fn outline(pts: &[(f64, f64)], w: f64, h: f64) -> BezPath {
    let v: Vec<Point> = pts.iter().map(|(x, y)| Point::new(x * w, y * h)).collect();
    bc_geom::polyline_path(&v, true)
}

/// A rectangle by its corners.
fn rect_at(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
    Affine::translate(((x0 + x1) / 2.0, (y0 + y1) / 2.0)) * bc_geom::rect_path(x1 - x0, y1 - y0, 0.0)
}

fn tshirt(w: f64, h: f64) -> (BezPath, bc_geom::Rect) {
    let body = outline(&[(0.36, 0.0), (0.42, 0.05), (0.5, 0.07), (0.58, 0.05), (0.64, 0.0), (0.86, 0.06), (1.0, 0.27), (0.85, 0.36), (0.8, 0.28), (0.8, 1.0), (0.2, 1.0), (0.2, 0.28), (0.15, 0.36), (0.0, 0.27), (0.14, 0.06)], w, h);
    (body, bc_geom::Rect::new(0.3 * w, 0.16 * h, 0.7 * w, 0.6 * h))
}

fn onesie(w: f64, h: f64) -> (BezPath, bc_geom::Rect) {
    let body = outline(&[(0.35, 0.0), (0.5, 0.06), (0.65, 0.0), (0.88, 0.08), (1.0, 0.3), (0.84, 0.36), (0.8, 0.3), (0.8, 0.8), (0.62, 1.0), (0.38, 1.0), (0.2, 0.8), (0.2, 0.3), (0.16, 0.36), (0.0, 0.3), (0.12, 0.08)], w, h);
    (body, bc_geom::Rect::new(0.3 * w, 0.18 * h, 0.7 * w, 0.62 * h))
}

fn mug(w: f64, h: f64) -> (BezPath, bc_geom::Rect) {
    // The wrap round the mug; the handle sits at the left/right ends.
    (rect_at(0.0, 0.0, w, h), bc_geom::Rect::new(0.08 * w, 0.1 * h, 0.92 * w, 0.9 * h))
}

fn tote(w: f64, h: f64) -> (BezPath, bc_geom::Rect) {
    let mut p = rect_at(0.0, 0.25 * h, w, h);
    for x in [0.22, 0.62] {
        p.extend(rect_at(x * w, 0.0, (x + 0.16) * w, 0.25 * h).elements().iter().copied());
    }
    (p, bc_geom::Rect::new(0.15 * w, 0.37 * h, 0.85 * w, 0.88 * h))
}

fn card(w: f64, h: f64) -> (BezPath, bc_geom::Rect) {
    // A folded card shown open flat: the right half is the front.
    let mut p = rect_at(0.0, 0.0, w, h);
    p.move_to((w / 2.0, 0.0));
    p.line_to((w / 2.0, h));
    (p, bc_geom::Rect::new(w / 2.0 + 0.08 * w, 0.08 * h, w - 0.08 * w, 0.92 * h))
}

fn pillow(w: f64, h: f64) -> (BezPath, bc_geom::Rect) {
    (Affine::translate((w / 2.0, h / 2.0)) * bc_geom::rect_path(w, h, 0.06 * w.min(h)), bc_geom::Rect::new(0.15 * w, 0.15 * h, 0.85 * w, 0.85 * h))
}

pub const PRODUCTS: [Product; 6] = [
    Product { name: "T-shirt", sizes: &[("Youth", 420.0, 560.0), ("Adult S", 460.0, 680.0), ("Adult M", 510.0, 710.0), ("Adult L", 560.0, 740.0), ("Adult XL", 610.0, 760.0)], build: tshirt },
    Product { name: "Baby bodysuit", sizes: &[("0–3 m", 220.0, 330.0), ("3–6 m", 240.0, 360.0), ("6–12 m", 260.0, 390.0)], build: onesie },
    Product { name: "Mug wrap", sizes: &[("11 oz", 216.0, 89.0), ("15 oz", 222.0, 102.0), ("Tumbler 20 oz", 240.0, 165.0)], build: mug },
    Product { name: "Tote bag", sizes: &[("Small", 330.0, 450.0), ("Standard", 380.0, 520.0)], build: tote },
    Product { name: "Greeting card (open)", sizes: &[("A2 4¼×5½ in", 216.0, 140.0), ("A7 5×7 in", 254.0, 178.0), ("A6", 210.0, 148.0)], build: card },
    Product { name: "Cushion", sizes: &[("40 cm", 400.0, 400.0), ("45 cm", 450.0, 450.0)], build: pillow },
];

/// Browsing state for the Art tab (the art folder itself is saved in the view preferences).
pub struct ArtBrowser {
    files: Vec<PathBuf>,
    scanned: Option<PathBuf>,
    filter: String,
    /// Outline previews, normalised to 0..1; `None` when a file has no outline (or failed).
    thumbs: HashMap<PathBuf, Option<Vec<Vec<Point>>>>,
    /// Longer side of a library shape when added (mm).
    shape_size: f64,
    /// Border length and height (mm).
    border_len: f64,
    border_h: f64,
    product: usize,
    product_size: usize,
}

impl Default for ArtBrowser {
    fn default() -> Self {
        Self { files: Vec::new(), scanned: None, filter: String::new(), thumbs: HashMap::new(), shape_size: 60.0, border_len: 200.0, border_h: 20.0, product: 0, product_size: 0 }
    }
}

type Thumb = Option<Vec<Vec<Point>>>;

/// Previews of the library shapes and of the borders, made once.
fn library_thumbs() -> &'static (Vec<Thumb>, Vec<Thumb>) {
    static THUMBS: std::sync::OnceLock<(Vec<Thumb>, Vec<Thumb>)> = std::sync::OnceLock::new();
    THUMBS.get_or_init(|| {
        let shapes = shapelib::shapes().iter().map(|s| normalise(std::slice::from_ref(&s.unit))).collect();
        let borders = shapelib::BORDERS.iter().map(|b| normalise(std::slice::from_ref(shapelib::border_preview(b)))).collect();
        (shapes, borders)
    })
}

impl ArtBrowser {
    fn rescan(&mut self, folder: &Path) {
        self.files.clear();
        self.thumbs.clear();
        let mut stack = vec![folder.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| ART_EXTENSIONS.contains(&x.to_string_lossy().to_lowercase().as_str())) {
                    self.files.push(p);
                }
                if self.files.len() >= MAX_FILES {
                    break;
                }
            }
        }
        self.files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
        self.scanned = Some(folder.to_path_buf());
    }
}

/// Outlines in a 0..1 box (keeping proportions), for previews.
fn normalise(paths: &[BezPath]) -> Option<Vec<Vec<Point>>> {
    let lines: Vec<Vec<Point>> = paths.iter().flat_map(|p| bc_geom::flatten(p, 0.5)).map(|l| {
        let mut pts = l.points;
        if l.closed && let Some(&f) = pts.first() {
            pts.push(f);
        }
        pts
    }).filter(|l| l.len() >= 2).collect();
    let all: Vec<Point> = lines.iter().flatten().copied().collect();
    let (x0, x1) = all.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
    let (y0, y1) = all.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
    let size = (x1 - x0).max(y1 - y0);
    if !(size.is_finite() && size > 0.0) {
        return None;
    }
    let (ox, oy) = ((size - (x1 - x0)) / 2.0, (size - (y1 - y0)) / 2.0);
    Some(lines.into_iter().map(|l| l.into_iter().map(|p| Point::new((p.x - x0 + ox) / size, (p.y - y0 + oy) / size)).collect()).collect())
}

/// Preview outlines for an art file.
fn file_thumb(path: &Path, fonts: &bc_io::fonts::FontLibrary) -> Option<Vec<Vec<Point>>> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let bytes = std::fs::read(path).ok()?;
    let paths: Vec<BezPath> = match ext.as_str() {
        "svg" => bc_io::svg::import(&bytes, fonts.database()).ok()?.shapes.into_iter().map(|s| s.path).collect(),
        "dxf" => bc_io::dxf_import::import(&bytes).ok()?.shapes.into_iter().map(|s| s.path).collect(),
        "pdf" | "ai" => bc_io::pdf_import::import(&bytes).or_else(|_| bc_io::eps::import(&bytes)).ok()?.shapes.into_iter().map(|s| s.path).collect(),
        "eps" => bc_io::eps::import(&bytes).ok()?.shapes.into_iter().map(|s| s.path).collect(),
        "emf" | "wmf" => bc_io::metafile::import(&bytes).ok()?.shapes.into_iter().map(|s| s.path).collect(),
        "bcproj" => bc_doc::Document::from_json(std::str::from_utf8(&bytes).ok()?)
            .ok()?
            .shapes
            .iter()
            .filter(|s| !matches!(s.kind, ShapeKind::Image { .. }))
            .map(|s| s.world_path())
            .collect(),
        "lbrn2" | "lbrn" => bc_io::lightburn::import(&bytes, 1000.0, fonts)
            .ok()?
            .shapes
            .into_iter()
            .filter(|s| !matches!(s.kind, ShapeKind::Image { .. }))
            .map(|s| bc_doc::Shape { id: 0, kind: s.kind, transform: s.transform, layer: 0, locked: false, group: None, effects: Vec::new() }.world_path())
            .collect(),
        _ => return None,
    };
    normalise(&paths)
}

/// A clickable preview tile.
fn tile(ui: &mut egui::Ui, label: &str, thumb: Option<&Vec<Vec<Point>>>, placeholder: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(THUMB, THUMB + 16.0), egui::Sense::click());
    let visuals = ui.visuals();
    let bg = if response.hovered() { visuals.widgets.hovered.weak_bg_fill } else { visuals.extreme_bg_color };
    let image = egui::Rect::from_min_size(rect.min, egui::vec2(THUMB, THUMB));
    ui.painter().rect_filled(image, 4.0, bg);
    let ink = visuals.text_color();
    match thumb {
        Some(lines) => {
            let inner = image.shrink(8.0);
            for l in lines {
                let pts: Vec<egui::Pos2> = l.iter().map(|p| inner.min + egui::vec2(p.x as f32, p.y as f32) * inner.width()).collect();
                ui.painter().add(egui::Shape::line(pts, Stroke::new(1.2, ink)));
            }
        }
        None => {
            ui.painter().text(image.center(), egui::Align2::CENTER_CENTER, placeholder, egui::FontId::proportional(24.0), ink.gamma_multiply(0.6));
        }
    }
    let name: String = if label.chars().count() > 13 { label.chars().take(12).chain(['…']).collect() } else { label.to_string() };
    ui.painter().text(egui::pos2(rect.center().x, rect.bottom() - 7.0), egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(11.0), ink);
    response.on_hover_text(label)
}

impl BurnCraftApp {
    /// The Art tab.
    pub(crate) fn art_panel(&mut self, ui: &mut egui::Ui) {
        let units = self.prefs.units;
        egui::CollapsingHeader::new("Shapes").default_open(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Size");
                units.drag(ui, &mut self.art.shape_size, 1.0..=2000.0);
            })
            .response
            .on_hover_text("The longer side of the shape you add");
            let thumbs = &library_thumbs().0;
            let mut picked = None;
            for cat in shapelib::CATEGORIES {
                egui::CollapsingHeader::new(cat).id_salt(("shape-cat", cat)).default_open(cat == "Square").show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (i, s) in shapelib::shapes().iter().enumerate().filter(|(_, s)| s.category == cat) {
                            if tile(ui, s.name, thumbs[i].as_ref(), "?").clicked() {
                                picked = Some(i);
                            }
                        }
                    });
                });
            }
            if let Some(i) = picked {
                let s = &shapelib::shapes()[i];
                let name = if s.name == s.category || s.category == "Others" { s.name.to_lowercase() } else { format!("{} {}", s.name, s.category).to_lowercase() };
                self.add_art(s.path(self.art.shape_size.max(1.0)), &name);
            }
        });
        egui::CollapsingHeader::new("Borders").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Length");
                units.drag(ui, &mut self.art.border_len, 5.0..=3000.0);
                ui.label("Height");
                units.drag(ui, &mut self.art.border_h, 2.0..=500.0);
            });
            ui.small("Strips for edges, frames and shelf trims: the pattern repeats to fill the length.");
            let thumbs = &library_thumbs().1;
            let mut picked = None;
            ui.horizontal_wrapped(|ui| {
                for (i, b) in shapelib::BORDERS.iter().enumerate() {
                    if tile(ui, b.name, thumbs[i].as_ref(), "?").clicked() {
                        picked = Some(i);
                    }
                }
            });
            if let Some(i) = picked {
                let (l, h) = (self.art.border_len.max(5.0), self.art.border_h.max(2.0));
                let b = &shapelib::BORDERS[i];
                self.add_art(Affine::translate((-l / 2.0, -h / 2.0)) * (b.build)(l, h), &format!("{} border", b.name.to_lowercase()));
            }
        });
        egui::CollapsingHeader::new("Templates").show(ui, |ui| {
            ui.small("Ready-made blanks at true size, added at the centre of the bed.");
            ui.horizontal_wrapped(|ui| {
                for (i, t) in TEMPLATES.iter().enumerate() {
                    let shapes = (t.shapes)();
                    let paths: Vec<BezPath> = shapes
                        .iter()
                        .map(|(kind, tr)| bc_doc::Shape { id: 0, kind: kind.clone(), transform: *tr, layer: 0, locked: false, group: None, effects: Vec::new() }.world_path())
                        .collect();
                    if tile(ui, t.name, normalise(&paths).as_ref(), "?").on_hover_text("Click to add it at the centre of the bed").clicked() {
                        self.add_template(i);
                    }
                }
            });
        });
        egui::CollapsingHeader::new("Product templates").show(ui, |ui| {
            ui.small("A product's outline to size your design on, with its usual design area. It goes on tool layer Tool1, so it's never sent to the laser.");
            let prod = self.art.product.min(PRODUCTS.len() - 1);
            egui::ComboBox::from_label("Product").selected_text(PRODUCTS[prod].name).show_ui(ui, |ui| {
                for (i, p) in PRODUCTS.iter().enumerate() {
                    if ui.selectable_value(&mut self.art.product, i, p.name).clicked() {
                        self.art.product_size = 0;
                    }
                }
            });
            let prod = self.art.product.min(PRODUCTS.len() - 1);
            let sizes = PRODUCTS[prod].sizes;
            let si = self.art.product_size.min(sizes.len() - 1);
            egui::ComboBox::from_label("Size").selected_text(format!("{}  ({:.0} × {:.0} mm)", sizes[si].0, sizes[si].1, sizes[si].2)).show_ui(ui, |ui| {
                for (i, (n, w, h)) in sizes.iter().enumerate() {
                    ui.selectable_value(&mut self.art.product_size, i, format!("{n}  ({w:.0} × {h:.0} mm)"));
                }
            });
            if ui.button("Add template").clicked() {
                let (label, w, h) = sizes[si];
                self.add_product(PRODUCTS[prod].name, label, (PRODUCTS[prod].build)(w, h));
            }
        });
        egui::CollapsingHeader::new("Sign / plaque").show(ui, |ui| {
            ui.small("Sign blanks with mounting holes, and edge-lit signs.");
            if ui.button("Open the sign maker…").clicked() {
                self.run(crate::commands::Command::BoxGenerator);
                if let Some(crate::app::Dialog::Box(d)) = &mut self.dialog {
                    d.tab = crate::dialogs::MakerTab::Sign;
                }
            }
        });
        egui::CollapsingHeader::new("Monogram").show(ui, |ui| {
            ui.small("Three initials, the surname's large in the middle, with an optional frame.");
            if ui.button("Make a monogram…").clicked() {
                self.run(crate::commands::Command::Monogram);
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("My art folder");
            if ui.button("Choose…").clicked()
                && let Some(folder) = rfd::FileDialog::new().pick_folder()
            {
                self.prefs.art_folder = Some(folder);
            }
            if self.prefs.art_folder.is_some() && ui.small_button("⟳").on_hover_text("Look for new files").clicked() {
                self.art.scanned = None;
            }
        });
        let Some(folder) = self.prefs.art_folder.clone() else {
            ui.weak("Choose a folder of BurnCraft, SVG, DXF, LightBurn or image files to browse and drop them into your design.");
            return;
        };
        if self.art.scanned.as_ref() != Some(&folder) {
            self.art.rescan(&folder);
        }
        ui.weak(folder.display().to_string());
        ui.add(egui::TextEdit::singleline(&mut self.art.filter).hint_text("Search").desired_width(ui.available_width()));
        let filter = self.art.filter.to_lowercase();
        let shown: Vec<PathBuf> = self.art.files.iter().filter(|p| p.to_string_lossy().to_lowercase().contains(&filter)).take(300).cloned().collect();
        // Build a few previews per frame so big folders don't stall the window.
        let mut budget = 4;
        for p in &shown {
            if budget == 0 {
                ui.ctx().request_repaint();
                break;
            }
            if !self.art.thumbs.contains_key(p) {
                let thumb = file_thumb(p, &self.fonts);
                self.art.thumbs.insert(p.clone(), thumb);
                budget -= 1;
            }
        }
        if shown.is_empty() {
            ui.weak("No art files found here.");
        }
        let mut clicked = None;
        ui.horizontal_wrapped(|ui| {
            for p in &shown {
                let name = p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let is_image = p.extension().is_some_and(|x| ["png", "jpg", "jpeg", "bmp", "psd"].contains(&x.to_string_lossy().to_lowercase().as_str()));
                let thumb = self.art.thumbs.get(p).and_then(|t| t.as_ref());
                if tile(ui, &name, thumb, if is_image { "🖼" } else { "…" }).clicked() {
                    clicked = Some(p.clone());
                }
            }
        });
        if let Some(p) = clicked
            && let Err(e) = self.import_file(&p)
        {
            self.error(format!("Could not import {}: {e}", p.display()));
        }
    }

    /// Adds a library shape or border (centred on the origin) at the centre of the bed on the
    /// current layer, selected.
    fn add_art(&mut self, path: BezPath, name: &str) {
        let (w, h) = self.doc.bed_size;
        self.checkpoint_named(&format!("Add {name}"));
        let id = self.doc.add_shape(ShapeKind::Path(path), Affine::translate((w / 2.0, h / 2.0)), self.current_layer);
        self.selection = [id].into();
        self.status = format!("Added {name}");
    }

    /// Adds a product template (outline and design area) centred on the bed, on tool layer Tool1.
    fn add_product(&mut self, name: &str, size: &str, (body, area): (BezPath, bc_geom::Rect)) {
        let (bw, bh) = self.doc.bed_size;
        let b = bc_geom::bounds(&body).unwrap_or(area);
        let at = Affine::translate((bw / 2.0 - b.center().x, bh / 2.0 - b.center().y));
        let layer = bc_doc::CUT_LAYER_COUNT; // Tool1: shown, never sent to the laser
        self.checkpoint_named(&format!("Add {name} template"));
        let ids: std::collections::BTreeSet<u64> = [body, rect_at(area.x0, area.y0, area.x1, area.y1)].into_iter().map(|p| self.doc.add_shape(ShapeKind::Path(p), at, layer)).collect();
        self.doc.group(&ids);
        self.selection = ids;
        self.status = format!("Added a {name} ({size}) template on tool layer Tool1: the inner box is the usual design area");
    }

    /// Adds template `i` at the centre of the bed on the current layer, grouped and selected.
    pub(crate) fn add_template(&mut self, i: usize) {
        let t = &TEMPLATES[i];
        let (w, h) = self.doc.bed_size;
        let centre = Affine::translate((w / 2.0, h / 2.0));
        self.checkpoint_named(&format!("Add {}", t.name));
        let ids: std::collections::BTreeSet<u64> = (t.shapes)().into_iter().map(|(kind, tr)| self.doc.add_shape(kind, centre * tr, self.current_layer)).collect();
        self.doc.group(&ids);
        self.selection = ids;
        self.status = format!("Added {}", t.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_sensible_blanks() {
        for t in &TEMPLATES {
            let shapes = (t.shapes)();
            assert!(!shapes.is_empty(), "{}", t.name);
            let b = shapes
                .iter()
                .map(|(k, tr)| bc_doc::Shape { id: 0, kind: k.clone(), transform: *tr, layer: 0, locked: false, group: None, effects: Vec::new() })
                .filter_map(|s| { assert!(s.is_closed(), "{} has an open shape", t.name); s.bounds() })
                .reduce(|a, b| a.union(b))
                .unwrap();
            assert!(b.center().distance(Point::ORIGIN) < 1.0, "{} is centred: {b:?}", t.name);
            assert!((20.0..=250.0).contains(&b.width().max(b.height())), "{}: {b:?}", t.name);
        }
    }

    #[test]
    fn product_templates_have_their_design_area_inside() {
        for p in &PRODUCTS {
            for &(label, w, h) in p.sizes {
                let (body, area) = (p.build)(w, h);
                let b = bc_geom::bounds(&body).unwrap();
                assert!((b.width() - w).abs() < 0.5 && (b.height() - h).abs() < 0.5, "{} {label}: {b:?}", p.name);
                assert!(b.contains(area.origin()) && b.contains(Point::new(area.x1, area.y1)), "{} {label}", p.name);
            }
        }
    }

    #[test]
    fn previews_fit_a_unit_box() {
        let mut p = BezPath::new();
        p.move_to((10.0, 10.0));
        p.line_to((110.0, 10.0));
        p.line_to((110.0, 60.0));
        let lines = normalise(&[p]).unwrap();
        let pts: Vec<Point> = lines.into_iter().flatten().collect();
        assert!(pts.iter().all(|q| (0.0..=1.0).contains(&q.x) && (0.0..=1.0).contains(&q.y)));
        assert!(pts.iter().any(|q| q.x.abs() < 1e-9) && pts.iter().any(|q| (q.x - 1.0).abs() < 1e-9), "fills the width");
        assert!(normalise(&[]).is_none());
    }
}