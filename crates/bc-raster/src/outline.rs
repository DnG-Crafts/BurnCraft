//! Smooth tracing: outlines found between the pixels, at sub-pixel precision, and single-stroke
//! centre lines, ready for curve fitting (see `bc_geom::curvefit`).
//!
//! Outlines: the picture's own shading near the threshold says where an edge runs, so the mask
//! is turned into an "ink level" field, softened very slightly, and its outlines found where it
//! crosses one half (marching squares). Unlike following pixel edges, this gives outlines without
//! stair-steps. Centre lines: the mask is thinned to a one-pixel skeleton (Zhang–Suen) and the
//! skeleton walked into open polylines.
//!
//! All coordinates are pixel corners: pixel (x, y) covers x..x+1, y..y+1, Y down.

use crate::Gray;
use std::collections::{HashMap, HashSet};

/// A point in pixel coordinates.
pub type Pt = (f64, f64);

/// What counts as ink.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ink {
    /// Pixels darker than this (0..1).
    Darker(f32),
    /// Everything that isn't the background: the background is found by flooding in from the
    /// picture's edges, through pixels within this tolerance (0..1) of the edges' average shade.
    /// For a photo of an object on a plain backdrop.
    Subject(f32),
}

/// A binary mask, row-major.
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub px: Vec<bool>,
}

impl Mask {
    fn get(&self, x: isize, y: isize) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height && self.px[y as usize * self.width + x as usize]
    }
}

/// The threshold that best splits the picture into dark and light (Otsu's method), 0..1.
pub fn auto_threshold(gray: &Gray) -> f32 {
    let mut hist = [0u32; 256];
    for v in &gray.data {
        hist[(v.clamp(0.0, 1.0) * 255.0).round() as usize] += 1;
    }
    let total: f64 = hist.iter().map(|&n| n as f64).sum();
    if total == 0.0 {
        return 0.5;
    }
    let sum_all: f64 = (0..256).map(|t| t as f64 * hist[t] as f64).sum();
    let mut var = [0f64; 256];
    let (mut w_b, mut sum_b) = (0f64, 0f64);
    for t in 0..256 {
        w_b += hist[t] as f64;
        let w_f = total - w_b;
        if w_b == 0.0 || w_f == 0.0 {
            continue;
        }
        sum_b += t as f64 * hist[t] as f64;
        let (m_b, m_f) = (sum_b / w_b, (sum_all - sum_b) / w_f);
        var[t] = w_b * w_f * (m_b - m_f) * (m_b - m_f);
    }
    let max = var.iter().cloned().fold(0.0, f64::max);
    if max <= 0.0 {
        return 0.5;
    }
    // The middle of any flat top, then just above it (pixels darker than the threshold are ink).
    let tops: Vec<usize> = (0..256).filter(|&t| var[t] >= max * 0.999).collect();
    let t = tops.iter().sum::<usize>() / tops.len();
    ((t as f32 + 0.5) / 255.0).clamp(0.01, 0.99)
}

/// The ink mask, with regions (ink or gaps) smaller than `min_pixels` removed.
pub fn mask(gray: &Gray, ink: Ink, min_pixels: usize) -> Mask {
    let (w, h) = (gray.width, gray.height);
    let px = match ink {
        Ink::Darker(t) => gray.data.iter().map(|v| *v < t).collect(),
        Ink::Subject(tol) => subject(gray, tol),
    };
    let mut m = Mask { width: w, height: h, px };
    if min_pixels > 0 {
        despeckle(&mut m, min_pixels);
    }
    m
}

/// Foreground by background removal: flood in from the edges through background-like pixels.
fn subject(gray: &Gray, tol: f32) -> Vec<bool> {
    let (w, h) = (gray.width, gray.height);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let edge: Vec<usize> = (0..w).flat_map(|x| [x, (h - 1) * w + x]).chain((0..h).flat_map(|y| [y * w, y * w + w - 1])).collect();
    let mean = edge.iter().map(|&i| gray.data[i]).sum::<f32>() / edge.len() as f32;
    let near = |i: usize| (gray.data[i] - mean).abs() <= tol;
    let mut bg = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for i in edge {
        if !bg[i] && near(i) {
            bg[i] = true;
            stack.push(i);
        }
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut go = |j: usize| {
            if !bg[j] && near(j) {
                bg[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            go(i - 1);
        }
        if x + 1 < w {
            go(i + 1);
        }
        if y > 0 {
            go(i - w);
        }
        if y + 1 < h {
            go(i + w);
        }
    }
    bg.iter().map(|b| !b).collect()
}

/// Removes connected regions of either colour smaller than `min` pixels (gaps that touch the
/// picture's edge are background, and kept).
fn despeckle(m: &mut Mask, min: usize) {
    let (w, h) = (m.width, m.height);
    for target in [true, false] {
        let mut seen = vec![false; w * h];
        for start in 0..m.px.len() {
            if seen[start] || m.px[start] != target {
                continue;
            }
            let mut region = vec![start];
            let mut stack = vec![start];
            seen[start] = true;
            let mut at_edge = false;
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                at_edge |= x == 0 || y == 0 || x == w - 1 || y == h - 1;
                let mut push = |j: usize| {
                    if !seen[j] && m.px[j] == target {
                        seen[j] = true;
                        stack.push(j);
                        region.push(j);
                    }
                };
                if x > 0 {
                    push(i - 1);
                }
                if x + 1 < w {
                    push(i + 1);
                }
                if y > 0 {
                    push(i - w);
                }
                if y + 1 < h {
                    push(i + w);
                }
            }
            if region.len() < min && !(!target && at_edge) {
                for i in region {
                    m.px[i] = !target;
                }
            }
        }
    }
}

/// A closed outline: its points (not repeated at the end), and whether it's an outside edge
/// (ink inside) rather than the edge of a hole. Outside edges run one way round, holes the other.
pub struct Outline {
    pub points: Vec<Pt>,
    pub outer: bool,
}

/// Outlines of the ink, at sub-pixel precision.
pub fn outlines(gray: &Gray, ink: Ink, m: &Mask) -> Vec<Outline> {
    let levels = ink_levels(gray, ink, m);
    let (field, fw, fh) = soften(m, &levels);
    let mut out = Vec::new();
    for mut pts in iso_loops(&field, fw, fh) {
        if pts.len() < 4 {
            continue;
        }
        let outer = encloses_ink(&pts, &field, fw, fh);
        if (signed_area(&pts) > 0.0) != outer {
            pts.reverse();
        }
        if signed_area(&pts).abs() >= 1.0 {
            out.push(Outline { points: pts, outer });
        }
    }
    out
}

fn signed_area(p: &[Pt]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1).sum::<f64>() / 2.0
}

/// How much each pixel is ink, 0..1 with 0.5 at the threshold: from the picture's own shading
/// near the threshold (anti-aliased edges say where an edge really runs), but always on the
/// mask's side of 0.5, so despeckling counts.
fn ink_levels(gray: &Gray, ink: Ink, m: &Mask) -> Vec<f32> {
    // How far from the threshold (0..1) a shade still counts as partly ink.
    const SPAN: f32 = 96.0 / 255.0;
    gray.data
        .iter()
        .zip(&m.px)
        .map(|(&v, &is_ink)| {
            let raw = match ink {
                Ink::Darker(t) => 0.5 + (t - v) / SPAN,
                // A flood fill has no shading to go on.
                Ink::Subject(_) => if is_ink { 1.0 } else { 0.0 },
            };
            if is_ink { raw.clamp(0.51, 1.0) } else { raw.clamp(0.0, 0.49) }
        })
        .collect()
}

/// The ink levels lightly blurred (stair-steps become slopes, while a one-pixel line survives),
/// with a blank border so every outline closes. Value (i, j) is the middle of pixel (i-1, j-1).
fn soften(m: &Mask, ink: &[f32]) -> (Vec<f32>, usize, usize) {
    let (w, h) = (m.width + 2, m.height + 2);
    let mut f = vec![0f32; w * h];
    for y in 0..m.height {
        for x in 0..m.width {
            f[(y + 1) * w + x + 1] = ink[y * m.width + x];
        }
    }
    let k = [1.0 / 6.0, 4.0 / 6.0, 1.0 / 6.0];
    let mut t = vec![0f32; w * h];
    for y in 0..h {
        for x in 1..w - 1 {
            t[y * w + x] = k[0] * f[y * w + x - 1] + k[1] * f[y * w + x] + k[2] * f[y * w + x + 1];
        }
    }
    let mut g = vec![0f32; w * h];
    for y in 1..h - 1 {
        for x in 0..w {
            g[y * w + x] = k[0] * t[(y - 1) * w + x] + k[1] * t[y * w + x] + k[2] * t[(y + 1) * w + x];
        }
    }
    (g, w, h)
}

/// Closed loops where `f` crosses one half (marching squares), in pixel-corner coordinates.
fn iso_loops(f: &[f32], w: usize, h: usize) -> Vec<Vec<Pt>> {
    const ISO: f32 = 0.5;
    let at = |i: usize, j: usize| f[j * w + i];
    // Edge keys: the horizontal edge from grid point (i, j) is 2n, the vertical one 2n + 1.
    let hk = |i: usize, j: usize| ((j * w + i) * 2) as u64;
    let vk = |i: usize, j: usize| ((j * w + i) * 2 + 1) as u64;
    let mut point: HashMap<u64, Pt> = HashMap::new();
    let mut link: HashMap<u64, Vec<u64>> = HashMap::new();
    let cross = |a: f32, b: f32| ((ISO - a) / (b - a)).clamp(0.0, 1.0) as f64;
    for j in 0..h - 1 {
        for i in 0..w - 1 {
            let (tl, tr, br, bl) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            let case = ((tl > ISO) as u8) << 3 | ((tr > ISO) as u8) << 2 | ((br > ISO) as u8) << 1 | (bl > ISO) as u8;
            if case == 0 || case == 15 {
                continue;
            }
            // Grid point (i, j) is the middle of pixel (i - 1, j - 1): corner coordinates i - 0.5.
            let (x, y) = (i as f64 - 0.5, j as f64 - 0.5);
            let (top, bottom, left, right) = (hk(i, j), hk(i, j + 1), vk(i, j), vk(i + 1, j));
            if (tl > ISO) != (tr > ISO) {
                point.entry(top).or_insert((x + cross(tl, tr), y));
            }
            if (bl > ISO) != (br > ISO) {
                point.entry(bottom).or_insert((x + cross(bl, br), y + 1.0));
            }
            if (tl > ISO) != (bl > ISO) {
                point.entry(left).or_insert((x, y + cross(tl, bl)));
            }
            if (tr > ISO) != (br > ISO) {
                point.entry(right).or_insert((x + 1.0, y + cross(tr, br)));
            }
            let centre = (tl + tr + br + bl) / 4.0 > ISO;
            let segs: Vec<(u64, u64)> = match case {
                1 | 14 => vec![(left, bottom)],
                2 | 13 => vec![(bottom, right)],
                3 | 12 => vec![(left, right)],
                4 | 11 => vec![(top, right)],
                6 | 9 => vec![(top, bottom)],
                7 | 8 => vec![(left, top)],
                // Saddles: the middle decides which corners join.
                5 if centre => vec![(left, top), (bottom, right)],
                5 => vec![(top, right), (left, bottom)],
                10 if centre => vec![(top, right), (left, bottom)],
                10 => vec![(left, top), (bottom, right)],
                _ => vec![],
            };
            for (a, b) in segs {
                link.entry(a).or_default().push(b);
                link.entry(b).or_default().push(a);
            }
        }
    }
    let mut keys: Vec<u64> = link.keys().copied().collect();
    keys.sort_unstable();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut loops = Vec::new();
    for start in keys {
        if seen.contains(&start) {
            continue;
        }
        let mut lp = Vec::new();
        let (mut prev, mut cur) = (u64::MAX, start);
        loop {
            seen.insert(cur);
            lp.push(point[&cur]);
            match link[&cur].iter().copied().find(|&n| n != prev && !seen.contains(&n)) {
                Some(n) => {
                    prev = cur;
                    cur = n;
                }
                None => break,
            }
        }
        loops.push(lp);
    }
    loops
}

/// Whether the ink is inside this outline (an outside edge) rather than outside it (a hole):
/// looks just to each side of its longest edge.
fn encloses_ink(pts: &[Pt], f: &[f32], w: usize, h: usize) -> bool {
    let sample = |p: Pt| {
        let (gx, gy) = (p.0 + 0.5, p.1 + 0.5);
        let (i, j) = (gx.floor().clamp(0.0, (w - 2) as f64) as usize, gy.floor().clamp(0.0, (h - 2) as f64) as usize);
        let (tx, ty) = ((gx - i as f64).clamp(0.0, 1.0) as f32, (gy - j as f64).clamp(0.0, 1.0) as f32);
        let v = |a: usize, b: usize| f[b * w + a];
        let top = v(i, j) * (1.0 - tx) + v(i + 1, j) * tx;
        let bottom = v(i, j + 1) * (1.0 - tx) + v(i + 1, j + 1) * tx;
        top * (1.0 - ty) + bottom * ty
    };
    let n = pts.len();
    let dist = |a: Pt, b: Pt| (a.0 - b.0).hypot(a.1 - b.1);
    let k = (0..n).max_by(|&a, &b| dist(pts[a], pts[(a + 1) % n]).total_cmp(&dist(pts[b], pts[(b + 1) % n]))).unwrap_or(0);
    let (a, b) = (pts[k], pts[(k + 1) % n]);
    let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let len = dist(a, b).max(1e-9);
    let nrm = (-(b.1 - a.1) / len * 0.35, (b.0 - a.0) / len * 0.35);
    let (plus, minus) = ((mid.0 + nrm.0, mid.1 + nrm.1), (mid.0 - nrm.0, mid.1 - nrm.1));
    point_in(pts, if sample(plus) >= sample(minus) { plus } else { minus })
}

fn point_in(pts: &[Pt], p: Pt) -> bool {
    let n = pts.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < a.0 + (p.1 - a.1) / (b.1 - a.1) * (b.0 - a.0) {
            inside = !inside;
        }
    }
    inside
}

/// Single-stroke centre lines of the ink: open polylines through the middle of each line,
/// in pixel-corner coordinates. Strokes shorter than `min_points` pixels (spurs) are dropped.
pub fn centre_lines(m: &Mask, min_points: usize) -> Vec<Vec<Pt>> {
    let skeleton = thin(m);
    skeleton_paths(&skeleton, m.width, m.height).into_iter().filter(|p| p.len() >= min_points.max(2)).collect()
}

/// Zhang–Suen thinning to a one-pixel-wide skeleton.
fn thin(m: &Mask) -> Vec<bool> {
    let (w, h) = (m.width, m.height);
    // Pad by one pixel so ink touching the edge thins too.
    let (pw, ph) = (w + 2, h + 2);
    let mut g = vec![false; pw * ph];
    for y in 0..h {
        for x in 0..w {
            g[(y + 1) * pw + x + 1] = m.get(x as isize, y as isize);
        }
    }
    // Neighbours clockwise from north (Y down).
    let offs: [(isize, isize); 8] = [(0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)];
    loop {
        let mut changed = false;
        for step in 0..2 {
            let mut remove = Vec::new();
            for y in 1..ph as isize - 1 {
                for x in 1..pw as isize - 1 {
                    let i = y as usize * pw + x as usize;
                    if !g[i] {
                        continue;
                    }
                    let p: [bool; 8] = std::array::from_fn(|k| g[(y + offs[k].1) as usize * pw + (x + offs[k].0) as usize]);
                    let b = p.iter().filter(|v| **v).count();
                    if !(2..=6).contains(&b) || (0..8).filter(|&k| !p[k] && p[(k + 1) % 8]).count() != 1 {
                        continue;
                    }
                    let (p2, p4, p6, p8) = (p[0], p[2], p[4], p[6]);
                    let keep = if step == 0 { (p2 && p4 && p6) || (p4 && p6 && p8) } else { (p2 && p4 && p8) || (p2 && p6 && p8) };
                    if !keep {
                        remove.push(i);
                    }
                }
            }
            changed |= !remove.is_empty();
            for i in remove {
                g[i] = false;
            }
        }
        if !changed {
            break;
        }
    }
    // Back to the picture's size.
    (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| g[(y + 1) * pw + x + 1]).collect()
}

/// Walks a one-pixel skeleton into open polylines between nodes (ends and junctions); pixel
/// centres, in corner coordinates.
fn skeleton_paths(g: &[bool], w: usize, h: usize) -> Vec<Vec<Pt>> {
    let neighbours = |i: usize| -> Vec<usize> {
        let (x, y) = ((i % w) as isize, (i / w) as isize);
        let mut v = Vec::new();
        for dy in -1..=1isize {
            for dx in -1..=1isize {
                let (nx, ny) = (x + dx, y + dy);
                if (dx, dy) != (0, 0) && nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && g[ny as usize * w + nx as usize] {
                    v.push(ny as usize * w + nx as usize);
                }
            }
        }
        v
    };
    let pt = |i: usize| ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
    let edge = |a: usize, b: usize| if a < b { (a, b) } else { (b, a) };
    let mut used: HashSet<(usize, usize)> = HashSet::new();
    let walk = |start: usize, first: usize, used: &mut HashSet<(usize, usize)>| -> Vec<Pt> {
        let mut path = vec![pt(start)];
        let (mut prev, mut cur) = (start, first);
        loop {
            path.push(pt(cur));
            let ns = neighbours(cur);
            if ns.len() != 2 {
                break;
            }
            let Some(next) = ns.into_iter().find(|&n| n != prev) else { break };
            if !used.insert(edge(cur, next)) {
                break;
            }
            prev = cur;
            cur = next;
        }
        path
    };
    let mut paths = Vec::new();
    // Chains from every end and junction first, then any plain loops left over.
    for nodes_only in [true, false] {
        for (i, _) in g.iter().enumerate().filter(|(_, on)| **on) {
            if (neighbours(i).len() == 2) == nodes_only {
                continue;
            }
            for n in neighbours(i) {
                if used.insert(edge(i, n)) {
                    paths.push(walk(i, n, &mut used));
                }
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: usize, h: usize, dark: impl Fn(usize, usize) -> bool) -> Gray {
        Gray { width: w, height: h, data: (0..w * h).map(|i| if dark(i % w, i / w) { 0.0 } else { 1.0 }).collect() }
    }

    #[test]
    fn a_square_with_a_hole_has_two_outlines() {
        let g = picture(40, 40, |x, y| (5..35).contains(&x) && (5..35).contains(&y) && !((15..25).contains(&x) && (15..25).contains(&y)));
        let m = mask(&g, Ink::Darker(0.5), 0);
        let o = outlines(&g, Ink::Darker(0.5), &m);
        assert_eq!(o.len(), 2);
        assert_eq!(o.iter().filter(|l| l.outer).count(), 1);
        let outer = o.iter().find(|l| l.outer).unwrap();
        assert!((signed_area(&outer.points).abs() - 900.0).abs() < 20.0, "{}", signed_area(&outer.points));
        assert!(signed_area(&outer.points).signum() != signed_area(&o.iter().find(|l| !l.outer).unwrap().points).signum());
    }

    #[test]
    fn auto_threshold_falls_between_dark_and_light() {
        let g = Gray { width: 10, height: 1, data: vec![0.1, 0.1, 0.1, 0.1, 0.1, 0.8, 0.8, 0.8, 0.8, 0.8] };
        let t = auto_threshold(&g);
        assert!(t > 0.1 && t < 0.8, "{t}");
    }

    #[test]
    fn subject_is_found_on_a_plain_background() {
        let g = Gray { width: 30, height: 30, data: (0..900).map(|i| if (10..20).contains(&(i % 30)) && (10..20).contains(&(i / 30)) { 0.4 } else { 0.9 }).collect() };
        let m = mask(&g, Ink::Subject(0.2), 0);
        assert!(!m.px[0] && m.px[15 * 30 + 15]);
        assert_eq!(outlines(&g, Ink::Subject(0.2), &m).len(), 1);
    }

    #[test]
    fn a_thick_bar_thins_to_one_stroke() {
        let g = picture(60, 20, |x, y| (5..55).contains(&x) && (7..13).contains(&y));
        let lines = centre_lines(&mask(&g, Ink::Darker(0.5), 0), 3);
        assert_eq!(lines.len(), 1);
        let ys: Vec<f64> = lines[0].iter().map(|p| p.1).collect();
        assert!(ys.iter().all(|y| (*y - 10.0).abs() < 2.0), "runs along the middle: {ys:?}");
    }

    #[test]
    fn specks_are_removed() {
        let g = picture(20, 20, |x, y| (x, y) == (3, 3) || ((10..18).contains(&x) && (10..18).contains(&y)));
        let m = mask(&g, Ink::Darker(0.5), 4);
        assert!(!m.px[3 * 20 + 3]);
        assert_eq!(outlines(&g, Ink::Darker(0.5), &m).len(), 1);
    }
}
