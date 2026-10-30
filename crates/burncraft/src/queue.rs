//! The production queue: several designs, each with its own number of copies, burned one after
//! another. Each copy is a separate run, so you can load fresh material in between; the design
//! open in the editor isn't touched.

use bc_doc::Document;
use eframe::egui::{self, RichText};

/// One design in the queue.
pub struct QueueItem {
    pub name: String,
    /// The design as it was added (a snapshot; later edits to the open design don't change it).
    pub doc: Document,
    pub copies: u32,
    /// Copies burned so far.
    pub done: u32,
}

/// What the queue window asks the app to do.
pub enum QueueAction {
    AddCurrent,
    AddFiles,
    /// Burn the next copy (of item `index`).
    Run(usize),
    Frame(usize),
}

#[derive(Default)]
pub struct ProductionQueue {
    pub open: bool,
    pub items: Vec<QueueItem>,
    /// The item being burned, and the device's completed-job count when it started.
    running: Option<(usize, u64)>,
    pub status: String,
}

impl ProductionQueue {
    /// The next item with copies left to burn.
    pub fn next(&self) -> Option<usize> {
        self.items.iter().position(|i| i.done < i.copies)
    }

    pub fn add(&mut self, name: String, doc: Document) {
        self.items.push(QueueItem { name, doc, copies: 1, done: 0 });
    }

    /// Records that item `index` was sent to the laser; `completed` is the device's count of
    /// completed jobs at that moment.
    pub fn started(&mut self, index: usize, completed: u64) {
        self.running = Some((index, completed));
        if let Some(item) = self.items.get(index) {
            self.status = format!("Burning {} (copy {} of {})…", item.name, item.done + 1, item.copies);
        }
    }

    /// Follows the running job: a completed one counts as a burned copy; one stopped or failed
    /// (the device's job ended without completing) doesn't.
    pub fn track(&mut self, completed: u64, job_running: bool) {
        let Some((index, before)) = self.running else { return };
        if completed > before {
            self.running = None;
            if let Some(item) = self.items.get_mut(index) {
                item.done += 1;
            }
            self.status = match self.next() {
                Some(n) => format!("Done. Next: {} (copy {} of {}). Load the material, then Burn Next.", self.items[n].name, self.items[n].done + 1, self.items[n].copies),
                None => "Everything in the queue is done.".into(),
            };
        } else if !job_running {
            self.running = None;
            self.status = "The job ended without finishing (stopped or failed), so it isn't counted.".into();
        }
    }

    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    pub fn ui(&mut self, ctx: &egui::Context, connected: bool) -> Option<QueueAction> {
        if !self.open {
            return None;
        }
        let mut action = None;
        let mut open = true;
        let busy = self.running();
        crate::dialogs::centered(ctx, "Production Queue").open(&mut open).resizable(false).auto_sized().show(ctx, |ui| {
            ui.set_width(460.0);
            ui.weak("Designs to burn one after another, each as many times as you need. Each copy is its own run, so you can load fresh material in between.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Add This Design").on_hover_text("A copy of the design as it is now").clicked() {
                    action = Some(QueueAction::AddCurrent);
                }
                if ui.button("Add Files…").on_hover_text("BurnCraft projects (.bcproj)").clicked() {
                    action = Some(QueueAction::AddFiles);
                }
            });
            ui.add_space(4.0);
            if self.items.is_empty() {
                ui.weak("The queue is empty.");
            } else {
                let mut remove = None;
                let mut swap = None;
                let next = self.next();
                let running = self.running.map(|r| r.0);
                egui::Grid::new("queue-grid").num_columns(4).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
                    ui.strong("Design");
                    ui.strong("Copies");
                    ui.strong("Done");
                    ui.label("");
                    ui.end_row();
                    let count = self.items.len();
                    for (i, item) in self.items.iter_mut().enumerate() {
                        let name = RichText::new(&item.name);
                        ui.label(if running == Some(i) || (running.is_none() && next == Some(i)) { name.strong() } else { name });
                        ui.add_enabled(!busy, egui::DragValue::new(&mut item.copies).range(1..=9999));
                        ui.label(format!("{} / {}", item.done.min(item.copies), item.copies));
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(!busy, |ui| {
                                if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).on_hover_text("Earlier").clicked() {
                                    swap = Some((i, i - 1));
                                }
                                if ui.add_enabled(i + 1 < count, egui::Button::new("⏷").small()).on_hover_text("Later").clicked() {
                                    swap = Some((i, i + 1));
                                }
                                if ui.small_button("🗙").on_hover_text("Remove from the queue").clicked() {
                                    remove = Some(i);
                                }
                            });
                        });
                        ui.end_row();
                    }
                });
                if let Some((a, b)) = swap {
                    self.items.swap(a, b);
                }
                if let Some(i) = remove {
                    self.items.remove(i);
                }
                let (done, total): (u32, u32) = self.items.iter().fold((0, 0), |(d, t), i| (d + i.done.min(i.copies), t + i.copies));
                ui.weak(format!("{done} of {total} pieces burned."));
            }
            ui.separator();
            ui.horizontal(|ui| {
                let next = self.next();
                let can = connected && !busy && next.is_some();
                let tip = if !connected { "Connect to the laser first" } else if busy { "Wait for the current piece to finish" } else { "Nothing left to burn" };
                if ui.add_enabled(can, egui::Button::new(RichText::new("Burn Next").strong())).on_disabled_hover_text(tip).clicked()
                    && let Some(n) = next
                {
                    action = Some(QueueAction::Run(n));
                }
                if ui.add_enabled(can, egui::Button::new("Frame Next")).on_hover_text("Trace the next design's outline with the laser off, to check the material is in place").on_disabled_hover_text(tip).clicked()
                    && let Some(n) = next
                {
                    action = Some(QueueAction::Frame(n));
                }
                if ui.add_enabled(!busy && self.items.iter().any(|i| i.done > 0), egui::Button::new("Start Over")).on_hover_text("Set every design back to none burned").clicked() {
                    self.items.iter_mut().for_each(|i| i.done = 0);
                    self.status.clear();
                }
            });
            if !self.status.is_empty() {
                ui.add(egui::Label::new(&self.status).wrap());
            }
        });
        self.open = open;
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_count_only_when_a_job_completes() {
        let mut q = ProductionQueue::default();
        q.add("Coasters".into(), Document::default());
        q.add("Tags".into(), Document::default());
        q.items[0].copies = 2;
        assert_eq!(q.next(), Some(0));
        q.started(0, 5);
        q.track(5, true);
        assert!(q.running(), "still burning");
        q.track(6, false);
        assert_eq!((q.items[0].done, q.next()), (1, Some(0)), "one of two coasters done");
        // A stopped run isn't counted.
        q.started(0, 6);
        q.track(6, false);
        assert_eq!(q.items[0].done, 1);
        q.started(0, 6);
        q.track(7, false);
        assert_eq!(q.next(), Some(1), "on to the tags");
        q.started(1, 7);
        q.track(8, false);
        assert_eq!(q.next(), None);
    }
}
