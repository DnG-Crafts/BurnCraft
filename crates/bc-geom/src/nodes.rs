//! Node editing on paths: on-curve points (nodes) and curve control points (handles).

use crate::{BezPath, PathEl, Point, Vec2};
use kurbo::{CubicBez, Line, ParamCurve, ParamCurveNearest, PathSeg, QuadBez};

/// Which point of a path element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleKind {
    /// The element's end point (an on-curve node).
    Node,
    /// First control point of a cubic (or the control point of a quadratic).
    Control1,
    /// Second control point of a cubic.
    Control2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle {
    /// Index into the path's elements.
    pub element: usize,
    pub kind: HandleKind,
}

/// Every editable point of the path, with its position.
pub fn handles(path: &BezPath) -> Vec<(Handle, Point)> {
    let mut out = Vec::new();
    for (i, el) in path.elements().iter().enumerate() {
        let h = |kind| Handle { element: i, kind };
        match *el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => out.push((h(HandleKind::Node), p)),
            PathEl::QuadTo(c, p) => {
                out.push((h(HandleKind::Control1), c));
                out.push((h(HandleKind::Node), p));
            }
            PathEl::CurveTo(c1, c2, p) => {
                out.push((h(HandleKind::Control1), c1));
                out.push((h(HandleKind::Control2), c2));
                out.push((h(HandleKind::Node), p));
            }
            PathEl::ClosePath => {}
        }
    }
    out
}

/// Moves a handle by `delta`. Moving a node also moves its neighbouring control points, and any
/// node at the same position in the same subpath (the start/end of a closed curve).
pub fn move_handle(path: &BezPath, handle: Handle, delta: Vec2) -> BezPath {
    let mut els: Vec<PathEl> = path.elements().to_vec();
    let Some(&el) = els.get(handle.element) else { return path.clone() };
    match handle.kind {
        HandleKind::Control1 => match &mut els[handle.element] {
            PathEl::QuadTo(c, _) | PathEl::CurveTo(c, _, _) => *c += delta,
            _ => {}
        },
        HandleKind::Control2 => {
            if let PathEl::CurveTo(_, c2, _) = &mut els[handle.element] {
                *c2 += delta;
            }
        }
        HandleKind::Node => {
            let Some(original) = el.end_point() else { return path.clone() };
            // Nodes to move: this one, plus coincident nodes in the same subpath.
            let (start, end) = subpath_range(&els, handle.element);
            let targets: Vec<usize> = (start..end).filter(|&i| i == handle.element || els[i].end_point().is_some_and(|p| p.distance(original) < 1e-9)).collect();
            for i in targets {
                match &mut els[i] {
                    PathEl::MoveTo(p) | PathEl::LineTo(p) | PathEl::QuadTo(_, p) => *p += delta,
                    PathEl::CurveTo(_, c2, p) => {
                        *c2 += delta;
                        *p += delta;
                    }
                    PathEl::ClosePath => {}
                }
                if let Some(PathEl::CurveTo(c1, _, _)) = els.get_mut(i + 1) {
                    *c1 += delta;
                }
            }
        }
    }
    BezPath::from_vec(els)
}

/// Element index range [start, end) of the subpath containing `index`.
fn subpath_range(els: &[PathEl], index: usize) -> (usize, usize) {
    let start = (0..=index).rev().find(|&i| matches!(els[i], PathEl::MoveTo(_))).unwrap_or(0);
    let end = (start + 1..els.len()).find(|&i| matches!(els[i], PathEl::MoveTo(_))).unwrap_or(els.len());
    (start, end)
}

/// Removes a node. Deleting a subpath's first node makes the next node its start.
/// Returns `None` if the subpath would be left with fewer than two nodes.
pub fn delete_node(path: &BezPath, element: usize) -> Option<BezPath> {
    let mut els: Vec<PathEl> = path.elements().to_vec();
    let (start, end) = subpath_range(&els, element);
    let nodes = (start..end).filter(|&i| !matches!(els[i], PathEl::ClosePath)).count();
    if nodes <= 2 {
        return None;
    }
    match els[element] {
        PathEl::MoveTo(_) => {
            let next = els[element + 1].end_point()?;
            els.remove(element);
            els[element] = PathEl::MoveTo(next);
        }
        PathEl::ClosePath => return None,
        _ => {
            els.remove(element);
        }
    }
    Some(BezPath::from_vec(els))
}

/// The drawable segment ending at each element (lines, curves, and the implicit closing line).
fn segments_with_index(els: &[PathEl]) -> Vec<(usize, PathSeg)> {
    let mut out = Vec::new();
    let (mut last, mut first) = (Point::ORIGIN, Point::ORIGIN);
    for (i, el) in els.iter().enumerate() {
        match *el {
            PathEl::MoveTo(p) => {
                last = p;
                first = p;
            }
            PathEl::LineTo(p) => {
                out.push((i, PathSeg::Line(Line::new(last, p))));
                last = p;
            }
            PathEl::QuadTo(c, p) => {
                out.push((i, PathSeg::Quad(QuadBez::new(last, c, p))));
                last = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                out.push((i, PathSeg::Cubic(CubicBez::new(last, c1, c2, p))));
                last = p;
            }
            PathEl::ClosePath => {
                if last.distance(first) > 1e-9 {
                    out.push((i, PathSeg::Line(Line::new(last, first))));
                }
                last = first;
            }
        }
    }
    out
}

/// Splits the segment nearest to `p` (within `tolerance`) at the closest point, adding a node there.
pub fn insert_node(path: &BezPath, p: Point, tolerance: f64) -> Option<BezPath> {
    let mut els: Vec<PathEl> = path.elements().to_vec();
    let (index, seg, t) = segments_with_index(&els)
        .into_iter()
        .map(|(i, seg)| {
            let n = seg.nearest(p, 1e-6);
            (i, seg, n.t, n.distance_sq)
        })
        .filter(|(.., d)| d.sqrt() <= tolerance)
        .min_by(|a, b| a.3.total_cmp(&b.3))
        .map(|(i, seg, t, _)| (i, seg, t))?;
    if !(1e-6..1.0 - 1e-6).contains(&t) {
        return None; // too close to an existing node
    }
    let to_el = |s: PathSeg| match s {
        PathSeg::Line(l) => PathEl::LineTo(l.p1),
        PathSeg::Quad(q) => PathEl::QuadTo(q.p1, q.p2),
        PathSeg::Cubic(c) => PathEl::CurveTo(c.p1, c.p2, c.p3),
    };
    let (a, b) = (seg.subsegment(0.0..t), seg.subsegment(t..1.0));
    if matches!(els[index], PathEl::ClosePath) {
        // The closing line: add a node before closing.
        els.insert(index, PathEl::LineTo(seg.eval(t)));
    } else {
        els[index] = to_el(b);
        els.insert(index, to_el(a));
    }
    Some(BezPath::from_vec(els))
}
