//! Generators for laser-cut parts: finger-jointed boxes and living hinges.
//!
//! Box panels are drawn at their full outer size, then notches `thickness` deep are cut
//! along jointed edges. Mating edges use opposite patterns (one starts with a finger, the
//! other with a slot), and the bottom/top panels own the corner cubes, so every bit of the
//! box's volume belongs to exactly one panel.

use crate::ops::{boolean, offset, BooleanOp};
use crate::{rect_path, Affine, BezPath, Point};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxStyle {
    Closed,
    OpenTop,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxSpec {
    /// Box size in mm: width (X), depth, height.
    pub width: f64,
    pub depth: f64,
    pub height: f64,
    /// Whether the size above is the inside (true) or outside (false) of the box.
    pub inside: bool,
    /// Material thickness, mm.
    pub thickness: f64,
    /// Target finger width, mm (adjusted so each edge has a whole, odd number of fingers).
    pub finger: f64,
    /// Width of the laser cut, mm. Outlines grow by half of it so parts fit snugly.
    pub kerf: f64,
    pub style: BoxStyle,
}

/// A part outline with its origin at the top-left of its bounding box.
#[derive(Debug, Clone)]
pub struct Panel {
    pub name: &'static str,
    pub outline: BezPath,
    pub size: (f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    /// No joint: a straight edge.
    Plain,
    /// Fingers at both ends (even segments full, odd segments notched).
    Fingers,
    /// Slots at both ends (even segments notched).
    Slots,
}

/// Odd number of fingers for an edge of length `len`, closest to the target finger width.
fn finger_count(len: f64, finger: f64) -> usize {
    let ideal = (len / finger.max(0.1)).max(1.0);
    let lower = (ideal.floor() as usize) | 1; // nearest odd at or below (rounded up if even)
    let lower = if lower as f64 > ideal { lower.saturating_sub(2).max(1) } else { lower };
    let upper = lower + 2;
    let n = if (ideal - lower as f64).abs() <= (upper as f64 - ideal).abs() { lower } else { upper };
    n.max(3)
}

/// One panel: `w` × `h` with edges (top, right, bottom, left).
fn panel(w: f64, h: f64, t: f64, finger: f64, edges: [Edge; 4]) -> BezPath {
    let mut notches = Vec::new();
    for (side, edge) in edges.into_iter().enumerate() {
        let len = if side % 2 == 0 { w } else { h };
        let n = finger_count(len, finger);
        let f = len / n as f64;
        for i in 0..n {
            let notched = match edge {
                Edge::Plain => false,
                Edge::Fingers => i % 2 == 1,
                Edge::Slots => i % 2 == 0,
            };
            if !notched {
                continue;
            }
            let (a, b) = (i as f64 * f, (i + 1) as f64 * f);
            let (x0, y0, x1, y1) = match side {
                0 => (a, 0.0, b, t),
                1 => (w - t, a, w, b),
                2 => (a, h - t, b, h),
                _ => (0.0, a, t, b),
            };
            // Slightly oversize the notch along the edge so neighbouring notches merge cleanly.
            notches.push(Affine::translate((x0, y0)) * Affine::translate((-1e-9, -1e-9)) * rect_path_corner(x1 - x0 + 2e-9, y1 - y0 + 2e-9));
        }
    }
    let outline = rect_path_corner(w, h);
    if notches.is_empty() {
        return outline;
    }
    let mut all = vec![outline];
    all.extend(notches);
    boolean(&all, BooleanOp::Subtract).unwrap_or_else(|| rect_path_corner(w, h))
}

/// A `w` × `h` rectangle with its top-left corner at the origin.
fn rect_path_corner(w: f64, h: f64) -> BezPath {
    Affine::translate((w / 2.0, h / 2.0)) * rect_path(w, h, 0.0)
}

/// The panels of a finger-jointed box.
pub fn finger_box(spec: &BoxSpec) -> Result<Vec<Panel>, String> {
    let t = spec.thickness;
    if t <= 0.0 {
        return Err("material thickness must be more than 0".into());
    }
    let open = spec.style == BoxStyle::OpenTop;
    let (w, d, h) = if spec.inside {
        (spec.width + 2.0 * t, spec.depth + 2.0 * t, spec.height + if open { t } else { 2.0 * t })
    } else {
        (spec.width, spec.depth, spec.height)
    };
    let min = 3.0 * t;
    if w < min || d < min || h < min {
        return Err(format!("every outside dimension must be at least 3 × the thickness ({min:.1} mm)"));
    }
    let top = if open { Edge::Plain } else { Edge::Slots };
    let mut parts: Vec<(&'static str, f64, f64, [Edge; 4])> = vec![
        ("Bottom", w, d, [Edge::Fingers; 4]),
        ("Front", w, h, [top, Edge::Fingers, Edge::Slots, Edge::Fingers]),
        ("Back", w, h, [top, Edge::Fingers, Edge::Slots, Edge::Fingers]),
        ("Left", d, h, [top, Edge::Slots, Edge::Slots, Edge::Slots]),
        ("Right", d, h, [top, Edge::Slots, Edge::Slots, Edge::Slots]),
    ];
    if !open {
        parts.insert(1, ("Top", w, d, [Edge::Fingers; 4]));
    }
    Ok(parts
        .into_iter()
        .map(|(name, pw, ph, edges)| {
            let mut outline = panel(pw, ph, t, spec.finger, edges);
            if spec.kerf > 0.0
                && let Some(grown) = offset(&outline, spec.kerf / 2.0)
            {
                outline = Affine::translate((spec.kerf / 2.0, spec.kerf / 2.0)) * grown;
            }
            Panel { name, outline, size: (pw + spec.kerf.max(0.0), ph + spec.kerf.max(0.0)) }
        })
        .collect())
}

/// Places panels in rows no wider than `max_width`, `gap` apart, starting at the origin.
pub fn layout(panels: &[Panel], gap: f64, max_width: f64) -> Vec<(&'static str, BezPath)> {
    let (mut x, mut y, mut row_h) = (0.0, 0.0, 0.0f64);
    panels
        .iter()
        .map(|p| {
            if x > 0.0 && x + p.size.0 > max_width {
                x = 0.0;
                y += row_h + gap;
                row_h = 0.0;
            }
            let placed = Affine::translate((x, y)) * p.outline.clone();
            x += p.size.0 + gap;
            row_h = row_h.max(p.size.1);
            (p.name, placed)
        })
        .collect()
}

/// A straight-line living hinge filling `width` × `height` (origin top-left): columns of cuts
/// `spacing` apart, each cut `cut` long with `bridge` of material between cuts, alternate
/// columns offset by half a period. The material bends about lines parallel to the cuts.
pub fn living_hinge(width: f64, height: f64, cut: f64, bridge: f64, spacing: f64) -> BezPath {
    let mut path = BezPath::new();
    if width <= 0.0 || height <= 0.0 || cut <= 0.0 || spacing <= 0.0 {
        return path;
    }
    let period = cut + bridge.max(0.0);
    let columns = (width / spacing).floor() as usize;
    let x0 = (width - columns as f64 * spacing) / 2.0;
    for c in 0..=columns {
        let x = x0 + c as f64 * spacing;
        let phase = -cut / 2.0 + if c % 2 == 1 { period / 2.0 } else { 0.0 };
        let mut y = phase;
        while y < height {
            let (a, b) = (y.max(0.0), (y + cut).min(height));
            if b - a > 0.5 {
                path.move_to(Point::new(x, a));
                path.line_to(Point::new(x, b));
            }
            y += period;
        }
    }
    path
}

/// A jigsaw puzzle `width` × `height` (origin top-left) cut into `cols` × `rows` pieces: the outline
/// (with rounded corners of `corner` mm) plus the inside cut lines, each edge with a knob. `knob` scales
/// the knobs (1 = classic); `seed` picks which way each knob points and varies them slightly.
pub fn jigsaw(width: f64, height: f64, cols: usize, rows: usize, knob: f64, corner: f64, seed: u64) -> BezPath {
    let (cols, rows) = (cols.max(1), rows.max(1));
    let (cw, ch) = (width / cols as f64, height / rows as f64);
    let mut rng = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        // xorshift: deterministic per seed.
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut out = Affine::translate((width / 2.0, height / 2.0)) * rect_path(width, height, corner.clamp(0.0, cw.min(ch) / 2.0));
    // One cut line per inside grid line, made of one knobbed edge per cell it crosses.
    let edge = |path: &mut BezPath, a: Point, b: Point, first: bool, next: &mut dyn FnMut() -> f64| {
        let along = b - a;
        let len = along.hypot();
        let n = crate::Vec2::new(-along.y, along.x) / len;
        let side = if next() < 0.5 { 1.0 } else { -1.0 };
        let centre = 0.5 + (next() - 0.5) * 0.12;
        let size = knob * (0.9 + next() * 0.2);
        let p = |u: f64, v: f64| a + along * (centre + (u - 0.5) * size).clamp(0.0, 1.0) + n * (v * size * side * len);
        if first {
            path.move_to(a);
        }
        path.line_to(p(0.38, 0.0));
        path.curve_to(p(0.42, 0.0), p(0.44, 0.06), p(0.40, 0.10));
        path.curve_to(p(0.34, 0.18), p(0.40, 0.28), p(0.50, 0.28));
        path.curve_to(p(0.60, 0.28), p(0.66, 0.18), p(0.60, 0.10));
        path.curve_to(p(0.56, 0.06), p(0.58, 0.0), p(0.62, 0.0));
        path.line_to(b);
    };
    for i in 1..cols {
        let x = i as f64 * cw;
        for j in 0..rows {
            edge(&mut out, Point::new(x, j as f64 * ch), Point::new(x, (j + 1) as f64 * ch), j == 0, &mut next);
        }
    }
    for j in 1..rows {
        let y = j as f64 * ch;
        for i in 0..cols {
            edge(&mut out, Point::new(i as f64 * cw, y), Point::new((i + 1) as f64 * cw, y), i == 0, &mut next);
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::area;

    fn spec(style: BoxStyle) -> BoxSpec {
        BoxSpec { width: 100.0, depth: 60.0, height: 40.0, inside: false, thickness: 3.0, finger: 10.0, kerf: 0.0, style }
    }

    /// Panel area × thickness must equal the box's wall volume: nothing missing, nothing doubled.
    fn check_volume(s: BoxSpec) {
        let panels = finger_box(&s).unwrap();
        let t = s.thickness;
        let solid: f64 = panels.iter().map(|p| area(&p.outline)).sum::<f64>() * t;
        let (w, d, h) = (s.width, s.depth, s.height);
        let hollow = match s.style {
            BoxStyle::Closed => (w - 2.0 * t) * (d - 2.0 * t) * (h - 2.0 * t),
            BoxStyle::OpenTop => (w - 2.0 * t) * (d - 2.0 * t) * (h - t),
        };
        let expected = w * d * h - hollow;
        assert!((solid - expected).abs() < 1e-3 * expected, "{:?}: panels {solid:.1} vs walls {expected:.1}", s.style);
    }

    #[test]
    fn closed_box_volume_is_exact() {
        check_volume(spec(BoxStyle::Closed));
    }

    #[test]
    fn open_box_volume_is_exact() {
        let panels = finger_box(&spec(BoxStyle::OpenTop)).unwrap();
        assert_eq!(panels.len(), 5);
        check_volume(spec(BoxStyle::OpenTop));
    }

    #[test]
    fn inside_dimensions_add_the_walls() {
        let s = BoxSpec { inside: true, ..spec(BoxStyle::Closed) };
        let bottom = &finger_box(&s).unwrap()[0];
        let b = crate::bounds(&bottom.outline).unwrap();
        assert!((b.width() - 106.0).abs() < 1e-6 && (b.height() - 66.0).abs() < 1e-6);
    }

    #[test]
    fn finger_counts_are_odd_and_close_to_target() {
        assert_eq!(finger_count(100.0, 10.0), 9); // 9 or 11 → 11.1 vs 9.1 mm: 9 is closer to 10
        assert_eq!(finger_count(40.0, 10.0), 3);
        assert_eq!(finger_count(5.0, 10.0), 3);
        assert!(finger_count(1000.0, 7.0) % 2 == 1);
    }

    #[test]
    fn kerf_grows_parts() {
        let plain = finger_box(&spec(BoxStyle::Closed)).unwrap();
        let kerfed = finger_box(&BoxSpec { kerf: 0.2, ..spec(BoxStyle::Closed) }).unwrap();
        assert!(area(&kerfed[0].outline) > area(&plain[0].outline));
        let b = crate::bounds(&kerfed[0].outline).unwrap();
        assert!(b.x0.abs() < 1e-6 && b.y0.abs() < 1e-6, "still starts at the origin");
    }

    #[test]
    fn layout_keeps_parts_apart() {
        let panels = finger_box(&spec(BoxStyle::Closed)).unwrap();
        let placed = layout(&panels, 5.0, 250.0);
        let boxes: Vec<_> = placed.iter().map(|(_, p)| crate::bounds(p).unwrap()).collect();
        for (i, a) in boxes.iter().enumerate() {
            assert!(a.x1 <= 250.0 + 1e-6 || a.x0 == 0.0);
            for b in &boxes[i + 1..] {
                assert!(a.intersect(*b).area() <= 1e-9, "overlap");
            }
        }
    }

    /// Writes a laid-out box and a hinge as polylines ("x,y x,y …" per line) to `BC_BOX_DUMP`.
    #[test]
    #[ignore = "diagnostic"]
    fn dump() {
        let panels = finger_box(&spec(BoxStyle::Closed)).unwrap();
        let mut out = String::new();
        let mut paths: Vec<BezPath> = layout(&panels, 5.0, 330.0).into_iter().map(|(_, p)| p).collect();
        paths.push(Affine::translate((240.0, 140.0)) * living_hinge(60.0, 40.0, 12.0, 3.0, 2.5));
        for p in paths {
            for l in crate::flatten(&p, 0.05) {
                let pts: Vec<String> = l.points.iter().map(|q| format!("{:.2},{:.2}", q.x, q.y)).collect();
                out += &format!("{}{}\n", pts.join(" "), if l.closed { " Z" } else { "" });
            }
        }
        std::fs::write(std::env::var("BC_BOX_DUMP").unwrap(), out).unwrap();
    }

    #[test]
    #[ignore = "diagnostic"]
    fn dump_jigsaw() {
        let p = jigsaw(150.0, 100.0, 6, 4, 1.0, 4.0, 3);
        let mut out = String::new();
        for l in crate::flatten(&p, 0.05) {
            out += &l.points.iter().map(|q| format!("{:.2},{:.2}", q.x, q.y)).collect::<Vec<_>>().join(" ");
            out += if l.closed { " Z\n" } else { "\n" };
        }
        std::fs::write(std::env::var("BC_BOX_DUMP").unwrap(), out).unwrap();
    }

    #[test]
    fn jigsaw_has_one_knob_per_inside_edge() {
        let p = jigsaw(120.0, 80.0, 4, 3, 1.0, 3.0, 7);
        // (4 - 1) × 3 vertical + (3 - 1) × 4 horizontal = 17 inside edges, 4 curves each.
        let curves = p.elements().iter().filter(|e| matches!(e, crate::PathEl::CurveTo(..))).count();
        let corner_curves = 4; // rounded outline corners
        assert_eq!(curves, 17 * 4 + corner_curves);
        let b = crate::bounds(&p).unwrap();
        assert!(b.x0 >= -1e-9 && b.y0 >= -1e-9 && b.x1 <= 120.0 + 1e-9 && b.y1 <= 80.0 + 1e-9, "knobs stay inside the outline: {b:?}");
        // Same seed, same puzzle; a different seed changes it.
        assert_eq!(jigsaw(120.0, 80.0, 4, 3, 1.0, 3.0, 7), p);
        assert_ne!(jigsaw(120.0, 80.0, 4, 3, 1.0, 3.0, 8), p);
    }
    #[test]
    fn hinge_cuts_stay_inside_and_alternate() {
        let p = living_hinge(30.0, 50.0, 15.0, 3.0, 2.0);
        let b = crate::bounds(&p).unwrap();
        assert!(b.x0 >= 0.0 && b.x1 <= 30.0 && b.y0 >= 0.0 && b.y1 <= 50.0);
        let lines = crate::flatten(&p, 0.01);
        // Column 0 starts at the edge; column 1 is offset by half a period.
        let first_y = |x: f64| lines.iter().filter(|l| (l.points[0].x - x).abs() < 1e-9).map(|l| l.points[0].y).fold(f64::MAX, f64::min);
        let (x0, x1) = (lines[0].points[0].x, lines.iter().map(|l| l.points[0].x).find(|&x| x > lines[0].points[0].x).unwrap());
        assert!(first_y(x0) < 1e-9 && first_y(x1) > 1.0);
    }
}
