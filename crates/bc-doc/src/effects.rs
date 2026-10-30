//! Live effects: changes a shape carries without its own outline being altered. They're applied,
//! in order, every time the shape's outline is worked out (drawing, selecting, burning), so they
//! can be tweaked, reordered or removed at any time, then applied for good when final.
//!
//! Effects work in document millimetres, on the shape as placed. Effects that carry a second
//! outline (a blend's end shape, a clipping mask) keep it in the shape's own coordinates, so it
//! moves, turns and scales with the shape.

use bc_geom::{Affine, BezPath, Point, Vec2};
use serde::{Deserialize, Serialize};

/// One live effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    /// Grow (positive) or shrink (negative) the outline by this many mm, with round corners.
    Offset { distance: f64 },
    /// Round sharp corners to this radius (mm).
    RoundCorners { radius: f64 },
    /// Bend inside the shape's own bounds (see `bc_geom::warp`).
    Warp { kind: bc_geom::warp::Warp, amount: f64 },
    /// Remove points within this many mm of a straight line.
    Simplify { tolerance: f64 },
    /// Turn the outline into smooth curves, within this many mm.
    Smooth { tolerance: f64 },
    /// Copies turned round a centre `radius` mm below the shape's middle, like the petals of a
    /// flower; `weld` joins overlapping copies into one outline.
    RadialRepeat { copies: u32, radius: f64, weld: bool },
    /// Bend the shape by a grid of handles over its bounds: `cols` × `rows` cells, and each
    /// handle's move from its place as a fraction of the bounds' width and height (row by row from
    /// the top, `(cols + 1) × (rows + 1)` of them).
    Envelope { cols: u32, rows: u32, moves: Vec<(f64, f64)> },
    /// The shape morphing into `end` (in the shape's own coordinates) in `steps` in-between
    /// shapes, with both ends kept.
    Blend { steps: u32, end: BezPath },
    /// Only the parts of the shape inside `mask` (in the shape's own coordinates) are kept.
    Clip { mask: BezPath },
}

impl Effect {
    /// The effects that can be added, with their starting settings.
    /// (Blend and Clip are made from two selected shapes, so they aren't in this list.)
    pub fn choices() -> [Effect; 7] {
        [
            Effect::Offset { distance: 1.0 },
            Effect::RoundCorners { radius: 2.0 },
            Effect::Warp { kind: bc_geom::warp::Warp::Arch, amount: 0.2 },
            Effect::Simplify { tolerance: 0.1 },
            Effect::Smooth { tolerance: 0.2 },
            Effect::RadialRepeat { copies: 8, radius: 20.0, weld: false },
            Effect::envelope(2, 2),
        ]
    }

    /// An envelope with every handle in its place.
    pub fn envelope(cols: u32, rows: u32) -> Effect {
        Effect::Envelope { cols, rows, moves: vec![(0.0, 0.0); ((cols + 1) * (rows + 1)) as usize] }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Effect::Offset { .. } => "Offset",
            Effect::RoundCorners { .. } => "Round corners",
            Effect::Warp { .. } => "Warp",
            Effect::Simplify { .. } => "Simplify",
            Effect::Smooth { .. } => "Smooth",
            Effect::RadialRepeat { .. } => "Radial repeat",
            Effect::Envelope { .. } => "Envelope",
            Effect::Blend { .. } => "Blend",
            Effect::Clip { .. } => "Clip",
        }
    }

    /// The effect applied to `path`. Anything that would leave nothing (too big an inward offset,
    /// say) passes the path through unchanged, so a setting never makes the shape vanish.
    pub fn apply(&self, path: BezPath) -> BezPath {
        self.apply_placed(path, Affine::IDENTITY)
    }

    /// [`apply`](Self::apply) for a shape placed by `placement` (which moves the outlines the
    /// effect carries).
    pub fn apply_placed(&self, path: BezPath, placement: Affine) -> BezPath {
        let out = match self {
            Effect::RadialRepeat { copies, radius, weld } => radial_repeat(&path, *copies, *radius, *weld),
            Effect::Envelope { cols, rows, moves } => bc_geom::bounds(&path).map(|b| envelope(&path, b, *cols, *rows, moves)),
            Effect::Blend { steps, end } => Some(blend(&path, &(placement * end.clone()), *steps)),
            Effect::Clip { mask } => Some(clip(&path, &(placement * mask.clone()))),
            Effect::Offset { distance } if distance.abs() > 1e-9 => bc_geom::ops::offset(&path, *distance),
            Effect::Offset { .. } => None,
            Effect::RoundCorners { radius } => (*radius > 0.0).then(|| bc_geom::fillet(&path, *radius)),
            Effect::Warp { kind, amount } => bc_geom::bounds(&path).map(|b| bc_geom::warp::warp(&path, b, *kind, *amount)),
            Effect::Simplify { tolerance } => Some(bc_geom::warp::simplify(&path, tolerance.max(0.001), false)),
            Effect::Smooth { tolerance } => Some(bc_geom::warp::simplify(&path, tolerance.max(0.001), true)),
        };
        match out {
            Some(p) if !p.elements().is_empty() => p,
            _ => path,
        }
    }
}

/// `effects` applied in order.
pub fn apply_all(effects: &[Effect], path: BezPath) -> BezPath {
    apply_all_placed(effects, path, Affine::IDENTITY)
}

/// `effects` applied in order to a shape placed by `placement`. Results are remembered, so a
/// shape whose outline and effects haven't changed costs nothing to work out again.
pub fn apply_all_placed(effects: &[Effect], path: BezPath, placement: Affine) -> BezPath {
    if effects.is_empty() {
        return path;
    }
    let key = memo::key(effects, &path, placement);
    if let Some(p) = memo::get(key) {
        return p;
    }
    let out = effects.iter().fold(path, |p, e| e.apply_placed(p, placement));
    memo::put(key, &out);
    out
}

/// [`apply_all_placed`] for drawing on screen: a result not worked out yet is worked out on a
/// background thread and `None` comes back meanwhile (draw the shape without its effects, and
/// again while [`busy`]), so slow effects never hold up the window.
pub fn apply_all_in_background(effects: &[Effect], path: &BezPath, placement: Affine) -> Option<BezPath> {
    if effects.is_empty() {
        return Some(path.clone());
    }
    let key = memo::key(effects, path, placement);
    if let Some(p) = memo::get(key) {
        return Some(p);
    }
    if memo::start(key) {
        let (effects, path) = (effects.to_vec(), path.clone());
        std::thread::spawn(move || {
            let out = effects.iter().fold(path, |p, e| e.apply_placed(p, placement));
            memo::put(key, &out);
            memo::finish(key);
        });
    }
    None
}

/// Whether effects are being worked out in the background.
pub fn busy() -> bool {
    memo::busy()
}

/// Effect results by what they were worked out from (shared between threads).
mod memo {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::hash::{Hash, Hasher};
    use std::sync::{Arc, Mutex, OnceLock};

    /// Results kept before the memory is cleared out (a few designs' worth).
    const MAX_ENTRIES: usize = 4096;

    fn results() -> &'static Mutex<HashMap<u64, Arc<BezPath>>> {
        static R: OnceLock<Mutex<HashMap<u64, Arc<BezPath>>>> = OnceLock::new();
        R.get_or_init(Default::default)
    }

    fn pending() -> &'static Mutex<HashSet<u64>> {
        static P: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
        P.get_or_init(Default::default)
    }

    /// A fingerprint of the effects, the outline they start from and the placement.
    pub fn key(effects: &[Effect], path: &BezPath, placement: Affine) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{effects:?}").hash(&mut h);
        for c in placement.as_coeffs() {
            c.to_bits().hash(&mut h);
        }
        for el in path.elements() {
            std::mem::discriminant(el).hash(&mut h);
            let pts: &[Point] = match el {
                bc_geom::PathEl::MoveTo(p) | bc_geom::PathEl::LineTo(p) => std::slice::from_ref(p),
                bc_geom::PathEl::QuadTo(a, _) => std::slice::from_ref(a),
                bc_geom::PathEl::CurveTo(a, _, _) => std::slice::from_ref(a),
                bc_geom::PathEl::ClosePath => &[],
            };
            for p in pts {
                (p.x.to_bits(), p.y.to_bits()).hash(&mut h);
            }
            if let Some(e) = el.end_point() {
                (e.x.to_bits(), e.y.to_bits()).hash(&mut h);
            }
        }
        h.finish()
    }

    pub fn get(key: u64) -> Option<BezPath> {
        results().lock().ok()?.get(&key).map(|p| (**p).clone())
    }

    pub fn put(key: u64, path: &BezPath) {
        if let Ok(mut r) = results().lock() {
            if r.len() >= MAX_ENTRIES {
                r.clear();
            }
            r.insert(key, Arc::new(path.clone()));
        }
    }

    /// Marks `key` as being worked out; false if it already is.
    pub fn start(key: u64) -> bool {
        pending().lock().is_ok_and(|mut p| p.insert(key))
    }

    pub fn finish(key: u64) {
        if let Ok(mut p) = pending().lock() {
            p.remove(&key);
        }
    }

    pub fn busy() -> bool {
        pending().lock().is_ok_and(|p| !p.is_empty())
    }
}

/// `copies` copies of `path` turned evenly round a point `radius` mm below its middle.
fn radial_repeat(path: &BezPath, copies: u32, radius: f64, weld: bool) -> Option<BezPath> {
    let b = bc_geom::bounds(path)?;
    let centre = Point::new(b.center().x, b.center().y + radius);
    let n = copies.clamp(1, 360);
    let parts: Vec<BezPath> = (0..n).map(|k| Affine::rotate_about(std::f64::consts::TAU * k as f64 / n as f64, centre) * path.clone()).collect();
    if weld && let Some(u) = bc_geom::ops::boolean(&parts, bc_geom::ops::BooleanOp::Union) {
        return Some(u);
    }
    Some(parts.into_iter().flat_map(|p| p.into_elements()).collect())
}

/// Bernstein weight `i` of degree `n` at `t`.
fn bernstein(n: u32, i: u32, t: f64) -> f64 {
    let binom = (0..i).fold(1.0, |acc, k| acc * (n - k) as f64 / (k + 1) as f64);
    binom * t.powi(i as i32) * (1.0 - t).powi((n - i) as i32)
}

/// Bends `path` by the envelope's handles: each point moves by a smooth blend (a Bézier patch) of
/// the handles' moves, so the shape bends without kinks along the grid lines.
fn envelope(path: &BezPath, area: bc_geom::Rect, cols: u32, rows: u32, moves: &[(f64, f64)]) -> BezPath {
    let (cols, rows) = (cols.max(1), rows.max(1));
    if moves.len() != ((cols + 1) * (rows + 1)) as usize || moves.iter().all(|m| *m == (0.0, 0.0)) {
        return path.clone();
    }
    let (w, h) = (area.width().max(1e-9), area.height().max(1e-9));
    let map = |p: Point| {
        let (u, v) = (((p.x - area.x0) / w).clamp(0.0, 1.0), ((p.y - area.y0) / h).clamp(0.0, 1.0));
        let mut d = Vec2::ZERO;
        for j in 0..=rows {
            let bv = bernstein(rows, j, v);
            for i in 0..=cols {
                let (mx, my) = moves[(j * (cols + 1) + i) as usize];
                d += Vec2::new(mx * w, my * h) * (bv * bernstein(cols, i, u));
            }
        }
        p + d
    };
    bc_geom::warp::map_points(path, area, map)
}

/// The outline as `n` evenly spaced steps round each subpath (open ones get both ends).
fn resampled(path: &BezPath, n: usize) -> Vec<(Vec<Point>, bool)> {
    bc_geom::flatten(path, 0.02)
        .into_iter()
        .filter(|l| l.points.len() >= 2)
        .map(|l| {
            let mut pts = l.points;
            if l.closed && pts.first() != pts.last() {
                pts.push(pts[0]);
            }
            let mut lens = vec![0.0];
            for w in pts.windows(2) {
                lens.push(lens[lens.len() - 1] + w[0].distance(w[1]));
            }
            let total = lens[lens.len() - 1];
            let count = if l.closed { n } else { n + 1 };
            let out = (0..count)
                .map(|k| {
                    let s = total * k as f64 / n as f64;
                    let i = lens.partition_point(|&x| x < s).clamp(1, pts.len() - 1);
                    let span = (lens[i] - lens[i - 1]).max(1e-12);
                    pts[i - 1].lerp(pts[i], ((s - lens[i - 1]) / span).clamp(0.0, 1.0))
                })
                .collect();
            (out, l.closed)
        })
        .collect()
}

/// Lines a closed loop `b` up with `a`: the same direction, and starting from the point that
/// makes the two match best, so the in-between shapes don't twist.
fn align_loop(a: &[Point], b: &mut [Point]) {
    let area = |p: &[Point]| p.iter().zip(p.iter().cycle().skip(1)).map(|(q, r)| q.x * r.y - r.x * q.y).sum::<f64>();
    if area(a).signum() != area(b).signum() {
        b.reverse();
    }
    let ca = a.iter().fold(Vec2::ZERO, |s, p| s + p.to_vec2()) / a.len() as f64;
    let cb = b.iter().fold(Vec2::ZERO, |s, p| s + p.to_vec2()) / b.len() as f64;
    let n = b.len().min(a.len());
    let cost = |k: usize| (0..n).step_by((n / 32).max(1)).map(|i| ((a[i] - ca) - (b[(i + k) % n] - cb)).hypot2()).sum::<f64>();
    if let Some(best) = (0..n).min_by(|&x, &y| cost(x).total_cmp(&cost(y))) {
        b.rotate_left(best);
    }
}

/// `path`, `steps` shapes morphing towards `end`, and `end`.
fn blend(path: &BezPath, end: &BezPath, steps: u32) -> BezPath {
    const N: usize = 256;
    let a = resampled(path, N);
    let mut b = resampled(end, N);
    if a.is_empty() || b.is_empty() {
        return path.clone();
    }
    // Pair the subpaths in order (repeating the last of the shorter list).
    let mut pairs: Vec<(Vec<Point>, Vec<Point>, bool)> = Vec::new();
    for i in 0..a.len().max(b.len()) {
        let (pa, ca) = &a[i.min(a.len() - 1)];
        let k = i.min(b.len() - 1);
        let closed = *ca && b[k].1;
        if closed {
            align_loop(pa, &mut b[k].0);
        }
        pairs.push((pa.clone(), b[k].0.clone(), closed));
    }
    let mut out = path.clone();
    let n = steps.clamp(1, 500);
    for s in 1..=n {
        let t = s as f64 / (n + 1) as f64;
        for (pa, pb, closed) in &pairs {
            let pts: Vec<Point> = pa.iter().zip(pb.iter()).map(|(p, q)| p.lerp(*q, t)).collect();
            // Straight segments between 256 evenly spaced points: smooth enough, and no overshoot
            // where a curve fit would meet itself.
            out.extend(bc_geom::warp::simplify(&bc_geom::polyline_path(&pts, *closed), 0.01, false));
        }
    }
    out.extend(end.iter());
    out
}

/// The open subpaths of `path` (lines that enclose nothing).
fn open_only(path: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    let mut sub: Vec<bc_geom::PathEl> = Vec::new();
    let flush = |sub: &mut Vec<bc_geom::PathEl>, out: &mut BezPath| {
        let p: BezPath = sub.iter().copied().collect();
        if bc_geom::curveops::closed_only(&p).elements().is_empty() {
            out.extend(p);
        }
        sub.clear();
    };
    for el in path.elements() {
        if matches!(el, bc_geom::PathEl::MoveTo(_)) {
            flush(&mut sub, &mut out);
        }
        sub.push(*el);
    }
    flush(&mut sub, &mut out);
    out
}

/// What of `path` lies inside `mask`: closed outlines cut to it, open lines trimmed.
fn clip(path: &BezPath, mask: &BezPath) -> BezPath {
    let closed = bc_geom::curveops::closed_only(path);
    let mut out = if closed.elements().is_empty() { BezPath::new() } else { bc_geom::ops::boolean(&[closed, mask.clone()], bc_geom::ops::BooleanOp::Intersect).unwrap_or_default() };
    let open = open_only(path);
    if !open.elements().is_empty() {
        out.extend(bc_geom::ops::clip_lines(&open, mask));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> BezPath {
        bc_geom::rect_path(20.0, 20.0, 0.0)
    }

    #[test]
    fn effects_stack_in_order() {
        let grown = apply_all(&[Effect::Offset { distance: 3.0 }], square());
        assert!((bc_geom::bounds(&grown).unwrap().width() - 26.0).abs() < 0.1);
        let both = apply_all(&[Effect::Offset { distance: 3.0 }, Effect::Warp { kind: bc_geom::warp::Warp::Arch, amount: 0.3 }], square());
        assert!(bc_geom::bounds(&both).unwrap().height() > 27.0, "arched after growing");
    }

    #[test]
    fn radial_repeat_makes_copies_round_a_centre() {
        let petals = Effect::RadialRepeat { copies: 6, radius: 30.0, weld: false }.apply(square());
        let b = bc_geom::bounds(&petals).unwrap();
        assert!(b.width() > 70.0 && b.height() > 70.0, "{b:?}");
        let welded = Effect::RadialRepeat { copies: 12, radius: 8.0, weld: true }.apply(square());
        assert!(bc_geom::ops::area(&welded) < 12.0 * 400.0, "overlaps are joined");
    }

    #[test]
    fn an_envelope_moves_a_corner_and_leaves_the_rest() {
        let mut e = Effect::envelope(1, 1);
        if let Effect::Envelope { moves, .. } = &mut e {
            moves[3] = (0.5, 0.5); // bottom-right handle out by half the size
        }
        let b = bc_geom::bounds(&e.apply(square())).unwrap();
        assert!((b.x0 + 10.0).abs() < 0.1 && (b.y0 + 10.0).abs() < 0.1, "top-left stays: {b:?}");
        assert!((b.x1 - 20.0).abs() < 0.2 && (b.y1 - 20.0).abs() < 0.2, "bottom-right moved: {b:?}");
        assert_eq!(bc_geom::bounds(&Effect::envelope(3, 2).apply(square())).unwrap().width(), 20.0, "untouched");
    }

    #[test]
    fn a_blend_steps_from_one_shape_to_the_other() {
        let end = Affine::translate((100.0, 0.0)) * bc_geom::ellipse_path(5.0, 5.0);
        let b = Effect::Blend { steps: 3, end }.apply(square());
        let subpaths = b.elements().iter().filter(|e| matches!(e, bc_geom::PathEl::MoveTo(_))).count();
        assert_eq!(subpaths, 5, "start, 3 steps, end");
        let bb = bc_geom::bounds(&b).unwrap();
        assert!((bb.x0 + 10.0).abs() < 0.1 && (bb.x1 - 105.0).abs() < 0.1, "{bb:?}");
        // The end shape moves with the shape.
        let moved = Effect::Blend { steps: 1, end: bc_geom::ellipse_path(5.0, 5.0) }.apply_placed(square(), Affine::translate((50.0, 0.0)));
        assert!((bc_geom::bounds(&moved).unwrap().x1 - 55.0).abs() < 0.1);
    }

    #[test]
    fn a_clip_keeps_only_whats_inside_the_mask() {
        let mask = Affine::translate((10.0, 0.0)) * bc_geom::rect_path(20.0, 40.0, 0.0);
        let c = Effect::Clip { mask }.apply(square());
        assert!((bc_geom::ops::area(&c) - 200.0).abs() < 0.5, "the right half of the square");
        let mut line = BezPath::new();
        line.move_to((-50.0, 0.0));
        line.line_to((50.0, 0.0));
        let clipped = Effect::Clip { mask: bc_geom::rect_path(20.0, 20.0, 0.0) }.apply(line);
        assert!((bc_geom::bounds(&clipped).unwrap().width() - 20.0).abs() < 0.1);
    }

    #[test]
    fn slow_effects_are_worked_out_in_the_background_and_remembered() {
        let effects = [Effect::RadialRepeat { copies: 24, radius: 15.0, weld: true }];
        let start = Affine::translate((123.0, 45.0));
        let first = apply_all_in_background(&effects, &square(), start);
        assert!(first.is_none(), "not ready on the first frame");
        let mut ready = None;
        for _ in 0..500 {
            if let Some(p) = apply_all_in_background(&effects, &square(), start) {
                ready = Some(p);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = ready.expect("worked out in the background");
        assert_eq!(ready.elements(), apply_all_placed(&effects, square(), start).elements(), "the same as working it out directly");
    }

    #[test]
    fn a_bad_setting_never_loses_the_shape() {
        let gone = Effect::Offset { distance: -50.0 }.apply(square());
        assert_eq!(bc_geom::bounds(&gone).unwrap().width(), 20.0);
    }
}
