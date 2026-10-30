//! The top icon toolbar and the left tool strip.

use crate::app::BurnCraftApp;
use crate::canvas::Tool;
use crate::commands::Command;
use crate::icons::{self, Icon};
use crate::panels::LaserTab;
use bc_doc::edit::Align;
use bc_geom::ops::BooleanOp;
use eframe::egui;

const TOP: f32 = 28.0;
const LEFT: f32 = 30.0;
/// Space between left-strip icons.
const GAP: f32 = 3.0;

impl BurnCraftApp {
    /// An icon that runs a command; greyed out when the command can't run.
    fn command_icon(&mut self, ui: &mut egui::Ui, icon: Icon, cmd: Command, size: f32) {
        let tip = match cmd.shortcut() {
            Some(s) => format!("{} ({})", cmd.label().trim_end_matches('…'), ui.ctx().format_shortcut(&s)),
            None => cmd.label().trim_end_matches('…').to_string(),
        };
        if icons::button(ui, icon, size, false, self.can_run(cmd), &tip).clicked() {
            self.run(cmd);
        }
    }

    pub(crate) fn top_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (icon, cmd) in [(Icon::New, Command::New), (Icon::Open, Command::Open), (Icon::Save, Command::Save), (Icon::Import, Command::Import), (Icon::Export, Command::ExportSvg)] {
                self.command_icon(ui, icon, cmd, TOP);
            }
            ui.separator();
            self.command_icon(ui, Icon::Undo, Command::Undo, TOP);
            self.command_icon(ui, Icon::Redo, Command::Redo, TOP);
            ui.separator();
            for (icon, cmd) in [(Icon::Cut, Command::Cut), (Icon::Copy, Command::Copy), (Icon::Paste, Command::Paste), (Icon::Delete, Command::Delete)] {
                self.command_icon(ui, icon, cmd, TOP);
            }
            ui.separator();
            self.command_icon(ui, Icon::ZoomFit, Command::ZoomToBed, TOP);
            if icons::button(ui, Icon::ZoomIn, TOP, false, true, "Zoom in").clicked() {
                self.canvas.zoom_by(1.25);
            }
            if icons::button(ui, Icon::ZoomOut, TOP, false, true, "Zoom out").clicked() {
                self.canvas.zoom_by(0.8);
            }
            self.command_icon(ui, Icon::ZoomSelection, Command::ZoomToSelection, TOP);
            ui.separator();
            if icons::button(ui, Icon::Camera, TOP, self.laser_tab == LaserTab::Camera, true, "Camera").clicked() {
                self.laser_tab = LaserTab::Camera;
            }
            self.command_icon(ui, Icon::Device, Command::Devices, TOP);
            self.command_icon(ui, Icon::Settings, Command::Settings, TOP);
            self.command_icon(ui, Icon::Tools, Command::MaterialTest, TOP);
            self.command_icon(ui, Icon::Preview, Command::Preview, TOP);
            ui.separator();
            self.command_icon(ui, Icon::Group, Command::Group, TOP);
            self.command_icon(ui, Icon::Ungroup, Command::Ungroup, TOP);
            ui.separator();
            self.command_icon(ui, Icon::MirrorH, Command::Mirror(true), TOP);
            self.command_icon(ui, Icon::MirrorV, Command::Mirror(false), TOP);
            ui.separator();
            for (icon, a) in [
                (Icon::AlignLeft, Align::Left),
                (Icon::AlignHCenter, Align::HCenter),
                (Icon::AlignRight, Align::Right),
                (Icon::AlignTop, Align::Top),
                (Icon::AlignVCenter, Align::VCenter),
                (Icon::AlignBottom, Align::Bottom),
                (Icon::AlignCenters, Align::Centers),
            ] {
                self.command_icon(ui, icon, Command::Align(a), TOP);
            }
            self.command_icon(ui, Icon::DistributeH, Command::Distribute(true), TOP);
            self.command_icon(ui, Icon::DistributeV, Command::Distribute(false), TOP);
        });
    }

    /// The left strip's icon groups (separated by lines), top to bottom.
    fn left_groups() -> Vec<Vec<LeftItem>> {
        use LeftItem::*;
        vec![
            Tool::ALL.into_iter().map(ToolButton).collect(),
            vec![Cmd(Icon::Offset, Command::Offset), Cmd(Icon::Weld, Command::Boolean(BooleanOp::Union)), Cmd(Icon::Subtract, Command::Boolean(BooleanOp::Subtract)), Cmd(Icon::Intersect, Command::Boolean(BooleanOp::Intersect))],
            vec![Cmd(Icon::GridArray, Command::GridArray), Cmd(Icon::CircularArray, Command::CircularArray)],
            vec![Cmd(Icon::Trace, Command::TraceImage), Cmd(Icon::Barcode, Command::Barcode)],
            vec![ToolButton(Tool::Trim), ToolButton(Tool::Knife), ToolButton(Tool::ShapeBuilder), ToolButton(Tool::Dimension), ToolButton(Tool::Eraser)],
            vec![ToolButton(Tool::Corner)],
        ]
    }

    /// The narrowest the left strip may be so all its icons fit in `height` without scrolling:
    /// it gains a column whenever one column would be too tall.
    pub(crate) fn left_toolbar_min_width(height: f32) -> f32 {
        let groups = Self::left_groups();
        let row = LEFT + GAP;
        // Separators, the corner size controls, and room for the polygon/star fields.
        let extra = groups.len() as f32 * 9.0 + 110.0;
        let cols = (1..=4)
            .find(|&c| groups.iter().map(|g| g.len().div_ceil(c) as f32 * row).sum::<f32>() + extra <= height)
            .unwrap_or(4);
        cols as f32 * row + 14.0
    }

    fn left_item(&mut self, ui: &mut egui::Ui, item: LeftItem) {
        match item {
            LeftItem::ToolButton(tool) => {
                let tip = match (tool, tool.key()) {
                    (Tool::Trim, _) => "Trim: click a piece of outline to delete it back to the lines crossing it".to_string(),
                    (Tool::Knife, _) => "Knife: drag a line across shapes to split them".to_string(),
                    (Tool::ShapeBuilder, _) => "Shape Builder (B): click or drag across overlapping areas to merge them (Alt: delete)".to_string(),
                    (Tool::Dimension, _) => "Dimension: drag between two points to label a distance (never cut)".to_string(),
                    (Tool::Eraser, _) => "Image eraser: drag over a picture to rub parts of it out".to_string(),
                    (Tool::Corner, _) => "Round corner: click corners to round them one at a time with this radius (Tools > Round All Corners does a whole selection)".to_string(),
                    (_, Some(k)) => format!("{} ({})", tool.label(), k.name()),
                    (Tool::Select, None) => format!("{} (Esc)", tool.label()),
                    (_, None) => tool.label().to_string(),
                };
                if icons::button(ui, tool.icon(), LEFT, self.canvas.tool == tool, true, &tip).clicked() {
                    self.canvas.set_tool(tool);
                }
            }
            LeftItem::Cmd(icon, cmd) => self.command_icon(ui, icon, cmd, LEFT),
        }
    }

    /// Icons in rows of as many as fit the strip's width, centred.
    fn left_group(&mut self, ui: &mut egui::Ui, items: Vec<LeftItem>) {
        let width = ui.available_width();
        let cols = (((width + GAP) / (LEFT + GAP)).floor() as usize).max(1);
        for row in items.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                let used = row.len() as f32 * (LEFT + GAP) - GAP;
                ui.add_space(((width - used) / 2.0).max(0.0));
                for &item in row {
                    self.left_item(ui, item);
                }
            });
        }
    }

    pub(crate) fn left_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = GAP;
            let mut groups = Self::left_groups().into_iter();
            if let Some(tools) = groups.next() {
                self.left_group(ui, tools);
            }
            // Number fields fill the bar's width (minus a margin), with units in the label.
            let field = egui::vec2((ui.available_width() - 6.0).max(30.0), 20.0);
            if matches!(self.canvas.tool, Tool::Polygon | Tool::Star) {
                ui.vertical_centered(|ui| {
                    ui.small(if self.canvas.tool == Tool::Polygon { "Sides" } else { "Points" });
                    ui.add_sized(field, egui::DragValue::new(&mut self.canvas.polygon_sides).range(3..=64));
                    if self.canvas.tool == Tool::Star {
                        ui.small("Inner %");
                        let mut pct = self.canvas.star_ratio * 100.0;
                        if ui.add_sized(field, egui::DragValue::new(&mut pct).range(5.0..=95.0)).changed() {
                            self.canvas.star_ratio = pct / 100.0;
                        }
                    }
                });
            }
            for group in groups {
                ui.separator();
                self.left_group(ui, group);
            }
            if self.canvas.tool == Tool::Eraser {
                ui.vertical_centered(|ui| {
                    let c = &mut self.canvas;
                    ui.selectable_value(&mut c.eraser_by_colour, false, "Brush").on_hover_text("Drag over a picture to rub it out");
                    ui.selectable_value(&mut c.eraser_by_colour, true, "Colour").on_hover_text("Click an area of one colour (a background, say) to clear it");
                    if c.eraser_by_colour {
                        ui.small("Tolerance");
                        let mut pct = c.eraser_tolerance * 100.0;
                        if ui.add_sized(field, egui::DragValue::new(&mut pct).range(0.0..=100.0).speed(0.5).suffix("%")).on_hover_text("How different a colour may be and still be cleared").changed() {
                            c.eraser_tolerance = pct / 100.0;
                        }
                        ui.checkbox(&mut c.eraser_everywhere, "All").on_hover_text("Clear that colour everywhere in the picture, not just the area touching the click");
                    } else {
                        ui.small("Brush (mm)");
                        ui.add_sized(field, egui::DragValue::new(&mut c.eraser_mm).range(0.2..=100.0).speed(0.1));
                    }
                });
            }
            let u = self.prefs.units;
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    use bc_geom::CornerStyle;
                    const SMALL: f32 = 20.0;
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.add_space(((ui.available_width() - (2.0 * SMALL + 2.0)) / 2.0).max(0.0));
                    for (style, icon, tip) in [(CornerStyle::Round, Icon::Corner, "Round corners"), (CornerStyle::Chamfer, Icon::Chamfer, "Chamfer (bevel) corners")] {
                        if icons::button(ui, icon, SMALL, self.corner_style == style, true, tip).clicked() {
                            self.corner_style = style;
                        }
                    }
                });
                ui.small(format!("{} ({})", if self.corner_style == bc_geom::CornerStyle::Round { "Radius" } else { "Chamfer" }, u.suffix().trim()));
                let k = u.mm_per_unit();
                let mut v = self.corner_radius / k;
                let drag = egui::DragValue::new(&mut v).range(0.0..=500.0 / k).speed(0.1 / k).fixed_decimals(u.decimals());
                if ui.add_sized(field, drag).on_hover_text("Corner radius used by Round Corners").changed() {
                    self.corner_radius = v * k;
                }
            });
        });
    }
}

/// One button on the left strip.
#[derive(Clone, Copy)]
enum LeftItem {
    ToolButton(Tool),
    Cmd(Icon, Command),
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_gains_columns_on_short_windows() {
        let one = LEFT + GAP + 14.0;
        let w = |h: f32| BurnCraftApp::left_toolbar_min_width(h);
        assert_eq!(w(1400.0), one, "a tall window fits one column");
        assert_eq!(w(700.0), 2.0 * (LEFT + GAP) + 14.0, "a laptop-height window needs two");
        assert!(w(420.0) >= 3.0 * (LEFT + GAP) + 14.0, "a very short window needs three or more");
        assert!(w(700.0) <= w(500.0) && w(1400.0) <= w(700.0), "shorter never needs fewer columns");
    }
}