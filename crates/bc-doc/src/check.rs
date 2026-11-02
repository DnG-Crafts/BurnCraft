//! Design Check: problems in a design that would spoil the job, found before it's sent.

use crate::{Document, LayerMode, ShapeKind};
use bc_geom::{Point, Vec2};
use std::collections::{BTreeSet, HashMap};

pub type Selection = BTreeSet<u64>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Problem {
    /// Open shapes on a Fill layer: there's no inside to fill.
    OpenOnFill,
    /// Open shapes on a cutting layer: fine for score lines, but a part won't drop out.
    OpenOnLine,
    /// The same shape twice on the same layer: it's cut twice.
    Duplicate,
    /// Lines of different shapes on top of each other (shared edges): cut twice.
    Overlap,
    /// Shapes too small to see (under 0.1 mm), often left over from editing.
    Tiny,
    /// Shapes that are partly or wholly outside the work area.
    Outside,
}

impl Problem {
    pub fn describe(self) -> &'static str {
        match self {
            Problem::OpenOnFill => "open shape(s) on Fill layers: they have no inside to fill",
            Problem::OpenOnLine => "open shape(s) on cutting layers: fine for score lines, but a part cut with one won't drop out",
            Problem::Duplicate => "shape(s) with an exact copy on the same layer: they'd be cut twice",
            Problem::Overlap => "shape(s) with lines on top of another shape's (shared edges): those lines are cut twice",
            Problem::Tiny => "tiny shape(s), smaller than 0.1 mm: usually left over from editing",
            Problem::Outside => "shape(s) outside the work area",
        }
    }
}

/// One kind of problem and the shapes that have it.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub problem: Problem,
    pub shapes: Selection,
}

/// Lines closer than this (mm) count as on top of each other.
const OVERLAP_DISTANCE: f64 = 0.02;
/// Shared runs shorter than this (mm) are ignored (shapes touching at a corner, say).
const OVERLAP_LENGTH: f64 = 0.5;
/// Beyond this many line segments, the overlap check is skipped (it would be slow).
const MAX_SEGMENTS: usize = 60_000;

/// Checks the shapes that will be output (layers with Output on, not the tool layers).
pub fn check(doc: &Document, bed: (f64, f64)) -> Vec<Finding> {
    let output: Vec<_> = doc.shapes.iter().filter(|s| !crate::is_tool_layer(s.layer) && doc.layers.get(s.layer).is_some_and(|l| l.output)).collect();
    let mut found: Vec<(Problem, Selection)> = Vec::new();
    let mut add = |p: Problem, id: u64| match found.iter_mut().find(|(q, _)| *q == p) {
        Some((_, ids)) => {
            ids.insert(id);
        }
        None => found.push((p, Selection::from([id]))),
    };
    for s in &output {
        let image = matches!(s.kind, ShapeKind::Image { .. });
        let mode = doc.layers[s.layer].mode;
        if !image && !s.is_closed() {
            match mode {
                LayerMode::Fill => add(Problem::OpenOnFill, s.id),
                LayerMode::Line | LayerMode::FillThenLine => add(Problem::OpenOnLine, s.id),
                LayerMode::Image => {}
            }
        }
        if let Some(b) = s.bounds() {
            if !image && b.width().max(b.height()) < 0.1 {
                add(Problem::Tiny, s.id);
            }
            if b.x0 < -1e-6 || b.y0 < -1e-6 || b.x1 > bed.0 + 1e-6 || b.y1 > bed.1 + 1e-6 {
                add(Problem::Outside, s.id);
            }
        }
    }
    let output_ids: Selection = output.iter().map(|s| s.id).collect();
    for id in crate::edit::find_duplicates(doc) {
        if output_ids.contains(&id) {
            add(Problem::Duplicate, id);
        }
    }
    for id in overlapping(doc, &output_ids) {
        add(Problem::Overlap, id);
    }
    found.sort_by_key(|(p, _)| *p);
    found.into_iter().map(|(problem, shapes)| Finding { problem, shapes }).collect()
}

/// Shapes (among `ids`, on cutting layers) with a line on top of a line of another shape on the
/// same layer. Exact duplicates are left to the duplicate check.
fn overlapping(doc: &Document, ids: &Selection) -> Selection {
    let dupes = crate::edit::find_duplicates(doc);
    let mut segments: Vec<(u64, usize, Point, Point)> = Vec::new();
    for s in doc.shapes.iter().filter(|s| ids.contains(&s.id) && !dupes.contains(&s.id)) {
        if !matches!(doc.layers[s.layer].mode, LayerMode::Line | LayerMode::FillThenLine) || matches!(s.kind, ShapeKind::Image { .. }) {
            continue;
        }
        for line in bc_geom::flatten(&s.world_path(), 0.05) {
            let mut pts = line.points;
            if line.closed && let Some(&first) = pts.first() {
                pts.push(first);
            }
            for w in pts.windows(2) {
                if w[0].distance(w[1]) > 1e-6 {
                    segments.push((s.id, s.layer, w[0], w[1]));
                }
            }
        }
        if segments.len() > MAX_SEGMENTS {
            return Selection::new();
        }
    }
    // Segments by the 5 mm grid cells their bounding boxes touch.
    const CELL: f64 = 5.0;
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, (_, _, a, b)) in segments.iter().enumerate() {
        let (x0, x1) = ((a.x.min(b.x) / CELL).floor() as i64, (a.x.max(b.x) / CELL).floor() as i64);
        let (y0, y1) = ((a.y.min(b.y) / CELL).floor() as i64, (a.y.max(b.y) / CELL).floor() as i64);
        for x in x0..=x1 {
            for y in y0..=y1 {
                grid.entry((x, y)).or_default().push(i);
            }
        }
    }
    let mut out = Selection::new();
    for cell in grid.values() {
        for (k, &i) in cell.iter().enumerate() {
            for &j in &cell[k + 1..] {
                let (si, li, a, b) = segments[i];
                let (sj, lj, c, d) = segments[j];
                if si != sj && li == lj && !(out.contains(&si) && out.contains(&sj)) && shared_run(a, b, c, d) > OVERLAP_LENGTH {
                    out.insert(si);
                    out.insert(sj);
                }
            }
        }
    }
    out
}

/// How long (mm) segments a–b and c–d run on top of each other.
fn shared_run(a: Point, b: Point, c: Point, d: Point) -> f64 {
    let len = a.distance(b);
    let dir: Vec2 = (b - a) / len;
    let off = |p: Point| (p - a).cross(dir).abs();
    if off(c) > OVERLAP_DISTANCE || off(d) > OVERLAP_DISTANCE {
        return 0.0;
    }
    let along = |p: Point| (p - a).dot(dir);
    let (t0, t1) = (along(c).min(along(d)), along(c).max(along(d)));
    (t1.min(len) - t0.max(0.0)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bc_geom::Affine;

    fn rect(doc: &mut Document, x: f64, y: f64, w: f64, h: f64, layer: usize) -> u64 {
        doc.add_shape(ShapeKind::Rect { width: w, height: h, corner_radius: 0.0 }, Affine::translate((x + w / 2.0, y + h / 2.0)), layer)
    }

    fn problems(doc: &Document) -> Vec<(Problem, Vec<u64>)> {
        check(doc, (300.0, 200.0)).into_iter().map(|f| (f.problem, f.shapes.into_iter().collect())).collect()
    }

    #[test]
    fn a_clean_design_has_no_problems() {
        let mut doc = Document::default();
        rect(&mut doc, 10.0, 10.0, 20.0, 20.0, 0);
        rect(&mut doc, 50.0, 10.0, 20.0, 20.0, 0);
        assert!(problems(&doc).is_empty());
    }

    #[test]
    fn finds_each_kind_of_problem() {
        let mut doc = Document::default();
        doc.layers[1].mode = LayerMode::Fill;
        // Two squares sharing an edge (x = 30), and a copy of the first.
        let a = rect(&mut doc, 10.0, 10.0, 20.0, 20.0, 0);
        let b = rect(&mut doc, 30.0, 10.0, 20.0, 20.0, 0);
        let copy = rect(&mut doc, 10.0, 10.0, 20.0, 20.0, 0);
        let line = |doc: &mut Document, layer, from: (f64, f64), to: (f64, f64)| doc.add_shape(ShapeKind::Path(bc_geom::polyline_path(&[Point::new(from.0, from.1), Point::new(to.0, to.1)], false)), Affine::IDENTITY, layer);
        let open_fill = line(&mut doc, 1, (100.0, 100.0), (120.0, 100.0));
        let tiny = rect(&mut doc, 150.0, 150.0, 0.05, 0.05, 0);
        let outside = rect(&mut doc, 290.0, 10.0, 20.0, 20.0, 0);
        // On a layer with Output off: never reported.
        doc.layers[2].output = false;
        rect(&mut doc, 400.0, 400.0, 1.0, 1.0, 2);
        let p = problems(&doc);
        assert_eq!(
            p,
            vec![
                (Problem::OpenOnFill, vec![open_fill]),
                (Problem::Duplicate, vec![copy]),
                (Problem::Overlap, vec![a, b]),
                (Problem::Tiny, vec![tiny]),
                (Problem::Outside, vec![outside]),
            ]
        );
    }

    #[test]
    fn runs_on_top_of_each_other() {
        let p = |x: f64, y: f64| Point::new(x, y);
        assert!((shared_run(p(0.0, 0.0), p(10.0, 0.0), p(4.0, 0.0), p(20.0, 0.0)) - 6.0).abs() < 1e-9);
        assert_eq!(shared_run(p(0.0, 0.0), p(10.0, 0.0), p(4.0, 1.0), p(20.0, 1.0)), 0.0, "parallel, apart");
        assert_eq!(shared_run(p(0.0, 0.0), p(10.0, 0.0), p(10.0, 0.0), p(10.0, 5.0)), 0.0, "only touching");
    }
}
