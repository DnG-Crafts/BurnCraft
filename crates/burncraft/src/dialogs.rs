//! Modal-style tool windows: text, arrays, offset, material test and job preview.

use std::collections::BTreeSet;

use bc_doc::{Layer, LayerMode};
use bc_geom::Point;
use bc_io::fonts::FontLibrary;
use bc_job::{Job, JobLayer, JobPath};
use eframe::egui::{self, Color32, Stroke};

/// How a dialog was closed this frame.
pub enum Outcome {
    Ok,
    Cancel,
}

pub(crate) fn ok_cancel(ui: &mut egui::Ui, ok_label: &str, ok_enabled: bool) -> Option<Outcome> {
    let mut out = None;
    ui.separator();
    ui.horizontal(|ui| {
        if ui.add_enabled(ok_enabled, egui::Button::new(ok_label)).clicked() {
            out = Some(Outcome::Ok);
        }
        if ui.button("Cancel").clicked() {
            out = Some(Outcome::Cancel);
        }
    });
    out
}

/// A window that first opens centred on the app. Once moved, it stays where it was put (egui
/// remembers each window's position, also between runs).
pub(crate) fn centered<'a>(ctx: &egui::Context, title: impl Into<egui::WidgetText>) -> egui::Window<'a> {
    let title = title.into();
    // Its own id: positions remembered from before windows opened centred are left behind.
    // A window whose title changes while open (it shows a name being edited) must set a fixed
    // id with `.id(…)` instead, or it's rebuilt at each change.
    let id = egui::Id::new(("centred-window", title.text().to_owned()));
    egui::Window::new(title).id(id).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center())
}

/// A small tool dialog: centred at first, fixed size.
pub(crate) fn window<'a>(ctx: &egui::Context, title: &str) -> egui::Window<'a> {
    centered(ctx, title).collapsible(false).resizable(false)
}

// --- Text -------------------------------------------------------------------

pub struct TextDialog {
    /// Shape being edited, or `None` for new text.
    pub target: Option<u64>,
    /// Where new text is centred (mm).
    pub at: Point,
    pub text: String,
    pub font: String,
    pub height: f64,
    pub bold: bool,
    pub italic: bool,
    pub style: bc_doc::TextStyle,
    filter: String,
    /// Pair kerning: the letter after which the space is being adjusted.
    kern_at: usize,
    /// The character grid: the font it was made for, its characters, and their drawn outlines
    /// (as lines within a 0..1 square), made as they scroll into view.
    glyphs: Option<(String, Vec<char>)>,
    glyph_lines: std::collections::HashMap<char, Vec<Vec<egui::Pos2>>>,
}

impl TextDialog {
    #[allow(clippy::too_many_arguments)]
    pub fn new(target: Option<u64>, at: Point, text: String, font: String, height: f64, bold: bool, italic: bool, style: bc_doc::TextStyle) -> Self {
        Self { target, at, text, font, height, bold, italic, style, filter: String::new(), kern_at: 0, glyphs: None, glyph_lines: Default::default() }
    }

    /// Pair kerning: pick two neighbouring letters and widen or tighten the space between them.
    fn kerning_ui(&mut self, ui: &mut egui::Ui) {
        let chars: Vec<char> = self.text.chars().collect();
        // Pairs of letters on the same line.
        let pairs: Vec<usize> = (0..chars.len().saturating_sub(1)).filter(|&i| chars[i] != '\n' && chars[i + 1] != '\n').collect();
        if pairs.is_empty() {
            return;
        }
        if !pairs.contains(&self.kern_at) {
            self.kern_at = pairs[0];
        }
        let label = |i: usize| format!("{}{}", chars[i], chars[i + 1]);
        ui.horizontal(|ui| {
            ui.label("Space between").on_hover_text("Pair kerning: adjust the gap between two particular letters");
            egui::ComboBox::from_id_salt("kern-pair").selected_text(label(self.kern_at)).width(60.0).height(240.0).show_ui(ui, |ui| {
                for &i in &pairs {
                    let adjusted = self.style.kerning.iter().any(|(k, d)| *k == i && *d != 0.0);
                    let text = if adjusted { format!("{} •", label(i)) } else { label(i) };
                    ui.selectable_value(&mut self.kern_at, i, text);
                }
            });
            let at = self.kern_at;
            let mut gap = self.style.kerning.iter().find(|(k, _)| *k == at).map_or(0.0, |(_, d)| *d);
            if ui.add(egui::DragValue::new(&mut gap).range(-50.0..=50.0).speed(0.05).suffix(" mm")).changed() {
                self.style.kerning.retain(|(k, _)| *k != at);
                if gap != 0.0 {
                    self.style.kerning.push((at, gap));
                }
            }
            if !self.style.kerning.is_empty() && ui.small_button("Reset all").on_hover_text("Remove every pair adjustment").clicked() {
                self.style.kerning.clear();
            }
        });
    }

    /// A grid of every character the font has (swashes, symbols, accents…): click one to add it.
    fn characters_ui(&mut self, ui: &mut egui::Ui, fonts: &FontLibrary) {
        egui::CollapsingHeader::new("Insert a character").id_salt("glyph-grid").show(ui, |ui| {
            if self.glyphs.as_ref().is_none_or(|(f, _)| *f != self.font) {
                self.glyphs = Some((self.font.clone(), fonts.characters(&self.font)));
                self.glyph_lines.clear();
            }
            let Some((_, chars)) = &self.glyphs else { return };
            let chars = chars.clone();
            ui.weak(format!("{} characters. Click one to add it to the end of the text.", chars.len()));
            const COLS: usize = 12;
            const CELL: f32 = 30.0;
            let rows = chars.len().div_ceil(COLS);
            egui::ScrollArea::vertical().max_height(200.0).auto_shrink([false, true]).show_rows(ui, CELL + 2.0, rows, |ui, range| {
                for row in range {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for &c in chars.iter().skip(row * COLS).take(COLS) {
                            let (rect, r) = ui.allocate_exact_size(egui::vec2(CELL, CELL), egui::Sense::click());
                            let visuals = ui.style().interact(&r);
                            ui.painter().rect_filled(rect, 3.0, if r.hovered() { visuals.bg_fill } else { ui.visuals().extreme_bg_color });
                            let lines = self.glyph_lines.entry(c).or_insert_with(|| glyph_lines(fonts, &self.font, c));
                            let ink = ui.visuals().strong_text_color();
                            for line in lines.iter() {
                                let pts: Vec<egui::Pos2> = line.iter().map(|p| rect.min + egui::vec2(p.x, p.y) * (CELL - 8.0) + egui::vec2(4.0, 4.0)).collect();
                                ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.0, ink)));
                            }
                            if r.on_hover_text(format!("{c}  U+{:04X}", c as u32)).clicked() {
                                self.text.push(c);
                            }
                        }
                    });
                }
            });
        });
    }

    pub fn ui(&mut self, ctx: &egui::Context, fonts: &FontLibrary) -> Option<Outcome> {
        let mut out = None;
        window(ctx, if self.target.is_some() { "Edit text" } else { "New text" }).show(ctx, |ui| {
            ui.add(egui::TextEdit::multiline(&mut self.text).desired_rows(3).desired_width(360.0).hint_text("Type your text"));
            ui.horizontal(|ui| {
                ui.label("Font");
                egui::ComboBox::from_id_salt("font").selected_text(&self.font).width(220.0).height(320.0).show_ui(ui, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Search fonts"));
                    let f = self.filter.to_lowercase();
                    for family in fonts.families().iter().filter(|n| n.to_lowercase().contains(&f)) {
                        ui.selectable_value(&mut self.font, family.clone(), family);
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut self.height).range(0.5..=500.0).speed(0.2).suffix(" mm"));
                ui.checkbox(&mut self.bold, "Bold");
                ui.checkbox(&mut self.italic, "Italic");
            });
            ui.horizontal(|ui| {
                use bc_doc::TextAlign;
                ui.label("Align");
                for (a, l) in [(TextAlign::Left, "Left"), (TextAlign::Center, "Centre"), (TextAlign::Right, "Right")] {
                    ui.radio_value(&mut self.style.align, a, l);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Letter spacing");
                ui.add(egui::DragValue::new(&mut self.style.letter_spacing).range(-10.0..=50.0).speed(0.05).suffix(" mm"));
                ui.label("Line spacing");
                ui.add(egui::DragValue::new(&mut self.style.line_spacing).range(0.3..=5.0).speed(0.01).suffix("×"));
            });
            self.kerning_ui(ui);
            self.curve_ui(ui);
            self.characters_ui(ui, fonts);
            out = ok_cancel(ui, "OK", !self.text.trim().is_empty());
        });
        out
    }
}

/// A character drawn from the font, as lines in a 0..1 square (for the character grid).
fn glyph_lines(fonts: &FontLibrary, font: &str, c: char) -> Vec<Vec<egui::Pos2>> {
    let Some(outline) = fonts.outline(&c.to_string(), font, 10.0, false, false, &bc_doc::TextStyle::default()) else { return Vec::new() };
    let Some(b) = bc_geom::bounds(&outline) else { return Vec::new() };
    let size = b.width().max(b.height()).max(1e-6);
    let (ox, oy) = ((size - b.width()) / 2.0, (size - b.height()) / 2.0);
    bc_geom::flatten(&outline, size / 60.0)
        .into_iter()
        .map(|l| {
            let mut pts: Vec<egui::Pos2> = l.points.iter().map(|p| egui::pos2(((p.x - b.x0 + ox) / size) as f32, ((p.y - b.y0 + oy) / size) as f32)).collect();
            if l.closed && let Some(&first) = pts.first() {
                pts.push(first);
            }
            pts
        })
        .collect()
}

impl TextDialog {
    /// Straight, around a circle, or along a path (set with Tools > Put Text on Path).
    fn curve_ui(&mut self, ui: &mut egui::Ui) {
        use bc_doc::TextCurve;
        #[derive(PartialEq, Clone, Copy)]
        enum Kind {
            Straight,
            Top,
            Bottom,
            Path,
        }
        let kind = match &self.style.curve {
            None => Kind::Straight,
            Some(TextCurve::Circle { bottom: false, .. }) => Kind::Top,
            Some(TextCurve::Circle { bottom: true, .. }) => Kind::Bottom,
            Some(TextCurve::Path { .. }) => Kind::Path,
        };
        let mut chosen = kind;
        ui.horizontal(|ui| {
            ui.label("Shape");
            ui.selectable_value(&mut chosen, Kind::Straight, "Straight");
            ui.selectable_value(&mut chosen, Kind::Top, "◠ Arc over").on_hover_text("Around the top of a circle, reading clockwise");
            ui.selectable_value(&mut chosen, Kind::Bottom, "◡ Arc under").on_hover_text("Along the bottom of a circle, letters upright");
            if kind == Kind::Path {
                ui.selectable_value(&mut chosen, Kind::Path, "Along path");
            }
        });
        if chosen != kind {
            let radius = match &self.style.curve {
                Some(TextCurve::Circle { radius, .. }) => *radius,
                _ => (self.height * 4.0).max(20.0),
            };
            self.style.curve = match chosen {
                Kind::Straight => None,
                Kind::Top => Some(TextCurve::Circle { radius, bottom: false }),
                Kind::Bottom => Some(TextCurve::Circle { radius, bottom: true }),
                Kind::Path => self.style.curve.take(),
            };
        }
        match &mut self.style.curve {
            Some(TextCurve::Circle { radius, .. }) => {
                ui.horizontal(|ui| {
                    ui.label("Circle radius");
                    ui.add(egui::DragValue::new(radius).range(1.0..=2000.0).speed(0.2).suffix(" mm"));
                });
            }
            Some(TextCurve::Path { offset, flip, .. }) => {
                ui.horizontal(|ui| {
                    ui.label("Position along path");
                    ui.add(egui::DragValue::new(offset).range(-10_000.0..=10_000.0).speed(0.5).suffix(" mm"));
                    ui.checkbox(flip, "Flip side");
                });
            }
            None => {}
        }
    }
}

// --- Trace image ------------------------------------------------------------

pub struct TraceDialog {
    pub target: u64,
    pub settings: bc_job::TraceSettings,
    pub delete_image: bool,
    /// Traced lines for the current settings.
    pub result: Option<bc_geom::BezPath>,
    computed_for: Option<bc_job::TraceSettings>,
    message: String,
}

impl TraceDialog {
    pub fn new(target: u64) -> Self {
        Self { target, settings: bc_job::TraceSettings::default(), delete_image: false, result: None, computed_for: None, message: String::new() }
    }

    pub fn ui(&mut self, ctx: &egui::Context, shape: &bc_doc::Shape) -> Option<Outcome> {
        use bc_job::TraceKind;
        if self.computed_for != Some(self.settings) {
            self.computed_for = Some(self.settings);
            match bc_job::trace_image_with(shape, &self.settings) {
                Ok(p) => {
                    let n = bc_geom::flatten(&p, 0.05).len();
                    self.message = match self.settings.kind {
                        TraceKind::Outline => format!("{n} outline{}", if n == 1 { "" } else { "s" }),
                        TraceKind::CentreLine => format!("{n} stroke{}", if n == 1 { "" } else { "s" }),
                    };
                    self.result = Some(p).filter(|p| !p.elements().is_empty());
                }
                Err(e) => {
                    self.message = e;
                    self.result = None;
                }
            }
        }
        let mut out = None;
        let s = &mut self.settings;
        window(ctx, "Trace image").show(ctx, |ui| {
            egui::Grid::new("trace").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("Make");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut s.kind, TraceKind::Outline, "Outlines").on_hover_text("Closed outlines round the dark areas, to cut or fill");
                    ui.radio_value(&mut s.kind, TraceKind::CentreLine, "Centre lines").on_hover_text("One stroke along the middle of each line: line art engraved once instead of outlined");
                });
                ui.end_row();
                ui.label("Trace");
                ui.horizontal(|ui| {
                    if ui.radio(!s.subject, "Dark areas").clicked() && s.subject {
                        s.subject = false;
                        s.threshold = 0.5;
                    }
                    if ui.radio(s.subject, "Object on a plain background").on_hover_text("For a photo of a real thing: the background is found from the picture's edges and left out").clicked() && !s.subject {
                        s.subject = true;
                        s.threshold = 0.15;
                    }
                });
                ui.end_row();
                if s.subject {
                    ui.label("Background tolerance").on_hover_text("How different from the background the object must be. Raise it if shadows or texture are traced as part of the object.");
                    ui.add(egui::Slider::new(&mut s.threshold, 0.02..=0.6));
                } else {
                    ui.label("Threshold").on_hover_text("Pixels darker than this are traced");
                    ui.horizontal(|ui| {
                        ui.add(egui::Slider::new(&mut s.threshold, 0.05..=0.95));
                        if ui.button("Auto").on_hover_text("The level that best splits this picture into dark and light").clicked()
                            && let Ok(t) = bc_job::auto_threshold(shape)
                        {
                            s.threshold = t;
                        }
                    });
                }
                ui.end_row();
                ui.label("Curves");
                ui.checkbox(&mut s.smooth, "Smooth curves").on_hover_text("Smooth curves with sharp corners kept. Off: straight lines along the pixels (for pixel art).");
                ui.end_row();
                ui.label("Accuracy");
                ui.add(egui::Slider::new(&mut s.tolerance, 0.01..=2.0).logarithmic(true).suffix(" mm")).on_hover_text("How far the lines may stray from the picture: larger is smoother, with fewer points");
                ui.end_row();
                ui.label("Ignore smaller than");
                ui.add(egui::Slider::new(&mut s.min_area, 0.0..=50.0).suffix(" mm²"));
                ui.end_row();
                if s.kind == TraceKind::Outline {
                    ui.label("");
                    ui.checkbox(&mut s.outer_only, "Outside edges only").on_hover_text("Leave out holes: just the outer shape, e.g. for a cut line round a picture");
                    ui.end_row();
                }
            });
            ui.checkbox(&mut self.delete_image, "Delete the image afterwards");
            ui.label(&self.message);
            out = ok_cancel(ui, "Trace", self.result.is_some());
        });
        out
    }
}

// --- Barcode ----------------------------------------------------------------

pub struct BarcodeDialog {
    pub target: Option<u64>,
    pub at: Point,
    pub symbology: bc_doc::Symbology,
    pub content: String,
    pub size: f64,
}

impl BarcodeDialog {
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        let mut out = None;
        window(ctx, if self.target.is_some() { "Edit barcode" } else { "New barcode" }).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for s in bc_doc::Symbology::ALL {
                    ui.radio_value(&mut self.symbology, s, s.label());
                }
            });
            ui.add(egui::TextEdit::singleline(&mut self.content).desired_width(320.0).hint_text("Content, e.g. a URL or part number"));
            ui.weak("Variable text fields such as {serial} work here too.");
            ui.horizontal(|ui| {
                ui.label(if self.symbology.is_qr() { "Size" } else { "Bar height" });
                ui.add(egui::DragValue::new(&mut self.size).range(2.0..=300.0).speed(0.2).suffix(" mm"));
            });
            if self.symbology == bc_doc::Symbology::QrLogo
                && let Some(f) = bc_io::barcode::logo_fraction(&self.content)
            {
                ui.weak(format!("Logo space: {:.1} mm square in the middle. Put your logo there, a little smaller than the space.", f * self.size));
            }
            let preview = bc_io::barcode::outline(self.symbology, &self.content, self.size);
            if let Err(e) = &preview
                && !self.content.is_empty() {
                    ui.colored_label(Color32::from_rgb(200, 60, 40), e);
                }
            out = ok_cancel(ui, "OK", preview.is_ok());
        });
        out
    }
}

pub struct NestDialog {
    pub spacing: f64,
    pub margin: f64,
    pub allow_rotate: bool,
    /// Pack by real outlines (parts tuck into each other and into holes) instead of bounding boxes.
    pub true_shape: bool,
    /// Rotations tried by true-shape nesting, in degrees (0 = none).
    pub rotation_step: f64,
    /// A finer grid: tighter packing, slower.
    pub fine: bool,
}

impl NestDialog {
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        let mut out = None;
        window(ctx, "Nest selected shapes").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.true_shape, true, "True shape").on_hover_text("Packs by the real outlines: parts tuck into each other's curves and notches, and small parts go inside holes");
                ui.radio_value(&mut self.true_shape, false, "Bounding box").on_hover_text("Rows of rectangles: fastest, but wastes material around odd shapes");
            });
            ui.weak(if self.true_shape { "Largest parts first, each as high and far left as it fits. Groups stay together." } else { "Packs the selection into the work area in rows, largest first (by bounding box)." });
            egui::Grid::new("nest").num_columns(2).show(ui, |ui| {
                ui.label("Spacing");
                ui.add(egui::DragValue::new(&mut self.spacing).range(0.0..=50.0).speed(0.1).suffix(" mm"));
                ui.end_row();
                ui.label("Edge margin");
                ui.add(egui::DragValue::new(&mut self.margin).range(0.0..=100.0).speed(0.5).suffix(" mm"));
                ui.end_row();
                if self.true_shape {
                    ui.label("Rotation");
                    ui.horizontal(|ui| {
                        for (step, label) in [(0.0, "None"), (90.0, "90°"), (45.0, "45°"), (15.0, "15°")] {
                            ui.selectable_value(&mut self.rotation_step, step, label);
                        }
                    })
                    .response
                    .on_hover_text("Smaller steps find tighter fits but take longer");
                    ui.end_row();
                    ui.label("Precision");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.fine, false, "Normal");
                        ui.radio_value(&mut self.fine, true, "Fine (slower)");
                    });
                    ui.end_row();
                }
            });
            if !self.true_shape {
                ui.checkbox(&mut self.allow_rotate, "Allow 90° rotation");
            }
            out = ok_cancel(ui, "Nest", true);
        });
        out
    }
}

// --- Arrays and offset ------------------------------------------------------

pub struct GridArrayDialog {
    pub columns: usize,
    pub rows: usize,
    pub gap_x: f64,
    pub gap_y: f64,
}

impl GridArrayDialog {
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        let mut out = None;
        window(ctx, "Grid array").show(ctx, |ui| {
            egui::Grid::new("grid-array").num_columns(2).show(ui, |ui| {
                ui.label("Columns");
                ui.add(egui::DragValue::new(&mut self.columns).range(1..=100));
                ui.end_row();
                ui.label("Rows");
                ui.add(egui::DragValue::new(&mut self.rows).range(1..=100));
                ui.end_row();
                ui.label("Horizontal gap");
                ui.add(egui::DragValue::new(&mut self.gap_x).speed(0.1).suffix(" mm"));
                ui.end_row();
                ui.label("Vertical gap");
                ui.add(egui::DragValue::new(&mut self.gap_y).speed(0.1).suffix(" mm"));
                ui.end_row();
            });
            out = ok_cancel(ui, "Create", self.columns * self.rows > 1);
        });
        out
    }
}

pub struct CircularArrayDialog {
    pub count: usize,
    pub center_x: f64,
    pub center_y: f64,
    pub rotate: bool,
}

impl CircularArrayDialog {
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        let mut out = None;
        window(ctx, "Circular array").show(ctx, |ui| {
            egui::Grid::new("circ-array").num_columns(2).show(ui, |ui| {
                ui.label("Copies (total)");
                ui.add(egui::DragValue::new(&mut self.count).range(2..=360));
                ui.end_row();
                ui.label("Centre X");
                ui.add(egui::DragValue::new(&mut self.center_x).speed(0.5).suffix(" mm"));
                ui.end_row();
                ui.label("Centre Y");
                ui.add(egui::DragValue::new(&mut self.center_y).speed(0.5).suffix(" mm"));
                ui.end_row();
            });
            ui.checkbox(&mut self.rotate, "Rotate copies");
            out = ok_cancel(ui, "Create", true);
        });
        out
    }
}

pub struct OffsetDialog {
    pub distance: f64,
    pub delete_originals: bool,
}

impl OffsetDialog {
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        let mut out = None;
        window(ctx, "Offset shapes").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Distance");
                ui.add(egui::DragValue::new(&mut self.distance).speed(0.1).suffix(" mm"));
            });
            ui.weak("Positive grows outward, negative shrinks inward. Corners are rounded.");
            ui.checkbox(&mut self.delete_originals, "Delete original shapes");
            out = ok_cancel(ui, "Offset", self.distance != 0.0);
        });
        out
    }
}

/// Place on Each Slot: copies the design onto every jig slot.
pub struct PlaceSlotsDialog {
    pub slots: usize,
    pub fit: bool,
    pub margin: f64,
    pub keep_original: bool,
}

impl PlaceSlotsDialog {
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        let mut out = None;
        window(ctx, "Place on each slot").show(ctx, |ui| {
            ui.label(format!("Copies the design onto the centre of each of the {} slot(s), turned to match.", self.slots));
            ui.weak("Slots are the selected closed shapes on tool layers Tool1/Tool2 (draw them, or use\nCamera > Detect items). Everything else selected is the design.");
            ui.checkbox(&mut self.fit, "Scale to fit each slot");
            ui.add_enabled_ui(self.fit, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Margin");
                    ui.add(egui::DragValue::new(&mut self.margin).range(0.0..=50.0).speed(0.1).suffix(" mm"));
                });
            });
            ui.checkbox(&mut self.keep_original, "Keep the original design");
            out = ok_cancel(ui, "Place", true);
        });
        out
    }
}

// --- Material test ----------------------------------------------------------

/// What the material test window asks for.
pub enum TestAction {
    Preview,
    Frame,
    Start,
    Close,
}

/// Which test pattern the material test window makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestKind {
    /// Speed × power grid.
    Material,
    /// Filled squares at different line intervals.
    Interval,
    /// A line along a ramp, to find the focus height.
    Focus,
    /// A strip cut into pieces, to measure the kerf.
    Kerf,
}

pub struct MaterialTest {
    pub kind: TestKind,
    /// Interval test: from, to, count.
    pub interval_min: f64,
    pub interval_max: f64,
    pub interval_count: usize,
    /// Focus test: ramp length and how far its far end is raised (mm).
    pub ramp_length: f64,
    pub ramp_rise: f64,
    /// Kerf test: strip size and number of pieces.
    pub kerf_width: f64,
    pub kerf_height: f64,
    pub kerf_pieces: usize,
    /// Power (%) and speed (mm/s) for the interval, focus and kerf tests.
    pub test_power: f64,
    pub test_speed: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub fill: bool,
    pub power_min: f64,
    pub power_max: f64,
    pub rows: usize,
    pub speed_min: f64,
    pub speed_max: f64,
    pub columns: usize,
    pub cell: f64,
    pub gap: f64,
    pub passes: u32,
    pub interval: f64,
    pub labels: bool,
    pub label_power: f64,
    pub label_speed: f64,
    /// How speeds are shown (the selected machine's units); set by the app each frame.
    pub speed: crate::units::Speed,
}

impl Default for MaterialTest {
    fn default() -> Self {
        Self {
            kind: TestKind::Material,
            interval_min: 0.05,
            interval_max: 0.25,
            interval_count: 5,
            ramp_length: 100.0,
            ramp_rise: 10.0,
            kerf_width: 50.0,
            kerf_height: 15.0,
            kerf_pieces: 5,
            test_power: 30.0,
            test_speed: 100.0,
            origin_x: 10.0,
            origin_y: 10.0,
            fill: true,
            power_min: 10.0,
            power_max: 60.0,
            rows: 5,
            speed_min: 50.0,
            speed_max: 250.0,
            columns: 5,
            cell: 8.0,
            gap: 2.0,
            passes: 1,
            interval: 0.1,
            labels: true,
            label_power: 15.0,
            label_speed: 200.0,
            speed: Default::default(),
        }
    }
}

fn lerp(a: f64, b: f64, i: usize, n: usize) -> f64 {
    if n <= 1 { a } else { a + (b - a) * i as f64 / (n - 1) as f64 }
}

/// Burn paths for a small text label centred on `centre`.
fn label_paths(fonts: &FontLibrary, text: &str, centre: Point, size: f64) -> Vec<JobPath> {
    let family = fonts.default_family();
    let Some(outline) = fonts.outline(text, &family, size, false, false, &bc_doc::TextStyle::default()) else { return Vec::new() };
    let placed = bc_geom::Affine::translate(centre.to_vec2()) * outline;
    bc_geom::flatten(&placed, 0.02)
        .into_iter()
        .map(|mut line| {
            if line.closed {
                line.points.push(line.points[0]);
            }
            JobPath::burn(line.points)
        })
        .collect()
}

impl MaterialTest {
    pub fn job(&self, fonts: &FontLibrary) -> Job {
        match self.kind {
            TestKind::Material => self.grid_job(fonts),
            TestKind::Interval => self.interval_job(fonts),
            TestKind::Focus => self.focus_job(fonts),
            TestKind::Kerf => self.kerf_job(fonts),
        }
    }

    fn layer(&self, index: usize, name: String, mode: LayerMode, power: f64, speed: f64) -> Layer {
        let mut settings = Layer::new(index % bc_doc::CUT_LAYER_COUNT);
        settings.name = name;
        settings.mode = mode;
        settings.min_power = power;
        settings.max_power = power;
        settings.speed = speed;
        settings
    }

    fn labels_layer(&self, paths: Vec<JobPath>) -> Option<JobLayer> {
        (!paths.is_empty()).then(|| JobLayer { index: 0, settings: self.layer(0, "Labels".into(), LayerMode::Line, self.label_power, self.label_speed), paths })
    }

    /// Filled squares, one per line interval, labelled with the interval.
    fn interval_job(&self, fonts: &FontLibrary) -> Job {
        let n = self.interval_count.max(1);
        let mut job = Job::default();
        let mut labels = Vec::new();
        for i in 0..n {
            let interval = lerp(self.interval_min, self.interval_max, i, n).max(0.01);
            let (x, y) = (self.origin_x + i as f64 * (self.cell + self.gap), self.origin_y + if self.labels { 6.0 } else { 0.0 });
            let ring = vec![Point::new(x, y), Point::new(x + self.cell, y), Point::new(x + self.cell, y + self.cell), Point::new(x, y + self.cell)];
            let mut settings = self.layer(i, format!("{interval:.3} mm"), LayerMode::Fill, self.test_power, self.test_speed);
            settings.interval = interval;
            let paths = bc_job::scanlines(&[ring], interval, true).into_iter().map(JobPath::burn).collect();
            job.layers.push(JobLayer { index: i % bc_doc::CUT_LAYER_COUNT, settings, paths });
            if self.labels {
                labels.extend(label_paths(fonts, &format!("{interval:.2}"), Point::new(x + self.cell / 2.0, self.origin_y + 2.0), 2.5));
            }
        }
        job.layers.extend(self.labels_layer(labels));
        job
    }

    /// A line along a ramp with ticks: lay the material on a slope rising `ramp_rise` mm over
    /// `ramp_length`; where the line is thinnest is in focus, and the labels give the height there.
    fn focus_job(&self, fonts: &FontLibrary) -> Job {
        let (x0, y) = (self.origin_x, self.origin_y + if self.labels { 8.0 } else { 0.0 });
        let len = self.ramp_length.max(10.0);
        let mut paths = vec![JobPath::burn(vec![Point::new(x0, y), Point::new(x0 + len, y)])];
        let mut labels = Vec::new();
        let ticks = 10;
        for i in 0..=ticks {
            let x = x0 + len * i as f64 / ticks as f64;
            paths.push(JobPath::burn(vec![Point::new(x, y + 1.0), Point::new(x, y + 4.0)]));
            if self.labels {
                labels.extend(label_paths(fonts, &format!("{:.1}", self.ramp_rise * i as f64 / ticks as f64), Point::new(x, y - 4.0), 2.5));
            }
        }
        let mut job = Job::default();
        job.layers.push(JobLayer { index: 0, settings: self.layer(0, "Focus line".into(), LayerMode::Line, self.test_power, self.test_speed), paths });
        job.layers.extend(self.labels_layer(labels));
        job
    }

    /// A strip cut into equal pieces: push them together and measure to get the kerf.
    fn kerf_job(&self, fonts: &FontLibrary) -> Job {
        let (x0, y0) = (self.origin_x, self.origin_y + if self.labels { 6.0 } else { 0.0 });
        let (w, h) = (self.kerf_width.max(5.0), self.kerf_height.max(3.0));
        let n = self.kerf_pieces.max(2);
        let mut paths = Vec::new();
        for i in 1..n {
            let x = x0 + w * i as f64 / n as f64;
            paths.push(JobPath::burn(vec![Point::new(x, y0), Point::new(x, y0 + h)]));
        }
        paths.push(JobPath::burn(vec![Point::new(x0, y0), Point::new(x0 + w, y0), Point::new(x0 + w, y0 + h), Point::new(x0, y0 + h), Point::new(x0, y0)]));
        let labels = if self.labels { label_paths(fonts, &format!("{w:.0} mm, {n} pieces"), Point::new(x0 + w / 2.0, self.origin_y + 2.0), 2.5) } else { Vec::new() };
        let mut job = Job::default();
        job.layers.push(JobLayer { index: 0, settings: self.layer(0, "Kerf cut".into(), LayerMode::Line, self.test_power, self.test_speed), paths });
        job.layers.extend(self.labels_layer(labels));
        job
    }

    /// Builds the test grid as a job: one layer per cell (power by row, speed by column), plus labels.
    fn grid_job(&self, fonts: &FontLibrary) -> Job {
        let label_margin = if self.labels { 14.0 } else { 0.0 };
        let (x0, y0) = (self.origin_x + label_margin, self.origin_y + if self.labels { 8.0 } else { 0.0 });
        let mut job = Job::default();
        let mut cell_index = 0;
        for r in 0..self.rows.max(1) {
            for c in 0..self.columns.max(1) {
                let power = lerp(self.power_min, self.power_max, r, self.rows);
                let speed = lerp(self.speed_min, self.speed_max, c, self.columns);
                let (x, y) = (x0 + c as f64 * (self.cell + self.gap), y0 + r as f64 * (self.cell + self.gap));
                let ring = vec![Point::new(x, y), Point::new(x + self.cell, y), Point::new(x + self.cell, y + self.cell), Point::new(x, y + self.cell)];
                let mut settings = Layer::new(cell_index % bc_doc::CUT_LAYER_COUNT);
                settings.name = format!("{power:.0}% {}", self.speed.format(speed));
                settings.speed = speed;
                settings.min_power = power;
                settings.max_power = power;
                settings.passes = self.passes;
                settings.interval = self.interval;
                settings.mode = if self.fill { LayerMode::Fill } else { LayerMode::Line };
                let paths: Vec<JobPath> = if self.fill {
                    bc_job::scanlines(&[ring], self.interval.max(0.01), true).into_iter().map(JobPath::burn).collect()
                } else {
                    let mut closed = ring.clone();
                    closed.push(ring[0]);
                    vec![JobPath::burn(closed)]
                };
                let paths = (0..self.passes.max(1)).flat_map(|_| paths.iter().cloned()).collect();
                job.layers.push(JobLayer { index: cell_index % bc_doc::CUT_LAYER_COUNT, settings, paths });
                cell_index += 1;
            }
        }
        if self.labels {
            let family = fonts.default_family();
            let mut paths = Vec::new();
            let mut label = |text: &str, centre: Point| {
                if let Some(outline) = fonts.outline(text, &family, 2.5, false, false, &bc_doc::TextStyle::default()) {
                    let placed = bc_geom::Affine::translate(centre.to_vec2()) * outline;
                    for mut line in bc_geom::flatten(&placed, 0.02) {
                        if line.closed {
                            line.points.push(line.points[0]);
                        }
                        paths.push(JobPath::burn(line.points));
                    }
                }
            };
            for r in 0..self.rows.max(1) {
                let y = y0 + r as f64 * (self.cell + self.gap) + self.cell / 2.0;
                label(&format!("{:.0}%", lerp(self.power_min, self.power_max, r, self.rows)), Point::new(self.origin_x + label_margin / 2.0, y));
            }
            for c in 0..self.columns.max(1) {
                let x = x0 + c as f64 * (self.cell + self.gap) + self.cell / 2.0;
                label(&self.speed.number(lerp(self.speed_min, self.speed_max, c, self.columns)), Point::new(x, self.origin_y + 4.0));
            }
            let mut settings = Layer::new(0);
            settings.name = "Labels".into();
            settings.speed = self.label_speed;
            settings.min_power = self.label_power;
            settings.max_power = self.label_power;
            if !paths.is_empty() {
                job.layers.push(JobLayer { index: 0, settings, paths });
            }
        }
        job
    }

    /// Settings for the interval, focus and kerf tests.
    fn other_test_ui(&mut self, ui: &mut egui::Ui) {
        let mm = |ui: &mut egui::Ui, v: &mut f64, range: std::ops::RangeInclusive<f64>| ui.add(egui::DragValue::new(v).range(range).speed(0.1).suffix(" mm"));
        ui.weak(match self.kind {
            TestKind::Interval => "Fills squares at different line intervals: pick the coarsest that still looks solid.",
            TestKind::Focus => "Lay the material on a ramp: one end flat on the bed, the far end raised by the rise.\nWhere the line is thinnest is in focus; the labels give the height there.",
            _ => "Cuts a strip into pieces. Push the pieces together and measure them:\nkerf = (strip width − measured width) ÷ number of pieces.",
        });
        egui::Grid::new("other-test").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            match self.kind {
                TestKind::Interval => {
                    ui.label("Interval");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.interval_min).range(0.01..=1.0).speed(0.005).prefix("from ").suffix(" mm"));
                        ui.add(egui::DragValue::new(&mut self.interval_max).range(0.01..=1.0).speed(0.005).prefix("to ").suffix(" mm"));
                        ui.add(egui::DragValue::new(&mut self.interval_count).range(2..=12).suffix(" squares"));
                    });
                    ui.end_row();
                    ui.label("Square size");
                    mm(ui, &mut self.cell, 3.0..=50.0);
                    ui.end_row();
                }
                TestKind::Focus => {
                    ui.label("Ramp length / rise");
                    ui.horizontal(|ui| {
                        mm(ui, &mut self.ramp_length, 20.0..=500.0);
                        mm(ui, &mut self.ramp_rise, 1.0..=50.0);
                    });
                    ui.end_row();
                }
                _ => {
                    ui.label("Strip");
                    ui.horizontal(|ui| {
                        mm(ui, &mut self.kerf_width, 10.0..=300.0);
                        ui.label("×");
                        mm(ui, &mut self.kerf_height, 5.0..=100.0);
                        ui.add(egui::DragValue::new(&mut self.kerf_pieces).range(2..=20).suffix(" pieces"));
                    });
                    ui.end_row();
                }
            }
            ui.label("Power / speed");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.test_power).range(0.0..=100.0).suffix(" %"));
                ui.add(self.speed.edit(&mut self.test_speed, 1.0..=2000.0));
            });
            ui.end_row();
            ui.label("Position");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.origin_x).speed(0.5).prefix("X ").suffix(" mm"));
                ui.add(egui::DragValue::new(&mut self.origin_y).speed(0.5).prefix("Y ").suffix(" mm"));
            });
            ui.end_row();
            ui.checkbox(&mut self.labels, "Labels");
            ui.end_row();
        });
    }

    pub fn ui(&mut self, ctx: &egui::Context, connected: bool) -> Option<TestAction> {
        let mut out = None;
        centered(ctx, "Test patterns").collapsible(false).resizable(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.kind, TestKind::Material, "Speed × power");
                ui.selectable_value(&mut self.kind, TestKind::Interval, "Line interval");
                ui.selectable_value(&mut self.kind, TestKind::Focus, "Focus");
                ui.selectable_value(&mut self.kind, TestKind::Kerf, "Kerf");
            });
            ui.separator();
            if self.kind != TestKind::Material {
                self.other_test_ui(ui);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Preview").clicked() { out = Some(TestAction::Preview); }
                    if ui.add_enabled(connected, egui::Button::new("Frame")).clicked() { out = Some(TestAction::Frame); }
                    if ui.add_enabled(connected, egui::Button::new("Start")).clicked() { out = Some(TestAction::Start); }
                    if ui.button("Close").clicked() { out = Some(TestAction::Close); }
                });
                return;
            }
            ui.weak(format!("Burns a grid of squares: power changes down the rows, {} across the columns.", self.speed.label().to_lowercase()));
            egui::Grid::new("material-test").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label("Power %");
                ui.add(egui::DragValue::new(&mut self.power_min).range(0.0..=100.0).prefix("from "));
                ui.add(egui::DragValue::new(&mut self.power_max).range(0.0..=100.0).prefix("to "));
                ui.add(egui::DragValue::new(&mut self.rows).range(1..=10).suffix(" rows"));
                ui.end_row();
                ui.label(self.speed.label());
                ui.add(self.speed.edit(&mut self.speed_min, 1.0..=2000.0).prefix("from "));
                ui.add(self.speed.edit(&mut self.speed_max, 1.0..=2000.0).prefix("to "));
                ui.add(egui::DragValue::new(&mut self.columns).range(1..=10).suffix(" columns"));
                ui.end_row();
                ui.label("Cells");
                ui.add(egui::DragValue::new(&mut self.cell).range(2.0..=50.0).speed(0.5).prefix("size ").suffix(" mm"));
                ui.add(egui::DragValue::new(&mut self.gap).range(0.0..=20.0).speed(0.2).prefix("gap ").suffix(" mm"));
                ui.add(egui::DragValue::new(&mut self.passes).range(1..=20).suffix(" pass(es)"));
                ui.end_row();
                ui.label("Mode");
                ui.radio_value(&mut self.fill, true, "Fill");
                ui.radio_value(&mut self.fill, false, "Line");
                ui.add_enabled(self.fill, egui::DragValue::new(&mut self.interval).range(0.02..=1.0).speed(0.005).prefix("interval ").suffix(" mm"));
                ui.end_row();
                ui.label("Position");
                ui.add(egui::DragValue::new(&mut self.origin_x).speed(0.5).prefix("X ").suffix(" mm"));
                ui.add(egui::DragValue::new(&mut self.origin_y).speed(0.5).prefix("Y ").suffix(" mm"));
                ui.end_row();
                ui.checkbox(&mut self.labels, "Labels");
                ui.add_enabled(self.labels, egui::DragValue::new(&mut self.label_power).range(0.0..=100.0).prefix("power ").suffix(" %"));
                ui.add_enabled(self.labels, self.speed.edit(&mut self.label_speed, 1.0..=2000.0).prefix("speed "));
                ui.end_row();
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Preview").clicked() { out = Some(TestAction::Preview); }
                if ui.add_enabled(connected, egui::Button::new("Frame")).clicked() { out = Some(TestAction::Frame); }
                if ui.add_enabled(connected, egui::Button::new("Start")).clicked() { out = Some(TestAction::Start); }
                if ui.button("Close").clicked() { out = Some(TestAction::Close); }
            });
        });
        out
    }
}

// --- Run in sections --------------------------------------------------------

/// What the sections window asks for, for section `k`.
pub enum SectionAction {
    Preview(usize),
    Frame(usize),
    Start(usize),
    Close,
}

/// Runs a design taller than the bed in bands, for machines the material can slide through.
pub struct SectionsDialog {
    /// The whole job, in design coordinates.
    pub job: Job,
    pub height: f64,
    /// Bed Y each section is moved to.
    pub bed_top: f64,
    pub marks: bool,
    /// Sections already sent, to tick them off.
    pub done: BTreeSet<usize>,
}

/// Arm length of the registration crosses.
const MARK: f64 = 3.0;

impl SectionsDialog {
    pub fn new(job: Job, bed_height: f64) -> Self {
        Self { job, height: (bed_height - 2.0 * MARK - 2.0).max(10.0), bed_top: MARK + 1.0, marks: true, done: BTreeSet::new() }
    }

    pub fn plan(&self) -> Option<bc_job::Sections> {
        bc_job::Sections::plan(&self.job, self.height)
    }

    /// Registration crosses on the boundary at design Y `y`, just left and right of the design.
    fn crosses(&self, y: f64) -> Vec<JobPath> {
        let Some((a, b)) = self.job.bounds() else { return Vec::new() };
        [(a.x - 2.0 * MARK).max(MARK), b.x + 2.0 * MARK]
            .into_iter()
            .flat_map(|x| {
                [
                    JobPath::burn(vec![Point::new(x - MARK, y), Point::new(x + MARK, y)]),
                    JobPath::burn(vec![Point::new(x, y - MARK), Point::new(x, y + MARK)]),
                ]
            })
            .collect()
    }

    /// Section `k`, moved to the top of the bed. With marks on, it also burns crosses on the boundary
    /// with the next section; after moving the material they show where that section starts.
    pub fn section(&self, k: usize) -> Job {
        let Some(plan) = self.plan() else { return Job::default() };
        let (y0, y1) = plan.band(k);
        let mut job = self.job.section(y0, y1, self.bed_top);
        if self.marks && plan.count > 1 {
            let mut paths = Vec::new();
            if k + 1 < plan.count {
                paths.extend(self.crosses(y1));
            }
            if k > 0 {
                // Re-burning the previous section's crosses on top of themselves: if the material
                // was lined up they land exactly on the old marks.
                paths.extend(self.crosses(y0));
            }
            let shift = bc_geom::Vec2::new(0.0, self.bed_top - y0);
            for p in &mut paths {
                for q in &mut p.points {
                    *q += shift;
                }
            }
            let mut settings = Layer::new(0);
            settings.name = "Registration marks".into();
            settings.speed = 100.0;
            settings.min_power = 15.0;
            settings.max_power = 15.0;
            job.layers.push(JobLayer { index: 0, settings, paths });
        }
        job
    }

    pub fn ui(&mut self, ctx: &egui::Context, bed: (f64, f64), connected: bool) -> Option<SectionAction> {
        let mut out = None;
        centered(ctx, "Run in sections").collapsible(false).resizable(false).show(ctx, |ui| {
            ui.weak("For designs taller than the bed, on lasers with a pass-through slot or open bottom.\nEach section is moved to the top of the bed and run on its own; feed the material\nforward by the section height between runs.");
            let Some((a, b)) = self.job.bounds() else {
                ui.label("The design has nothing to burn.");
                if ui.button("Close").clicked() {
                    out = Some(SectionAction::Close);
                }
                return;
            };
            ui.label(format!("Design: {:.1} × {:.1} mm   Bed: {:.0} × {:.0} mm", b.x - a.x, b.y - a.y, bed.0, bed.1));
            if b.x - a.x > bed.0 + 1e-6 {
                ui.colored_label(ui.visuals().warn_fg_color, "The design is wider than the bed; sections only split it top to bottom.");
            }
            let max_height = (bed.1 - self.bed_top - if self.marks { MARK } else { 0.0 }).max(1.0);
            egui::Grid::new("sections").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label("Section height / feed");
                ui.add(egui::DragValue::new(&mut self.height).range(1.0..=max_height).speed(0.5).suffix(" mm"));
                ui.end_row();
                ui.label("Start at bed Y");
                ui.add(egui::DragValue::new(&mut self.bed_top).range(0.0..=bed.1 / 2.0).speed(0.2).suffix(" mm"));
                ui.end_row();
                ui.checkbox(&mut self.marks, "Registration marks");
                ui.weak("crosses at each boundary, beside the design");
                ui.end_row();
            });
            self.height = self.height.clamp(1.0, max_height);
            ui.separator();
            let Some(plan) = self.plan() else { return };
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                egui::Grid::new("section-list").num_columns(5).spacing([8.0, 4.0]).striped(true).show(ui, |ui| {
                    for k in 0..plan.count {
                        let (y0, y1) = plan.band(k);
                        ui.label(if self.done.contains(&k) { format!("{} (sent)", k + 1) } else { format!("{}", k + 1) });
                        ui.label(format!("Y {:.1} – {:.1}", y0, y1.min(b.y)));
                        if ui.button("Preview").clicked() {
                            out = Some(SectionAction::Preview(k));
                        }
                        if ui.add_enabled(connected, egui::Button::new("Frame")).clicked() {
                            out = Some(SectionAction::Frame(k));
                        }
                        if ui.add_enabled(connected, egui::Button::new("Start")).clicked() {
                            out = Some(SectionAction::Start(k));
                        }
                        ui.end_row();
                    }
                });
            });
            if self.marks && plan.count > 1 {
                ui.weak(format!(
                    "After each section, feed the material {:.1} mm so the crosses sit at bed Y {:.1}.\nFrame the next section to check before starting.",
                    plan.height, self.bed_top
                ));
            } else {
                ui.weak(format!("After each section, feed the material forward {:.1} mm.", plan.height));
            }
            ui.separator();
            if ui.button("Close").clicked() {
                out = Some(SectionAction::Close);
            }
        });
        out
    }
}

// --- Preview ----------------------------------------------------------------

pub enum PreviewAction {
    Frame,
    Start,
    Close,
}

pub struct Preview {
    pub job: Job,
    pub title: String,
    /// How much of the job to draw, 0..=1.
    progress: f32,
    show_travel: bool,
    stats: bc_job::JobStats,
    /// Show the job burned into a material instead of as lines.
    realistic: bool,
    material: bc_job::render::Material,
    /// The last realistic picture: (material, progress in %, texture, area it covers).
    picture: Option<(bc_job::render::Material, u32, egui::TextureHandle, bc_geom::Rect)>,
    /// The last Lines picture: (progress in ‰, show travel, size in px, texture, head position).
    lines: Option<(u32, bool, [usize; 2], egui::TextureHandle, Point)>,
}

impl Preview {
    pub fn new(job: Job, title: String, rapid_speed: f64) -> Self {
        let stats = job.stats(Point::ORIGIN, rapid_speed);
        Self { job, title, progress: 1.0, show_travel: true, stats, realistic: false, material: bc_job::render::Material::Plywood, picture: None, lines: None }
    }

    pub fn ui(&mut self, ctx: &egui::Context, bed: (f64, f64), connected: bool) -> Option<PreviewAction> {
        let mut out = None;
        let mut open = true;
        centered(ctx, format!("Preview: {}", self.title)).id(egui::Id::new("preview-window")).open(&mut open).default_size([760.0, 600.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Burn {:.0} mm · travel {:.0} mm · about {}",
                    self.stats.cut_mm,
                    self.stats.travel_mm,
                    format_duration(self.stats.seconds)
                ));
            });
            if !self.job.skipped.is_empty() {
                for (i, why) in &self.job.skipped {
                    ui.colored_label(Color32::from_rgb(200, 120, 0), format!("Layer {}: {why}", bc_doc::layer_label(*i)));
                }
            }
            let mut scrubbing = false;
            ui.horizontal(|ui| {
                scrubbing = ui.add(egui::Slider::new(&mut self.progress, 0.0..=1.0).show_value(false).text("progress")).dragged();
                ui.selectable_value(&mut self.realistic, false, "Lines");
                ui.selectable_value(&mut self.realistic, true, "Realistic").on_hover_text("The job burned into a material; darkness follows each layer's power, speed and passes");
                if self.realistic {
                    egui::ComboBox::from_id_salt("preview-material").selected_text(self.material.label()).show_ui(ui, |ui| {
                        for m in bc_job::render::Material::ALL {
                            ui.selectable_value(&mut self.material, m, m.label());
                        }
                    });
                } else {
                    ui.checkbox(&mut self.show_travel, "Show travel");
                }
            });
            let size = ui.available_size() - egui::vec2(0.0, 36.0);
            let (rect, _) = ui.allocate_exact_size(size.max(egui::vec2(200.0, 150.0)), egui::Sense::hover());
            if self.realistic {
                self.draw_realistic(ui, rect, scrubbing);
            } else {
                self.draw_lines(ui, rect, bed, scrubbing);
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(connected, egui::Button::new("Frame")).clicked() { out = Some(PreviewAction::Frame); }
                if ui.add_enabled(connected, egui::Button::new("Start")).clicked() { out = Some(PreviewAction::Start); }
                if !connected {
                    ui.weak("Connect to the laser to frame or start.");
                }
            });
        });
        if !open {
            out = Some(PreviewAction::Close);
        }
        out
    }

    /// The job burned into the chosen material, zoomed to the job. Re-rendered when the material
    /// or progress changes (not while the progress slider is being dragged).
    fn draw_realistic(&mut self, ui: &egui::Ui, rect: egui::Rect, scrubbing: bool) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_gray(40));
        let Some((a, b)) = self.job.bounds() else { return };
        let percent = (self.progress * 100.0).round() as u32;
        let stale = self.picture.as_ref().is_none_or(|(m, p, _, _)| *m != self.material || *p != percent);
        if stale && !scrubbing {
            let area = bc_geom::Rect::new(a.x, a.y, b.x, b.y).inflate(4.0, 4.0);
            // Up to 10 px/mm, and at most ~1800 px across.
            let k = (1800.0 / area.width().max(area.height())).min(10.0);
            let pic = bc_job::render::render(&self.job, area, k, self.material, self.progress);
            let image = egui::ColorImage::from_rgb([pic.width, pic.height], &pic.rgb);
            let tex = ui.ctx().load_texture("burn-preview", image, egui::TextureOptions::LINEAR);
            self.picture = Some((self.material, percent, tex, pic.area));
        }
        let Some((_, _, tex, area)) = &self.picture else { return };
        let aspect = (area.width() / area.height()) as f32;
        let mut size = rect.size() - egui::vec2(16.0, 16.0);
        if size.x / size.y > aspect {
            size.x = size.y * aspect;
        } else {
            size.y = size.x / aspect;
        }
        let shown = egui::Rect::from_center_size(rect.center(), size);
        painter.image(tex.id(), shown, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        if stale {
            painter.text(shown.left_top() + egui::vec2(6.0, 6.0), egui::Align2::LEFT_TOP, "release to update", egui::FontId::proportional(12.0), Color32::WHITE);
        }
    }

    /// The job as lines on the bed, rendered into a picture at the window's resolution (photos are
    /// millions of short runs: far too many to hand to the GPU one by one).
    fn draw_lines(&mut self, ui: &egui::Ui, rect: egui::Rect, bed: (f64, f64), scrubbing: bool) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_gray(250));
        let scale = ((rect.width() - 20.0) / bed.0 as f32).min((rect.height() - 20.0) / bed.1 as f32).max(0.01);
        let shown = egui::Rect::from_center_size(rect.center(), egui::vec2(bed.0 as f32, bed.1 as f32) * scale);
        let ppp = ui.ctx().pixels_per_point();
        let px_per_mm = (scale * ppp) as f64;
        let size = [((bed.0 * px_per_mm).ceil() as usize).max(1), ((bed.1 * px_per_mm).ceil() as usize).max(1)];
        let permille = (self.progress * 1000.0).round() as u32;
        let stale = self.lines.as_ref().is_none_or(|(p, t, s, _, _)| *p != permille || *t != self.show_travel || *s != size);
        if stale && (!scrubbing || self.lines.is_none()) {
            let area = bc_geom::Rect::new(0.0, 0.0, bed.0, bed.1);
            let (pic, head) = bc_job::render::render_lines(&self.job, area, px_per_mm, self.progress, self.show_travel);
            let image = egui::ColorImage::from_rgb([pic.width, pic.height], &pic.rgb);
            let tex = ui.ctx().load_texture("preview-lines", image, egui::TextureOptions::LINEAR);
            self.lines = Some((permille, self.show_travel, size, tex, head));
        }
        let Some((_, _, _, tex, head)) = &self.lines else { return };
        painter.image(tex.id(), shown, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        painter.rect_stroke(shown, 0.0, Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Middle);
        let at = shown.min + egui::vec2(head.x as f32, head.y as f32) * scale;
        painter.circle_stroke(at, 4.0, Stroke::new(1.5, Color32::RED));
        if stale && scrubbing {
            painter.text(shown.left_top() + egui::vec2(6.0, 6.0), egui::Align2::LEFT_TOP, "release to update", egui::FontId::proportional(12.0), Color32::DARK_GRAY);
        }
    }
}

pub fn format_duration(seconds: f64) -> String {
    let s = seconds.round() as u64;
    if s >= 3600 { format!("{}h {:02}m", s / 3600, (s % 3600) / 60) } else { format!("{}m {:02}s", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_test_grid_layout() {
        let t = MaterialTest { labels: false, fill: false, rows: 3, columns: 4, ..Default::default() };
        let job = t.job(&FontLibrary::from_database(Default::default()));
        assert_eq!(job.layers.len(), 12);
        assert_eq!(job.layers[0].settings.max_power, t.power_min);
        assert_eq!(job.layers[11].settings.max_power, t.power_max);
        assert_eq!(job.layers[11].settings.speed, t.speed_max);
        let (min, max) = job.bounds().unwrap();
        assert_eq!(min, Point::new(10.0, 10.0));
        assert!((max.x - (10.0 + 4.0 * 8.0 + 3.0 * 2.0)).abs() < 1e-9);
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(65.0), "1m 05s");
        assert_eq!(format_duration(3720.0), "1h 02m");
    }
}

// --- Box and living-hinge generator ---------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MakerTab {
    Box,
    Hinge,
    Puzzle,
    Gear,
    Sign,
    Packaging,
}

/// Sign / plaque settings.
#[derive(Clone)]
pub struct SignSettings {
    pub shape: bc_geom::makers::SignShape,
    pub width: f64,
    pub height: f64,
    pub corner: f64,
    /// Engraved border: distance in from the edge and band width (0 for none).
    pub border_inset: f64,
    pub border_width: f64,
    pub text: String,
    pub font: String,
    /// Tallest the capitals may be; the text shrinks to fit the sign.
    pub text_height: f64,
    pub holes: u32,
    pub hole_diameter: f64,
    pub hole_inset: f64,
    /// Add a tab and a slotted base for an edge-lit acrylic sign.
    pub edge_lit: bool,
    pub thickness: f64,
}

impl SignSettings {
    /// The engraved border band, centred on the origin, if any.
    pub fn border(&self) -> Option<bc_geom::BezPath> {
        use bc_geom::makers::sign_outline;
        if self.border_width <= 0.0 {
            return None;
        }
        let i = self.border_inset;
        let (w, h) = (self.width - 2.0 * i, self.height - 2.0 * i);
        let (wi, hi) = (w - 2.0 * self.border_width, h - 2.0 * self.border_width);
        if wi <= 0.0 || hi <= 0.0 {
            return None;
        }
        let mut band = sign_outline(self.shape, w, h, (self.corner - i).max(0.0));
        band.extend(sign_outline(self.shape, wi, hi, (self.corner - i - self.border_width).max(0.0)));
        Some(band)
    }

    /// Room for the text: inside the border (or a margin) and clear of the mounting holes.
    pub fn text_box(&self) -> (f64, f64) {
        let inner = self.border_inset + self.border_width.max(0.0) + 3.0;
        let holes = if self.holes > 0 { self.hole_inset + self.hole_diameter + 2.0 } else { 0.0 };
        let side = inner.max(if self.shape == bc_geom::makers::SignShape::Oval { holes } else { 0.0 });
        let w = (self.width - 2.0 * side) * if self.shape == bc_geom::makers::SignShape::Oval { 0.75 } else { 1.0 };
        let h = self.height - 2.0 * inner.max(if self.shape == bc_geom::makers::SignShape::Oval { 0.0 } else { holes });
        (w.max(1.0), h.max(1.0))
    }
}

/// Jigsaw puzzle settings.
#[derive(Clone, Copy)]
pub struct PuzzleSettings {
    pub width: f64,
    pub height: f64,
    pub cols: usize,
    pub rows: usize,
    pub knob: f64,
    pub corner: f64,
    pub seed: u64,
}

pub struct BoxDialog {
    pub tab: MakerTab,
    pub spec: bc_geom::boxgen::BoxSpec,
    /// Living hinge region and pattern (mm).
    pub hinge_size: (f64, f64),
    pub hinge_cut: f64,
    pub hinge_bridge: f64,
    pub hinge_spacing: f64,
    pub puzzle: PuzzleSettings,
    /// Where the puzzle's top-left corner goes (over the selection it was opened with), if anywhere.
    pub place_at: Option<bc_geom::Point>,
    pub gear: bc_geom::makers::GearSpec,
    /// Teeth on a second, meshing gear (0 for none).
    pub gear2_teeth: u32,
    pub sign: SignSettings,
    /// Packaging dieline: which kind, its sizes, and how the fold lines are scored.
    pub dieline: bc_geom::dieline::DielineKind,
    pub dieline_spec: bc_geom::dieline::DielineSpec,
    message: String,
}

impl Default for BoxDialog {
    fn default() -> Self {
        use bc_geom::boxgen::{BoxSpec, BoxStyle};
        Self {
            tab: MakerTab::Box,
            spec: BoxSpec { width: 100.0, depth: 80.0, height: 60.0, inside: true, thickness: 3.0, finger: 10.0, kerf: 0.1, style: BoxStyle::Closed },
            hinge_size: (60.0, 40.0),
            hinge_cut: 12.0,
            hinge_bridge: 3.0,
            hinge_spacing: 2.0,
            puzzle: PuzzleSettings { width: 200.0, height: 150.0, cols: 6, rows: 4, knob: 1.0, corner: 4.0, seed: 1 },
            place_at: None,
            gear: bc_geom::makers::GearSpec { teeth: 24, module: 2.0, pressure_angle: 20.0, bore: 6.0, keyway: 0.0 },
            gear2_teeth: 0,
            sign: SignSettings {
                shape: bc_geom::makers::SignShape::Rounded,
                width: 200.0,
                height: 80.0,
                corner: 8.0,
                border_inset: 5.0,
                border_width: 1.5,
                text: "Welcome".into(),
                font: String::new(),
                text_height: 30.0,
                holes: 2,
                hole_diameter: 4.0,
                hole_inset: 8.0,
                edge_lit: false,
                thickness: 3.0,
            },
            dieline: bc_geom::dieline::DielineKind::TuckBox,
            dieline_spec: bc_geom::dieline::DielineSpec::default(),
            message: String::new(),
        }
    }
}

/// Moves a path so its bounding box's top-left corner is at the origin.
fn top_left(p: bc_geom::BezPath) -> bc_geom::BezPath {
    match bc_geom::bounds(&p) {
        Some(b) => bc_geom::Affine::translate((-b.x0, -b.y0)) * p,
        None => p,
    }
}

impl BoxDialog {
    /// The packaging dieline for the current settings.
    pub fn dieline(&self) -> Result<bc_geom::dieline::Dieline, String> {
        bc_geom::dieline::dieline(self.dieline, &self.dieline_spec)
    }

    /// The parts to create, as (name, path with its top-left at the origin), or an error.
    pub fn parts(&self, max_width: f64) -> Result<Vec<(&'static str, bc_geom::BezPath)>, String> {
        use bc_geom::boxgen;
        match self.tab {
            MakerTab::Box => Ok(boxgen::layout(&boxgen::finger_box(&self.spec)?, 5.0, max_width)),
            MakerTab::Puzzle => {
                let p = self.puzzle;
                Ok(vec![("Jigsaw puzzle", boxgen::jigsaw(p.width, p.height, p.cols, p.rows, p.knob, p.corner, p.seed))])
            }
            MakerTab::Hinge => {
                let (w, h) = self.hinge_size;
                let p = boxgen::living_hinge(w, h, self.hinge_cut, self.hinge_bridge, self.hinge_spacing);
                if p.elements().is_empty() { Err("the hinge settings produce no cuts".into()) } else { Ok(vec![("Living hinge", p)]) }
            }
            MakerTab::Packaging => Ok(vec![(self.dieline.label(), self.dieline()?.cut)]),
            MakerTab::Gear => {
                use bc_geom::makers::gear;
                let first = top_left(gear(&self.gear));
                let mut parts = vec![("Gear", first)];
                if self.gear2_teeth >= 4 {
                    let second = bc_geom::makers::GearSpec { teeth: self.gear2_teeth, ..self.gear };
                    let dx = self.gear.outside_diameter() + 5.0;
                    parts.push(("Gear 2", bc_geom::Affine::translate((dx, 0.0)) * top_left(gear(&second))));
                }
                Ok(parts)
            }
            MakerTab::Sign => {
                use bc_geom::makers::{edge_lit_stand, mounting_holes, sign_outline, SignShape};
                let s = &self.sign;
                if s.width < 10.0 || s.height < 10.0 {
                    return Err("the sign is too small".into());
                }
                let mut outline = sign_outline(s.shape, s.width, s.height, s.corner);
                let mut parts = Vec::new();
                if s.edge_lit {
                    let (tab, base) = edge_lit_stand(s.width, s.thickness);
                    let tab = bc_geom::Affine::translate((0.0, s.height / 2.0 - 0.01)) * tab;
                    outline = bc_geom::ops::boolean(&[outline, tab], bc_geom::ops::BooleanOp::Union).ok_or("couldn't add the tab")?;
                    let below = s.height + s.thickness * 2.0 + 10.0;
                    let b = bc_geom::bounds(&base).ok_or("empty base")?;
                    parts.push(("Sign base", bc_geom::Affine::translate((s.width / 2.0 - b.width() / 2.0, below)) * top_left(base)));
                }
                if s.holes > 0 {
                    outline.extend(mounting_holes(s.holes, s.width, s.height, s.hole_diameter, s.hole_inset, s.shape == SignShape::Oval));
                }
                // Keep the sign's own top-left at the origin even when a tab hangs below it.
                parts.insert(0, ("Sign", bc_geom::Affine::translate((s.width / 2.0, s.height / 2.0)) * outline));
                Ok(parts)
            }
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context, max_width: f64, families: &[String]) -> Option<Outcome> {
        use bc_geom::boxgen::BoxStyle;
        let mut out = None;
        window(ctx, "Makers").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, MakerTab::Box, "Box");
                ui.selectable_value(&mut self.tab, MakerTab::Hinge, "Living hinge");
                ui.selectable_value(&mut self.tab, MakerTab::Puzzle, "Jigsaw puzzle");
                ui.selectable_value(&mut self.tab, MakerTab::Gear, "Gears");
                ui.selectable_value(&mut self.tab, MakerTab::Sign, "Sign / plaque");
                ui.selectable_value(&mut self.tab, MakerTab::Packaging, "Packaging");
            });
            ui.separator();
            let mm = |ui: &mut egui::Ui, v: &mut f64, range: std::ops::RangeInclusive<f64>| ui.add(egui::DragValue::new(v).range(range).speed(0.1).suffix(" mm"));
            match self.tab {
                MakerTab::Packaging => {
                    use bc_geom::dieline::DielineKind;
                    let kind = self.dieline;
                    let s = &mut self.dieline_spec;
                    egui::Grid::new("dieline-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Type");
                        egui::ComboBox::from_id_salt("dieline-kind").selected_text(kind.label()).show_ui(ui, |ui| {
                            for k in DielineKind::ALL {
                                ui.selectable_value(&mut self.dieline, k, k.label());
                            }
                        });
                        ui.end_row();
                        let envelope = kind == DielineKind::Envelope;
                        ui.label(if envelope { "Pocket width × height" } else { "Width × depth × height" });
                        ui.horizontal(|ui| {
                            mm(ui, &mut s.width, 5.0..=1000.0);
                            if !envelope {
                                mm(ui, &mut s.depth, 3.0..=1000.0);
                            }
                            mm(ui, &mut s.height, 5.0..=1000.0);
                        });
                        ui.end_row();
                        if !envelope {
                            ui.label("Glue tab");
                            mm(ui, &mut s.glue, 4.0..=100.0);
                            ui.end_row();
                        }
                        if kind != DielineKind::PillowBox {
                            ui.label(if envelope { "Seal flap" } else { "Tuck flap" });
                            mm(ui, &mut s.tuck, 4.0..=200.0);
                            ui.end_row();
                        }
                    });
                    if let Ok(d) = self.dieline() {
                        ui.weak(format!("The flat net is {:.0} × {:.0} mm.", d.size.0, d.size.1));
                    }
                    ui.weak("The outline is cut. The fold lines go on their own Score layer, set to low power so they mark without cutting through: test it on your card first.");
                }
                MakerTab::Gear => {
                    let g = &mut self.gear;
                    egui::Grid::new("gear-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Teeth");
                        ui.add(egui::DragValue::new(&mut g.teeth).range(6..=300));
                        ui.end_row();
                        ui.label("Module");
                        ui.add(egui::DragValue::new(&mut g.module).range(0.3..=20.0).speed(0.05).suffix(" mm"))
                            .on_hover_text("Tooth size: pitch diameter ÷ teeth. Gears must share a module to mesh.");
                        ui.end_row();
                        ui.label("Pressure angle");
                        ui.add(egui::DragValue::new(&mut g.pressure_angle).range(14.5..=30.0).speed(0.1).suffix("°"));
                        ui.end_row();
                        ui.label("Bore / keyway");
                        ui.horizontal(|ui| {
                            mm(ui, &mut g.bore, 0.0..=500.0);
                            mm(ui, &mut g.keyway, 0.0..=50.0);
                        });
                        ui.end_row();
                        ui.label("Second gear teeth");
                        ui.add(egui::DragValue::new(&mut self.gear2_teeth).range(0..=300)).on_hover_text("0 for just one gear");
                        ui.end_row();
                    });
                    let g = self.gear;
                    ui.weak(format!("Pitch Ø {:.2} mm, outside Ø {:.2} mm.", g.pitch_diameter(), g.outside_diameter()));
                    if self.gear2_teeth >= 4 {
                        let cd = g.module * (g.teeth + self.gear2_teeth) as f64 / 2.0;
                        ui.weak(format!("Mount the pair {cd:.2} mm apart (centre to centre); ratio {:.3}:1.", self.gear2_teeth as f64 / g.teeth as f64));
                    }
                }
                MakerTab::Sign => {
                    use bc_geom::makers::SignShape;
                    let s = &mut self.sign;
                    egui::Grid::new("sign-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Shape");
                        egui::ComboBox::from_id_salt("sign-shape").selected_text(s.shape.label()).show_ui(ui, |ui| {
                            for shape in SignShape::ALL {
                                ui.selectable_value(&mut s.shape, shape, shape.label());
                            }
                        });
                        ui.end_row();
                        ui.label("Width × height");
                        ui.horizontal(|ui| {
                            mm(ui, &mut s.width, 10.0..=2000.0);
                            mm(ui, &mut s.height, 10.0..=2000.0);
                        });
                        ui.end_row();
                        if matches!(s.shape, SignShape::Rounded | SignShape::Plaque) {
                            ui.label("Corner");
                            mm(ui, &mut s.corner, 0.0..=200.0);
                            ui.end_row();
                        }
                        ui.label("Text");
                        ui.add(egui::TextEdit::multiline(&mut s.text).desired_rows(2).desired_width(220.0));
                        ui.end_row();
                        ui.label("Font");
                        egui::ComboBox::from_id_salt("sign-font").selected_text(s.font.as_str()).height(300.0).show_ui(ui, |ui| {
                            for f in families {
                                ui.selectable_value(&mut s.font, f.clone(), f);
                            }
                        });
                        ui.end_row();
                        ui.label("Text height (max)");
                        mm(ui, &mut s.text_height, 2.0..=500.0).on_hover_text("The text shrinks if it wouldn't fit");
                        ui.end_row();
                        ui.label("Border inset / width");
                        ui.horizontal(|ui| {
                            mm(ui, &mut s.border_inset, 0.0..=100.0);
                            mm(ui, &mut s.border_width, 0.0..=50.0).on_hover_text("0 for no border");
                        });
                        ui.end_row();
                        ui.label("Mounting holes");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut s.holes).range(0..=4));
                            ui.label("Ø");
                            mm(ui, &mut s.hole_diameter, 1.0..=30.0);
                            ui.label("in");
                            mm(ui, &mut s.hole_inset, 0.0..=100.0);
                        });
                        ui.end_row();
                        ui.checkbox(&mut s.edge_lit, "Edge-lit base");
                        ui.add_enabled_ui(s.edge_lit, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("material");
                                mm(ui, &mut s.thickness, 1.0..=20.0);
                            });
                        });
                        ui.end_row();
                    });
                    ui.weak("The outline and holes go on a cut layer; the border and text on an engrave (Fill) layer.");
                }
                MakerTab::Box => {
                    let s = &mut self.spec;
                    egui::Grid::new("box-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Width × depth × height");
                        ui.horizontal(|ui| {
                            mm(ui, &mut s.width, 5.0..=2000.0);
                            mm(ui, &mut s.depth, 5.0..=2000.0);
                            mm(ui, &mut s.height, 5.0..=2000.0);
                        });
                        ui.end_row();
                        ui.label("Measured");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut s.inside, true, "Inside");
                            ui.radio_value(&mut s.inside, false, "Outside");
                        });
                        ui.end_row();
                        ui.label("Lid");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut s.style, BoxStyle::Closed, "Closed");
                            ui.radio_value(&mut s.style, BoxStyle::OpenTop, "Open top");
                        });
                        ui.end_row();
                        ui.label("Material thickness");
                        mm(ui, &mut s.thickness, 0.5..=30.0);
                        ui.end_row();
                        ui.label("Finger width");
                        mm(ui, &mut s.finger, 2.0..=200.0).on_hover_text("Adjusted per edge so every edge has a whole, odd number of fingers");
                        ui.end_row();
                        ui.label("Kerf");
                        ui.add(egui::DragValue::new(&mut s.kerf).range(0.0..=1.0).speed(0.005).suffix(" mm"))
                            .on_hover_text("Width of the laser cut. Parts grow by half of it so joints are snug. Use a material test or a cut-and-measure to find it (often 0.05–0.2 mm).");
                        ui.end_row();
                    });
                }
                MakerTab::Puzzle => {
                    let p = &mut self.puzzle;
                    egui::Grid::new("puzzle-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Width × height");
                        ui.horizontal(|ui| {
                            mm(ui, &mut p.width, 10.0..=2000.0);
                            mm(ui, &mut p.height, 10.0..=2000.0);
                        });
                        ui.end_row();
                        ui.label("Pieces across × down");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut p.cols).range(1..=60));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut p.rows).range(1..=60));
                            ui.weak(format!("= {} pieces", p.cols * p.rows));
                        });
                        ui.end_row();
                        ui.label("Knob size");
                        let mut pct = p.knob * 100.0;
                        if ui.add(egui::Slider::new(&mut pct, 60.0..=140.0).suffix(" %")).changed() {
                            p.knob = pct / 100.0;
                        }
                        ui.end_row();
                        ui.label("Corner radius");
                        mm(ui, &mut p.corner, 0.0..=50.0);
                        ui.end_row();
                        ui.label("Layout");
                        ui.horizontal(|ui| {
                            if ui.button("Shuffle").on_hover_text("A different arrangement of knobs").clicked() {
                                p.seed = p.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) >> 1;
                            }
                            ui.weak(format!("#{}", p.seed % 10_000));
                        });
                        ui.end_row();
                    });
                    ui.weak(if self.place_at.is_some() { "Placed over the selected shape: engrave a photo there, then cut the puzzle." } else { "Tip: select a photo first to cut it into a puzzle." });
                }
                MakerTab::Hinge => {
                    egui::Grid::new("hinge-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Area width × height");
                        ui.horizontal(|ui| {
                            mm(ui, &mut self.hinge_size.0, 5.0..=2000.0);
                            mm(ui, &mut self.hinge_size.1, 5.0..=2000.0);
                        });
                        ui.end_row();
                        ui.label("Cut length");
                        mm(ui, &mut self.hinge_cut, 1.0..=500.0);
                        ui.end_row();
                        ui.label("Bridge between cuts");
                        mm(ui, &mut self.hinge_bridge, 0.5..=50.0);
                        ui.end_row();
                        ui.label("Line spacing");
                        mm(ui, &mut self.hinge_spacing, 0.3..=50.0).on_hover_text("Closer lines bend more easily but are weaker");
                        ui.end_row();
                    });
                    ui.weak("Cuts run up and down; the material bends left–right. Place the pattern over the part of a panel that should flex.");
                }
            }
            let result = self.parts(max_width);
            self.message = match &result {
                Ok(parts) => format!("{} part(s)", parts.len()),
                Err(e) => e.clone(),
            };
            ui.weak(&self.message);
            out = ok_cancel(ui, "Create", result.is_ok());
        });
        out
    }
}
// --- Warp and simplify (live preview) -------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShapeEdit {
    Warp { kind: bc_geom::warp::Warp, amount: f64 },
    Simplify { tolerance: f64, smooth: bool },
}

/// Edits the selected shapes live: the document shows the result while the dialog is open,
/// and Cancel puts the originals back.
pub struct ShapeEditDialog {
    pub edit: ShapeEdit,
    /// The shapes as they were when the dialog opened.
    pub originals: Vec<bc_doc::Shape>,
    /// The settings last applied to the document.
    pub applied: Option<ShapeEdit>,
    pub info: String,
}

impl ShapeEditDialog {
    pub fn new(edit: ShapeEdit, originals: Vec<bc_doc::Shape>) -> Self {
        Self { edit, originals, applied: None, info: String::new() }
    }

    /// The edited version of each original (id, new path).
    pub fn results(&self) -> Vec<(u64, bc_geom::BezPath)> {
        let area = self.originals.iter().filter_map(|s| s.bounds()).reduce(|a, b| a.union(b));
        self.originals
            .iter()
            .map(|s| {
                let path = s.world_path();
                let new = match self.edit {
                    ShapeEdit::Warp { kind, amount } => area.map(|a| bc_geom::warp::warp(&path, a, kind, amount)).unwrap_or(path),
                    ShapeEdit::Simplify { tolerance, smooth } => bc_geom::warp::simplify(&path, tolerance, smooth),
                };
                (s.id, new)
            })
            .collect()
    }

    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        let mut out = None;
        let title = match self.edit {
            ShapeEdit::Warp { .. } => "Warp",
            ShapeEdit::Simplify { .. } => "Simplify / Smooth",
        };
        window(ctx, title).show(ctx, |ui| {
            match &mut self.edit {
                ShapeEdit::Warp { kind, amount } => {
                    ui.horizontal(|ui| {
                        for k in bc_geom::warp::Warp::ALL {
                            ui.selectable_value(kind, k, k.label());
                        }
                    });
                    let mut pct = *amount * 100.0;
                    if ui.add(egui::Slider::new(&mut pct, -100.0..=100.0).suffix(" %").text("Amount")).changed() {
                        *amount = pct / 100.0;
                    }
                    ui.weak(match kind {
                        bc_geom::warp::Warp::Arch => "Bends the selection into an arch (negative sags).",
                        bc_geom::warp::Warp::Bulge => "Swells the middle sideways (negative pinches).",
                        bc_geom::warp::Warp::Taper => "Top wider than the bottom (negative: narrower): perspective, or tapered tumblers.",
                        bc_geom::warp::Warp::Wave => "A wave along the width.",
                        bc_geom::warp::Warp::Bridge => "The bottom edge bows up (negative: down); the top stays straight.",
                        bc_geom::warp::Warp::Flag => "A wave that grows towards the right, like a flag.",
                        bc_geom::warp::Warp::Barrel => "Swells out from the centre like a fisheye lens (negative pinches in).",
                        bc_geom::warp::Warp::TunaCan => "Wrapped round a can on its side; the amount is how far round it goes.",
                        bc_geom::warp::Warp::TitleCrawl => "Recedes towards the top, like film titles rolling away.",
                        bc_geom::warp::Warp::Rays => "Fans out from a point above (negative: the other way).",
                        bc_geom::warp::Warp::Rotate3d => "Turned about the upright axis, in perspective (negative: the other way).",
                    });
                }
                ShapeEdit::Simplify { tolerance, smooth } => {
                    ui.add(egui::Slider::new(tolerance, 0.01..=5.0).logarithmic(true).suffix(" mm").text("Tolerance"))
                        .on_hover_text("Points closer than this to a straight line are removed");
                    ui.checkbox(smooth, "Smooth into curves");
                }
            }
            ui.weak(&self.info);
            out = ok_cancel(ui, "OK", true);
        });
        out
    }
}
// --- Copy along path --------------------------------------------------------------

pub struct CopyAlongDialog {
    pub count: usize,
    pub rotate: bool,
}

impl Default for CopyAlongDialog {
    fn default() -> Self {
        Self { count: 8, rotate: true }
    }
}

impl CopyAlongDialog {
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        let mut out = None;
        window(ctx, "Copy Along Path").show(ctx, |ui| {
            ui.weak("Copies the selected objects evenly along the largest selected shape.");
            ui.horizontal(|ui| {
                ui.label("Copies");
                ui.add(egui::DragValue::new(&mut self.count).range(1..=1000));
            });
            ui.checkbox(&mut self.rotate, "Rotate copies to follow the path");
            out = ok_cancel(ui, "Copy", self.count > 0);
        });
        out
    }
}