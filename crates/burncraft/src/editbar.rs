//! The numeric edit bar: position, size, scale, rotation and anchor point of the selection,
//! plus text options when text is selected.

use crate::app::BurnCraftApp;
use crate::canvas::Tool;
use crate::icons::{self, Icon};
use crate::units::Units;
use bc_doc::{edit, ShapeKind, TextAlign, TextStyle, TextVAlign};
use bc_geom::{Affine, Point, Rect};
use eframe::egui;

/// Height of the edit bar's row: two rows of fields in a framed group.
const BAR_HEIGHT: f32 = 58.0;
/// Width of length fields in the bar.
const FIELD: f32 = 78.0;
/// Gap between the two rows of fields.
const ROW_GAP: f32 = 4.0;

/// Text parameters edited from the bar: font, height, bold, italic, style.
pub type TextParams = (String, f64, bool, bool, TextStyle);

/// The selection-relative point an anchor index (0..9, row-major from the top-left) refers to.
fn anchor_point(b: Rect, anchor: usize) -> Point {
    let f = [0.0, 0.5, 1.0];
    Point::new(b.x0 + b.width() * f[anchor % 3], b.y0 + b.height() * f[(anchor / 3).min(2)])
}

fn about(p: Point, t: Affine) -> Affine {
    Affine::translate(p.to_vec2()) * t * Affine::translate(-p.to_vec2())
}

impl BurnCraftApp {
    /// Converts a document Y to the displayed Y (counted up from the bed bottom when Y-up is on).
    pub(crate) fn display_y(&self, y: f64) -> f64 {
        if self.prefs.y_up { self.doc.bed_size.1 - y } else { y }
    }

    pub(crate) fn edit_bar(&mut self, ui: &mut egui::Ui) {
        let u = self.prefs.units;
        let bounds = self.doc.bounds_of(self.selection.iter().copied());
        let enabled = bounds.is_some();
        let b = bounds.unwrap_or(Rect::ZERO);
        let anchor = anchor_point(b, self.prefs.anchor);
        let now = ui.input(|i| i.time);
        // A fixed-height row with everything centred in it. (A plain horizontal row centres each
        // item against the row's height so far, so the first groups sat higher than later ones.)
        let layout = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), BAR_HEIGHT), layout, |ui| {
            ui.set_min_height(BAR_HEIGHT);
            ui.spacing_mut().item_spacing.x = 6.0;
            group_top(ui, |ui| {
                if !enabled {
                    ui.disable();
                }
                {
                    // Position.
                    let (mut x, mut y) = (anchor.x, self.display_y(anchor.y));
                    let mut moved = false;
                    egui::Grid::new("pos").num_columns(2).spacing([4.0, ROW_GAP]).show(ui, |ui| {
                        ui.label("XPos");
                        moved |= u.spin(ui, &mut x, -10000.0..=10000.0, FIELD);
                        ui.end_row();
                        ui.label("YPos");
                        moved |= u.spin(ui, &mut y, -10000.0..=10000.0, FIELD);
                        ui.end_row();
                    });
                    if moved {
                        let to = Point::new(x, self.display_y(y));
                        self.checkpoint_merged("Move", now);
                        edit::translate(&mut self.doc, &self.selection, to - anchor);
                    }
                    // Lock aspect.
                    let lock_icon = if self.lock_aspect { Icon::Lock } else { Icon::Unlocked };
                    let gap = (group_height(ui) - 30.0) / 2.0;
                    ui.vertical(|ui| {
                        ui.add_space(gap);
                        if icons::button(ui, lock_icon, 30.0, self.lock_aspect, true, "Lock aspect ratio").clicked() {
                            self.lock_aspect = !self.lock_aspect;
                        }
                    });
                    // Size and percentage.
                    let (mut w, mut h) = (b.width(), b.height());
                    let (mut pw, mut ph) = (100.0f64, 100.0f64);
                    let (mut wc, mut hc, mut pc_w, mut pc_h) = (false, false, false, false);
                    egui::Grid::new("size").num_columns(3).spacing([4.0, ROW_GAP]).show(ui, |ui| {
                        ui.label("Width");
                        wc = u.spin(ui, &mut w, 0.01..=10000.0, FIELD);
                        pc_w = crate::units::spin(ui, &mut pw, 1.0..=10000.0, 1.0, 0.5, 1, " %", 58.0);
                        ui.end_row();
                        ui.label("Height");
                        hc = u.spin(ui, &mut h, 0.01..=10000.0, FIELD);
                        pc_h = crate::units::spin(ui, &mut ph, 1.0..=10000.0, 1.0, 0.5, 1, " %", 58.0);
                        ui.end_row();
                    });
                    let (mut sx, mut sy) = (1.0, 1.0);
                    if wc && b.width() > 1e-9 {
                        sx = w / b.width();
                        sy = if self.lock_aspect { sx } else { 1.0 };
                    }
                    if hc && b.height() > 1e-9 {
                        sy = h / b.height();
                        sx = if self.lock_aspect { sy } else { 1.0 };
                    }
                    if pc_w {
                        sx = pw / 100.0;
                        sy = if self.lock_aspect { sx } else { 1.0 };
                    }
                    if pc_h {
                        sy = ph / 100.0;
                        sx = if self.lock_aspect { sy } else { 1.0 };
                    }
                    if (sx - 1.0).abs() > 1e-12 || (sy - 1.0).abs() > 1e-12 {
                        self.checkpoint_merged("Resize", now);
                        edit::transform(&mut self.doc, &self.selection, about(anchor, Affine::scale_non_uniform(sx, sy)));
                    }
                }
            });
            // Anchor point (works with or without a selection), in its own box: a 3 × 3 block of dots.
            group(ui, |ui| {
                const CELL: f32 = 11.0;
                const GAP: f32 = 4.0;
                let side = 3.0 * CELL + 2.0 * GAP;
                let (block, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
                let ink = ui.visuals().strong_text_color();
                for i in 0..9 {
                    let (row, col) = ((i / 3) as f32, (i % 3) as f32);
                    let cell = egui::Rect::from_min_size(block.min + egui::vec2(col * (CELL + GAP), row * (CELL + GAP)), egui::vec2(CELL, CELL));
                    let r = ui.interact(cell, ui.id().with(("anchor-dot", i)), egui::Sense::click());
                    if i == self.prefs.anchor {
                        ui.painter().circle_filled(cell.center(), 4.0, ui.visuals().selection.stroke.color);
                    } else {
                        let stroke = if r.hovered() { ink } else { ink.gamma_multiply(0.6) };
                        ui.painter().circle_stroke(cell.center(), 3.5, egui::Stroke::new(1.0, stroke));
                    }
                    if r.on_hover_text("XPos/YPos, resizing and rotation use this point of the selection").clicked() {
                        self.prefs.anchor = i;
                    }
                }
            });
            group(ui, |ui| {
                {
                    ui.add_enabled(enabled, egui::Label::new("Rotate"));
                    let h = ui.spacing().interact_size.y;
                    let field = egui::DragValue::new(&mut self.rotate_field).range(-360.0..=360.0).speed(0.5).suffix("°");
                    let r = ui
                        .add_enabled_ui(enabled, |ui| ui.add_sized([56.0, h], field))
                        .inner
                        .on_hover_text("Type an angle and press Enter to turn the selection (positive = anticlockwise)");
                    let apply = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if apply && self.rotate_field != 0.0 {
                        // Positive = counter-clockwise on screen, like the Y-up angle convention.
                        let angle = -self.rotate_field.to_radians();
                        self.checkpoint_named("Rotate");
                        edit::transform(&mut self.doc, &self.selection, about(anchor, Affine::rotate(angle)));
                        self.rotate_field = 0.0;
                    }
                }
                let units_label = match u {
                    Units::Millimetres => "mm",
                    Units::Inches => "in",
                };
                if ui.add(egui::Button::new(units_label).min_size(egui::vec2(34.0, 22.0))).on_hover_text("Switch between millimetres and inches").clicked() {
                    self.prefs.units = if u == Units::Millimetres { Units::Inches } else { Units::Millimetres };
                }
            });
            // What the selection handles may do: switch off to protect against an accidental drag.
            group_top(ui, |ui| {
                egui::Grid::new("handle-ops").num_columns(2).spacing([8.0, ROW_GAP]).show(ui, |ui| {
                    let p = &mut self.prefs;
                    ui.checkbox(&mut p.allow_move, "Move").on_hover_text("Dragging the selection moves it");
                    ui.checkbox(&mut p.allow_size, "Size").on_hover_text("The corner and side handles resize");
                    ui.end_row();
                    ui.checkbox(&mut p.allow_rotate, "Rotate").on_hover_text("The rotate handles turn the selection");
                    ui.checkbox(&mut p.allow_shear, "Shear").on_hover_text("The shear handles slant the selection");
                    ui.end_row();
                });
            });
            self.text_bar(ui);
        });
    }

    /// The current text parameters: from the selected text, or the defaults for new text.
    fn current_text(&self) -> (bool, TextParams) {
        let selected = self.doc.shapes.iter().find(|s| self.selection.contains(&s.id) && matches!(s.kind, ShapeKind::Text { .. }));
        match selected.map(|s| &s.kind) {
            Some(ShapeKind::Text { font, height, bold, italic, style, .. }) => (true, (font.clone(), *height, *bold, *italic, style.clone())),
            _ => (false, self.last_text.clone()),
        }
    }

    /// Applies new text parameters to every selected text shape (or to the defaults for new text).
    fn set_text(&mut self, params: TextParams, now: f64) {
        let targets: Vec<u64> = self.doc.shapes.iter().filter(|s| self.selection.contains(&s.id) && matches!(s.kind, ShapeKind::Text { .. })).map(|s| s.id).collect();
        self.last_text = params.clone();
        if targets.is_empty() {
            return;
        }
        let (font, height, bold, italic, style) = params;
        let mut updates = Vec::new();
        for id in &targets {
            if let Some(ShapeKind::Text { text, style: own, .. }) = self.doc.shape(*id).map(|s| &s.kind) {
                // Each text keeps its own curve (circle or path); the rest of the style is shared.
                let style = bc_doc::TextStyle { curve: own.curve.clone(), ..style.clone() };
                match self.fonts.outline(text, &font, height, bold, italic, &style) {
                    Some(outline) => updates.push((*id, ShapeKind::Text { text: text.clone(), font: font.clone(), height, bold, italic, style, outline })),
                    None => self.status = format!("Font \"{font}\" can't render this text"),
                }
            }
        }
        if !updates.is_empty() {
            self.checkpoint_merged("Text style", now);
            for (id, kind) in updates {
                edit::replace_text(&mut self.doc, id, kind);
            }
        }
    }

    fn text_bar(&mut self, ui: &mut egui::Ui) {
        let (has_text, mut p) = self.current_text();
        let active = has_text || self.canvas.tool == Tool::Text;
        let before = p.clone();
        group_top(ui, |ui| {
            if !active {
                ui.disable();
            }
            {
                egui::Grid::new("text-bar").num_columns(8).spacing([4.0, ROW_GAP]).show(ui, |ui| {
                    // Row 1: font, size, letter spacing, horizontal alignment.
                    ui.label("Font");
                    egui::ComboBox::from_id_salt("bar-font").selected_text(&p.0).width(210.0).height(400.0).show_ui(ui, |ui| {
                        for family in self.fonts.families() {
                            ui.selectable_value(&mut p.0, family.clone(), family);
                        }
                    });
                    ui.label("Height");
                    self.prefs.units.spin(ui, &mut p.1, 0.5..=500.0, 70.0);
                    ui.label("HSpace").on_hover_text("Extra space between letters");
                    crate::units::spin(ui, &mut p.4.letter_spacing, -10.0..=50.0, 0.1, 0.05, 2, " mm", 60.0);
                    ui.label("Align X").on_hover_text("Line alignment, and which side stays put when the text changes");
                    egui::ComboBox::from_id_salt("bar-align").selected_text(match p.4.align {
                        TextAlign::Left => "Left",
                        TextAlign::Center => "Middle",
                        TextAlign::Right => "Right",
                    }).width(72.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut p.4.align, TextAlign::Left, "Left");
                        ui.selectable_value(&mut p.4.align, TextAlign::Center, "Middle");
                        ui.selectable_value(&mut p.4.align, TextAlign::Right, "Right");
                    });
                    ui.end_row();
                    // Row 2: style switches, line spacing, vertical alignment.
                    ui.label("");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut p.2, "Bold");
                        ui.checkbox(&mut p.3, "Italic");
                        ui.checkbox(&mut p.4.upper_case, "Upper Case");
                    });
                    ui.label("");
                    ui.checkbox(&mut p.4.weld, "Welded").on_hover_text("Join overlapping letters into one outline (script fonts)");
                    ui.label("VSpace").on_hover_text("Line spacing, as a multiple of the font's normal spacing");
                    crate::units::spin(ui, &mut p.4.line_spacing, 0.3..=5.0, 0.1, 0.01, 2, "×", 60.0);
                    ui.label("Align Y").on_hover_text("Which edge stays put when the text changes (lines added, size changed)");
                    egui::ComboBox::from_id_salt("bar-valign").selected_text(match p.4.valign {
                        TextVAlign::Top => "Top",
                        TextVAlign::Middle => "Middle",
                        TextVAlign::Bottom => "Bottom",
                    }).width(72.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut p.4.valign, TextVAlign::Top, "Top");
                        ui.selectable_value(&mut p.4.valign, TextVAlign::Middle, "Middle");
                        ui.selectable_value(&mut p.4.valign, TextVAlign::Bottom, "Bottom");
                    });
                    ui.end_row();
                });
            }
        });
        if p != before {
            let now = ui.input(|i| i.time);
            self.set_text(p, now);
        }
    }
}

/// A subtly framed group of controls in the bar, its contents centred vertically.
fn group(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    framed(ui, egui::Align::Center, add);
}

/// A framed group whose contents start at the top: for side-by-side two-row grids, which only
/// line up when they start on the same line.
fn group_top(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    framed(ui, egui::Align::Min, add);
}

/// Height of the inside of a group: two rows of fields.
fn group_height(ui: &egui::Ui) -> f32 {
    2.0 * ui.spacing().interact_size.y + ROW_GAP
}

fn framed(ui: &mut egui::Ui, align: egui::Align, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            // Same height for every box (see the note in `edit_bar`).
            let height = group_height(ui);
            let layout = egui::Layout::left_to_right(align);
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), layout, |ui| {
                ui.set_min_height(height);
                ui.spacing_mut().item_spacing.x = 6.0;
                add(ui);
            });
        });
}
