//! Finding items laid on the bed in an aligned camera frame, for batch engraving.

use crate::{Frame, Homography};
use bc_geom::Point;
use bc_geom::fit::{OrientedRect, min_area_rect};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectOptions {
    /// Size of one sample on the bed (mm).
    pub resolution: f64,
    /// How far (0–255) a sample's brightness must differ from the bed's to count as an item.
    pub threshold: u8,
    /// Smallest item kept (mm²).
    pub min_area: f64,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self { resolution: 1.0, threshold: 40, min_area: 100.0 }
    }
}

/// Items on the bed, as rotated rectangles in bed millimetres, top to bottom then left to right.
/// The bed's own brightness is taken as the median of the picture, so items can be lighter or
/// darker than it. Anything covering more than half the bed is taken as background and skipped.
pub fn detect_items(frame: &Frame, bed_to_image: &Homography, bed: (f64, f64), options: &DetectOptions) -> Vec<OrientedRect> {
    let res = options.resolution.max(0.1);
    let (w, h) = ((bed.0 / res).ceil() as usize, (bed.1 / res).ceil() as usize);
    if w == 0 || h == 0 || frame.width == 0 || frame.height == 0 {
        return Vec::new();
    }
    // Sample the bed: the brightness under each cell centre, or None outside the picture.
    let luma = |x: usize, y: usize| -> u8 {
        let i = (y * frame.width + x) * 3;
        let (r, g, b) = (frame.rgb[i] as u32, frame.rgb[i + 1] as u32, frame.rgb[i + 2] as u32);
        ((r * 77 + g * 150 + b * 29) >> 8) as u8
    };
    let mut samples: Vec<Option<u8>> = Vec::with_capacity(w * h);
    for j in 0..h {
        for i in 0..w {
            let (u, v) = bed_to_image.apply(((i as f64 + 0.5) * res, (j as f64 + 0.5) * res));
            let inside = u >= 0.0 && v >= 0.0 && (u as usize) < frame.width && (v as usize) < frame.height;
            samples.push(inside.then(|| luma(u as usize, v as usize)));
        }
    }
    let mut seen: Vec<u8> = samples.iter().flatten().copied().collect();
    if seen.is_empty() {
        return Vec::new();
    }
    let mid = seen.len() / 2;
    let background = *seen.select_nth_unstable(mid).1 as i32;
    let mut mask: Vec<bool> = samples.iter().map(|s| s.is_some_and(|v| (v as i32 - background).abs() > options.threshold as i32)).collect();
    // Open (erode, then dilate) to drop specks and thin lines such as honeycomb edges.
    let at = |m: &[bool], i: isize, j: isize| i >= 0 && j >= 0 && (i as usize) < w && (j as usize) < h && m[j as usize * w + i as usize];
    let neighbours = |m: &[bool], i: usize, j: usize, all: bool| {
        let mut it = (-1..=1).flat_map(|dj| (-1..=1).map(move |di| (di, dj))).map(|(di, dj)| at(m, i as isize + di, j as isize + dj));
        if all { it.all(|b| b) } else { it.any(|b| b) }
    };
    let eroded: Vec<bool> = (0..w * h).map(|k| neighbours(&mask, k % w, k / w, true)).collect();
    mask = (0..w * h).map(|k| neighbours(&eroded, k % w, k / w, false)).collect();
    // Connected regions (8-way), each as the corners of its cells.
    let mut label = vec![false; w * h];
    let mut found = Vec::new();
    let max_cells = w * h / 2;
    for start in 0..w * h {
        if !mask[start] || label[start] {
            continue;
        }
        label[start] = true;
        let mut stack = vec![start];
        let mut cells = 0usize;
        let mut corners: Vec<Point> = Vec::new();
        while let Some(k) = stack.pop() {
            cells += 1;
            let (i, j) = (k % w, k / w);
            // Only cells on the region's edge can be hull corners.
            if !neighbours(&mask, i, j, true) {
                let (x, y) = (i as f64 * res, j as f64 * res);
                corners.extend([Point::new(x, y), Point::new(x + res, y), Point::new(x + res, y + res), Point::new(x, y + res)]);
            }
            for dj in -1..=1isize {
                for di in -1..=1isize {
                    let (ni, nj) = (i as isize + di, j as isize + dj);
                    if at(&mask, ni, nj) {
                        let n = nj as usize * w + ni as usize;
                        if !label[n] {
                            label[n] = true;
                            stack.push(n);
                        }
                    }
                }
            }
        }
        if (cells as f64) * res * res >= options.min_area && cells <= max_cells
            && let Some(r) = min_area_rect(&corners)
        {
            found.push(r);
        }
    }
    // Reading order: rows of items (by centre, within half an item's height), then left to right.
    found.sort_by(|a, b| a.center.y.total_cmp(&b.center.y));
    let mut rows: Vec<Vec<OrientedRect>> = Vec::new();
    for r in found {
        match rows.last_mut() {
            Some(row) if (r.center.y - row[0].center.y).abs() < row[0].height.min(r.height) / 2.0 => row.push(r),
            _ => rows.push(vec![r]),
        }
    }
    rows.into_iter()
        .flat_map(|mut row| {
            row.sort_by(|a, b| a.center.x.total_cmp(&b.center.x));
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grey 400×300 picture of a 200×150 mm bed (2 px/mm) with some items on it.
    fn picture(items: &[OrientedRect], value: u8) -> Frame {
        let (w, h) = (400usize, 300usize);
        let mut rgb = vec![120u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let p = Point::new(x as f64 / 2.0, y as f64 / 2.0);
                if items.iter().any(|r| {
                    let q = r.placement().inverse() * p;
                    q.x.abs() <= r.width / 2.0 && q.y.abs() <= r.height / 2.0
                }) {
                    rgb[(y * w + x) * 3..][..3].fill(value);
                }
                // Speckle noise that the opening should remove.
                if (x * 7 + y * 13) % 97 == 0 {
                    rgb[(y * w + x) * 3..][..3].fill(255);
                }
            }
        }
        Frame { width: w, height: h, rgb, sequence: 0 }
    }

    #[test]
    fn finds_turned_items_in_reading_order() {
        let items = [
            OrientedRect { center: Point::new(150.0, 40.0), width: 40.0, height: 20.0, angle: 0.3 },
            OrientedRect { center: Point::new(50.0, 40.0), width: 30.0, height: 30.0, angle: 0.0 },
            OrientedRect { center: Point::new(80.0, 110.0), width: 60.0, height: 25.0, angle: -0.6 },
        ];
        // Bed mm → pixels is a plain ×2 here.
        let bed_to_image = Homography([2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 1.0]);
        for value in [230u8, 20] {
            let found = detect_items(&picture(&items, value), &bed_to_image, (200.0, 150.0), &DetectOptions::default());
            assert_eq!(found.len(), 3, "{found:?}");
            for (f, want) in found.iter().zip([items[1], items[0], items[2]]) {
                let want = want.normalized();
                assert!((f.center - want.center).hypot() < 1.5, "{f:?} vs {want:?}");
                assert!((f.width - want.width).abs() < 3.0 && (f.height - want.height).abs() < 3.0, "{f:?} vs {want:?}");
                if want.width - want.height > 5.0 {
                    assert!((f.angle - want.angle).abs() < 0.08, "{f:?} vs {want:?}");
                }
            }
        }
    }
}
