//! Galvo settings, field coordinates and marking-list encoding.

use crate::protocol::*;
use serde::{Deserialize, Serialize};

/// The laser source type changes how power and frequency are sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Source {
    #[default]
    Fiber,
    Co2,
    Uv,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Fiber => "Fiber",
            Source::Co2 => "CO2 (galvo)",
            Source::Uv => "UV",
        }
    }
}

/// Device settings for a BJJCZ galvo laser. Defaults follow common EZCad2 values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GalvoSettings {
    pub source: Source,
    /// Width (and height) of the marking field, in mm.
    pub lens_size: f64,
    pub flip_x: bool,
    pub flip_y: bool,
    pub swap_xy: bool,
    /// Travel (jump) speed in mm/s.
    pub jump_speed: f64,
    /// Red-light framing speed in mm/s.
    pub light_speed: f64,
    /// Delays in µs.
    pub laser_on_delay: f64,
    pub laser_off_delay: f64,
    pub polygon_delay: f64,
    pub jump_delay_long: f64,
    pub jump_delay_short: f64,
    /// Jumps longer than this (mm) use the long delay.
    pub jump_distance_long: f64,
    /// Fiber MO (master oscillator) warm-up delay, in ms.
    pub open_mo_delay: f64,
    /// Fiber MOPA pulse width in ns (0 = don't send).
    pub pulse_width: u16,
    // Board initialisation values.
    pub first_pulse_killer: u16,
    pub pwm_half_period: u16,
    pub pwm_pulse_width: u16,
    pub standby_1: u16,
    pub standby_2: u16,
    pub timing_mode: u16,
    pub delay_mode: u16,
    pub laser_mode: u16,
    pub control_mode: u16,
    pub fpk2: [u16; 4],
    pub fly_res: [u16; 4],
}

impl Default for GalvoSettings {
    fn default() -> Self {
        Self {
            source: Source::Fiber,
            lens_size: 110.0,
            flip_x: false,
            flip_y: true,
            swap_xy: true,
            jump_speed: 2000.0,
            light_speed: 3000.0,
            laser_on_delay: 100.0,
            laser_off_delay: 100.0,
            polygon_delay: 100.0,
            jump_delay_long: 200.0,
            jump_delay_short: 8.0,
            jump_distance_long: 10.0,
            open_mo_delay: 8.0,
            pulse_width: 0,
            first_pulse_killer: 200,
            pwm_half_period: 125,
            pwm_pulse_width: 125,
            standby_1: 2000,
            standby_2: 20,
            timing_mode: 1,
            delay_mode: 1,
            laser_mode: 1,
            control_mode: 0,
            fpk2: [0xFFB, 1, 409, 100],
            fly_res: [0, 99, 1000, 25],
        }
    }
}

impl GalvoSettings {
    /// Galvo steps per mm.
    pub fn units_per_mm(&self) -> f64 {
        65535.0 / self.lens_size.max(1.0)
    }

    /// Field mm (origin top-left of the field, Y down) to galvo units, or `None` outside the field.
    pub fn to_galvo(&self, (x, y): (f64, f64)) -> Option<(u16, u16)> {
        let half = self.lens_size / 2.0;
        let (mut cx, mut cy) = (x - half, y - half);
        if self.flip_x {
            cx = -cx;
        }
        if self.flip_y {
            cy = -cy;
        }
        if self.swap_xy {
            (cx, cy) = (cy, cx);
        }
        let k = self.units_per_mm();
        let (gx, gy) = (0x8000 as f64 + cx * k, 0x8000 as f64 + cy * k);
        // One unit of slack so the exact field edge (65535.5) still counts as inside.
        let inside = |v: f64| (-1.0..=65536.0).contains(&v);
        (inside(gx) && inside(gy)).then(|| (gx.round().clamp(0.0, 65535.0) as u16, gy.round().clamp(0.0, 65535.0) as u16))
    }

    /// mm/s to the board's speed units.
    pub fn speed(&self, mm_s: f64) -> u16 {
        (mm_s * self.units_per_mm() / 1000.0).abs().min(65535.0) as u16
    }
}

/// A layer's marking parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarkSettings {
    /// mm/s
    pub speed: f64,
    /// Percent.
    pub power: f64,
    /// Pulse frequency, kHz.
    pub frequency: f64,
}

/// A layer to mark: settings and polylines in field mm (laser off along paths with `laser == false`).
#[derive(Debug, Clone, PartialEq)]
pub struct GalvoLayer {
    pub settings: MarkSettings,
    pub paths: Vec<(Vec<(f64, f64)>, bool)>,
}

/// Commands that set a layer's speed, power, frequency and delays.
fn layer_settings(s: &GalvoSettings, m: &MarkSettings, out: &mut Vec<Command>) {
    let f = m.frequency.max(1.0);
    match s.source {
        Source::Fiber => {
            out.push(command(LIST_QSWITCH_PERIOD, &[((20000.0 / f).round() as u32 & 0xFFFF) as u16]));
            out.push(command(LIST_MARK_CURRENT, &[(m.power.clamp(0.0, 100.0) * 4095.0 / 100.0).round() as u16]));
            if s.pulse_width > 0 {
                out.push(command(LIST_FIBER_YLPM_PULSE_WIDTH, &[s.pulse_width]));
            }
        }
        Source::Co2 | Source::Uv => {
            out.push(command(LIST_MARK_FREQ, &[((10000.0 / f).round() as u32 & 0xFFFF) as u16]));
            out.push(command(LIST_MARK_POWER_RATIO, &[(200.0 * m.power.clamp(0.0, 100.0) / f).round().min(65535.0) as u16]));
        }
    }
    out.push(command(LIST_MARK_SPEED, &[s.speed(m.speed)]));
    out.push(command(LIST_LASER_ON_DELAY, &signed_delay(s.laser_on_delay)));
    out.push(command(LIST_LASER_OFF_DELAY, &signed_delay(s.laser_off_delay)));
    out.push(command(LIST_POLYGON_DELAY, &[(s.polygon_delay / 10.0).clamp(0.0, 65535.0) as u16]));
}

/// Tracks the current point and emits jumps/marks with distances and jump delays.
struct Pen<'a> {
    s: &'a GalvoSettings,
    at: Option<(u16, u16)>,
    jump_delay: Option<u16>,
    skipped: usize,
}

impl Pen<'_> {
    fn distance(&self, to: (u16, u16)) -> f64 {
        self.at.map(|(x, y)| ((to.0 as f64 - x as f64).powi(2) + (to.1 as f64 - y as f64).powi(2)).sqrt()).unwrap_or(65535.0)
    }

    fn jump(&mut self, p: (f64, f64), out: &mut Vec<Command>) {
        let Some(g) = self.s.to_galvo(p) else {
            self.skipped += 1;
            return;
        };
        if self.at == Some(g) {
            return;
        }
        let d = self.distance(g);
        let long = d > self.s.jump_distance_long * self.s.units_per_mm();
        let delay = (if long { self.s.jump_delay_long } else { self.s.jump_delay_short }).round().clamp(0.0, 65535.0) as u16;
        if self.jump_delay != Some(delay) {
            out.push(command(LIST_JUMP_DELAY, &[delay, 0]));
            self.jump_delay = Some(delay);
        }
        out.push(command(LIST_JUMP_TO, &[g.0, g.1, 0, d.min(65535.0) as u16]));
        self.at = Some(g);
    }

    fn mark(&mut self, p: (f64, f64), out: &mut Vec<Command>) {
        let Some(g) = self.s.to_galvo(p) else {
            self.skipped += 1;
            return;
        };
        if self.at == Some(g) {
            return;
        }
        let d = self.distance(g);
        out.push(command(LIST_MARK_TO, &[g.0, g.1, 0, d.min(65535.0) as u16]));
        self.at = Some(g);
    }
}

/// The program-mode list for a job: ready, (fiber MO delay), port, jump speed, then each layer.
/// Returns the commands and how many points fell outside the field (and were left out).
pub fn encode_job(s: &GalvoSettings, port_bits: u16, layers: &[GalvoLayer]) -> (Vec<Command>, usize) {
    let mut out = vec![command(LIST_READY_MARK, &[])];
    if s.source == Source::Fiber {
        out.push(command(LIST_DELAY_TIME, &[(s.open_mo_delay * 100.0).round().clamp(0.0, 65535.0) as u16]));
    }
    out.push(command(LIST_WRITE_PORT, &[port_bits]));
    out.push(command(LIST_JUMP_SPEED, &[s.speed(s.jump_speed)]));
    let mut pen = Pen { s, at: None, jump_delay: None, skipped: 0 };
    for layer in layers {
        layer_settings(s, &layer.settings, &mut out);
        for (points, laser) in layer.paths.iter().filter(|(p, _)| p.len() >= 2) {
            pen.jump(points[0], &mut out);
            for &p in &points[1..] {
                if *laser {
                    pen.mark(p, &mut out);
                } else {
                    pen.jump(p, &mut out);
                }
            }
        }
    }
    out.push(command(LIST_END_OF_LIST, &[]));
    (out, pen.skipped)
}

/// A red-light list tracing `outline` (field mm) `loops` times.
pub fn encode_light(s: &GalvoSettings, port_bits: u16, outline: &[(f64, f64)], loops: usize) -> Vec<Command> {
    let mut out = vec![command(LIST_READY_MARK, &[]), command(LIST_WRITE_PORT, &[port_bits]), command(LIST_JUMP_SPEED, &[s.speed(s.light_speed)])];
    let mut pen = Pen { s, at: None, jump_delay: None, skipped: 0 };
    for _ in 0..loops {
        for &p in outline {
            pen.jump(p, &mut out);
        }
    }
    out.push(command(LIST_END_OF_LIST, &[]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_centre_and_orientation() {
        let s = GalvoSettings { flip_x: false, flip_y: false, swap_xy: false, ..Default::default() };
        assert_eq!(s.to_galvo((55.0, 55.0)), Some((0x8000, 0x8000)));
        let (x, _) = s.to_galvo((110.0, 55.0)).unwrap();
        assert_eq!(x, 0xFFFF);
        assert!(s.to_galvo((120.0, 55.0)).is_none(), "outside the field");
        let swapped = GalvoSettings { swap_xy: true, ..s.clone() };
        let a = s.to_galvo((80.0, 55.0)).unwrap();
        let b = swapped.to_galvo((80.0, 55.0)).unwrap();
        assert_eq!((a.0, a.1), (b.1, b.0));
    }

    #[test]
    fn job_list_structure() {
        let s = GalvoSettings { source: Source::Fiber, ..Default::default() };
        let layer = GalvoLayer {
            settings: MarkSettings { speed: 1000.0, power: 50.0, frequency: 30.0 },
            paths: vec![(vec![(50.0, 50.0), (60.0, 50.0), (60.0, 60.0)], true), (vec![(0.0, 0.0), (5.0, 0.0)], true)],
        };
        let (cmds, skipped) = encode_job(&s, 1, &[layer]);
        assert_eq!(skipped, 0);
        let ops: Vec<u16> = cmds.iter().map(|c| c[0]).collect();
        assert_eq!(ops[0], LIST_READY_MARK);
        assert!(ops.contains(&LIST_QSWITCH_PERIOD) && ops.contains(&LIST_MARK_CURRENT));
        let q = cmds.iter().find(|c| c[0] == LIST_QSWITCH_PERIOD).unwrap();
        assert_eq!(q[1], 667, "20000 / 30 kHz");
        let p = cmds.iter().find(|c| c[0] == LIST_MARK_CURRENT).unwrap();
        assert_eq!(p[1], 2048, "50% of 0xFFF");
        assert_eq!(ops.iter().filter(|&&o| o == LIST_MARK_TO).count(), 3);
        assert_eq!(ops.iter().filter(|&&o| o == LIST_JUMP_TO).count(), 2);
        // Both jumps are long (> 10 mm): the long delay is set once, before the first.
        let delays: Vec<&Command> = cmds.iter().filter(|c| c[0] == LIST_JUMP_DELAY).collect();
        assert_eq!(delays.len(), 1);
        assert_eq!(delays[0][1], 200);
        assert_eq!(*ops.last().unwrap(), LIST_END_OF_LIST);
    }

    #[test]
    fn co2_power_ratio() {
        let s = GalvoSettings { source: Source::Co2, ..Default::default() };
        let layer = GalvoLayer { settings: MarkSettings { speed: 100.0, power: 50.0, frequency: 20.0 }, paths: vec![(vec![(50.0, 50.0), (51.0, 50.0)], true)] };
        let (cmds, _) = encode_job(&s, 1, &[layer]);
        assert_eq!(cmds.iter().find(|c| c[0] == LIST_MARK_POWER_RATIO).unwrap()[1], 500, "200 × 50 / 20");
        assert_eq!(cmds.iter().find(|c| c[0] == LIST_MARK_FREQ).unwrap()[1], 500, "10000 / 20 kHz");
    }

    #[test]
    fn out_of_field_points_are_counted() {
        let s = GalvoSettings::default();
        let layer = GalvoLayer { settings: MarkSettings { speed: 100.0, power: 10.0, frequency: 30.0 }, paths: vec![(vec![(50.0, 50.0), (500.0, 50.0)], true)] };
        assert_eq!(encode_job(&s, 1, &[layer]).1, 1);
    }
}
