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
    // Each outline, and whether its start was set by hand (so ordering keeps it).
    let mut polylines: Vec<(Vec<Point>, bool)> = Vec::new();
    for s in shapes {
        let mut path = s.world_path();
        if settings.kerf.abs() > 1e-9 && s.is_closed() {
            // Offsetting the filled region grows the part and shrinks its holes together.
            if let Some(p) = bc_geom::ops::offset(&path, settings.kerf) {
                path = p;
            }
        }
        let mut lines: Vec<Vec<Point>> = bc_geom::flatten(&path, OUTPUT_TOLERANCE)
            .into_iter()
            .map(|mut l| {
                if l.closed {
                    l.points.push(l.points[0]);
                }
                l.points
            })
            .collect();
        let mut fixed = vec![false; lines.len()];
        if let Some(cut) = s.cut {
            if cut.reverse {
                lines.iter_mut().for_each(|l| l.reverse());
            }
            if let Some(local) = cut.point
                && let Some(i) = start_at(&mut lines, s.transform * local)
            {
                fixed[i] = true;
            }
        }
        polylines.extend(lines.into_iter().zip(fixed));
    }
    let material: Vec<Vec<Point>> = polylines.iter().map(|(p, _)| p).filter(|p| is_closed(p)).cloned().collect();
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
            if settings.overcut > 0.0 {
                let lead = settings.lead_in > 0.0 && path.len() > 1 && path[0] != *path.last().unwrap();
                overcut(&mut path, settings.overcut, lead);
            }
            out.push(JobPath::burn(path));
        }
    }
    out
}

/// Carries a closed cut on past its start for `length` mm, along the start of the outline again,
/// so the cut closes fully. With `has_lead`, the first point is a lead-in, not part of the ring.
fn overcut(path: &mut Vec<Point>, length: f64, has_lead: bool) {
    let ring: Vec<Point> = path[usize::from(has_lead)..].to_vec();
    let mut left = length;
    for w in ring.windows(2) {
        let d = w[0].distance(w[1]);
        if d >= left {
            if left > 1e-9 {
                path.push(w[0].lerp(w[1], left / d));
            }
            return;
        }
        left -= d;
        path.push(w[1]);
    }
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

/// Makes the outline nearest `p` start at its nearest point to `p`: a closed one is turned round
/// to start there, an open one runs from whichever end is nearer. Returns which outline.
fn start_at(lines: &mut [Vec<Point>], p: Point) -> Option<usize> {
    let (i, seg, q, _) = lines
        .iter()
        .enumerate()
        .flat_map(|(i, l)| {
            l.windows(2).enumerate().map(move |(k, w)| {
                let q = nearest_on_segment(w[0], w[1], p);
                (i, k, q, q.distance(p))
            })
        })
        .min_by(|a, b| a.3.total_cmp(&b.3))?;
    let line = &mut lines[i];
    if is_closed(line) {
        let mut ring: Vec<Point> = line[..line.len() - 1].to_vec();
        // Start at q: on the segment after vertex `seg`, or at a vertex if it's (nearly) one.
        let at = if q.distance(ring[seg]) < 1e-6 {
            seg
        } else if q.distance(ring[(seg + 1) % ring.len()]) < 1e-6 {
            (seg + 1) % ring.len()
        } else {
            ring.insert(seg + 1, q);
            seg + 1
        };
        ring.rotate_left(at);
        ring.push(ring[0]);
        *line = ring;
    } else if p.distance(*line.last().expect("lines have points")) < p.distance(line[0]) {
        line.reverse();
    }
    Some(i)
}

fn nearest_on_segment(a: Point, b: Point, p: Point) -> Point {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 <= 0.0 {
        return a;
    }
    a + ab * ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
}

/// Orders paths: deepest-nested first (holes before the parts around them), and within
/// each depth, greedily the nearest start point. Closed paths may start at any vertex, unless
/// their start was set by hand (`true`). Returns each path with its nesting depth.
pub(crate) fn order_paths(paths: Vec<(Vec<Point>, bool)>) -> Vec<(Vec<Point>, usize)> {
    let depth: Vec<usize> = paths
        .iter()
        .enumerate()
        .map(|(i, (p, _))| paths.iter().enumerate().filter(|(j, (q, _))| *j != i && is_closed(q) && polygon_contains(q, p[0])).count())
        .collect();
    let max_depth = depth.iter().copied().max().unwrap_or(0);
    let mut out = Vec::with_capacity(paths.len());
    let mut pos = Point::ORIGIN;
    let mut remaining: Vec<Option<(Vec<Point>, bool)>> = paths.into_iter().map(Some).collect();
    for d in (0..=max_depth).rev() {
        loop {
            // Nearest remaining path at this depth, and the vertex to start from.
            let best = remaining
                .iter()
                .enumerate()
                .filter(|(i, p)| p.is_some() && depth[*i] == d)
                .map(|(i, p)| {
                    let (p, fixed) = p.as_ref().unwrap();
                    let (k, dist) = if is_closed(p) && !fixed {
                        p[..p.len() - 1].iter().enumerate().map(|(k, v)| (k, v.distance(pos))).fold((0, f64::INFINITY), |a, b| if b.1 < a.1 { b } else { a })
                    } else {
                        (0, p[0].distance(pos))
                    };
                    (i, k, dist)
                })
                .min_by(|a, b| a.2.total_cmp(&b.2));
            let Some((i, k, _)) = best else { break };
            let (mut path, _) = remaining[i].take().unwrap();
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
