//! Display units for lengths. Everything is stored in millimetres; this only changes what's shown.

use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Units {
    #[default]
    Millimetres,
    Inches,
}

impl Units {
    /// Millimetres per display unit.
    pub fn mm_per_unit(self) -> f64 {
        match self {
            Units::Millimetres => 1.0,
            Units::Inches => 25.4,
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Units::Millimetres => " mm",
            Units::Inches => " in",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Units::Millimetres => "Millimetres",
            Units::Inches => "Inches",
        }
    }

    pub fn decimals(self) -> usize {
        match self {
            Units::Millimetres => 2,
            Units::Inches => 3,
        }
    }

    /// Formats a length given in mm.
    pub fn format(self, mm: f64) -> String {
        format!("{:.*}{}", self.decimals(), mm / self.mm_per_unit(), self.suffix())
    }

    /// A length field (stored in mm) with ▲▼ step buttons of one display unit (0.1 in for inches).
    /// Returns whether it changed.
    pub fn spin(self, ui: &mut egui::Ui, mm: &mut f64, range: std::ops::RangeInclusive<f64>, width: f32) -> bool {
        let k = self.mm_per_unit();
        let mut v = *mm / k;
        let step = match self {
            Units::Millimetres => 1.0,
            Units::Inches => 0.1,
        };
        let changed = spin(ui, &mut v, range.start() / k..=range.end() / k, step, 0.5 / k, self.decimals(), self.suffix(), width);
        if changed {
            *mm = v * k;
        }
        changed
    }

    /// A drag-value editor for a length stored in mm. Returns whether it changed.
    pub fn drag(self, ui: &mut egui::Ui, mm: &mut f64, range: std::ops::RangeInclusive<f64>) -> bool {
        let k = self.mm_per_unit();
        let mut v = *mm / k;
        let changed = ui
            .add(egui::DragValue::new(&mut v).range(range.start() / k..=range.end() / k).speed(0.5 / k).fixed_decimals(self.decimals()).suffix(self.suffix()))
            .changed();
        if changed {
            *mm = v * k;
        }
        changed
    }
}

/// Whether speeds are shown per second (usual for CO₂ and galvo lasers) or per minute (usual for
/// diode lasers and G-code, whose feed rates are per minute).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SpeedUnit {
    #[default]
    PerSecond,
    PerMinute,
}

impl SpeedUnit {
    pub const ALL: [SpeedUnit; 2] = [SpeedUnit::PerSecond, SpeedUnit::PerMinute];
}

/// How speeds are shown. Speeds are always stored in mm/s; this only changes what's shown and typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Speed {
    pub units: Units,
    pub per: SpeedUnit,
    /// Shown as a LaserPecker 2 "depth" (1-100 %, deeper is slower) instead of a speed.
    pub depth: bool,
}

impl Speed {
    pub fn new(units: Units, per: SpeedUnit) -> Self {
        Self { units, per, depth: false }
    }

    /// The same, shown as a LaserPecker 2 depth.
    pub fn as_depth(self) -> Self {
        Self { depth: true, ..self }
    }

    /// Display units per mm/s.
    fn factor(self) -> f64 {
        let time = match self.per {
            SpeedUnit::PerSecond => 1.0,
            SpeedUnit::PerMinute => 60.0,
        };
        time / self.units.mm_per_unit()
    }

    /// What the field is called: "Depth" on a LaserPecker 2, where it runs the other way to a speed.
    pub fn label(self) -> &'static str {
        if self.depth { "Depth" } else { "Speed" }
    }

    /// The unit's name, e.g. "mm/min".
    pub fn unit(self) -> &'static str {
        if self.depth {
            return "% depth";
        }
        match (self.units, self.per) {
            (Units::Millimetres, SpeedUnit::PerSecond) => "mm/s",
            (Units::Millimetres, SpeedUnit::PerMinute) => "mm/min",
            (Units::Inches, SpeedUnit::PerSecond) => "in/s",
            (Units::Inches, SpeedUnit::PerMinute) => "in/min",
        }
    }

    fn decimals(self) -> usize {
        if self.depth {
            return 0;
        }
        match (self.units, self.per) {
            (Units::Millimetres, SpeedUnit::PerSecond) => 1,
            (Units::Millimetres, SpeedUnit::PerMinute) => 0,
            (Units::Inches, SpeedUnit::PerSecond) => 2,
            (Units::Inches, SpeedUnit::PerMinute) => 1,
        }
    }

    /// A speed given in mm/s, in display units.
    pub fn value(self, mm_s: f64) -> f64 {
        if self.depth { bc_lp2::depth(mm_s) } else { mm_s * self.factor() }
    }

    /// A speed given in mm/s as a number in display units, without the unit.
    pub fn number(self, mm_s: f64) -> String {
        let v = self.value(mm_s);
        // Whole numbers without decimals ("1000", not "1000.0").
        if (v - v.round()).abs() < 1e-6 { format!("{v:.0}") } else { format!("{v:.*}", self.decimals()) }
    }

    /// A speed given in mm/s, e.g. "1200 mm/min".
    pub fn format(self, mm_s: f64) -> String {
        format!("{} {}", self.number(mm_s), self.unit())
    }

    /// A drag-value editor for a speed stored in mm/s; `range` is in mm/s.
    pub fn edit(self, mm_s: &mut f64, range: std::ops::RangeInclusive<f64>) -> SpeedEdit<'_> {
        SpeedEdit { speed: self, mm_s, range, prefix: "" }
    }
}

/// A speed field (see [`Speed::edit`]).
pub struct SpeedEdit<'a> {
    speed: Speed,
    mm_s: &'a mut f64,
    range: std::ops::RangeInclusive<f64>,
    prefix: &'static str,
}

impl SpeedEdit<'_> {
    pub fn prefix(mut self, prefix: &'static str) -> Self {
        self.prefix = prefix;
        self
    }
}

impl egui::Widget for SpeedEdit<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        if self.speed.depth {
            let mut depth = bc_lp2::depth(*self.mm_s);
            // Labelled "Depth" beside it (see `Speed::label`).
            let r = ui.add(egui::DragValue::new(&mut depth).range(1.0..=100.0).speed(0.25).max_decimals(0).prefix(self.prefix).suffix(" %"));
            if r.changed() {
                *self.mm_s = bc_lp2::speed_for_depth(depth);
            }
            return r;
        }
        let k = self.speed.factor();
        let mut v = *self.mm_s * k;
        let r = ui.add(
            egui::DragValue::new(&mut v)
                .range(self.range.start() * k..=self.range.end() * k)
                .speed(0.25 * k)
                .max_decimals(self.speed.decimals())
                .prefix(self.prefix)
                .suffix(format!(" {}", self.speed.unit())),
        );
        if r.changed() {
            *self.mm_s = v / k;
        }
        r
    }
}

/// A number field with small ▲▼ buttons beside it that step by `step`. Drag or type as usual.
#[allow(clippy::too_many_arguments)]
pub fn spin(ui: &mut egui::Ui, value: &mut f64, range: std::ops::RangeInclusive<f64>, step: f64, speed: f64, decimals: usize, suffix: &str, width: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        let h = ui.spacing().interact_size.y;
        let field = egui::DragValue::new(value).range(range.clone()).speed(speed).fixed_decimals(decimals).suffix(suffix);
        changed |= ui.add_sized([width, h], field).changed();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(13.0, h), egui::Sense::hover());
        let enabled = ui.is_enabled();
        for (up, half) in [(true, rect.split_top_bottom_at_fraction(0.5).0), (false, rect.split_top_bottom_at_fraction(0.5).1)] {
            let id = ui.id().with((rect.min.x as i32, rect.min.y as i32, up));
            let r = ui.interact(half, id, if enabled { egui::Sense::click() } else { egui::Sense::hover() });
            let visuals = ui.style().interact(&r);
            ui.painter().rect_filled(half.shrink(0.5), 2.0, if r.hovered() && enabled { visuals.bg_fill } else { ui.visuals().widgets.inactive.weak_bg_fill });
            let c = half.center();
            let (dx, dy) = (3.5, 2.0);
            let tri = if up {
                vec![c + egui::vec2(-dx, dy), c + egui::vec2(dx, dy), c + egui::vec2(0.0, -dy)]
            } else {
                vec![c + egui::vec2(-dx, -dy), c + egui::vec2(dx, -dy), c + egui::vec2(0.0, dy)]
            };
            let ink = if enabled { visuals.fg_stroke.color } else { ui.visuals().weak_text_color() };
            ui.painter().add(egui::Shape::convex_polygon(tri, ink, egui::Stroke::NONE));
            if r.clicked() {
                let next = (*value + if up { step } else { -step }).clamp(*range.start(), *range.end());
                if next != *value {
                    *value = next;
                    changed = true;
                }
            }
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(Units::Millimetres.format(12.345), "12.35 mm");
        assert_eq!(Units::Inches.format(25.4), "1.000 in");
    }

    #[test]
    fn speeds() {
        let per_min = Speed::new(Units::Millimetres, SpeedUnit::PerMinute);
        assert_eq!(per_min.format(50.0), "3000 mm/min");
        assert_eq!(Speed::default().format(16.25), "16.2 mm/s");
        assert_eq!(Speed::default().format(100.0), "100 mm/s");
        assert_eq!(Speed::new(Units::Inches, SpeedUnit::PerSecond).format(25.4), "1 in/s");
        assert_eq!(Speed::new(Units::Inches, SpeedUnit::PerMinute).format(10.0), "23.6 in/min");
        // Typed in mm/min, stored in mm/s, shown again as typed.
        let stored = 1000.0 / 60.0;
        assert_eq!(per_min.number(stored), "1000");
    }
}
