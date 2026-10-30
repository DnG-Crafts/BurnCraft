//! The Parameters & Constraints window, and keeping the design to its constraints.

use crate::app::BurnCraftApp;
use bc_doc::constraints::{self, Anchor, Axis, Constraint, ConstraintKind, Param};
use eframe::egui;

enum Change {
    /// A text field was clicked into: record undo before it's typed in.
    Checkpoint,
    AddParam,
    RemoveParam(usize),
    Add(ConstraintKind, f64),
    RemoveConstraint(usize),
}

fn anchor_combo(ui: &mut egui::Ui, id: egui::Id, anchor: &mut Anchor, axis: Axis) {
    egui::ComboBox::from_id_salt(id).width(70.0).selected_text(anchor.label(axis)).show_ui(ui, |ui| {
        for a in Anchor::ALL {
            ui.selectable_value(anchor, a, a.label(axis));
        }
    });
}

impl BurnCraftApp {
    /// Moves and resizes shapes so the design meets its constraints (every frame; it only
    /// changes anything when something is off).
    pub(crate) fn enforce_constraints(&mut self) {
        if self.doc.params.constraints.is_empty() {
            self.constraint_errors.clear();
            return;
        }
        let (changed, errors) = constraints::solve(&mut self.doc);
        if changed {
            self.revision += 1;
        }
        self.constraint_errors = errors;
    }

    /// The selection as groups ("units"), one representative shape each, largest first.
    fn selected_units(&self) -> Vec<u64> {
        let mut units: Vec<(u64, f64)> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for &id in &self.selection {
            let unit = constraints::unit(&self.doc, id);
            let key = *unit.iter().min().unwrap_or(&id);
            if seen.insert(key) {
                let area = self.doc.bounds_of(unit.iter().copied()).map_or(0.0, |b| b.area());
                units.push((id, area));
            }
        }
        units.sort_by(|a, b| b.1.total_cmp(&a.1));
        units.into_iter().map(|(id, _)| id).collect()
    }

    pub(crate) fn params_window(&mut self, ctx: &egui::Context) {
        if !self.show_params {
            return;
        }
        let mut open = true;
        let mut changes: Vec<Change> = Vec::new();
        let units = self.selected_units();
        let units_measure = |doc: &bc_doc::Document, kind: &ConstraintKind| Constraint { kind: kind.clone(), value: String::new(), enabled: true }.measure(doc).unwrap_or(0.0);
        crate::dialogs::centered(ctx, "Parameters & Constraints").open(&mut open).default_width(520.0).show(ctx, |ui| {
            let (values, _) = self.doc.params.values();
            ui.strong("Parameters");
            ui.weak("Named values to use in the constraints below, e.g. thickness = 3. A value can be a\nformula using the parameters above it: thickness * 2 + 0.2");
            egui::Grid::new("params").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                for (i, p) in self.doc.params.params.iter_mut().enumerate() {
                    if ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(110.0)).gained_focus() {
                        changes.push(Change::Checkpoint);
                    }
                    if ui.add(egui::TextEdit::singleline(&mut p.value).desired_width(160.0)).gained_focus() {
                        changes.push(Change::Checkpoint);
                    }
                    match values.get(p.name.trim()) {
                        Some(v) => ui.label(format!("= {v:.3}")),
                        None => ui.colored_label(ui.visuals().error_fg_color, "?"),
                    };
                    if ui.small_button("🗙").on_hover_text("Remove").clicked() {
                        changes.push(Change::RemoveParam(i));
                    }
                    ui.end_row();
                }
            });
            if ui.button("Add parameter").clicked() {
                changes.push(Change::AddParam);
            }
            ui.separator();
            ui.strong("Constraints");
            ui.weak("Locked sizes and distances (mm). The design keeps to them as you edit; type a\nnumber or a formula. A distance moves its second shape (and that shape's group).");
            let doc_snapshot = self.doc.clone();
            egui::Grid::new("constraints").num_columns(4).spacing([8.0, 4.0]).striped(true).show(ui, |ui| {
                for (i, c) in self.doc.params.constraints.iter_mut().enumerate() {
                    ui.checkbox(&mut c.enabled, "").on_hover_text("On / off");
                    ui.vertical(|ui| {
                        ui.label(c.describe(&doc_snapshot));
                        ui.horizontal(|ui| match &mut c.kind {
                            ConstraintKind::Size { axis, anchor, .. } => {
                                ui.weak("keep");
                                anchor_combo(ui, egui::Id::new(("size-anchor", i)), anchor, *axis);
                                ui.weak("edge still");
                            }
                            ConstraintKind::Distance { axis, from_anchor, to_anchor, .. } => {
                                ui.weak("from");
                                anchor_combo(ui, egui::Id::new(("from-anchor", i)), from_anchor, *axis);
                                ui.weak("to");
                                anchor_combo(ui, egui::Id::new(("to-anchor", i)), to_anchor, *axis);
                            }
                        });
                    });
                    ui.vertical(|ui| {
                        if ui.add(egui::TextEdit::singleline(&mut c.value).desired_width(140.0)).gained_focus() {
                            changes.push(Change::Checkpoint);
                        }
                        match constraints::eval(&c.value, &values) {
                            Ok(v) => ui.weak(format!("= {v:.3} mm")),
                            Err(e) => ui.colored_label(ui.visuals().error_fg_color, e),
                        };
                    });
                    if ui.small_button("🗙").on_hover_text("Remove this constraint").clicked() {
                        changes.push(Change::RemoveConstraint(i));
                    }
                    ui.end_row();
                }
            });
            if self.doc.params.constraints.is_empty() {
                ui.weak("None yet.");
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Add:");
                let one = units.len() == 1;
                let two = units.len() == 2;
                for (label, axis) in [("Lock width", Axis::X), ("Lock height", Axis::Y)] {
                    if ui.add_enabled(one, egui::Button::new(label)).on_disabled_hover_text("Select one shape or group").clicked() {
                        let kind = ConstraintKind::Size { shape: units[0], axis, anchor: Anchor::Start };
                        changes.push(Change::Add(kind.clone(), units_measure(&self.doc, &kind)));
                    }
                }
                for (label, axis) in [("Lock distance across", Axis::X), ("Lock distance down", Axis::Y)] {
                    let tip = "Select two shapes or groups: the larger is the reference, the smaller moves";
                    if ui.add_enabled(two, egui::Button::new(label)).on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
                        let kind = ConstraintKind::Distance { axis, from: units[0], from_anchor: Anchor::Start, to: units[1], to_anchor: Anchor::Start };
                        changes.push(Change::Add(kind.clone(), units_measure(&self.doc, &kind)));
                    }
                }
            });
            for e in &self.constraint_errors {
                ui.colored_label(ui.visuals().warn_fg_color, e);
            }
        });
        self.show_params = open;
        if !changes.is_empty() {
            self.checkpoint_named("Edit parameters");
        }
        for change in changes {
            match change {
                Change::Checkpoint => {}
                Change::AddParam => {
                    let name = self.doc.params.new_name();
                    self.doc.params.params.push(Param { name, value: "0".into() });
                }
                Change::RemoveParam(i) => {
                    self.doc.params.params.remove(i);
                }
                Change::Add(kind, now) => self.doc.params.constraints.push(Constraint { kind, value: format!("{now:.2}"), enabled: true }),
                Change::RemoveConstraint(i) => {
                    self.doc.params.constraints.remove(i);
                }
            }
        }
    }
}
