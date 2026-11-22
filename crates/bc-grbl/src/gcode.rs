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
    /// How many times `paths` repeats the layer (one pass after another), so Z can step down
    /// between passes.
    pub passes: u32,
    /// Z axis: the layer's offset (positive raises the head), and how far to lower it after each
    /// pass, in mm. Only written when [`GcodeOptions::z`] is set.
    pub z_offset: f64,
    pub z_step: f64,
    /// Hold the laser on this long (ms) at the start of each burn before moving, and at its end
    /// before turning off (piercing thick material).
    pub delay_on: f64,
    pub delay_off: f64,
    pub paths: Vec<GrblPath>,
}

/// How the machine's Z axis is driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZAxis {
    /// Move Z only by relative amounts (G91), from wherever it was focused, and come back at the
    /// end. Otherwise Z positions are absolute, with 0 at focus.
    pub relative: bool,
    /// Positive Z moves the head down (or the bed up) on this machine.
    pub reversed: bool,
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
    /// The letter the Y coordinate is written as: `Y`, or `A` or `Z` for a rotary wired to
    /// that axis.
    pub y_axis: char,
    /// The machine has a Z axis to use for layer offsets and per-pass steps.
    pub z: Option<ZAxis>,
}

impl Default for GcodeOptions {
    fn default() -> Self {
        Self { s_max: 1000.0, dialect: Dialect::Grbl, custom: CustomGcode::default(), start_gcode: String::new(), end_gcode: String::new(), air_m7: false, y_axis: 'Y', z: None }
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
/// A pause of `ms` milliseconds: G4 takes seconds on GRBL-style controllers and milliseconds on
/// Marlin and Smoothieware.
fn dwell(d: Dialect, ms: f64) -> String {
    match d {
        Dialect::Marlin | Dialect::Smoothieware => format!("G4 P{}", ms.round() as i64),
        _ => format!("G4 P{}", fmt(ms / 1000.0)),
    }
}

/// Lines that hold the laser on at power `s` for `ms`, with the head still. In GRBL's dynamic
/// power mode (M4) the laser goes off when the head stops, so it switches to constant power (M3)
/// for the pause and back to M4 after.
fn hold_on(d: Dialect, c: &CustomGcode, s: &str, ms: f64) -> Vec<String> {
    match d {
        Dialect::Grbl => vec![format!("M3 S{s}"), dwell(d, ms), format!("M4 S{s}")],
        Dialect::GrblM3 => vec![format!("M3 S{s}"), dwell(d, ms)],
        Dialect::Custom if c.inline_power => vec![format!("{} S{s}", c.laser_on), dwell(d, ms)],
        // The laser is already on (switched on by command before the burn).
        _ => vec![dwell(d, ms)],
    }
}

/// Moves Z from `from` to `to` (mm above focus): relative (G91) or absolute.
fn z_move(out: &mut Vec<String>, axis: ZAxis, from: f64, to: f64) {
    let sign = if axis.reversed { -1.0 } else { 1.0 };
    if axis.relative {
        out.push("G91".into());
        out.push(format!("G0 Z{}", fmt(sign * (to - from))));
        out.push("G90".into());
    } else {
        out.push(format!("G0 Z{}", fmt(sign * to)));
    }
}

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
    let y_letter = options.y_axis;
    let s_text = |s: f64| if d == Dialect::Smoothieware { format!("{:.3}", s) } else { format!("{}", s.round() as i64) };
    let mut feed: Option<i64> = None;
    let mut s_value: Option<String> = None;
    let mut laser_on = false;
    let mut pos: Option<(f64, f64)> = None;
    let mut air: Option<bool> = None;
    // Where Z is, relative to focus (positive = head raised).
    let mut z = 0.0;
    for layer in layers {
        if air != Some(layer.air_assist) {
            if !(air_on.is_empty() && air_off.is_empty()) {
                out.push(if layer.air_assist { air_on } else { air_off }.to_string());
            }
            air = Some(layer.air_assist);
        }
        let layer_feed = (layer.speed * 60.0).round() as i64;
        let passes = layer.passes.max(1) as usize;
        let per_pass = if layer.paths.len() % passes == 0 { (layer.paths.len() / passes).max(1) } else { layer.paths.len().max(1) };
        for (i, path) in layer.paths.iter().enumerate() {
            if let Some(axis) = options.z
                && i % per_pass == 0
            {
                let target = layer.z_offset - layer.z_step * (i / per_pass) as f64;
                if (target - z).abs() > 1e-6 {
                    if let PowerStyle::Commands { off, .. } = style
                        && laser_on
                    {
                        out.push(off.to_string());
                        laser_on = false;
                    }
                    z_move(&mut out, axis, z, target);
                    z = target;
                }
            }
            if path.points.len() < 2 {
                continue;
            }
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
            let burning = path.power > 0.0;
            if burning && layer.delay_on > 0.0 {
                out.extend(hold_on(d, c, &s, layer.delay_on));
                s_value = Some(s.clone());
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
            if burning && layer.delay_off > 0.0 {
                out.extend(hold_on(d, c, &s, layer.delay_off));
                s_value = Some(s.clone());
            }
            pos = Some(*path.points.last().unwrap());
        }
    }
    if let Some(axis) = options.z
        && z.abs() > 1e-6
    {
        // Back to focus, where the job started.
        z_move(&mut out, axis, z, 0.0);
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
