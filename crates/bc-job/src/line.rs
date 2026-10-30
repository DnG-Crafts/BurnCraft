//! Line (vector cut) output: kerf, ordering, tabs and lead-ins.

use crate::{JobPath, OUTPUT_TOLERANCE};
use bc_doc::{Layer, Shape};
use bc_geom::{Point, Vec2};

pub(crate) fn is_closed(path: &[Point]) -> bool {
    path.len() > 3 && path[0].distance(*path.last().unwrap()) < 1e-9
}

/// Even-odd point-in-polygon test for a closed polyline (last point = first).
pub(crate) fn polygon_contains(poly: &[Point], p: Point) -> bool {
    let mut inside = false;
    for w in poly.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

/// Outlines for cutting: kerf-compensated, ordered, with tabs and lead-ins.
pub(crate) fn line_paths(shapes: &[&Shape], settings: &Layer) -> Vec<JobPath> {
    let mut polylines: Vec<Vec<Point>> = Vec::new();
    for s in shapes {
        let mut path = s.world_path();
        if settings.kerf.abs() > 1e-9 && s.is_closed() {
            // Offsetting the filled region grows the part and shrinks its holes together.
            if let Some(p) = bc_geom::ops::offset(&path, settings.kerf) {
                path = p;
            }
        }
        for mut l in bc_geom::flatten(&path, OUTPUT_TOLERANCE) {
            if l.closed {
                l.points.push(l.points[0]);
            }
            polylines.push(l.points);
        }
    }
    let material: Vec<Vec<Point>> = polylines.iter().filter(|p| is_closed(p)).cloned().collect();
    let in_material = |p: Point| material.iter().filter(|ring| polygon_contains(ring, p)).count() % 2 == 1;

    let mut out = Vec::new();
    for (path, depth) in order_paths(polylines) {
        if !is_closed(&path) {
            out.push(JobPath::burn(path));
            continue;
        }
        let mut path = path;
        if settings.lead_in > 0.0
            && let Some(lead) = lead_in_point(&path, settings.lead_in, &in_material) {
                path.insert(0, lead);
            }
        let wants_tabs = settings.tab_count > 0 && settings.tab_size > 0.0 && !(settings.tabs_skip_inner && depth > 0);
        if wants_tabs {
            let lead = settings.lead_in > 0.0 && path.len() > 1 && path[0] != *path.last().unwrap();
            out.extend(split_for_tabs(&path, settings.tab_count, settings.tab_size, lead).into_iter().map(JobPath::burn));
        } else {
            out.push(JobPath::burn(path));
        }
    }
    out
}

/// A point `length` mm from the start of a closed path, on the scrap side. The lead comes in
/// along the bisector of the corner at the start, so it never runs along an adjacent edge.
fn lead_in_point(path: &[Point], length: f64, in_material: &impl Fn(Point) -> bool) -> Option<Point> {
    let start = path[0];
    let next = path.iter().skip(1).find(|p| p.distance(start) > 1e-9)?;
    let prev = path.iter().rev().skip(1).find(|p| p.distance(start) > 1e-9)?;
    let perp = |d: Vec2| Vec2::new(-d.y, d.x);
    let (out_dir, in_dir) = ((*next - start).normalize(), (start - *prev).normalize());
    let bisector = perp(out_dir) + perp(in_dir);
    let dir = if bisector.hypot() < 1e-6 { perp(out_dir) } else { bisector.normalize() };
    let side = if in_material(start + dir * 0.01) { -1.0 } else { 1.0 };
    Some(start + dir * (length * side))
}

/// Splits a closed path into burn pieces with `count` gaps of `size` mm spread evenly along it.
/// If `has_lead` is set, the first segment is a lead-in and is never interrupted.
fn split_for_tabs(path: &[Point], count: u32, size: f64, has_lead: bool) -> Vec<Vec<Point>> {
    let (lead, ring) = if has_lead { (Some(path[0]), &path[1..]) } else { (None, path) };
    let perimeter: f64 = ring.windows(2).map(|w| w[0].distance(w[1])).sum();
    if perimeter <= size * count as f64 * 1.5 {
        return vec![path.to_vec()]; // too small for tabs
    }
    // Gaps centred at (i + 0.5) / count of the perimeter.
    let gaps: Vec<(f64, f64)> = (0..count).map(|i| {
        let c = perimeter * (i as f64 + 0.5) / count as f64;
        (c - size / 2.0, c + size / 2.0)
    }).collect();
    let mut pieces: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = lead.into_iter().collect();
    current.push(ring[0]);
    let mut dist = 0.0;
    let mut gap_index = 0;
    for w in ring.windows(2) {
        let (a, b) = (w[0], w[1]);
        let seg = a.distance(b);
        let at = |d: f64| a.lerp(b, ((d - dist) / seg).clamp(0.0, 1.0));
        while gap_index < gaps.len() && gaps[gap_index].0 < dist + seg {
            let (g0, g1) = gaps[gap_index];
            if g0 >= dist {
                current.push(at(g0));
                pieces.push(std::mem::take(&mut current));
            }
            if g1 <= dist + seg {
                current = vec![at(g1)];
                gap_index += 1;
            } else {
                break; // gap continues into the next segment
            }
        }
        let in_gap = gap_index < gaps.len() && gaps[gap_index].0 < dist + seg && gaps[gap_index].1 > dist + seg;
        if !in_gap && !current.is_empty() {
            current.push(b);
        }
        dist += seg;
    }
    if current.len() > 1 {
        pieces.push(current);
    }
    pieces.retain(|p| p.len() > 1 && p.windows(2).any(|w| w[0].distance(w[1]) > 1e-9));
    pieces
}

/// Orders paths: deepest-nested first (holes before the parts around them), and within
/// each depth, greedily the nearest start point. Closed paths may start at any vertex.
/// Returns each path with its nesting depth.
pub(crate) fn order_paths(paths: Vec<Vec<Point>>) -> Vec<(Vec<Point>, usize)> {
    let depth: Vec<usize> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| paths.iter().enumerate().filter(|(j, q)| *j != i && is_closed(q) && polygon_contains(q, p[0])).count())
        .collect();
    let max_depth = depth.iter().copied().max().unwrap_or(0);
    let mut out = Vec::with_capacity(paths.len());
    let mut pos = Point::ORIGIN;
    let mut remaining: Vec<Option<Vec<Point>>> = paths.into_iter().map(Some).collect();
    for d in (0..=max_depth).rev() {
        loop {
            // Nearest remaining path at this depth, and the vertex to start from.
            let best = remaining
                .iter()
                .enumerate()
                .filter(|(i, p)| p.is_some() && depth[*i] == d)
                .map(|(i, p)| {
                    let p = p.as_ref().unwrap();
                    let (k, dist) = if is_closed(p) {
                        p[..p.len() - 1].iter().enumerate().map(|(k, v)| (k, v.distance(pos))).fold((0, f64::INFINITY), |a, b| if b.1 < a.1 { b } else { a })
                    } else {
                        (0, p[0].distance(pos))
                    };
                    (i, k, dist)
                })
                .min_by(|a, b| a.2.total_cmp(&b.2));
            let Some((i, k, _)) = best else { break };
            let mut path = remaining[i].take().unwrap();
            if is_closed(&path) && k > 0 {
                path.pop();
                path.rotate_left(k);
                path.push(path[0]);
            }
            pos = *path.last().unwrap();
            out.push((path, d));
        }
    }
    out
}

/// Splits burning paths into dashes for perforated cutting (cut `perf_cut`, skip `perf_skip`), when
/// the layer asks for it. Travel moves are untouched.
pub(crate) fn perforate(paths: Vec<JobPath>, settings: &bc_doc::Layer) -> Vec<JobPath> {
    if !settings.perforate || settings.perf_cut <= 0.0 || settings.perf_skip <= 0.0 {
        return paths;
    }
    let (cut, skip) = (settings.perf_cut, settings.perf_skip);
    let mut out = Vec::new();
    for path in paths {
        if path.power <= 0.0 || path.points.len() < 2 {
            out.push(path);
            continue;
        }
        // Walk the polyline, switching between cutting and skipping at the right distances.
        let mut cutting = true;
        let mut left = cut; // distance remaining in the current dash or gap
        let mut current: Vec<bc_geom::Point> = vec![path.points[0]];
        for w in path.points.windows(2) {
            let (mut a, b) = (w[0], w[1]);
            let mut seg = a.distance(b);
            while seg > 1e-12 {
                let step = left.min(seg);
                let p = a.lerp(b, step / seg);
                if cutting {
                    current.push(p);
                }
                (a, seg, left) = (p, seg - step, left - step);
                if left <= 1e-12 {
                    if cutting {
                        out.push(JobPath { points: std::mem::take(&mut current), power: path.power });
                    } else {
                        current = vec![p];
                    }
                    cutting = !cutting;
                    left = if cutting { cut } else { skip };
                }
            }
        }
        if cutting && current.len() >= 2 {
            out.push(JobPath { points: current, power: path.power });
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{doc_with, rect};
    use crate::{build, BuildOptions};
    use bc_doc::LayerMode;

    fn square(x: f64, y: f64, s: f64) -> Vec<Point> {
        vec![Point::new(x, y), Point::new(x + s, y), Point::new(x + s, y + s), Point::new(x, y + s), Point::new(x, y)]
    }

    #[test]
    fn closed_paths_start_at_nearest_vertex() {
        let doc = doc_with(&[(rect(10.0, 10.0), (50.0, 50.0), 0)]);
        let job = build(&doc, None, BuildOptions::default());
        let p = &job.layers[0].paths[0].points;
        assert_eq!(p[0], Point::new(45.0, 45.0), "nearest corner to the origin");
        assert_eq!(p[0], *p.last().unwrap());
    }

    #[test]
    fn tabs_leave_gaps() {
        let pieces = split_for_tabs(&square(0.0, 0.0, 10.0), 4, 1.0, false);
        assert_eq!(pieces.len(), 5, "4 gaps split the ring into 5 runs (first and last meet at the start)");
        let cut: f64 = pieces.iter().map(|p| p.windows(2).map(|w| w[0].distance(w[1])).sum::<f64>()).sum();
        assert!((cut - 36.0).abs() < 1e-9, "40 mm perimeter minus 4 × 1 mm tabs, got {cut}");
    }

    #[test]
    fn tabs_skip_holes_and_tiny_shapes() {
        let mut doc = doc_with(&[(rect(40.0, 40.0), (100.0, 100.0), 0), (rect(10.0, 10.0), (100.0, 100.0), 0)]);
        doc.layers[0].tab_count = 2;
        let job = build(&doc, None, BuildOptions::default());
        assert_eq!(job.layers[0].paths.len(), 1 + 3, "hole uncut by tabs; outer split into 3 pieces");
        assert_eq!(split_for_tabs(&square(0.0, 0.0, 1.0), 4, 1.0, false).len(), 1);
    }

    #[test]
    fn kerf_grows_parts_and_shrinks_holes() {
        let mut doc = doc_with(&[(rect(40.0, 40.0), (100.0, 100.0), 0)]);
        doc.layers[0].kerf = 0.5;
        let job = build(&doc, None, BuildOptions::default());
        let xs: Vec<f64> = job.layers[0].paths[0].points.iter().map(|p| p.x).collect();
        let w = xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
        assert!((w - 41.0).abs() < 1e-3, "width {w}");
    }

    #[test]
    fn lead_in_starts_in_scrap() {
        let mut doc = doc_with(&[(rect(40.0, 40.0), (100.0, 100.0), 0), (rect(10.0, 10.0), (100.0, 100.0), 0)]);
        doc.layers[0].lead_in = 2.0;
        doc.layers[0].mode = LayerMode::Line;
        let job = build(&doc, None, BuildOptions::default());
        let hole = &job.layers[0].paths[0].points;
        let outer = &job.layers[0].paths[1].points;
        let inside = |p: Point, half: f64| (p.x - 100.0).abs() < half && (p.y - 100.0).abs() < half;
        assert!(inside(hole[0], 5.0), "hole lead-in starts inside the hole: {:?}", hole[0]);
        assert!(!inside(outer[0], 20.0), "outer lead-in starts outside the part: {:?}", outer[0]);
        assert!((hole[0].distance(hole[1]) - 2.0).abs() < 1e-9);
    }
}
