//! G-code generation for laser controllers: GRBL (M4 or M3), Smoothieware, Marlin and a custom dialect.

use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// A polyline in machine mm, with a power fraction of the layer's range (0 = laser off).
#[derive(Debug, Clone, PartialEq)]
pub struct GrblPath {
    pub points: Vec<(f64, f64)>,
    pub power: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrblLayer {
    /// mm/s
    pub speed: f64,
    /// Percent of full power.
    pub min_power: f64,
    pub max_power: f64,
    pub air_assist: bool,
    pub paths: Vec<GrblPath>,
}

/// Which G-code flavour the controller speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Dialect {
    /// GRBL 1.1 laser mode with dynamic power (M4).
    #[default]
    Grbl,
    /// GRBL with constant power (M3), for controllers without M4.
    GrblM3,
    /// Smoothieware laser module: power is a 0–1 fraction on each G1.
    Smoothieware,
    /// Marlin with the laser feature: M3 S… / M5 around cuts.
    Marlin,
    /// User-defined commands.
    Custom,
}

impl Dialect {
    pub const ALL: [Dialect; 5] = [Dialect::Grbl, Dialect::GrblM3, Dialect::Smoothieware, Dialect::Marlin, Dialect::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Dialect::Grbl => "GRBL",
            Dialect::GrblM3 => "GRBL-M3",
            Dialect::Smoothieware => "Smoothieware",
            Dialect::Marlin => "Marlin",
            Dialect::Custom => "Custom G-code",
        }
    }

    /// The S value meaning full power, by convention.
    pub fn default_s_max(self) -> f64 {
        match self {
            Dialect::Grbl | Dialect::GrblM3 | Dialect::Custom => 1000.0,
            Dialect::Smoothieware => 1.0,
            Dialect::Marlin => 255.0,
        }
    }

    /// GRBL-style real-time commands and character-counting flow control.
    pub fn is_grbl(self) -> bool {
        matches!(self, Dialect::Grbl | Dialect::GrblM3)
    }
}

/// Commands for the custom dialect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomGcode {
    /// Lines sent before the job (one per line).
    pub start: String,
    /// Lines sent after the job.
    pub end: String,
    /// Turns the laser on (followed by ` S<power>`).
    pub laser_on: String,
    pub laser_off: String,
    /// Put `S<power>` on each G1 instead of using laser on/off commands around cuts.
    pub inline_power: bool,
    pub home: String,
    pub air_on: String,
    pub air_off: String,
}

impl Default for CustomGcode {
    fn default() -> Self {
        Self {
            start: "G21\nG90".into(),
            end: String::new(),
            laser_on: "M3".into(),
            laser_off: "M5".into(),
            inline_power: true,
            home: "G28".into(),
            air_on: "M8".into(),
            air_off: "M9".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcodeOptions {
    /// The S value that means full power (`$30` on GRBL).
    pub s_max: f64,
    pub dialect: Dialect,
    pub custom: CustomGcode,
    /// The machine's own lines sent at the start and end of every job (any dialect).
    pub start_gcode: String,
    pub end_gcode: String,
    /// Air assist with M7 (mist) instead of M8 (flood).
    pub air_m7: bool,
    /// Rotary on a 4th axis: write the Y coordinate as A.
    pub y_as_a: bool,
}

impl Default for GcodeOptions {
    fn default() -> Self {
        Self { s_max: 1000.0, dialect: Dialect::Grbl, custom: CustomGcode::default(), start_gcode: String::new(), end_gcode: String::new(), air_m7: false, y_as_a: false }
    }
}

fn gcode_lines(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from)
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// How the dialect turns the laser on and off.
enum PowerStyle<'a> {
    /// `S` on each G1; G0 moves with the laser off (GRBL laser mode, Smoothieware, custom inline).
    Inline,
    /// Laser-on command with S before cuts, laser-off command before travel (Marlin, custom).
    Commands { on: &'a str, off: &'a str },
}

/// Generates the program for `layers`.
pub fn generate(layers: &[GrblLayer], options: &GcodeOptions) -> Vec<String> {
    let d = options.dialect;
    let c = &options.custom;
    let style = match d {
        Dialect::Marlin => PowerStyle::Commands { on: "M3", off: "M5" },
        Dialect::Custom if !c.inline_power => PowerStyle::Commands { on: &c.laser_on, off: &c.laser_off },
        _ => PowerStyle::Inline,
    };
    let (air_on, air_off) = match d {
        Dialect::Custom => (c.air_on.as_str(), c.air_off.as_str()),
        _ if options.air_m7 => ("M7", "M9"),
        _ => ("M8", "M9"),
    };
    let mut out: Vec<String> = match d {
        Dialect::Custom => gcode_lines(&c.start).collect(),
        _ => vec!["G21".into(), "G90".into()],
    };
    out.extend(gcode_lines(&options.start_gcode));
    match d {
        Dialect::Grbl => out.push("M4 S0".into()),
        Dialect::GrblM3 => out.push("M3 S0".into()),
        Dialect::Custom if c.inline_power => out.push(format!("{} S0", c.laser_on)),
        _ => {}
    }
    let y_letter = if options.y_as_a { 'A' } else { 'Y' };
    let s_text = |s: f64| if d == Dialect::Smoothieware { format!("{:.3}", s) } else { format!("{}", s.round() as i64) };
    let mut feed: Option<i64> = None;
    let mut s_value: Option<String> = None;
    let mut laser_on = false;
    let mut pos: Option<(f64, f64)> = None;
    let mut air: Option<bool> = None;
    for layer in layers {
        if air != Some(layer.air_assist) {
            if !(air_on.is_empty() && air_off.is_empty()) {
                out.push(if layer.air_assist { air_on } else { air_off }.to_string());
            }
            air = Some(layer.air_assist);
        }
        let layer_feed = (layer.speed * 60.0).round() as i64;
        for path in layer.paths.iter().filter(|p| p.points.len() >= 2) {
            let s = if path.power <= 0.0 {
                0.0
            } else {
                let pct = if path.power >= 1.0 { layer.max_power } else { layer.min_power + path.power as f64 * (layer.max_power - layer.min_power) };
                pct.clamp(0.0, 100.0) / 100.0 * options.s_max
            };
            let s = s_text(s);
            if pos != Some(path.points[0]) {
                if let PowerStyle::Commands { off, .. } = style
                    && laser_on
                {
                    out.push(off.to_string());
                    laser_on = false;
                }
                out.push(format!("G0 X{} {y_letter}{}", fmt(path.points[0].0), fmt(path.points[0].1)));
            }
            if let PowerStyle::Commands { on, off } = style {
                let wants_on = path.power > 0.0;
                if wants_on && (!laser_on || s_value.as_deref() != Some(s.as_str())) {
                    out.push(format!("{on} S{s}"));
                    laser_on = true;
                    s_value = Some(s.clone());
                } else if !wants_on && laser_on {
                    out.push(off.to_string());
                    laser_on = false;
                }
            }
            for &(x, y) in &path.points[1..] {
                let mut line = format!("G1 X{} {y_letter}{}", fmt(x), fmt(y));
                if matches!(style, PowerStyle::Inline) && s_value.as_deref() != Some(s.as_str()) {
                    let _ = write!(line, " S{s}");
                    s_value = Some(s.clone());
                }
                if feed != Some(layer_feed) {
                    let _ = write!(line, " F{layer_feed}");
                    feed = Some(layer_feed);
                }
                out.push(line);
            }
            pos = Some(*path.points.last().unwrap());
        }
    }
    match (&style, d) {
        (PowerStyle::Commands { off, .. }, _) => out.push(off.to_string()),
        (_, Dialect::Custom) => out.push(c.laser_off.clone()),
        (_, Dialect::Smoothieware) => {}
        _ => out.push("M5".into()),
    }
    if air == Some(true) && !air_off.is_empty() {
        out.push(air_off.to_string());
    }
    out.extend(gcode_lines(&options.end_gcode));
    if d == Dialect::Custom {
        out.extend(gcode_lines(&c.end));
    }
    out.retain(|l| !l.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(paths: Vec<GrblPath>) -> GrblLayer {
        GrblLayer { speed: 50.0, min_power: 10.0, max_power: 60.0, air_assist: true, paths }
    }

    fn sq() -> GrblPath {
        GrblPath { points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)], power: 1.0 }
    }

    #[test]
    fn rotary_on_a_axis() {
        let out = generate(&[layer(vec![sq()])], &GcodeOptions { y_as_a: true, ..Default::default() });
        assert!(out.iter().any(|l| l.starts_with("G1 X10 A10")));
        assert!(!out.iter().any(|l| l.contains(" Y")));
    }

    #[test]
    fn machine_start_end_and_m7_air() {
        let options = GcodeOptions { s_max: 255.0, start_gcode: "M3030\n\n".into(), end_gcode: " M3001 ".into(), air_m7: true, ..Default::default() };
        let out = generate(&[layer(vec![sq()])], &options);
        assert_eq!(&out[..3], &["G21", "G90", "M3030"]);
        assert!(out.contains(&"M7".to_string()) && !out.contains(&"M8".to_string()));
        assert_eq!(out.last().unwrap(), "M3001");
        assert!(out.iter().any(|l| l.ends_with("S153 F3000")), "60% of 255: {out:?}");
    }

    fn opts(d: Dialect) -> GcodeOptions {
        GcodeOptions { s_max: d.default_s_max(), dialect: d, ..Default::default() }
    }

    #[test]
    fn grbl_square_program() {
        let g = generate(&[layer(vec![sq()])], &opts(Dialect::Grbl));
        // The head position is unknown at the start, so the first path is always reached with G0.
        assert_eq!(g[..7], ["G21", "G90", "M4 S0", "M8", "G0 X0 Y0", "G1 X10 Y0 S600 F3000", "G1 X10 Y10"]);
        assert_eq!(g[g.len() - 2..], ["M5", "M9"]);
        assert_eq!(g.iter().filter(|l| l.starts_with("G0")).count(), 1);
        assert_eq!(generate(&[layer(vec![sq()])], &opts(Dialect::GrblM3))[2], "M3 S0");
    }

    #[test]
    fn travel_power_levels_and_overscan() {
        let g = generate(
            &[layer(vec![
                GrblPath { points: vec![(5.0, 5.0), (7.0, 5.0)], power: 0.0 },
                GrblPath { points: vec![(7.0, 5.0), (9.5, 5.0)], power: 0.5 },
            ])],
            &opts(Dialect::Grbl),
        );
        assert!(g.contains(&"G0 X5 Y5".to_string()));
        assert!(g.contains(&"G1 X7 Y5 S0 F3000".to_string()), "overscan moves at cut speed with S0: {g:?}");
        assert!(g.contains(&"G1 X9.5 Y5 S350".to_string()), "half power between 10% and 60%: {g:?}");
    }

    #[test]
    fn smoothieware_uses_fractional_power() {
        let g = generate(&[layer(vec![sq()])], &opts(Dialect::Smoothieware));
        assert!(g.contains(&"G1 X10 Y0 S0.600 F3000".to_string()), "{g:?}");
        assert!(!g.iter().any(|l| l.starts_with("M3") || l.starts_with("M4")));
    }

    #[test]
    fn marlin_switches_the_laser_around_cuts() {
        let two = vec![sq(), GrblPath { points: vec![(50.0, 50.0), (60.0, 50.0)], power: 1.0 }];
        let g = generate(&[layer(two)], &opts(Dialect::Marlin));
        let first_on = g.iter().position(|l| l == "M3 S153").expect("60% of 255");
        let first_g1 = g.iter().position(|l| l.starts_with("G1")).unwrap();
        assert!(first_on < first_g1);
        let travel = g.iter().position(|l| l == "G0 X50 Y50").unwrap();
        assert_eq!(g[travel - 1], "M5", "laser off before travel: {g:?}");
        assert_eq!(g[travel + 1], "M3 S153", "and back on after: {g:?}");
        assert!(!g.iter().any(|l| l.starts_with("G1") && l.contains(" S")), "no inline S on Marlin");
    }

    #[test]
    fn custom_dialect_uses_its_commands() {
        let mut o = opts(Dialect::Custom);
        o.custom.start = "G21\nG90\n; hello".into();
        o.custom.end = "M2".into();
        o.custom.inline_power = false;
        o.custom.laser_on = "M106".into();
        o.custom.laser_off = "M107".into();
        let g = generate(&[layer(vec![sq()])], &o);
        assert_eq!(g[..3], ["G21", "G90", "; hello"]);
        assert!(g.contains(&"M106 S600".to_string()));
        assert_eq!(g.last().unwrap(), "M2");
        assert!(g.contains(&"M107".to_string()));
    }
}
