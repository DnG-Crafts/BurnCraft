//! True-shape nesting: packs parts by their real outlines, not their bounding boxes, so they tuck
//! into each other's curves and notches, and small parts can go inside holes in bigger ones.
//!
//! Each part (in each allowed rotation) is rasterised onto a grid, grown by half the spacing plus a
//! safety margin of most of a cell, so parts that don't overlap on the grid can't overlap for real.
//! Parts are placed largest first, each at the top-most then left-most spot where it fits
//! ("bottom-left fill", top-down). Grid rows are bit-packed so overlap tests are 64 cells at a time.

use crate::ops::offset;
use crate::{bounds, scanline_mask, Affine, BezPath, Rect};

pub struct NestOptions {
    /// Where parts may go (the bed minus its margin).
    pub area: Rect,
    /// Gap between parts, mm.
    pub spacing: f64,
    /// Rotations to try, in degrees (e.g. `[0, 90, 180, 270]`).
    pub rotations: Vec<f64>,
    /// Grid cell size, mm (smaller packs tighter but takes longer).
    pub cell: f64,
}

impl NestOptions {
    /// A cell size giving about `cells` grid cells over `area` (never finer than 0.25 mm).
    pub fn auto_cell(area: Rect, cells: f64) -> f64 {
        (area.width() * area.height() / cells).sqrt().max(0.25)
    }
}

/// A part rasterised in one rotation: bit-packed rows, and where its grid's corner sits.
struct Mask {
    rows: Vec<Vec<u64>>,
    cols: usize,
    /// The part's rotation about its centre, and the document point at the mask's (0, 0) corner.
    rotation: Affine,
    corner: crate::Point,
    filled: usize,
}

fn words(bits: usize) -> usize {
    bits.div_ceil(64)
}

/// Rasterises a part (several outlines) after rotating it, grown by `grow` mm.
fn mask(outlines: &[BezPath], rotation: Affine, grow: f64, cell: f64) -> Option<Mask> {
    let rotated: Vec<BezPath> = outlines.iter().map(|p| rotation * p.clone()).collect();
    // Each outline's region grown by `grow`; open lines (no region) use their grown bounding box.
    let grown: Vec<BezPath> = rotated
        .iter()
        .filter_map(|p| offset(p, grow).or_else(|| bounds(p).map(|b| crate::polyline_path(&[b.origin(), (b.x1, b.y0).into(), (b.x1, b.y1).into(), (b.x0, b.y1).into()].map(|q: crate::Point| q + (q - b.center()).normalize() * grow * std::f64::consts::SQRT_2), true))))
        .collect();
    let b = grown.iter().filter_map(bounds).reduce(|a, b| a.union(b))?;
    let cols = ((b.width() / cell).ceil() as usize).max(1);
    let rows = ((b.height() / cell).ceil() as usize).max(1);
    let area = Rect::new(b.x0, b.y0, b.x0 + cols as f64 * cell, b.y0 + rows as f64 * cell);
    let mut bits = vec![vec![0u64; words(cols)]; rows];
    let mut filled = 0;
    for g in &grown {
        for (i, inside) in scanline_mask(g, area, cols, rows).into_iter().enumerate() {
            let (r, c) = (i / cols, i % cols);
            if inside && bits[r][c / 64] & (1 << (c % 64)) == 0 {
                bits[r][c / 64] |= 1 << (c % 64);
                filled += 1;
            }
        }
    }
    Some(Mask { rows: bits, cols, rotation, corner: b.origin(), filled })
}

/// 64 grid bits of `row` starting at bit `start`.
fn window(row: &[u64], start: usize) -> u64 {
    let (w, o) = (start / 64, start % 64);
    let lo = row.get(w).copied().unwrap_or(0) >> o;
    let hi = if o > 0 { row.get(w + 1).copied().unwrap_or(0) << (64 - o) } else { 0 };
    lo | hi
}

struct Grid {
    rows: Vec<Vec<u64>>,
    width: usize,
}

impl Grid {
    fn fits(&self, m: &Mask, x: usize, y: usize) -> bool {
        m.rows.iter().enumerate().all(|(r, row)| row.iter().enumerate().all(|(k, &bits)| bits == 0 || window(&self.rows[y + r], x + 64 * k) & bits == 0))
    }

    fn place(&mut self, m: &Mask, x: usize, y: usize) {
        for (r, row) in m.rows.iter().enumerate() {
            let target = &mut self.rows[y + r];
            for (k, &bits) in row.iter().enumerate() {
                let start = x + 64 * k;
                let (w, o) = (start / 64, start % 64);
                target[w] |= bits << o;
                if o > 0 && w + 1 < target.len() {
                    target[w + 1] |= bits >> (64 - o);
                }
            }
        }
    }

    /// The top-most, then left-most, position where `m` fits.
    fn first_fit(&self, m: &Mask) -> Option<(usize, usize)> {
        let height = self.rows.len();
        if m.cols > self.width || m.rows.len() > height {
            return None;
        }
        (0..=height - m.rows.len()).find_map(|y| (0..=self.width - m.cols).find(|&x| self.fits(m, x, y)).map(|x| (x, y)))
    }
}

/// Nests `parts` (each one or more outlines, in document mm) into the area. Returns, for each
/// part, the transform that moves it into place, or `None` if it didn't fit.
pub fn nest(parts: &[Vec<BezPath>], options: &NestOptions) -> Vec<Option<Affine>> {
    let cell = options.cell.max(0.05);
    let (gw, gh) = ((options.area.width() / cell).floor() as usize, (options.area.height() / cell).floor() as usize);
    let mut grid = Grid { rows: vec![vec![0u64; words(gw) + 1]; gh], width: gw };
    // Grow by half the gap on each part, plus most of a cell so grid rounding can't make them touch.
    let grow = options.spacing.max(0.0) / 2.0 + cell * 0.75;
    let rotations = if options.rotations.is_empty() { vec![0.0] } else { options.rotations.clone() };
    // Every part's masks, one per rotation (about the part's centre).
    let masks: Vec<Vec<Mask>> = parts
        .iter()
        .map(|outlines| {
            let Some(centre) = outlines.iter().filter_map(bounds).reduce(|a, b| a.union(b)).map(|b| b.center()) else { return Vec::new() };
            rotations
                .iter()
                .filter_map(|deg| {
                    let r = Affine::translate(centre.to_vec2()) * Affine::rotate(deg.to_radians()) * Affine::translate(-centre.to_vec2());
                    mask(outlines, r, grow, cell)
                })
                .collect()
        })
        .collect();
    // Largest first.
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(masks[i].first().map(|m| m.filled).unwrap_or(0)));
    let mut out = vec![None; parts.len()];
    for i in order {
        let best = masks[i].iter().filter_map(|m| grid.first_fit(m).map(|(x, y)| (y, x, m))).min_by_key(|(y, x, _)| (*y, *x));
        if let Some((y, x, m)) = best {
            grid.place(m, x, y);
            let target = crate::Point::new(options.area.x0 + x as f64 * cell, options.area.y0 + y as f64 * cell);
            out[i] = Some(Affine::translate(target - m.corner) * m.rotation);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{boolean, BooleanOp};
    use crate::{ellipse_path, rect_path};

    fn opts(w: f64, h: f64, spacing: f64, rotations: Vec<f64>) -> NestOptions {
        let area = Rect::new(0.0, 0.0, w, h);
        NestOptions { area, spacing, rotations, cell: 0.5 }
    }

    /// No two placed parts overlap, and all lie inside the area.
    fn check(parts: &[Vec<BezPath>], placed: &[Option<Affine>], area: Rect) {
        let moved: Vec<BezPath> = parts.iter().zip(placed).filter_map(|(p, t)| t.map(|t| { let mut all = BezPath::new(); for o in p { all.extend(t * o.clone()); } all })).collect();
        for m in &moved {
            let b = bounds(m).unwrap();
            assert!(b.x0 >= area.x0 - 1e-6 && b.y0 >= area.y0 - 1e-6 && b.x1 <= area.x1 + 1e-6 && b.y1 <= area.y1 + 1e-6, "inside the area: {b:?}");
        }
        for (i, a) in moved.iter().enumerate() {
            for b in &moved[i + 1..] {
                let overlap = boolean(&[a.clone(), b.clone()], BooleanOp::Intersect).map(|p| crate::ops::area(&p)).unwrap_or(0.0);
                assert!(overlap < 1e-6, "parts overlap by {overlap} mm²");
            }
        }
    }

    #[test]
    fn circles_pack_tighter_than_their_boxes() {
        // 20 mm circles into a 100 × 100 mm area: boxes fit 4 × 4 = 16 with a 1 mm gap; true shape
        // nesting staggers the rows and fits more.
        let parts: Vec<Vec<BezPath>> = (0..30).map(|_| vec![ellipse_path(10.0, 10.0)]).collect();
        let o = opts(100.0, 100.0, 1.0, vec![0.0]);
        let placed = nest(&parts, &o);
        let count = placed.iter().filter(|p| p.is_some()).count();
        assert!(count > 16, "only {count} placed");
        check(&parts, &placed, o.area);
    }

    #[test]
    fn small_parts_go_inside_holes() {
        // A 60 mm square frame with a 40 mm hole, and a 20 mm square: the small one goes in the hole.
        let frame = boolean(&[rect_path(60.0, 60.0, 0.0), rect_path(40.0, 40.0, 0.0)], BooleanOp::Subtract).unwrap();
        let parts = vec![vec![frame], vec![rect_path(20.0, 20.0, 0.0)]];
        let o = opts(62.0, 62.0, 1.0, vec![0.0]);
        let placed = nest(&parts, &o);
        assert!(placed.iter().all(|p| p.is_some()), "both fit only if the small one is in the hole");
        check(&parts, &placed, o.area);
    }

    #[test]
    fn rotation_lets_long_parts_fit() {
        // A 90 × 10 mm bar doesn't fit a 40 × 100 area unless it's turned.
        let parts = vec![vec![rect_path(90.0, 10.0, 0.0)]];
        assert!(nest(&parts, &opts(40.0, 100.0, 1.0, vec![0.0]))[0].is_none());
        let placed = nest(&parts, &opts(40.0, 100.0, 1.0, vec![0.0, 90.0]));
        assert!(placed[0].is_some());
        check(&parts, &placed, Rect::new(0.0, 0.0, 40.0, 100.0));
    }

    #[test]
    fn spacing_is_kept() {
        let parts: Vec<Vec<BezPath>> = (0..2).map(|_| vec![rect_path(20.0, 20.0, 0.0)]).collect();
        let placed = nest(&parts, &opts(100.0, 30.0, 5.0, vec![0.0]));
        let xs: Vec<f64> = placed.iter().map(|t| bounds(&(t.unwrap() * rect_path(20.0, 20.0, 0.0))).unwrap().x0).collect();
        let gap = (xs[0] - xs[1]).abs() - 20.0;
        assert!((5.0..8.0).contains(&gap), "gap {gap}");
    }

    /// Nests a mixed set into 400 × 300 mm and writes the outlines to `BC_NEST_DUMP` (timing printed).
    #[test]
    #[ignore = "diagnostic"]
    fn dump_mixed() {
        let l_bracket = crate::polyline_path(&[(0.0, 0.0), (60.0, 0.0), (60.0, 15.0), (15.0, 15.0), (15.0, 50.0), (0.0, 50.0)].map(crate::Point::from), true);
        let frame = boolean(&[rect_path(70.0, 50.0, 6.0), rect_path(50.0, 30.0, 4.0)], BooleanOp::Subtract).unwrap();
        let mut parts: Vec<Vec<BezPath>> = Vec::new();
        for i in 0..60 {
            parts.push(vec![match i % 5 {
                0 => crate::star_path(5, 22.0, 9.0),
                1 => l_bracket.clone(),
                2 => ellipse_path(14.0, 14.0),
                3 => frame.clone(),
                _ => rect_path(18.0, 18.0, 3.0),
            }]);
        }
        let o = NestOptions { area: Rect::new(0.0, 0.0, 400.0, 300.0), spacing: 2.0, rotations: vec![0.0, 90.0, 180.0, 270.0], cell: NestOptions::auto_cell(Rect::new(0.0, 0.0, 400.0, 300.0), 250_000.0) };
        let start = std::time::Instant::now();
        let placed = nest(&parts, &o);
        println!("NEST placed {} of {} in {:?} (cell {:.2} mm)", placed.iter().filter(|p| p.is_some()).count(), parts.len(), start.elapsed(), o.cell);
        let mut out = String::new();
        for (p, t) in parts.iter().zip(&placed) {
            if let Some(t) = t {
                for line in crate::flatten(&(*t * p[0].clone()), 0.2) {
                    out += &line.points.iter().map(|q| format!("{:.1},{:.1}", q.x, q.y)).collect::<Vec<_>>().join(" ");
                    out += if line.closed { " Z\n" } else { "\n" };
                }
            }
        }
        std::fs::write(std::env::var("BC_NEST_DUMP").unwrap(), out).unwrap();
        check(&parts, &placed, o.area);
    }
}