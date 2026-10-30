//! G-code import (`.gcode`, `.nc`, `.gc`): the moves made with the laser on become lines.
//!
//! Understands G0/G1 lines and G2/G3 arcs (I/J centre or R radius), G90/G91 absolute and relative
//! positioning, G20/G21 inches and millimetres, and M3/M4/M5 with S for the laser. Each distinct
//! power (S value) gets its own colour, so the job's power levels land on separate layers.
//! G-code's Y points up; the result is flipped so the drawing looks the same as it would burn.

use crate::{ImportResult, Imported};
use bc_geom::{polyline_path, Point};

pub const EXTENSIONS: [&str; 3] = ["gcode", "nc", "gc"];

/// Largest file read, in lines (a raster engraving can be millions of moves).
const MAX_LINES: usize = 5_000_000;

#[derive(Clone, Copy, PartialEq)]
enum Motion {
    Rapid,
    Line,
    ArcCw,
    ArcCcw,
}

pub fn import(data: &[u8]) -> Result<ImportResult, String> {
    let text = String::from_utf8_lossy(data);
    let mut absolute = true;
    let mut scale = 1.0; // mm per unit
    let mut motion = Motion::Rapid;
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let mut laser_on = false;
    let mut power: Option<f64> = None;
    // (power, polyline in G-code coordinates)
    let mut paths: Vec<(Option<u64>, Vec<Point>)> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let mut current_power: Option<u64> = None;
    let mut moves = 0usize;
    let finish = |current: &mut Vec<Point>, paths: &mut Vec<(Option<u64>, Vec<Point>)>, p: Option<u64>| {
        if current.len() >= 2 {
            paths.push((p, std::mem::take(current)));
        }
        current.clear();
    };
    for line in text.lines().take(MAX_LINES) {
        // Strip ( … ) and ; comments.
        let mut clean = String::with_capacity(line.len());
        let mut depth = 0;
        for c in line.chars() {
            match c {
                ';' if depth == 0 => break,
                '(' => depth += 1,
                ')' if depth > 0 => depth -= 1,
                _ if depth == 0 => clean.push(c.to_ascii_uppercase()),
                _ => {}
            }
        }
        // Words: a letter followed by a number.
        let mut words: Vec<(char, f64)> = Vec::new();
        let chars: Vec<char> = clean.chars().filter(|c| !c.is_whitespace()).collect();
        let mut i = 0;
        while i < chars.len() {
            let letter = chars[i];
            i += 1;
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || matches!(chars[i], '.' | '-' | '+')) {
                i += 1;
            }
            if letter.is_ascii_alphabetic()
                && let Ok(v) = chars[start..i].iter().collect::<String>().parse::<f64>()
            {
                words.push((letter, v));
            }
        }
        let mut target: (Option<f64>, Option<f64>) = (None, None);
        let (mut ci, mut cj, mut r) = (None, None, None);
        let mut moved = false;
        for &(letter, v) in &words {
            match (letter, v as i64, v) {
                ('G', 0, _) => motion = Motion::Rapid,
                ('G', 1, _) => motion = Motion::Line,
                ('G', 2, _) => motion = Motion::ArcCw,
                ('G', 3, _) => motion = Motion::ArcCcw,
                ('G', 20, _) => scale = 25.4,
                ('G', 21, _) => scale = 1.0,
                ('G', 90, g) if (g - 90.0).abs() < 1e-9 => absolute = true,
                ('G', 91, g) if (g - 91.0).abs() < 1e-9 => absolute = false,
                ('M', 3 | 4, _) => laser_on = true,
                ('M', 5, _) => laser_on = false,
                ('S', _, s) => power = Some(s),
                ('X', _, v) => {
                    target.0 = Some(v * scale);
                    moved = true;
                }
                ('Y', _, v) => {
                    target.1 = Some(v * scale);
                    moved = true;
                }
                ('I', _, v) => ci = Some(v * scale),
                ('J', _, v) => cj = Some(v * scale),
                ('R', _, v) => r = Some(v * scale),
                _ => {}
            }
        }
        if !moved {
            continue;
        }
        moves += 1;
        let (nx, ny) = if absolute { (target.0.unwrap_or(x), target.1.unwrap_or(y)) } else { (x + target.0.unwrap_or(0.0), y + target.1.unwrap_or(0.0)) };
        let burning = laser_on && motion != Motion::Rapid && power.is_none_or(|s| s > 0.0);
        let key = power.map(|s| s.to_bits());
        if !burning || key != current_power {
            finish(&mut current, &mut paths, current_power);
            current_power = key;
        }
        if burning {
            if current.is_empty() {
                current.push(Point::new(x, y));
            }
            match motion {
                Motion::ArcCw | Motion::ArcCcw => current.extend(arc_points((x, y), (nx, ny), ci, cj, r, motion == Motion::ArcCw)),
                _ => current.push(Point::new(nx, ny)),
            }
        }
        (x, y) = (nx, ny);
    }
    finish(&mut current, &mut paths, current_power);
    if paths.is_empty() {
        return Err(if moves == 0 { "no moves found: is this G-code?".into() } else { "no moves with the laser on".into() });
    }
    // Flip Y (G-code's points up) within the drawing's own height.
    let top = paths.iter().flat_map(|(_, p)| p.iter().map(|q| q.y)).fold(f64::MIN, f64::max);
    let mut powers: Vec<Option<u64>> = Vec::new();
    for (p, _) in &paths {
        if !powers.contains(p) {
            powers.push(*p);
        }
    }
    // One palette colour per power level (by order of appearance), when there's more than one.
    let mut result = ImportResult::default();
    for (p, pts) in paths {
        let flipped: Vec<Point> = pts.iter().map(|q| Point::new(q.x, top - q.y)).collect();
        let index = powers.iter().position(|q| *q == p).unwrap_or(0);
        let color = (powers.len() > 1).then(|| bc_doc::Layer::color(index % bc_doc::CUT_LAYER_COUNT));
        result.shapes.push(Imported { path: polyline_path(&flipped, false), color });
    }
    if powers.len() > 1 {
        result.warnings.push(format!("{} power levels, each on its own layer", powers.len()));
    }
    Ok(result)
}

/// Points along an arc from `a` to `b` (excluding `a`), from its centre offset (I, J) or radius R.
fn arc_points(a: (f64, f64), b: (f64, f64), i: Option<f64>, j: Option<f64>, r: Option<f64>, clockwise: bool) -> Vec<Point> {
    let centre = match (i, j, r) {
        (i, j, _) if i.is_some() || j.is_some() => (a.0 + i.unwrap_or(0.0), a.1 + j.unwrap_or(0.0)),
        (_, _, Some(r)) => {
            // The centre on the side that gives the short arc (or the long one for negative R).
            let (mx, my) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let d = dx.hypot(dy);
            if d < 1e-12 || r.abs() < d / 2.0 {
                return vec![Point::new(b.0, b.1)];
            }
            let h = (r * r - d * d / 4.0).sqrt();
            let side = if clockwise == (r > 0.0) { -1.0 } else { 1.0 };
            (mx - side * h * dy / d, my + side * h * dx / d)
        }
        _ => return vec![Point::new(b.0, b.1)],
    };
    let radius = (a.0 - centre.0).hypot(a.1 - centre.1);
    let a0 = (a.1 - centre.1).atan2(a.0 - centre.0);
    let a1 = (b.1 - centre.1).atan2(b.0 - centre.0);
    let tau = std::f64::consts::TAU;
    let mut sweep = if clockwise { (a0 - a1).rem_euclid(tau) } else { (a1 - a0).rem_euclid(tau) };
    if sweep < 1e-9 {
        sweep = tau; // same start and end: a full circle
    }
    // About 0.05 mm between points, at least 8 per arc.
    let steps = ((radius * sweep / 0.05).ceil() as usize).clamp(8, 10_000);
    (1..=steps)
        .map(|k| {
            let t = a0 + if clockwise { -1.0 } else { 1.0 } * sweep * k as f64 / steps as f64;
            if k == steps { Point::new(b.0, b.1) } else { Point::new(centre.0 + radius * t.cos(), centre.1 + radius * t.sin()) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laser_moves_become_lines() {
        let g = b"G21 G90\nG0 X10 Y10\nM3 S500\nG1 X20 Y10 F1000\nY20 ; modal G1\nM5\nG0 X50 Y50\nM4 S500\nG1 X60\n";
        let r = import(g).unwrap();
        assert_eq!(r.shapes.len(), 2, "two separate burns");
        let b = bc_geom::bounds(&r.shapes[0].path).unwrap();
        assert!((b.width() - 10.0).abs() < 1e-9 && (b.height() - 10.0).abs() < 1e-9, "{b:?}");
        // Y flipped: the second burn (higher up in G-code) is drawn above the first.
        let b2 = bc_geom::bounds(&r.shapes[1].path).unwrap();
        assert!(b2.y0 < b.y0);
    }

    #[test]
    fn units_relative_moves_and_arcs() {
        let g = b"G20 G91\nM3 S1\nG1 X1\nG21\nG2 X10 Y0 I5 J0\n";
        let r = import(g).unwrap();
        let b = bc_geom::bounds(&r.shapes[0].path).unwrap();
        // 1 inch then a 10 mm half circle (radius 5) clockwise: goes up 5 mm in G-code's Y.
        assert!((b.width() - 35.4).abs() < 0.05, "{b:?}");
        assert!((b.height() - 5.0).abs() < 0.05, "{b:?}");
    }

    #[test]
    fn power_levels_get_their_own_colours() {
        let g = b"M3\nG1 X10 S100\nG1 X20 S900\nG1 X30 S100\n";
        let r = import(g).unwrap();
        let colours: Vec<Option<[u8; 3]>> = r.shapes.iter().map(|s| s.color).collect();
        assert_eq!(colours.len(), 3);
        assert_eq!(colours[0], colours[2]);
        assert_ne!(colours[0], colours[1]);
        // Travel only, or not G-code at all: an error.
        assert!(import(b"G0 X10 Y10\nG0 X20").is_err());
        assert!(import(b"hello").is_err());
    }
}
