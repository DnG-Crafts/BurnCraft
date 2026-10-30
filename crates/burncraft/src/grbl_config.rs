//! The Machine Settings window for GRBL controllers: read the controller's own settings (`$$`),
//! change them, write them back, and save or load them as a file.

use crate::device::{Device, DeviceCmd};
use bc_grbl::settings::{self, Kind};
use eframe::egui::{self, Color32, RichText};

/// Extensions offered for settings files.
const FILE_EXTENSIONS: [&str; 3] = ["txt", "nc", "gcode"];

#[derive(Default)]
pub struct GrblConfig {
    pub open: bool,
    /// The settings as the controller has them.
    read: Vec<(u32, String)>,
    /// The values shown and edited.
    rows: Vec<(u32, String)>,
    /// Which reading `read` came from (see `Device::grbl_settings_read`).
    reading: u64,
    /// A read was asked for when the window opened.
    asked: bool,
    /// The user clicked Write and is being asked to confirm.
    confirming: bool,
    status: Option<String>,
}

/// Whether two setting values are the same number ("6000" and "6000.000" are).
fn same(a: &str, b: &str) -> bool {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => (x - y).abs() < 1e-9,
        _ => a.trim() == b.trim(),
    }
}

impl GrblConfig {
    /// Settings whose shown value differs from the controller's.
    pub fn changes(&self) -> Vec<(u32, String)> {
        self.rows
            .iter()
            .filter(|(id, v)| self.read.iter().find(|(i, _)| i == id).is_none_or(|(_, r)| !same(r, v)))
            .map(|(id, v)| (*id, v.trim().to_string()))
            .collect()
    }

    /// Takes in a new reading from the controller. Edits not yet written are dropped.
    fn take_reading(&mut self, settings: &[(u32, String)]) {
        self.read = settings.to_vec();
        self.rows = settings.to_vec();
        self.confirming = false;
        self.status = Some(format!("Read {} settings from the controller", settings.len()));
    }

    /// Puts values from a settings file into the table, to be written with Write.
    pub fn load_values(&mut self, values: &[(u32, String)]) -> usize {
        for (id, v) in values {
            match self.rows.iter_mut().find(|(i, _)| i == id) {
                Some(row) => row.1 = v.clone(),
                None => self.rows.push((*id, v.clone())),
            }
        }
        values.len()
    }

    /// `s_max` is the machine's "Full-power S value", compared with the controller's `$30`.
    pub fn show(&mut self, ctx: &egui::Context, device: &mut Device, machine: &str, s_max: &mut f64) {
        if !self.open {
            self.asked = false;
            return;
        }
        let connected = device.connected;
        if connected && !self.asked {
            self.asked = true;
            device.send(DeviceCmd::ReadSettings);
        }
        if device.grbl_settings_read != self.reading {
            self.reading = device.grbl_settings_read;
            if let Some(s) = device.grbl_settings.clone() {
                self.take_reading(&s);
            }
        }
        if let Some(note) = device.grbl_settings_note.take() {
            self.status = Some(note);
        }
        let mut open = true;
        crate::dialogs::centered(ctx, "Machine Settings (GRBL)").open(&mut open).resizable(false).auto_sized().collapsible(false).show(ctx, |ui| {
            ui.set_width(660.0);
            ui.label(format!("The settings stored in {machine}'s controller."));
            ui.weak("Wrong values can stop the machine moving properly. Export a copy before changing anything.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(connected, egui::Button::new("Read from Controller")).on_disabled_hover_text("Connect to the laser first").clicked() {
                    device.send(DeviceCmd::ReadSettings);
                    self.status = Some("Reading…".into());
                }
                if ui.add_enabled(connected, egui::Button::new("Unlock ($X)")).on_hover_text("Clears an alarm so the machine accepts commands again").clicked() {
                    device.send(DeviceCmd::Unlock);
                }
                ui.separator();
                if ui.button("Import…").on_hover_text("Load settings from a file into the table; Write sends them to the controller").clicked() {
                    self.import();
                }
                if ui.add_enabled(!self.rows.is_empty(), egui::Button::new("Export…")).on_hover_text("Save the settings shown to a file").clicked() {
                    self.export(machine);
                }
            });
            ui.add_space(4.0);
            if self.rows.is_empty() {
                let text = if !connected {
                    "Connect to the laser to read its settings, or import a settings file."
                } else if self.status.as_deref() == Some("Reading…") || (self.status.is_none() && self.reading == 0) {
                    "Reading the settings…"
                } else {
                    "No settings to show."
                };
                ui.weak(text);
            } else {
                self.table(ui);
            }
            // The full-power S value BurnCraft sends must match $30, or power comes out wrong.
            if let Some(max) = self.read.iter().find(|(id, _)| *id == 30).and_then(|(_, v)| v.parse::<f64>().ok())
                && (max - *s_max).abs() > 1e-6
            {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("$30 is {max}, but this machine's Full-power S value is {s_max}."));
                    if ui.small_button("Use $30").clicked() {
                        *s_max = max;
                    }
                });
            }
            ui.separator();
            let changes = self.changes();
            let invalid: Vec<String> = changes.iter().filter_map(|(id, v)| settings::check(*id, v).err()).collect();
            if self.confirming && !changes.is_empty() {
                let list: Vec<String> = changes.iter().map(|(id, v)| format!("${id}={v}")).collect();
                ui.label(RichText::new(format!("Write {} to the controller?", list.join(", "))).strong());
                ui.horizontal(|ui| {
                    if ui.button("Write").clicked() {
                        device.send(DeviceCmd::WriteSettings(changes.clone()));
                        self.status = Some("Writing…".into());
                        self.confirming = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirming = false;
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    let label = if changes.is_empty() { "Write Changes".to_string() } else { format!("Write {} Change{}", changes.len(), if changes.len() == 1 { "" } else { "s" }) };
                    let can = connected && !changes.is_empty() && invalid.is_empty();
                    if ui.add_enabled(can, egui::Button::new(label)).clicked() {
                        self.confirming = true;
                    }
                    if ui.add_enabled(!changes.is_empty(), egui::Button::new("Undo Changes")).clicked() {
                        self.rows = self.read.clone();
                    }
                    if let Some(why) = invalid.first() {
                        ui.colored_label(ui.visuals().error_fg_color, why);
                    } else if let Some(s) = &self.status {
                        ui.add(egui::Label::new(RichText::new(s).weak()).wrap());
                    }
                });
            }
        });
        self.open = open;
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        let read = self.read.clone();
        // The list fills the window's width (not just its first frame's contents, which left the
        // unit column cut off once it widened).
        egui::ScrollArea::vertical().max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("grbl-settings").num_columns(4).striped(true).spacing([10.0, 4.0]).min_col_width(36.0).show(ui, |ui| {
                ui.strong("$");
                ui.strong("Setting");
                ui.strong("Value");
                ui.strong("Unit");
                ui.end_row();
                for (id, value) in &mut self.rows {
                    let info = settings::info_for(*id);
                    let changed = read.iter().find(|(i, _)| i == id).is_none_or(|(_, r)| !same(r, value));
                    let accent = ui.visuals().selection.stroke.color;
                    ui.label(RichText::new(format!("${id}")).monospace());
                    let name = info.map_or_else(|| format!("Setting {id}"), |i| i.name.to_string());
                    let name = if changed { RichText::new(name).strong().color(accent) } else { RichText::new(name) };
                    let r = ui.label(name);
                    if let Some(i) = info {
                        r.on_hover_text(i.help);
                    }
                    match info.map(|i| i.kind) {
                        Some(Kind::Switch) => {
                            let mut on = value.trim().parse::<f64>().is_ok_and(|v| v != 0.0);
                            let text = if on { "On" } else { "Off" };
                            if ui.checkbox(&mut on, text).changed() {
                                *value = if on { "1".into() } else { "0".into() };
                            }
                        }
                        Some(Kind::AxisMask) => {
                            let mut mask = value.trim().parse::<f64>().map_or(0, |v| v as u32);
                            let before = mask;
                            ui.horizontal(|ui| {
                                for (bit, axis) in [(1, "X"), (2, "Y"), (4, "Z")] {
                                    let mut set = mask & bit != 0;
                                    if ui.checkbox(&mut set, axis).changed() {
                                        mask = if set { mask | bit } else { mask & !bit };
                                    }
                                }
                            });
                            if mask != before {
                                *value = mask.to_string();
                            }
                        }
                        _ => {
                            let bad = settings::check(*id, value).is_err();
                            let mut edit = egui::TextEdit::singleline(value).desired_width(110.0);
                            if bad {
                                edit = edit.text_color(Color32::from_rgb(220, 70, 50));
                            }
                            ui.add(edit);
                        }
                    }
                    ui.weak(info.map_or("", |i| i.unit));
                    ui.end_row();
                }
            });
        });
    }

    fn import(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("GRBL settings", &FILE_EXTENSIONS).add_filter("All files", &["*"]).pick_file() else {
            return;
        };
        self.status = Some(match std::fs::read_to_string(&path) {
            Ok(text) => {
                let values = settings::parse(&text);
                if values.is_empty() {
                    format!("No $N=value lines in {}", path.display())
                } else {
                    let n = self.load_values(&values);
                    format!("Loaded {n} settings from the file. Check them, then Write to send them to the controller.")
                }
            }
            Err(e) => format!("Couldn't read {}: {e}", path.display()),
        });
    }

    fn export(&mut self, machine: &str) {
        let name: String = machine.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
        let Some(path) = rfd::FileDialog::new().add_filter("GRBL settings", &["txt"]).set_file_name(format!("{} GRBL settings.txt", name.trim())).save_file() else {
            return;
        };
        let header = format!("GRBL settings for {machine}\nSaved by BurnCraft {}. Load with Import in Machine Settings, or send each line in a console.", env!("CARGO_PKG_VERSION"));
        self.status = Some(match std::fs::write(&path, settings::to_file(&self.rows, &header)) {
            Ok(()) => format!("Saved to {}", path.display()),
            Err(e) => format!("Couldn't save {}: {e}", path.display()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_changes_are_written() {
        let mut c = GrblConfig::default();
        c.take_reading(&[(22, "0".into()), (110, "6000.000".into())]);
        assert!(c.changes().is_empty());
        c.rows[1].1 = "6000".into();
        assert!(c.changes().is_empty(), "the same number written differently isn't a change");
        c.rows[0].1 = "1".into();
        c.load_values(&[(130, "400".into())]);
        assert_eq!(c.changes(), vec![(22, "1".into()), (130, "400".into())]);
    }
}
