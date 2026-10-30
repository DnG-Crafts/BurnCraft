//! Packaging dielines: tuck-end boxes, pillow boxes and envelopes, as a cut outline plus fold
//! lines. On a laser, the fold lines are scored: cut at low power, or with a dashed line, so the
//! card folds cleanly along them.
//!
//! Coordinates: mm, the net's top-left corner at the origin, Y down.

use crate::ops::{boolean, BooleanOp};
use crate::{polyline_path, BezPath, Point};

/// A dieline: what to cut, and where to fold.
pub struct Dieline {
    /// The outline of the whole net.
    pub cut: BezPath,
    /// Fold lines (open lines), to score.
    pub folds: BezPath,
    /// The net's width and height (mm).
    pub size: (f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DielineKind {
    TuckBox,
    PillowBox,
    Envelope,
}

impl DielineKind {
    pub const ALL: [DielineKind; 3] = [DielineKind::TuckBox, DielineKind::PillowBox, DielineKind::Envelope];

    pub fn label(self) -> &'static str {
        match self {
            DielineKind::TuckBox => "Tuck-end box",
            DielineKind::PillowBox => "Pillow box",
            DielineKind::Envelope => "Envelope",
        }
    }
}

/// Sizes (mm). Each kind uses what it needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DielineSpec {
    /// Width of the front (the envelope's pocket width).
    pub width: f64,
    /// Depth (side panels); unused by the envelope.
    pub depth: f64,
    /// Height of the body (the pillow box's length, the envelope's pocket height).
    pub height: f64,
    /// Glue tab width.
    pub glue: f64,
    /// Tuck flap depth (the envelope's seal flap).
    pub tuck: f64,
}

impl Default for DielineSpec {
    fn default() -> Self {
        Self { width: 60.0, depth: 30.0, height: 90.0, glue: 12.0, tuck: 18.0 }
    }
}

pub fn dieline(kind: DielineKind, s: &DielineSpec) -> Result<Dieline, String> {
    match kind {
        DielineKind::TuckBox => tuck_box(s),
        DielineKind::PillowBox => pillow_box(s),
        DielineKind::Envelope => envelope(s),
    }
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
    poly(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

fn poly(pts: &[(f64, f64)]) -> BezPath {
    polyline_path(&pts.iter().map(|&(x, y)| Point::new(x, y)).collect::<Vec<_>>(), true)
}

fn line(path: &mut BezPath, x0: f64, y0: f64, x1: f64, y1: f64) {
    path.move_to((x0, y0));
    path.line_to((x1, y1));
}

fn union(regions: &[BezPath]) -> Result<BezPath, String> {
    boolean(regions, BooleanOp::Union).ok_or_else(|| "couldn't join the panels".to_string())
}

/// A straight tuck-end carton: four walls (front, side, back, side) and a glue tab, closing
/// panels with tuck flaps on the back wall, and dust flaps on the sides.
pub fn tuck_box(s: &DielineSpec) -> Result<Dieline, String> {
    let (w, d, h) = (s.width.max(5.0), s.depth.max(3.0), s.height.max(5.0));
    let glue = s.glue.max(4.0);
    let tuck = s.tuck.max(4.0).min(d * 1.2);
    let flap = d; // closing panel
    let dust = d * 0.9;
    let ch = (d * 0.3).min(6.0); // chamfer
    let inset = 2.0; // tuck clearance

    // Columns: glue | front | side | back | side.
    let x_front = glue;
    let x_side_l = x_front + w;
    let x_back = x_side_l + d;
    let x_side_r = x_back + w;
    let x_end = x_side_r + d;
    let top = flap + tuck;
    let bottom = top + h;

    let mut regions = vec![rect(x_front, top, x_end, bottom), rect(0.0, top + ch, x_front, bottom - ch)];
    // Closing panels with their tuck flaps, top and bottom.
    for (edge, dir) in [(top, -1.0), (bottom, 1.0)] {
        let far = edge + dir * flap;
        regions.push(rect(x_back, edge.min(far), x_side_r, edge.max(far)));
        regions.push(poly(&[(x_back + inset, far), (x_back + inset + ch, far + dir * tuck), (x_side_r - inset - ch, far + dir * tuck), (x_side_r - inset, far)]));
        for (a, b) in [(x_side_l, x_back), (x_side_r, x_end)] {
            regions.push(poly(&[(a, edge), (a + ch, edge + dir * dust), (b - ch, edge + dir * dust), (b, edge)]));
        }
    }
    let mut folds = BezPath::new();
    for x in [x_front, x_side_l, x_back, x_side_r] {
        line(&mut folds, x, top, x, bottom);
    }
    line(&mut folds, x_front, top, x_end, top);
    line(&mut folds, x_front, bottom, x_end, bottom);
    line(&mut folds, x_back, top - flap, x_side_r, top - flap);
    line(&mut folds, x_back, bottom + flap, x_side_r, bottom + flap);
    Ok(Dieline { cut: union(&regions)?, folds, size: (x_end, bottom + flap + tuck) })
}

/// A pillow box: four panels and a glue tab, with curved end caps on the wide panels.
pub fn pillow_box(s: &DielineSpec) -> Result<Dieline, String> {
    let (w, d, l) = (s.width.max(10.0), s.depth.max(5.0), s.height.max(10.0));
    let glue = s.glue.max(4.0);
    let cap = (d * 0.6).max(4.0);
    let xs = [glue, glue + w, glue + w + d, glue + 2.0 * w + d, glue + 2.0 * w + 2.0 * d];
    let (top, bottom) = (cap, cap + l);
    let net_h = bottom + cap;
    // A half-ellipse on a panel's edge, out to `peak`.
    let arc = |xa: f64, xb: f64, base: f64, peak: f64| {
        let (mid, rx) = ((xa + xb) / 2.0, (xb - xa) / 2.0);
        let pts: Vec<(f64, f64)> = (0..=24)
            .map(|i| {
                let t = std::f64::consts::PI * i as f64 / 24.0;
                (mid - rx * t.cos(), base + (peak - base) * t.sin())
            })
            .collect();
        poly(&pts)
    };
    let mut regions = vec![rect(0.0, top, xs[4], bottom)];
    for (a, b) in [(xs[0], xs[1]), (xs[2], xs[3])] {
        regions.push(arc(a, b, top, 0.0));
        regions.push(arc(a, b, bottom, net_h));
    }
    let mut folds = BezPath::new();
    for &x in &xs[..4] {
        line(&mut folds, x, top, x, bottom);
    }
    for (a, b) in [(xs[0], xs[1]), (xs[2], xs[3])] {
        line(&mut folds, a, top, b, top);
        line(&mut folds, a, bottom, b, bottom);
    }
    Ok(Dieline { cut: union(&regions)?, folds, size: (xs[4], net_h) })
}

/// A pocket envelope: the pocket with four flaps (the top one seals).
pub fn envelope(s: &DielineSpec) -> Result<Dieline, String> {
    let (w, h) = (s.width.max(20.0), s.height.max(20.0));
    let top_flap = s.tuck.max(h * 0.45);
    let bottom_flap = h * 0.35;
    let side = w * 0.28;
    let ch = (w.min(h) * 0.12).min(12.0);
    let (x0, x1, y0, y1) = (side, side + w, top_flap, top_flap + h);
    let regions = [
        rect(x0, y0, x1, y1),
        poly(&[(x0, y0), (x0 + ch, 0.0), (x1 - ch, 0.0), (x1, y0)]),
        poly(&[(x0, y1), (x0 + ch, y1 + bottom_flap), (x1 - ch, y1 + bottom_flap), (x1, y1)]),
        poly(&[(x0, y0), (0.0, y0 + ch), (0.0, y1 - ch), (x0, y1)]),
        poly(&[(x1, y0), (x1 + side, y0 + ch), (x1 + side, y1 - ch), (x1, y1)]),
    ];
    let mut folds = BezPath::new();
    line(&mut folds, x0, y0, x1, y0);
    line(&mut folds, x0, y1, x1, y1);
    line(&mut folds, x0, y0, x0, y1);
    line(&mut folds, x1, y0, x1, y1);
    Ok(Dieline { cut: union(&regions)?, folds, size: (x1 + side, y1 + bottom_flap) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(p: &BezPath) -> usize {
        p.elements().iter().filter(|e| matches!(e, crate::PathEl::MoveTo(_))).count()
    }

    #[test]
    fn each_kind_has_an_outline_and_folds() {
        for kind in DielineKind::ALL {
            let d = dieline(kind, &DielineSpec::default()).unwrap();
            let b = crate::bounds(&d.cut).unwrap();
            assert!((b.width() - d.size.0).abs() < 0.5 && (b.height() - d.size.1).abs() < 0.5, "{kind:?}: {b:?} vs {:?}", d.size);
            assert_eq!(lines(&d.cut), 1, "{kind:?}: one outline");
            assert!(lines(&d.folds) >= 4, "{kind:?}");
        }
    }

    #[test]
    fn a_tuck_box_net_is_the_sum_of_its_walls() {
        let s = DielineSpec::default();
        let d = tuck_box(&s).unwrap();
        assert!((d.size.0 - (s.glue + 2.0 * s.width + 2.0 * s.depth)).abs() < 1e-9);
        assert_eq!(lines(&d.folds), 8);
    }
}

