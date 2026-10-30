//! Right-hand panels (Cuts / Layers and Laser sections), the colour palette, and the Cut Settings Editor.

use crate::app::BurnCraftApp;
use crate::icons::Icon;
use bc_doc::{DitherMode, ImageAdjust, Layer, LayerMode, ShapeKind};
use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

/// A saved set of cut settings (for a material and operation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub settings: Layer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutsTab {
    Layers,
    Move,
    Console,
    /// How to use the current tool, plus general tips.
    Tips,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaserTab {
    Laser,
    Library,
    Art,
    Shape,
    Variables,
    Camera,
    History,
}

fn swatch(ui: &mut egui::Ui, index: usize, rgb: [u8; 3], size: f32) -> egui::Response {
    faded_swatch(ui, index, rgb, size, 1.0)
}

/// A layer colour swatch drawn at `opacity` (faded for layers with Output off).
fn faded_swatch(ui: &mut egui::Ui, index: usize, [r, g, b]: [u8; 3], size: f32, opacity: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size * 1.6, size), egui::Sense::click());
    let fill = Color32::from_rgb(r, g, b).gamma_multiply(opacity);
    ui.painter().rect_filled(rect, 2.0, fill);
    let text = if (r as u32 + g as u32 + b as u32) > 380 { Color32::BLACK } else { Color32::WHITE };
    let label = bc_doc::layer_label(index);
    // Longer labels (100-255, Tool1) a little smaller, to stay inside the swatch.
    let scale = match label.len() {
        0..=2 => 0.7,
        3 => 0.6,
        _ => 0.42,
    };
    let font = egui::FontId::proportional(size * scale);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, font, text.gamma_multiply(opacity.max(0.5)));
    response
}

/// The Envelope's handle grid over a preview of the shape: drag a handle to bend the shape.
fn envelope_editor(ui: &mut egui::Ui, before: &bc_geom::BezPath, cols: u32, rows: u32, moves: &mut [(f64, f64)]) {
    let Some(b) = bc_geom::bounds(before).filter(|b| b.width() > 0.0 && b.height() > 0.0) else { return };
    // The preview keeps the shape's proportions, with room round it for handles dragged outside.
    const ROOM: f32 = 32.0;
    let ratio = (b.height() / b.width()) as f32;
    let max_w = ui.available_width().min(260.0) - 2.0 * ROOM;
    let inner_h = (max_w * ratio).clamp(40.0, 160.0);
    let inner_w = (inner_h / ratio).min(max_w);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(inner_w + 2.0 * ROOM, inner_h + 2.0 * ROOM), egui::Sense::hover());
    let inner = rect.shrink(ROOM);
    let to_screen = |u: f64, v: f64| egui::pos2(inner.left() + u as f32 * inner.width(), inner.top() + v as f32 * inner.height());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    let bent = bc_doc::effects::Effect::Envelope { cols, rows, moves: moves.to_vec() }.apply(before.clone());
    let ink = ui.visuals().text_color();
    for line in bc_geom::flatten(&bent, b.width().max(b.height()) / 300.0) {
        let pts: Vec<egui::Pos2> = line.points.iter().map(|p| to_screen((p.x - b.x0) / b.width(), (p.y - b.y0) / b.height())).collect();
        painter.add(if line.closed { egui::Shape::closed_line(pts, egui::Stroke::new(1.2, ink)) } else { egui::Shape::line(pts, egui::Stroke::new(1.2, ink)) });
    }
    let accent = ui.visuals().selection.stroke.color;
    let at = |i: u32, j: u32, m: &[(f64, f64)]| {
        let (mx, my) = m[(j * (cols + 1) + i) as usize];
        to_screen(i as f64 / cols as f64 + mx, j as f64 / rows as f64 + my)
    };
    for j in 0..=rows {
        for i in 0..=cols {
            if i < cols {
                painter.line_segment([at(i, j, moves), at(i + 1, j, moves)], egui::Stroke::new(1.0, accent.gamma_multiply(0.5)));
            }
            if j < rows {
                painter.line_segment([at(i, j, moves), at(i, j + 1, moves)], egui::Stroke::new(1.0, accent.gamma_multiply(0.5)));
            }
        }
    }
    for j in 0..=rows {
        for i in 0..=cols {
            let k = (j * (cols + 1) + i) as usize;
            let c = at(i, j, moves);
            let handle = ui.interact(egui::Rect::from_center_size(c, egui::vec2(12.0, 12.0)), ui.id().with(("envelope-handle", k)), egui::Sense::drag());
            if handle.dragged() {
                let d = handle.drag_delta();
                // Kept inside the box, so a handle can't be dragged out of reach.
                let edge = rect.shrink(6.0);
                let (home_u, home_v) = (i as f64 / cols as f64, j as f64 / rows as f64);
                let u = (home_u + moves[k].0 + (d.x / inner.width()) as f64).clamp(((edge.left() - inner.left()) / inner.width()) as f64, ((edge.right() - inner.left()) / inner.width()) as f64);
                let v = (home_v + moves[k].1 + (d.y / inner.height()) as f64).clamp(((edge.top() - inner.top()) / inner.height()) as f64, ((edge.bottom() - inner.top()) / inner.height()) as f64);
                moves[k] = (u - home_u, v - home_v);
            }
            let fill = if handle.hovered() || handle.dragged() { accent } else { Color32::WHITE };
            painter.rect(egui::Rect::from_center_size(c, egui::vec2(8.0, 8.0)), 1.0, fill, egui::Stroke::new(1.0, accent), egui::StrokeKind::Middle);
        }
    }
}

/// A section title bar like a dock header.
fn section_header(ui: &mut egui::Ui, title: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::hover());
    // A deep blue in the light theme, where the selection colour is too pale under white text.
    let accent = if ui.visuals().dark_mode { ui.visuals().selection.bg_fill.gamma_multiply(0.85) } else { Color32::from_rgb(40, 115, 190) };
    ui.painter().rect_filled(rect, 0.0, accent);
    ui.painter().text(rect.left_center() + egui::vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(13.0), Color32::WHITE);
}

/// A button on the side ribbon: an icon over a short label, marked with a bar when selected.
/// `height` shrinks the button for short windows; under 44 px only the icon is shown.
fn ribbon_button(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool, height: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click());
    let v = ui.visuals();
    let bg = if selected {
        v.selection.bg_fill.gamma_multiply(0.35)
    } else if response.hovered() {
        v.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect.shrink2(egui::vec2(3.0, 0.0)), 4.0, bg);
    if selected {
        let bar = egui::Rect::from_min_size(rect.left_top() + egui::vec2(0.0, 8.0), egui::vec2(3.0, rect.height() - 16.0));
        ui.painter().rect_filled(bar, 1.5, v.selection.stroke.color);
    }
    let ink = if selected { v.strong_text_color() } else { v.text_color() };
    let labelled = height >= 44.0;
    let icon_size = (height - 10.0).min(22.0);
    let icon_y = if labelled { rect.top() + 18.0 } else { rect.center().y };
    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.center().x, icon_y), egui::vec2(icon_size, icon_size));
    crate::icons::paint(ui.painter(), icon_rect, icon, ink, v.selection.stroke.color);
    if labelled {
        ui.painter().text(egui::pos2(rect.center().x, rect.bottom() - 9.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(11.0), ink);
    }
    response
}

impl BurnCraftApp {
    pub(crate) fn palette(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
            // Only the colour layers in use: the tool layers (Tool1, Tool2) are used by features such
            // as dimensions and jig slots, and are never sent to the laser.
            let mut shown = self.doc.palette_layers();
            if self.current_layer < bc_doc::CUT_LAYER_COUNT && !shown.contains(&self.current_layer) {
                shown.push(self.current_layer);
                shown.sort_unstable();
            }
            let used = self.doc.used_layers();
            for i in shown {
                let r = swatch(ui, i, self.doc.layers[i].rgb(i), 20.0);
                if i == self.current_layer {
                    ui.painter().rect_stroke(r.rect.expand(1.5), 2.0, egui::Stroke::new(2.0, ui.visuals().strong_text_color()), egui::StrokeKind::Outside);
                }
                let r = r.on_hover_text("Put the selection on this layer, and draw new shapes on it. Right-click for its colour.");
                if r.clicked() {
                    self.assign_layer(i);
                }
                r.context_menu(|ui| {
                    ui.label(format!("Layer {} colour", bc_doc::layer_label(i)));
                    self.layer_color_picker(ui, i);
                    if self.doc.layers[i].custom_color.is_some() && ui.button("Standard colour").clicked() {
                        self.set_layer_color(i, None);
                        ui.close();
                    }
                    ui.separator();
                    self.remove_layer_button(ui, i, used.contains(&i));
                });
            }
            let add = ui.add_enabled(self.doc.next_free_layer().is_some(), egui::Button::new("+").min_size(egui::vec2(32.0, 20.0)));
            if add.on_hover_text("Add a layer in a new colour").on_disabled_hover_text(format!("All {} layers are in use", bc_doc::CUT_LAYER_COUNT)).clicked() {
                self.add_layer();
            }
        });
    }

    /// Adds the next free layer to the palette, in a colour unlike the others, and makes it current.
    pub(crate) fn add_layer(&mut self) {
        let Some(i) = self.doc.next_free_layer() else { return };
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0);
        self.checkpoint_named("Add layer");
        let color = self.doc.fresh_layer_color(seed);
        let layer = &mut self.doc.layers[i];
        *layer = Layer::new(i);
        layer.added = true;
        layer.custom_color = Some(color);
        self.current_layer = i;
    }

    /// The "Remove layer" menu item for layer `i`: removes it straight away when it's empty, or
    /// asks what to do with its shapes.
    fn remove_layer_button(&mut self, ui: &mut egui::Ui, i: usize, has_shapes: bool) {
        let shown = self.doc.palette_layers();
        let can = shown.len() > 1 || !shown.contains(&i);
        let label = if has_shapes { "Remove layer…" } else { "Remove layer" };
        if ui.add_enabled(can, egui::Button::new(label)).on_disabled_hover_text("A design always has at least one layer").clicked() {
            if has_shapes {
                let other = shown.iter().copied().find(|&j| j != i);
                self.removing_layer = Some((i, other));
            } else {
                self.remove_layer(i, None);
            }
            ui.close();
        }
    }

    /// Removes layer `i` (one undo step), moving its shapes to `move_to` or deleting them.
    fn remove_layer(&mut self, i: usize, move_to: Option<usize>) {
        self.checkpoint_named(&format!("Remove layer {}", bc_doc::layer_label(i)));
        let n = self.doc.remove_layer(i, move_to);
        self.selection.retain(|id| self.doc.shape(*id).is_some());
        let shown = self.doc.palette_layers();
        if !shown.contains(&self.current_layer) {
            self.current_layer = move_to.unwrap_or(shown[0]);
        }
        self.status = match (n, move_to) {
            (0, _) => format!("Removed layer {}", bc_doc::layer_label(i)),
            (n, Some(to)) => format!("Removed layer {}; moved its {n} shape(s) to layer {}", bc_doc::layer_label(i), bc_doc::layer_label(to)),
            (n, None) => format!("Removed layer {} and its {n} shape(s)", bc_doc::layer_label(i)),
        };
    }

    /// Asks what to do with the shapes on a layer being removed.
    pub(crate) fn remove_layer_window(&mut self, ctx: &egui::Context) {
        let Some((i, mut move_to)) = self.removing_layer else { return };
        let count = self.doc.shapes.iter().filter(|s| s.layer == i).count();
        let others: Vec<usize> = self.doc.palette_layers().into_iter().filter(|&j| j != i).collect();
        let (mut open, mut done) = (true, None);
        crate::dialogs::window(ctx, &format!("Remove layer {}", bc_doc::layer_label(i))).id(egui::Id::new("remove-layer")).open(&mut open).show(ctx, |ui| {
            ui.label(format!("Layer {} has {count} shape(s) on it. What should happen to them?", bc_doc::layer_label(i)));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let moving = move_to.is_some();
                if ui.add_enabled(!others.is_empty(), egui::RadioButton::new(moving, "Move them to layer")).clicked() && !moving {
                    move_to = others.first().copied();
                }
                if let Some(to) = move_to.as_mut() {
                    egui::ComboBox::from_id_salt("remove-layer-to").selected_text(bc_doc::layer_label(*to)).width(60.0).show_ui(ui, |ui| {
                        for &j in &others {
                            ui.selectable_value(to, j, bc_doc::layer_label(j));
                        }
                    });
                }
            });
            if ui.radio(move_to.is_none(), "Delete them too").clicked() {
                move_to = None;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Remove layer").clicked() {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => {
                self.removing_layer = None;
                self.remove_layer(i, move_to);
            }
            Some(false) => self.removing_layer = None,
            None => self.removing_layer = open.then_some((i, move_to)),
        }
    }

    /// A colour picker for layer `i`; dragging in it is one undo step.
    fn layer_color_picker(&mut self, ui: &mut egui::Ui, i: usize) {
        let [r, g, b] = self.doc.layers[i].rgb(i);
        let mut c = Color32::from_rgb(r, g, b);
        if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
            let now = ui.input(|i| i.time);
            self.checkpoint_merged("Layer colour", now);
            self.doc.layers[i].custom_color = Some([c.r(), c.g(), c.b()]);
        }
    }

    /// Sets (or with None, resets to standard) a layer's colour, as one undo step.
    fn set_layer_color(&mut self, i: usize, color: Option<[u8; 3]>) {
        if self.doc.layers[i].custom_color != color {
            self.checkpoint_named("Layer colour");
            self.doc.layers[i].custom_color = color;
        }
    }

    /// The right-hand side: Cuts / Layers on top, Laser below, each with tabs.
    pub(crate) fn right_panels(&mut self, ui: &mut egui::Ui, bed: (f64, f64)) -> (Option<crate::laser::LaserAction>, Option<crate::camera::CameraAction>) {
        let (mut laser, mut camera) = (None, None);
        // The ribbon down the right edge picks what the two panels show: the top group the upper
        // panel, the bottom group the lower one.
        egui::Panel::right("ribbon").resizable(false).exact_size(64.0).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let top_group = [
                (CutsTab::Layers, Icon::Layers, "Layers", "Cuts / Layers: the layer list and settings"),
                (CutsTab::Move, Icon::Move, "Move", "Move the laser head"),
                (CutsTab::Console, Icon::Console, "Console", "Send commands and see the controller's replies"),
                (CutsTab::Tips, Icon::Tips, "Tips", "How to use the current tool"),
            ];
            let bottom_group = [
                (LaserTab::Laser, Icon::Laser, "Laser", "Run, frame and home the laser"),
                (LaserTab::Library, Icon::Library, "Library", "Material Library: saved cut settings"),
                (LaserTab::Art, Icon::Art, "Art", "Ready-made art to add"),
                (LaserTab::Shape, Icon::Shape, "Shape", "The selected shape's settings"),
                (LaserTab::Variables, Icon::Variables, "Variables", "Variable text: serial numbers, dates, merged data"),
                (LaserTab::Camera, Icon::Camera, "Camera", "Camera over the bed"),
                (LaserTab::History, Icon::History, "History", "Every step; click one to go back to it"),
            ];
            // 11 buttons and the divider: shrink them to fit a short window (icons only below
            // 44 px).
            let room = ui.available_height() - 30.0;
            let h = (room / 11.0 - 2.0).clamp(30.0, 52.0);
            let tip = |label: &str, tip: &str| if h < 44.0 { format!("{label}: {tip}") } else { tip.to_string() };
            let mut cuts_tab = self.cuts_tab;
            let mut laser_tab = self.laser_tab;
            let mut top = |ui: &mut egui::Ui| {
                ui.add_space(4.0);
                for (tab, icon, label, t) in top_group {
                    if ribbon_button(ui, icon, label, cuts_tab == tab, h).on_hover_text(tip(label, t)).clicked() {
                        cuts_tab = tab;
                    }
                }
            };
            if room >= 11.0 * (h + 2.0) {
                // The top group at the top, beside the upper panel; the bottom group anchored to
                // the bottom of the window, beside the lower panel.
                top(ui);
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(4.0);
                    for (tab, icon, label, t) in bottom_group.into_iter().rev() {
                        if ribbon_button(ui, icon, label, laser_tab == tab, h).on_hover_text(tip(label, t)).clicked() {
                            laser_tab = tab;
                        }
                    }
                    ui.add_space(4.0);
                    ui.separator();
                });
            } else {
                // Too short even for the smallest buttons: one scrolling column.
                egui::ScrollArea::vertical().id_salt("ribbon-scroll").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                    top(ui);
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                    for (tab, icon, label, t) in bottom_group {
                        if ribbon_button(ui, icon, label, laser_tab == tab, h).on_hover_text(tip(label, t)).clicked() {
                            laser_tab = tab;
                        }
                    }
                });
            }
            self.cuts_tab = cuts_tab;
            self.laser_tab = laser_tab;
        });
        egui::Panel::right("panels").default_size(420.0).min_size(330.0).show(ui, |ui| {
            let half = (ui.available_height() - 30.0) / 2.0;
            let cuts_title = match self.cuts_tab {
                CutsTab::Layers => "Cuts / Layers",
                CutsTab::Move => "Move",
                CutsTab::Console => "Console",
                CutsTab::Tips => "Tips",
            };
            section_header(ui, cuts_title);
            ui.allocate_ui(egui::vec2(ui.available_width(), half), |ui| {
                ui.set_min_height(half);
                // The layer colours sit above the layer list, outside its scrolling.
                let top = ui.cursor().top();
                if self.cuts_tab == CutsTab::Layers {
                    self.palette(ui);
                    ui.separator();
                }
                let list_height = (half - (ui.cursor().top() - top)).max(40.0);
                egui::ScrollArea::vertical().id_salt("cuts-scroll").max_height(list_height).show(ui, |ui| match self.cuts_tab {
                    CutsTab::Layers => self.layers_panel(ui),
                    CutsTab::Move => {
                        let (u, y_up) = (self.prefs.units, self.prefs.y_up);
                        self.laser.move_panel(ui, bed, u, y_up);
                    }
                    CutsTab::Console => self.laser.console_panel(ui),
                    CutsTab::Tips => self.tips_panel(ui),
                });
            });
            ui.separator();
            section_header(
                ui,
                match self.laser_tab {
                    LaserTab::Laser => "Laser",
                    LaserTab::Library => "Material Library",
                    LaserTab::Art => "Art",
                    LaserTab::Shape => "Shape",
                    LaserTab::Variables => "Variables",
                    LaserTab::Camera => "Camera",
                    LaserTab::History => "History",
                },
            );
            let rest = ui.available_height();
            egui::ScrollArea::vertical().id_salt("laser-scroll").max_height(rest).show(ui, |ui| match self.laser_tab {
                LaserTab::Laser => {
                    let (a, devices) = self.laser.panel(ui);
                    laser = a;
                    if devices {
                        self.show_devices = true;
                    }
                }
                LaserTab::Library => self.library_panel(ui),
                LaserTab::Art => self.art_panel(ui),
                LaserTab::Shape => self.properties_panel(ui),
                LaserTab::Variables => self.variables_panel(ui),
                LaserTab::Camera => camera = self.camera.panel(ui, bed, self.laser.device.connected, self.laser.speed(self.prefs.units)),
                LaserTab::History => self.history_panel(ui),
            });
        });
        (laser, camera)
    }

    /// The Tips tab: how to use the tool in hand, then tips that apply everywhere.
    fn tips_panel(&mut self, ui: &mut egui::Ui) {
        let tool = self.canvas.tool;
        ui.horizontal(|ui| {
            crate::icons::button(ui, tool.icon(), 22.0, false, true, tool.label());
            ui.strong(format!("{} tool", tool.label()));
        });
        let bullet = |ui: &mut egui::Ui, text: &str| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label("•");
                ui.label(text);
            });
        };
        for tip in tool.tips() {
            bullet(ui, tip);
        }
        ui.add_space(6.0);
        ui.strong("Anywhere");
        for tip in [
            "Mouse wheel zooms; drag with the middle or right mouse button to pan.",
            "Esc goes back to the Select tool; Delete removes the selection.",
            "Click a palette colour to put the selection on that layer.",
            "Ctrl+Z undoes, Ctrl+Y redoes; the History tab lists every step.",
            "Alt+P previews the job before you run it.",
        ] {
            bullet(ui, tip);
        }
    }

    /// The layer table: one row per layer in use or added, plus quick settings for the current layer.
    pub(crate) fn layers_panel(&mut self, ui: &mut egui::Ui) {
        let speed = self.laser.speed(self.prefs.units);
        let mut used = self.doc.used_layers();
        used.extend((0..bc_doc::CUT_LAYER_COUNT).filter(|&i| self.doc.layers[i].added));
        used.sort_unstable();
        used.dedup();
        if used.is_empty() {
            ui.weak("No shapes yet. Draw or import something; click a colour above to set its layer, or + to add one.");
            return;
        }
        let mut layers = self.doc.layers.clone();
        let revision = self.revision;
        let mut open_editor = None;
        egui::Grid::new("layers").num_columns(7).striped(true).spacing([6.0, 3.0]).show(ui, |ui| {
            let speed_power = if speed.depth { "Dpt/Pwr" } else { "Spd/Pwr" };
            for h in ["#", "Layer", "Mode", speed_power, "Output", "Show", "Air"] {
                ui.strong(h);
            }
            ui.end_row();
            for &i in &used {
                let selected = i == self.current_layer;
                let name = ui.selectable_label(selected, &layers[i].name).on_hover_text("Right-click to remove this layer");
                if name.clicked() {
                    self.current_layer = i;
                }
                if !bc_doc::is_tool_layer(i) {
                    let has_shapes = self.doc.shapes.iter().any(|s| s.layer == i);
                    name.context_menu(|ui| self.remove_layer_button(ui, i, has_shapes));
                }
                if name.double_clicked() {
                    open_editor = Some(i);
                }
                let mut sw = faded_swatch(ui, i, layers[i].rgb(i), 16.0, if layers[i].output { 1.0 } else { 0.3 });
                if !layers[i].output {
                    sw = sw.on_hover_text("Output is off: this layer won't be sent to the laser");
                }
                if sw.double_clicked() {
                    open_editor = Some(i);
                }
                let l = &mut layers[i];
                egui::ComboBox::from_id_salt(("mode", i)).selected_text(l.mode.label()).width(80.0).show_ui(ui, |ui| {
                    for m in LayerMode::ALL {
                        ui.selectable_value(&mut l.mode, m, m.label());
                    }
                });
                if ui.selectable_label(selected, format!("{} / {:.0}", speed.number(l.speed), l.max_power)).on_hover_text("Double-click to edit").double_clicked() {
                    open_editor = Some(i);
                }
                if bc_doc::is_tool_layer(i) {
                    l.output = false;
                    ui.add_enabled(false, egui::Checkbox::without_text(&mut l.output)).on_disabled_hover_text("Tool layer: shown on the design but never sent to the laser");
                } else {
                    ui.checkbox(&mut l.output, "");
                }
                ui.checkbox(&mut l.visible, "");
                ui.checkbox(&mut l.air_assist, "");
                ui.end_row();
            }
        });
        if self.revision != revision {
            // A layer was removed from the list's menu: these copies of the settings are stale.
            return;
        }
        // Quick settings for the current layer.
        let cur = if used.contains(&self.current_layer) { self.current_layer } else { used[0] };
        ui.separator();
        egui::Grid::new("layer-quick").num_columns(2).spacing([8.0, 3.0]).show(ui, |ui| {
            let l = &mut layers[cur];
            ui.label("Layer Color");
            ui.horizontal(|ui| {
                let [r, g, b] = l.rgb(cur);
                let mut c = [r, g, b];
                if ui.color_edit_button_srgb(&mut c).on_hover_text("Choose this layer's colour").changed() {
                    l.custom_color = Some(c);
                }
                if l.custom_color.is_some() && ui.small_button("Standard").on_hover_text("Back to the layer's standard colour").clicked() {
                    l.custom_color = None;
                }
            });
            ui.end_row();
            ui.label(speed.label());
            ui.add(speed.edit(&mut l.speed, 0.1..=5000.0));
            ui.end_row();
            ui.label("Power (min / max)");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut l.min_power).range(0.0..=100.0).speed(0.5).suffix(" %"));
                ui.add(egui::DragValue::new(&mut l.max_power).range(0.0..=100.0).speed(0.5).suffix(" %"));
            });
            ui.end_row();
            ui.label("Pass Count");
            ui.add(egui::DragValue::new(&mut l.passes).range(1..=100));
            ui.end_row();
            ui.label("Interval");
            ui.add_enabled(l.mode != LayerMode::Line, egui::DragValue::new(&mut l.interval).range(0.01..=2.0).speed(0.005).suffix(" mm"));
            ui.end_row();
        });
        if ui.button("Edit cut settings…").clicked() {
            open_editor = Some(cur);
        }
        if let Some(i) = open_editor {
            self.current_layer = i;
            self.cut_editor = Some(i);
        }
        let now = ui.input(|i| i.time);
        self.commit_layers(now, layers);
    }

    /// Records changed layer settings as one (merged) undo step. `now` is the UI time in seconds.
    fn commit_layers(&mut self, now: f64, mut layers: Vec<Layer>) {
        for l in &mut layers {
            l.min_power = l.min_power.min(l.max_power);
        }
        if layers != self.doc.layers {
            self.checkpoint_merged("Layer settings", now);
            self.doc.layers = layers;
        }
    }

    /// The Cut Settings Editor window for one layer.
    pub(crate) fn cut_editor_window(&mut self, ctx: &egui::Context) {
        let speed = self.laser.speed(self.prefs.units);
        let Some(index) = self.cut_editor else { return };
        let mut layers = self.doc.layers.clone();
        let mut open = true;
        let mut changed = false;
        // A fixed id: the title shows the layer name, which changes as it's typed, and a window
        // whose id changes is rebuilt each time (taking the focus out of the name box).
        crate::dialogs::centered(ctx, format!("Cut Settings Editor: {}", layers[index].name)).id(egui::Id::new("cut-settings-editor")).open(&mut open).resizable(false).show(ctx, |ui| {
            let l = &mut layers[index];
            ui.horizontal(|ui| {
                swatch(ui, index, l.rgb(index), 18.0);
                ui.label("Name");
                ui.text_edit_singleline(&mut l.name);
                ui.label("Mode");
                egui::ComboBox::from_id_salt("editor-mode").selected_text(l.mode.label()).show_ui(ui, |ui| {
                    for m in LayerMode::ALL {
                        ui.selectable_value(&mut l.mode, m, m.label());
                    }
                });
            });
            ui.separator();
            ui.columns(2, |cols| {
                egui::Grid::new("editor-common").num_columns(2).spacing([8.0, 4.0]).show(&mut cols[0], |ui| {
                    ui.strong("Common");
                    ui.end_row();
                    ui.label(speed.label());
                    ui.add(speed.edit(&mut l.speed, 0.1..=5000.0));
                    ui.end_row();
                    ui.label("Max Power");
                    ui.add(egui::DragValue::new(&mut l.max_power).range(0.0..=100.0).speed(0.5).suffix(" %"));
                    ui.end_row();
                    ui.label("Min Power");
                    ui.add(egui::DragValue::new(&mut l.min_power).range(0.0..=100.0).speed(0.5).suffix(" %"));
                    ui.end_row();
                    ui.label("Number of Passes");
                    ui.add(egui::DragValue::new(&mut l.passes).range(1..=100));
                    ui.end_row();
                    ui.label("Air Assist");
                    ui.checkbox(&mut l.air_assist, "");
                    ui.end_row();
                    ui.label("Frequency").on_hover_text("Pulse frequency for galvo (fiber/UV/CO2) lasers");
                    ui.add(egui::DragValue::new(&mut l.frequency).range(1.0..=4000.0).speed(0.5).suffix(" kHz"));
                    ui.end_row();
                    ui.label("Perforate").on_hover_text("Cut in dashes: cut a length, skip a gap, repeat (fold lines, tear-offs). Line modes only.");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut l.perforate, "");
                        ui.add_enabled_ui(l.perforate, |ui| {
                            ui.label("cut");
                            ui.add(egui::DragValue::new(&mut l.perf_cut).range(0.05..=500.0).speed(0.05).suffix(" mm"));
                            ui.label("skip");
                            ui.add(egui::DragValue::new(&mut l.perf_skip).range(0.05..=500.0).speed(0.05).suffix(" mm"));
                        });
                    });
                    ui.end_row();
                    ui.label("Output / Show");
                    ui.horizontal(|ui| {
                        if bc_doc::is_tool_layer(index) {
                            l.output = false;
                            ui.add_enabled(false, egui::Checkbox::without_text(&mut l.output)).on_disabled_hover_text("Tool layer: never sent to the laser");
                        } else {
                            ui.checkbox(&mut l.output, "");
                        }
                        ui.checkbox(&mut l.visible, "");
                    });
                    ui.end_row();
                });
                let ui = &mut cols[1];
                if l.mode == LayerMode::Line || l.mode == LayerMode::FillThenLine {
                    egui::Grid::new("editor-line").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                        ui.strong("Cut");
                        ui.end_row();
                        ui.label("Kerf Offset");
                        ui.add(egui::DragValue::new(&mut l.kerf).range(-5.0..=5.0).speed(0.01).suffix(" mm"));
                        ui.end_row();
                        ui.label("Tabs per shape");
                        ui.add(egui::DragValue::new(&mut l.tab_count).range(0..=50));
                        ui.end_row();
                        ui.label("Tab size");
                        ui.add_enabled(l.tab_count > 0, egui::DragValue::new(&mut l.tab_size).range(0.05..=20.0).speed(0.05).suffix(" mm"));
                        ui.end_row();
                        ui.label("Skip inner shapes");
                        ui.add_enabled(l.tab_count > 0, egui::Checkbox::without_text(&mut l.tabs_skip_inner));
                        ui.end_row();
                        ui.label("Lead-In");
                        ui.add(egui::DragValue::new(&mut l.lead_in).range(0.0..=20.0).speed(0.1).suffix(" mm"));
                        ui.end_row();
                    });
                }
                if l.mode != LayerMode::Line {
                    egui::Grid::new("editor-scan").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                        ui.strong(if l.mode == LayerMode::Image { "Image" } else { "Fill" });
                        ui.end_row();
                        ui.label("Interval");
                        ui.add(egui::DragValue::new(&mut l.interval).range(0.01..=2.0).speed(0.005).suffix(" mm"));
                        ui.end_row();
                        ui.label("Lines per inch");
                        ui.label(format!("{:.0}", 25.4 / l.interval.max(0.001)));
                        ui.end_row();
                        ui.label("Bi-directional");
                        ui.checkbox(&mut l.bidirectional, "");
                        ui.end_row();
                        ui.label("Overscanning");
                        ui.add(egui::DragValue::new(&mut l.overscan).range(0.0..=50.0).speed(0.1).suffix(" mm"));
                        ui.end_row();
                        if l.mode == LayerMode::Image {
                            ui.label("Image Mode");
                            egui::ComboBox::from_id_salt("editor-dither").selected_text(l.dither.label()).show_ui(ui, |ui| {
                                for d in DitherMode::ALL {
                                    ui.selectable_value(&mut l.dither, d, d.label());
                                }
                            });
                            ui.end_row();
                        } else {
                            ui.label("Scan Angle");
                            ui.add(egui::DragValue::new(&mut l.scan_angle).range(-180.0..=180.0).suffix("°"));
                            ui.end_row();
                            ui.label("Cross-Hatch");
                            ui.checkbox(&mut l.cross_hatch, "");
                            ui.end_row();
                            ui.label("Fill Style").on_hover_text("Offset fill traces rings that follow the outline inward: smoother on lettering and logos, and quick on thin shapes");
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut l.offset_fill, false, "Scan lines");
                                ui.radio_value(&mut l.offset_fill, true, "Offset (contours)");
                            });
                            ui.end_row();
                        }
                    });
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Reset to defaults").clicked() {
                    let (name, output, visible) = (l.name.clone(), l.output, l.visible);
                    *l = Layer::new(index);
                    (l.name, l.output, l.visible) = (name, output, visible);
                }
                if ui.button("OK").clicked() {
                    changed = true;
                }
            });
        });
        let ok = changed;
        let now = ctx.input(|i| i.time);
        self.commit_layers(now, layers);
        if !open || ok {
            self.cut_editor = None;
        }
    }

    /// Settings specific to the single selected shape (position and size are in the edit bar).
    /// Live effects on the selected shape: added, tuned, reordered and removed at any time, then
    /// applied for good.
    fn effects_ui(&mut self, ui: &mut egui::Ui, shape: &bc_doc::Shape, now: f64) {
        use bc_doc::effects::Effect;
        let mut effects = shape.effects.clone();
        let mut apply = false;
        // A removed blend's end shape, given back as a shape of its own.
        let mut restored: Option<bc_geom::BezPath> = None;
        egui::CollapsingHeader::new(if effects.is_empty() { "Live effects".to_string() } else { format!("Live effects ({})", effects.len()) })
            .id_salt("live-effects")
            .default_open(!effects.is_empty())
            .show(ui, |ui| {
                ui.weak("Changes kept separate from the shape, so they can be adjusted or removed later. They apply in order, top first.");
                let mut remove = None;
                let mut swap = None;
                let count = effects.len();
                let placed = shape.transform * shape.local_path();
                for i in 0..count {
                    // The outline as this effect receives it (after the ones before it).
                    let before = bc_doc::effects::apply_all_placed(&effects[..i], placed.clone(), shape.transform);
                    let e = &mut effects[i];
                    let mut envelope_grid = false;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(e.name()).strong());
                        match e {
                            Effect::Offset { distance } => {
                                ui.add(egui::DragValue::new(distance).range(-100.0..=100.0).speed(0.05).suffix(" mm")).on_hover_text("Positive grows the shape, negative shrinks it");
                            }
                            Effect::RoundCorners { radius } => {
                                ui.add(egui::DragValue::new(radius).range(0.0..=200.0).speed(0.05).suffix(" mm"));
                            }
                            Effect::Warp { kind, amount } => {
                                egui::ComboBox::from_id_salt(("effect-warp", i)).selected_text(kind.label()).width(110.0).show_ui(ui, |ui| {
                                    for k in bc_geom::warp::Warp::ALL {
                                        ui.selectable_value(kind, k, k.label());
                                    }
                                });
                                let mut pct = *amount * 100.0;
                                if ui.add(egui::DragValue::new(&mut pct).range(-100.0..=100.0).speed(0.5).suffix(" %")).changed() {
                                    *amount = pct / 100.0;
                                }
                            }
                            Effect::Simplify { tolerance } | Effect::Smooth { tolerance } => {
                                ui.add(egui::DragValue::new(tolerance).range(0.001..=10.0).speed(0.01).suffix(" mm"));
                            }
                            Effect::RadialRepeat { copies, radius, weld } => {
                                ui.add(egui::DragValue::new(copies).range(1..=360).suffix(" copies"));
                                ui.add(egui::DragValue::new(radius).range(-1000.0..=1000.0).speed(0.2).suffix(" mm")).on_hover_text("How far below the shape's middle the centre is");
                                ui.checkbox(weld, "Weld").on_hover_text("Join overlapping copies into one outline");
                            }
                            Effect::Envelope { cols, rows, moves } => {
                                let (mut c, mut r) = (*cols, *rows);
                                ui.add(egui::DragValue::new(&mut c).range(1..=4)).on_hover_text("Columns of the handle grid");
                                ui.label("×");
                                ui.add(egui::DragValue::new(&mut r).range(1..=4)).on_hover_text("Rows of the handle grid");
                                if (c, r) != (*cols, *rows) {
                                    (*cols, *rows) = (c, r);
                                    *moves = vec![(0.0, 0.0); ((c + 1) * (r + 1)) as usize];
                                }
                                if ui.small_button("Reset").clicked() {
                                    moves.iter_mut().for_each(|m| *m = (0.0, 0.0));
                                }
                                envelope_grid = true;
                            }
                            Effect::Blend { steps, .. } => {
                                ui.add(egui::DragValue::new(steps).range(1..=500).suffix(" steps")).on_hover_text("In-between shapes from this shape to the end shape");
                            }
                            Effect::Clip { .. } => {
                                ui.weak("to a shape").on_hover_text("Only what's inside the clipping shape is kept. Remove the effect to bring the rest back.");
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("🗙").on_hover_text("Remove").clicked() {
                                remove = Some(i);
                            }
                            if ui.add_enabled(i + 1 < count, egui::Button::new("⏷").small()).on_hover_text("Later").clicked() {
                                swap = Some((i, i + 1));
                            }
                            if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).on_hover_text("Earlier").clicked() {
                                swap = Some((i, i - 1));
                            }
                        });
                    });
                    if envelope_grid && let Effect::Envelope { cols, rows, moves } = e {
                        envelope_editor(ui, &before, *cols, *rows, moves);
                    }
                }
                if let Some(i) = remove
                    && let Effect::Blend { end, .. } = effects.remove(i)
                {
                    restored = Some(shape.transform * end);
                }
                if let Some((a, b)) = swap {
                    effects.swap(a, b);
                }
                ui.horizontal(|ui| {
                    ui.menu_button("Add Effect ⏷", |ui| {
                        for e in Effect::choices() {
                            if ui.button(e.name()).clicked() {
                                effects.push(e);
                                ui.close();
                            }
                        }
                    });
                    if ui.add_enabled(!effects.is_empty(), egui::Button::new("Apply")).on_hover_text("Make the effects permanent: the shape becomes a path with them built in").clicked() {
                        apply = true;
                    }
                });
            });
        if effects != shape.effects {
            if restored.is_some() {
                self.checkpoint_named("Remove blend");
            } else {
                self.checkpoint_merged("Live effects", now);
            }
            if let Some(s) = self.doc.shape_mut(shape.id) {
                s.effects = effects;
            }
            if let Some(end) = restored {
                let id = self.doc.add_shape(ShapeKind::Path(end), bc_geom::Affine::IDENTITY, shape.layer);
                self.selection.insert(id);
                self.status = "Removed the blend: its end shape is a shape of its own again".into();
            }
        }
        if apply {
            self.checkpoint_named("Apply effects");
            bc_doc::edit::convert_to_path(&mut self.doc, &[shape.id].into());
        }
    }

    pub(crate) fn properties_panel(&mut self, ui: &mut egui::Ui) {
        let count = self.selection.len();
        if count == 0 {
            ui.weak("Nothing selected");
            return;
        }
        let single = if count == 1 { self.doc.shape(*self.selection.iter().next().unwrap()).cloned() } else { None };
        let locked = self.doc.shapes.iter().any(|s| self.selection.contains(&s.id) && s.locked);
        ui.label(match &single {
            Some(s) => format!("{} on layer {}{}", s.kind.type_name(), bc_doc::layer_label(s.layer), if locked { " (locked)" } else { "" }),
            None => format!("{count} shapes{}", if locked { " (some locked)" } else { "" }),
        });
        let Some(shape) = single else { return };
        let u = self.prefs.units;
        let now = ui.input(|i| i.time);
        if !matches!(shape.kind, ShapeKind::Image { .. }) && !shape.locked {
            self.effects_ui(ui, &shape, now);
        }
        match shape.kind {
            ShapeKind::Polygon { sides, radius } => {
                let (mut n, mut r) = (sides, radius);
                ui.horizontal(|ui| {
                    ui.label("Sides");
                    let a = ui.add(egui::DragValue::new(&mut n).range(3..=64)).changed();
                    ui.label("Radius");
                    let b = u.drag(ui, &mut r, 0.1..=5000.0);
                    if a || b {
                        self.checkpoint_merged("Polygon", now);
                        if let Some(s) = self.doc.shape_mut(shape.id) {
                            s.kind = ShapeKind::Polygon { sides: n, radius: r };
                        }
                    }
                });
            }
            ShapeKind::Star { points, outer, inner } => {
                let (mut n, mut o, mut i) = (points, outer, inner);
                ui.horizontal(|ui| {
                    ui.label("Points");
                    let a = ui.add(egui::DragValue::new(&mut n).range(3..=64)).changed();
                    ui.label("Outer");
                    let b = u.drag(ui, &mut o, 0.1..=5000.0);
                    ui.label("Inner");
                    let c = u.drag(ui, &mut i, 0.05..=5000.0);
                    if a || b || c {
                        self.checkpoint_merged("Star", now);
                        if let Some(s) = self.doc.shape_mut(shape.id) {
                            s.kind = ShapeKind::Star { points: n, outer: o, inner: i.min(o) };
                        }
                    }
                });
            }
            ShapeKind::Rect { width, height, corner_radius } => {
                let mut r = corner_radius;
                ui.horizontal(|ui| {
                    ui.label("Corner radius");
                    if u.drag(ui, &mut r, 0.0..=width.min(height) / 2.0) {
                        self.checkpoint_merged("Corner radius", now);
                        if let Some(s) = self.doc.shape_mut(shape.id) {
                            s.kind = ShapeKind::Rect { width, height, corner_radius: r };
                        }
                    }
                });
            }
            ShapeKind::Text { ref text, ref font, .. } => {
                ui.label(format!("\"{}\" in {font}", text.lines().next().unwrap_or_default()));
                if ui.button("Edit text…").clicked() {
                    self.open_text_dialog(Some(shape.id), bc_geom::Point::ORIGIN);
                }
            }
            ShapeKind::Image { adjust, .. } => {
                let mut a: ImageAdjust = adjust;
                egui::Grid::new("image-adjust").num_columns(2).show(ui, |ui| {
                    ui.label("Brightness");
                    ui.add(egui::Slider::new(&mut a.brightness, -1.0..=1.0));
                    ui.end_row();
                    ui.label("Contrast");
                    ui.add(egui::Slider::new(&mut a.contrast, -0.95..=0.95));
                    ui.end_row();
                    ui.label("Gamma");
                    ui.add(egui::Slider::new(&mut a.gamma, 0.2..=5.0).logarithmic(true));
                    ui.end_row();
                    ui.label("Effect");
                    egui::ComboBox::from_id_salt("image-effect").selected_text(a.effect.label()).show_ui(ui, |ui| {
                        for e in bc_doc::ImageEffect::ALL {
                            ui.selectable_value(&mut a.effect, e, e.label());
                        }
                    });
                    ui.end_row();
                    ui.label("Sharpen");
                    ui.add(egui::Slider::new(&mut a.sharpen, 0.0..=2.0)).on_hover_text("Crisper detail; engraving softens edges, so a little helps");
                    ui.end_row();
                    ui.label("Remove background");
                    ui.add(egui::Slider::new(&mut a.remove_background, 0.0..=0.5).custom_formatter(|v, _| if v <= 0.0 { "off".into() } else { format!("{v:.2}") }))
                        .on_hover_text("Turns a plain light backdrop around the subject white, so it isn't engraved. Raise it until the background clears.");
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut a.auto_level, "Auto levels").on_hover_text("Stretch to the full black-to-white range");
                    ui.checkbox(&mut a.invert, "Invert");
                    if ui.button("Reset").clicked() {
                        a = ImageAdjust::default();
                    }
                });
                ui.checkbox(&mut self.prefs.burn_preview, "Show as it will burn").on_hover_text("Draw images dithered at the layer's line interval, like the laser will (View > Burn Preview)");
                if a != adjust {
                    self.checkpoint_merged("Adjust image", now);
                    if let Some(ShapeKind::Image { adjust, .. }) = self.doc.shape_mut(shape.id).map(|s| &mut s.kind) {
                        *adjust = a;
                    }
                }
            }
            ShapeKind::Barcode { symbology, ref content, .. } => {
                ui.label(format!("{}: \"{content}\"", symbology.label()));
                if ui.button("Edit barcode…").clicked() {
                    self.open_text_dialog(Some(shape.id), bc_geom::Point::ORIGIN);
                }
            }
            ShapeKind::Ellipse { .. } | ShapeKind::Path(_) => {}
        }
    }

    pub(crate) fn variables_panel(&mut self, ui: &mut egui::Ui) {
        ui.weak("In text or barcodes, use {serial}, {serial:0000}, {date}, {time}, {row}, or a CSV column name like {Name}.");
        let mut v = self.doc.variables.clone();
        ui.horizontal(|ui| {
            ui.label("Serial");
            ui.add(egui::DragValue::new(&mut v.serial));
            ui.label("step");
            ui.add(egui::DragValue::new(&mut v.increment));
            ui.checkbox(&mut v.auto_advance, "Advance after each job");
        });
        ui.horizontal(|ui| {
            if ui.button("Load CSV…").clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("CSV", &["csv", "txt"]).pick_file() {
                    match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|d| bc_io::read_csv(&d)) {
                        Ok((headers, rows)) => {
                            v.csv_headers = headers;
                            v.csv_rows = rows;
                            v.row = 0;
                        }
                        Err(e) => self.status = format!("Could not read {}: {e}", path.display()),
                    }
                }
            if !v.csv_rows.is_empty() {
                if ui.small_button("◀").clicked() {
                    v.row = v.row.saturating_sub(1);
                }
                ui.label(format!("row {} of {}", v.row + 1, v.csv_rows.len()));
                if ui.small_button("▶").clicked() {
                    v.row = (v.row + 1).min(v.csv_rows.len() - 1);
                }
                if ui.small_button("Clear").clicked() {
                    v.csv_headers.clear();
                    v.csv_rows.clear();
                    v.row = 0;
                }
            }
        });
        if !v.csv_headers.is_empty() {
            ui.weak(format!("Columns: {}", v.csv_headers.join(", ")));
        }
        let clock = crate::app::clock();
        for s in &self.doc.shapes {
            let template = match &s.kind {
                ShapeKind::Text { text, .. } => text,
                ShapeKind::Barcode { content, .. } => content,
                _ => continue,
            };
            if bc_doc::vars::has_fields(template) {
                ui.label(format!("“{}” → “{}”", template.replace('\n', " "), v.evaluate(template, &clock).replace('\n', " ")));
            }
        }
        if v != self.doc.variables {
            let now = ui.input(|i| i.time);
            self.checkpoint_merged("Variable text", now);
            self.doc.variables = v;
        }
    }

    pub(crate) fn library_panel(&mut self, ui: &mut egui::Ui) {
        let speed = self.laser.speed(self.prefs.units);
        let layer = self.current_layer;
        ui.horizontal(|ui| {
            swatch(ui, layer, self.doc.layers[layer].rgb(layer), 16.0);
            ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text("e.g. 3 mm plywood – cut").desired_width(190.0));
            if ui.add_enabled(!self.preset_name.trim().is_empty(), egui::Button::new("Save layer")).on_hover_text("Save the current layer's settings under this name").clicked() {
                let mut settings = self.doc.layers[layer].clone();
                settings.visible = true;
                settings.output = true;
                let name = self.preset_name.trim().to_string();
                self.library.retain(|p| p.name != name);
                self.library.push(Preset { name, settings });
                self.library.sort_by_key(|p| p.name.to_lowercase());
                self.preset_name.clear();
            }
        });
        if self.library.is_empty() {
            ui.weak("Saved settings appear here. Pick a layer, set it up, name it and save.");
            return;
        }
        let mut apply = None;
        let mut remove = None;
        for (i, preset) in self.library.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.button(format!("Apply to {}", bc_doc::layer_label(layer))).clicked() {
                    apply = Some(i);
                }
                ui.label(&preset.name);
                let s = &preset.settings;
                ui.weak(format!("{} · {} · {:.0}–{:.0}% · ×{}", s.mode.label(), speed.format(s.speed), s.min_power, s.max_power, s.passes));
                if ui.small_button("🗑").on_hover_text("Remove from library").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = apply {
            let mut settings = self.library[i].settings.clone();
            let current = &self.doc.layers[layer];
            settings.visible = current.visible;
            settings.output = current.output;
            settings.name = current.name.clone();
            self.checkpoint_named("Apply library settings");
            self.doc.layers[layer] = settings;
            self.status = format!("Applied \"{}\" to layer {}", self.library[i].name, bc_doc::layer_label(layer));
        }
        if let Some(i) = remove {
            self.library.remove(i);
        }
    }

    /// Undo history: click an entry to go back (or forward) to just after it.
    fn history_panel(&mut self, ui: &mut egui::Ui) {
        let undo: Vec<String> = self.history.undo_labels().iter().map(|s| s.to_string()).collect();
        let redo: Vec<String> = self.history.redo_labels().iter().map(|s| s.to_string()).collect();
        if undo.is_empty() && redo.is_empty() {
            ui.weak("No edits yet.");
            return;
        }
        let mut undo_steps = 0;
        let mut redo_steps = 0;
        for (i, label) in undo.iter().enumerate() {
            if ui.selectable_label(false, label).on_hover_text("Undo back to just after this").clicked() {
                undo_steps = undo.len() - i - 1;
            }
        }
        ui.strong("▶ current");
        for (i, label) in redo.iter().enumerate() {
            if ui.selectable_label(false, egui::RichText::new(label).weak()).on_hover_text("Redo up to this").clicked() {
                redo_steps = i + 1;
            }
        }
        for _ in 0..undo_steps {
            self.undo();
        }
        for _ in 0..redo_steps {
            self.redo();
        }
    }
}
