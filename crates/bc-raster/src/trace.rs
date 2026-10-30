//! Bitmap tracing: outlines of dark regions as polygons (pixel coordinates, Y down).
//!
//! Boundaries are followed along pixel edges ("crack following"), so the raw
//! outlines are exact; they are then simplified with Ramer–Douglas–Peucker.
//! Holes come out as separate loops with the opposite winding, so filling the
//! result with the even-odd rule reproduces the image.

use crate::Bits;
use std::collections::HashMap;

pub type Polygon = Vec<(f64, f64)>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceOptions {
    /// Maximum deviation (pixels) when simplifying outlines.
    pub tolerance: f64,
    /// Loops enclosing fewer pixels than this are dropped (speckle removal).
    pub min_area: f64,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self { tolerance: 0.8, min_area: 4.0 }
    }
}

fn polygon_area(p: &[(f64, f64)]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1).sum::<f64>() / 2.0
}

fn rdp(points: &[(f64, f64)], tolerance: f64, out: &mut Vec<(f64, f64)>) {
    let (a, b) = (points[0], *points.last().unwrap());
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    let dist = |p: (f64, f64)| if len < 1e-12 { ((p.0 - a.0).powi(2) + (p.1 - a.1).powi(2)).sqrt() } else { ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len };
    let (idx, max) = points.iter().enumerate().skip(1).take(points.len().saturating_sub(2)).map(|(i, p)| (i, dist(*p))).fold((0, 0.0), |acc, x| if x.1 > acc.1 { x } else { acc });
    if max > tolerance && idx > 0 {
        rdp(&points[..=idx], tolerance, out);
        out.pop();
        rdp(&points[idx..], tolerance, out);
    } else {
        out.push(a);
        out.push(b);
    }
}

/// Simplifies a closed loop (first point not repeated).
fn simplify_loop(points: &[(f64, f64)], tolerance: f64) -> Polygon {
    if points.len() < 4 {
        return points.to_vec();
    }
    // Split the loop at the point farthest from the first, so both halves are open polylines.
    let far = (1..points.len()).max_by(|&i, &j| {
        let d = |k: usize| (points[k].0 - points[0].0).powi(2) + (points[k].1 - points[0].1).powi(2);
        d(i).total_cmp(&d(j))
    }).unwrap_or(1);
    let mut first = Vec::new();
    rdp(&points[..=far], tolerance, &mut first);
    let mut second_half: Vec<(f64, f64)> = points[far..].to_vec();
    second_half.push(points[0]);
    let mut second = Vec::new();
    rdp(&second_half, tolerance, &mut second);
    first.pop();
    second.pop();
    first.extend(second);
    first
}

/// Traces the dark regions of `bits`.
pub fn trace(bits: &Bits, options: TraceOptions) -> Vec<Polygon> {
    let (w, h) = (bits.width as i64, bits.height as i64);
    let dark = |x: i64, y: i64| x >= 0 && y >= 0 && x < w && y < h && bits.get(x as usize, y as usize);
    // Directed boundary edges, clockwise around dark pixels (screen coordinates).
    let mut out_edges: HashMap<(i64, i64), Vec<(i64, i64)>> = HashMap::new();
    let mut add = |a: (i64, i64), b: (i64, i64)| out_edges.entry(a).or_default().push(b);
    for y in 0..h {
        for x in 0..w {
            if !dark(x, y) {
                continue;
            }
            if !dark(x, y - 1) {
                add((x, y), (x + 1, y));
            }
            if !dark(x + 1, y) {
                add((x + 1, y), (x + 1, y + 1));
            }
            if !dark(x, y + 1) {
                add((x + 1, y + 1), (x, y + 1));
            }
            if !dark(x - 1, y) {
                add((x, y + 1), (x, y));
            }
        }
    }
    let mut loops = Vec::new();
    let mut starts: Vec<(i64, i64)> = out_edges.keys().copied().collect();
    starts.sort_unstable();
    for start in starts {
        while let Some(first_next) = out_edges.get_mut(&start).and_then(Vec::pop) {
            let mut points = vec![start];
            let (mut prev, mut cur) = (start, first_next);
            while cur != start {
                points.push(cur);
                let dir = (cur.0 - prev.0, cur.1 - prev.1);
                let Some(nexts) = out_edges.get_mut(&cur) else { break };
                // At a corner where two regions touch, turn right (keeps diagonal pixels separate).
                let right = (-dir.1, dir.0);
                let pick = nexts.iter().position(|n| (n.0 - cur.0, n.1 - cur.1) == right).unwrap_or(0);
                let next = nexts.swap_remove(pick);
                prev = cur;
                cur = next;
            }
            // Drop points in the middle of straight runs.
            let n = points.len();
            let corners: Vec<(f64, f64)> = (0..n)
                .filter(|&i| {
                    let (a, b, c) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
                    (b.0 - a.0) * (c.1 - b.1) != (b.1 - a.1) * (c.0 - b.0)
                })
                .map(|i| (points[i].0 as f64, points[i].1 as f64))
                .collect();
            if corners.len() >= 3 && polygon_area(&corners).abs() >= options.min_area {
                // Loops smaller than the tolerance would simplify away; keep their exact outline instead.
                let simplified = simplify_loop(&corners, options.tolerance);
                loops.push(if simplified.len() >= 3 { simplified } else { corners });
            }
        }
    }
    loops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(rows: &[&str]) -> Bits {
        let (w, h) = (rows[0].len(), rows.len());
        Bits { width: w, height: h, data: rows.iter().flat_map(|r| r.chars().map(|c| c == '#')).collect() }
    }

    #[test]
    fn square_is_four_corners() {
        let b = bits(&["....", ".##.", ".##.", "...."]);
        let loops = trace(&b, TraceOptions { min_area: 1.0, ..Default::default() });
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 4);
        assert!((polygon_area(&loops[0]).abs() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn ring_has_a_hole() {
        let b = bits(&["#####", "#...#", "#...#", "#...#", "#####"]);
        let loops = trace(&b, TraceOptions { min_area: 1.0, ..Default::default() });
        assert_eq!(loops.len(), 2);
        let areas: Vec<f64> = loops.iter().map(|l| polygon_area(l)).collect();
        assert!(areas[0].signum() != areas[1].signum(), "outer and hole wind opposite ways: {areas:?}");
        assert!((areas.iter().map(|a| a.abs()).fold(0.0, f64::max) - 25.0).abs() < 1e-9);
    }

    #[test]
    fn speckles_are_dropped_and_diagonals_stay_separate() {
        let b = bits(&["#....", ".#...", ".....", "..###", "..###"]);
        let loops = trace(&b, TraceOptions { min_area: 2.0, ..Default::default() });
        assert_eq!(loops.len(), 1, "two single pixels dropped, the 3×2 block kept");
        let all = trace(&b, TraceOptions { min_area: 0.5, ..Default::default() });
        assert_eq!(all.len(), 3, "diagonal pixels are separate loops: {all:?}");
    }

    #[test]
    fn simplification_straightens_staircases() {
        // A diagonal staircase edge: the outline should collapse to few points.
        let rows: Vec<String> = (0..20).map(|y| (0..20).map(|x| if x <= y { '#' } else { '.' }).collect()).collect();
        let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
        let loops = trace(&bits(&refs), TraceOptions { tolerance: 1.0, min_area: 1.0 });
        assert_eq!(loops.len(), 1);
        assert!(loops[0].len() <= 6, "got {} points", loops[0].len());
    }
}
