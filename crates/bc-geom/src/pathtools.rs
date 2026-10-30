//! Path tools: rhinestone templates, lattice, knockout recesses, pierce points and hatch fill.
//! All in document coordinates (mm).

use crate::ops::{boolean, offset, region, region_to_path, BooleanOp};
use crate::{flatten, polyline_path, BezPath, Point, Rect};
use geo::BooleanOps;

/// Everything in `paths` merged into one region (closed shapes only).
fn merged(paths: &[BezPath]) -> Option<BezPath> {
    let all = paths.iter().map(region).filter(|r| !r.0.is_empty()).reduce(|a, b| a.union(&b))?;
    let out = region_to_path(&all);
    (!out.elements().is_empty()).then_some(out)
}

fn circle(c: Point, r: f64) -> BezPath {
    crate::Affine::translate(c.to_vec2()) * crate::ellipse_path(r, r)
}

/// Where stones go in a rhinestone template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoneLayout {
    /// A row along the outline.
    Outline,
    /// Two rows: along the outline, and a stone's width inside it.
    DoubleOutline,
    /// Filling the shape, in a honeycomb pattern.
    Fill,
}

impl StoneLayout {
    pub const ALL: [StoneLayout; 3] = [StoneLayout::Outline, StoneLayout::DoubleOutline, StoneLayout::Fill];

    pub fn label(self) -> &'static str {
        match self {
            StoneLayout::Outline => "Along the outline",
            StoneLayout::DoubleOutline => "Two rows along the outline",
            StoneLayout::Fill => "Filled",
        }
    }
}

/// Points evenly spaced along each outline of `path`, about `pitch` apart.
fn along(path: &BezPath, pitch: f64, out: &mut Vec<Point>) {
    for line in flatten(path, 0.02) {
        let mut pts = line.points.clone();
        if line.closed && let Some(&first) = pts.first() {
            pts.push(first);
        }
        let len: f64 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        if len < pitch * 0.5 {
            continue;
        }
        let n = if line.closed { (len / pitch).floor().max(1.0) as usize } else { (len / pitch).floor() as usize + 1 };
        let step = if line.closed { len / n as f64 } else { len / (n.max(2) - 1) as f64 };
        let (mut i, mut walked) = (0, 0.0);
        for k in 0..n {
            let target = k as f64 * step;
            while i + 1 < pts.len() && walked + pts[i].distance(pts[i + 1]) < target {
                walked += pts[i].distance(pts[i + 1]);
                i += 1;
            }
            if i + 1 >= pts.len() {
                break;
            }
            let seg = pts[i].distance(pts[i + 1]).max(1e-12);
            out.push(pts[i].lerp(pts[i + 1], ((target - walked) / seg).clamp(0.0, 1.0)));
        }
    }
}

/// A rhinestone template: holes `diameter` mm across (the stone plus a little room) with `gap`
/// mm of material between them, laid out on the selection. Cut from template material, the
/// stones are brushed into the holes.
pub fn rhinestones(paths: &[BezPath], diameter: f64, gap: f64, layout: StoneLayout) -> Option<BezPath> {
    if diameter <= 0.0 {
        return None;
    }
    let pitch = diameter + gap.max(0.0);
    let r = diameter / 2.0;
    let mut centres = Vec::new();
    match layout {
        StoneLayout::Outline | StoneLayout::DoubleOutline => {
            // Closed shapes: their merged outline; open lines (single-line text) as they are.
            let outline = merged(paths);
            if let Some(o) = &outline {
                along(o, pitch, &mut centres);
                if layout == StoneLayout::DoubleOutline
                    && let Some(inner) = offset(o, -pitch)
                {
                    along(&inner, pitch, &mut centres);
                }
            }
            for p in paths.iter().filter(|p| region(p).0.is_empty()) {
                along(p, pitch, &mut centres);
            }
        }
        StoneLayout::Fill => {
            let inside = offset(&merged(paths)?, -r)?;
            let b = crate::bounds(&inside)?;
            let dy = pitch * 3f64.sqrt() / 2.0;
            let mut row = 0;
            let mut y = b.y0;
            while y <= b.y1 + 1e-9 {
                let mut x = b.x0 + if row % 2 == 1 { pitch / 2.0 } else { 0.0 };
                while x <= b.x1 + 1e-9 {
                    let p = Point::new(x, y);
                    if crate::contains(&inside, p) {
                        centres.push(p);
                    }
                    x += pitch;
                }
                y += dy;
                row += 1;
            }
        }
    }
    // Stones too close to one already placed (where outlines meet) are left out.
    let mut kept: Vec<Point> = Vec::new();
    for c in centres {
        if kept.iter().all(|k| k.distance(c) >= pitch * 0.85) {
            kept.push(c);
        }
    }
    let mut out = BezPath::new();
    for c in &kept {
        out.extend(circle(*c, r));
    }
    (!kept.is_empty()).then_some(out)
}

/// A lattice: square holes `cell - bar` mm wide on a `cell` mm grid, inside the shape, leaving
/// at least `bar` mm of material everywhere (round the edge too). For vents, speaker grilles and
/// decorative panels. The result includes the shape's outline.
pub fn lattice(paths: &[BezPath], cell: f64, bar: f64) -> Option<BezPath> {
    let shape = merged(paths)?;
    let b = crate::bounds(&shape)?;
    let cell = cell.max(bar + 0.5);
    let hole = (cell - bar).max(0.2);
    let inner = offset(&shape, -bar)?;
    let inside = |p: Point| crate::contains(&inner, p);
    let mut holes = Vec::new();
    // The grid is centred on the shape, so the margins come out even.
    let (cols, rows) = (((b.width() - bar) / cell).floor().max(0.0) as usize, ((b.height() - bar) / cell).floor().max(0.0) as usize);
    let x0 = b.center().x - (cols as f64 * cell - bar) / 2.0;
    let y0 = b.center().y - (rows as f64 * cell - bar) / 2.0;
    for row in 0..rows {
        for col in 0..cols {
            let r = Rect::new(x0 + col as f64 * cell, y0 + row as f64 * cell, x0 + col as f64 * cell + hole, y0 + row as f64 * cell + hole);
            if [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1), r.center()].into_iter().all(inside) {
                holes.push(polyline_path(&[Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1)], true));
            }
        }
    }
    if holes.is_empty() {
        return None;
    }
    let mut out = shape;
    for h in holes {
        out.extend(h);
    }
    Some(out)
}

/// A knockout: the `bottom` shapes with a recess where the `top` shapes sit, grown by `gap` mm
/// so the top pieces drop in (an inlay pocket, or a cut-out to fit a part into).
pub fn knockout(bottom: &[BezPath], top: &[BezPath], gap: f64) -> Option<BezPath> {
    let base = merged(bottom)?;
    let cutter = merged(top)?;
    let cutter = if gap > 0.0 { offset(&cutter, gap)? } else { cutter };
    boolean(&[base, cutter], BooleanOp::Subtract)
}

/// Pierce points: a small dot `diameter` mm across where each outline starts. Burned first, at
/// higher power or with a pause, they pierce thick material so each cut starts cleanly.
pub fn pierce_points(paths: &[BezPath], diameter: f64) -> Option<BezPath> {
    let mut out = BezPath::new();
    let mut any = false;
    for p in paths {
        for line in flatten(p, 0.05) {
            if let Some(&start) = line.points.first() {
                out.extend(circle(start, diameter.max(0.05) / 2.0));
                any = true;
            }
        }
    }
    any.then_some(out)
}

/// Hatch fill: parallel lines `spacing` mm apart at `angle` degrees across the shape (and at
/// right angles too when `cross`), clipped to it, running back and forth. Engraved as lines,
/// a hatch fills an area faster than scanning, for outlines, light shading or a crosshatch look.
pub fn hatch(paths: &[BezPath], spacing: f64, angle: f64, cross: bool) -> Option<BezPath> {
    let shape = merged(paths)?;
    let b = crate::bounds(&shape)?;
    let spacing = spacing.max(0.05);
    let c = b.center();
    let r = b.width().hypot(b.height()) / 2.0 + spacing;
    let mut lines = BezPath::new();
    let mut angles = vec![angle];
    if cross {
        angles.push(angle + 90.0);
    }
    for a in angles {
        let turn = crate::Affine::rotate_about(a.to_radians(), c);
        let mut y = -r;
        let mut flip = false;
        while y <= r {
            let (mut p0, mut p1) = (Point::new(c.x - r, c.y + y), Point::new(c.x + r, c.y + y));
            if flip {
                std::mem::swap(&mut p0, &mut p1);
            }
            lines.move_to(turn * p0);
            lines.line_to(turn * p1);
            y += spacing;
            flip = !flip;
        }
    }
    let out = crate::ops::clip_lines(&lines, &shape);
    (!out.elements().is_empty()).then_some(out)
}

/// Shadows and depth for a design, added beside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Depth {
    /// A copy of the outline offset by (dx, dy) mm, to sit behind the design.
    DropShadow { dx: f64, dy: f64 },
    /// A shadow lying on the ground behind: sheared sideways by `skew` (per mm of height) and
    /// flattened to `squash` of its height.
    CastShadow { skew: f64, squash: f64 },
    /// A 3D block look: the outline repeated at (dx, dy) behind, joined to the front by lines.
    Extrude { dx: f64, dy: f64 },
}

/// The shadow or 3D lines for `paths` (closed shapes; merged first).
pub fn depth(paths: &[BezPath], kind: Depth) -> Option<BezPath> {
    let shape = merged(paths)?;
    match kind {
        Depth::DropShadow { dx, dy } => Some(crate::Affine::translate((dx, dy)) * shape),
        Depth::CastShadow { skew, squash } => {
            let b = crate::bounds(&shape)?;
            let squash = squash.clamp(0.02, 2.0);
            let mut out = BezPath::new();
            for line in flatten(&shape, 0.02) {
                let pts: Vec<Point> = line
                    .points
                    .iter()
                    .map(|p| {
                        let rise = b.y1 - p.y;
                        Point::new(p.x + rise * skew, b.y1 - rise * squash)
                    })
                    .collect();
                out.extend(polyline_path(&pts, line.closed));
            }
            // Shearing can fold an outline over itself: tidy it into a clean region.
            let tidy = region_to_path(&region(&out));
            Some(if tidy.elements().is_empty() { out } else { tidy })
        }
        Depth::Extrude { dx, dy } => {
            let shift = crate::Vec2::new(dx, dy);
            let mut out = crate::Affine::translate(shift) * shape.clone();
            // Joining lines at the outline's corners and every few mm along curves.
            for line in flatten(&shape, 0.05) {
                let n = line.points.len();
                let mut spaced: Vec<Point> = Vec::new();
                let mut since = f64::INFINITY;
                for i in 0..n {
                    let (prev, here, next) = (line.points[(i + n - 1) % n], line.points[i], line.points[(i + 1) % n]);
                    let turn = (here - prev).normalize().dot((next - here).normalize());
                    since += prev.distance(here);
                    if turn < 0.95 || since >= 4.0 {
                        spaced.push(here);
                        since = 0.0;
                    }
                }
                for p in spaced {
                    out.move_to(p);
                    out.line_to(p + shift);
                }
            }
            Some(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> BezPath {
        polyline_path(&[Point::new(x, y), Point::new(x + s, y), Point::new(x + s, y + s), Point::new(x, y + s)], true)
    }

    fn count(p: &BezPath) -> usize {
        p.elements().iter().filter(|e| matches!(e, crate::PathEl::MoveTo(_))).count()
    }

    #[test]
    fn stones_go_round_the_outline_and_fill_inside() {
        let sq = square(0.0, 0.0, 40.0);
        // 160 mm round at a 4 mm pitch: 40 stones.
        let ring = rhinestones(std::slice::from_ref(&sq), 3.0, 1.0, StoneLayout::Outline).unwrap();
        assert!((38..=40).contains(&count(&ring)), "{}", count(&ring));
        let filled = rhinestones(std::slice::from_ref(&sq), 3.0, 1.0, StoneLayout::Fill).unwrap();
        assert!(count(&filled) > 60, "{}", count(&filled));
        let b = crate::bounds(&filled).unwrap();
        assert!(b.x0 >= -1e-6 && b.x1 <= 40.0 + 1e-6, "stones stay inside: {b:?}");
        let double = rhinestones(&[sq], 3.0, 1.0, StoneLayout::DoubleOutline).unwrap();
        assert!(count(&double) > count(&ring));
    }

    #[test]
    fn lattice_leaves_bars_and_a_border() {
        let l = lattice(&[square(0.0, 0.0, 50.0)], 10.0, 2.0).unwrap();
        // The outline plus a 4 × 4 grid of 8 mm holes.
        assert_eq!(count(&l), 17);
        assert!((crate::ops::area(&l) - (2500.0 - 16.0 * 64.0)).abs() < 1.0, "{}", crate::ops::area(&l));
    }

    #[test]
    fn knockout_makes_a_recess_with_a_gap() {
        let k = knockout(&[square(0.0, 0.0, 50.0)], &[square(20.0, 20.0, 10.0)], 0.5).unwrap();
        let hole = crate::ops::area(&square(0.0, 0.0, 50.0)) - crate::ops::area(&k);
        assert!(hole > 100.0 && hole < 125.0, "recess slightly bigger than the part: {hole}");
    }

    #[test]
    fn shadows_and_extrusion() {
        let sq = [square(0.0, 0.0, 20.0)];
        let drop = crate::bounds(&depth(&sq, Depth::DropShadow { dx: 3.0, dy: 4.0 }).unwrap()).unwrap();
        assert!((drop.x0 - 3.0).abs() < 1e-6 && (drop.y0 - 4.0).abs() < 1e-6);
        let cast = crate::bounds(&depth(&sq, Depth::CastShadow { skew: 1.0, squash: 0.3 }).unwrap()).unwrap();
        assert!(cast.width() > 30.0 && cast.height() < 8.0, "lies down: {cast:?}");
        let block = depth(&sq, Depth::Extrude { dx: 5.0, dy: -5.0 }).unwrap();
        assert!(count(&block) >= 5, "the back face and joining lines: {}", count(&block));
    }

    #[test]
    fn pierce_points_and_hatch() {
        let both = [square(0.0, 0.0, 10.0), square(20.0, 0.0, 10.0)];
        assert_eq!(count(&pierce_points(&both, 0.5).unwrap()), 2);
        let h = hatch(&[square(0.0, 0.0, 10.0)], 1.0, 0.0, false).unwrap();
        assert!((9..=11).contains(&count(&h)), "{}", count(&h));
        let b = crate::bounds(&h).unwrap();
        assert!(b.x0 >= -1e-6 && b.x1 <= 10.0 + 1e-6);
        assert!(count(&hatch(&[square(0.0, 0.0, 10.0)], 1.0, 0.0, true).unwrap()) > 18);
    }
}
