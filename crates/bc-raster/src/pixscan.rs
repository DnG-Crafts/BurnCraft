//! PixScan: placing a photo at true size. Print a sheet with a dark square in each corner, put the
//! item on it and photograph it from above; the squares are found, the perspective removed, and
//! the photo comes out as an image of known size in mm.

/// A point in photo pixels.
pub type Pt = (f64, f64);

/// The marker sheet: squares `size` mm across, their centres `inset` mm in from each edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarkerSheet {
    pub size: f64,
    pub inset: f64,
}

impl Default for MarkerSheet {
    fn default() -> Self {
        Self { size: 12.0, inset: 12.0 }
    }
}

impl MarkerSheet {
    /// Distance (mm) between the marker centres across and down, on a `w` × `h` mm page.
    pub fn span(&self, w: f64, h: f64) -> (f64, f64) {
        (w - 2.0 * self.inset, h - 2.0 * self.inset)
    }

    /// The sheet as an SVG page `w` × `h` mm, to print at actual size.
    pub fn svg(&self, w: f64, h: f64) -> String {
        let (s, i) = (self.size, self.inset);
        let mut out = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">\n<rect width=\"{w}\" height=\"{h}\" fill=\"white\"/>\n"
        );
        for (cx, cy) in [(i, i), (w - i, i), (w - i, h - i), (i, h - i)] {
            out += &format!("<rect x=\"{}\" y=\"{}\" width=\"{s}\" height=\"{s}\" fill=\"black\"/>\n", cx - s / 2.0, cy - s / 2.0);
        }
        let (sw, sh) = self.span(w, h);
        out += &format!(
            "<text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"3.5\" text-anchor=\"middle\" fill=\"#888\">BurnCraft PixScan sheet: print at 100% (actual size). Markers {sw:.0} × {sh:.0} mm apart. Keep the item inside the squares.</text>\n</svg>\n",
            w / 2.0,
            h - 4.0
        );
        out
    }
}

/// The 3 × 3 projective transform (row-major) taking `src[i]` to `dst[i]`.
pub fn homography(src: &[Pt; 4], dst: &[Pt; 4]) -> Option<[f64; 9]> {
    let mut a = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let ((x, y), (u, v)) = (src[i], dst[i]);
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for c in 0..8 {
        let pivot = (c..8).max_by(|&r1, &r2| a[r1][c].abs().total_cmp(&a[r2][c].abs()))?;
        if a[pivot][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, pivot);
        let pivot_row = a[c];
        for (r, row) in a.iter_mut().enumerate() {
            if r != c {
                let f = row[c] / pivot_row[c];
                for (v, p) in row.iter_mut().zip(pivot_row).skip(c) {
                    *v -= f * p;
                }
            }
        }
    }
    let mut h = [0.0; 9];
    for i in 0..8 {
        h[i] = a[i][8] / a[i][i];
    }
    h[8] = 1.0;
    Some(h)
}

fn apply(h: &[f64; 9], (x, y): Pt) -> Pt {
    let w = h[6] * x + h[7] * y + h[8];
    ((h[0] * x + h[1] * y + h[2]) / w, (h[3] * x + h[4] * y + h[5]) / w)
}

/// The part of the photo between `corners` (photo pixels: top-left, top-right, bottom-right,
/// bottom-left marker centres), straightened into a `w_mm` × `h_mm` image at `px_per_mm`.
pub fn rectify(photo: &image::RgbaImage, corners: &[Pt; 4], w_mm: f64, h_mm: f64, px_per_mm: f64) -> Option<image::RgbaImage> {
    let (ow, oh) = ((w_mm * px_per_mm).round().max(1.0) as u32, (h_mm * px_per_mm).round().max(1.0) as u32);
    if u64::from(ow) * u64::from(oh) > 80_000_000 {
        return None;
    }
    let target = [(0.0, 0.0), (ow as f64, 0.0), (ow as f64, oh as f64), (0.0, oh as f64)];
    // Output pixel -> photo pixel.
    let h = homography(&target, corners)?;
    let (iw, ih) = (photo.width() as f64, photo.height() as f64);
    let mut out = image::RgbaImage::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            let (sx, sy) = apply(&h, (x as f64 + 0.5, y as f64 + 0.5));
            let (sx, sy) = (sx - 0.5, sy - 0.5);
            if sx < 0.0 || sy < 0.0 || sx >= iw - 1.0 || sy >= ih - 1.0 {
                out.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
                continue;
            }
            let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
            let (fx, fy) = (sx - x0 as f64, sy - y0 as f64);
            let px: [u8; 4] = std::array::from_fn(|c| {
                let g = |xx: u32, yy: u32| photo.get_pixel(xx, yy)[c] as f64;
                let v = g(x0, y0) * (1.0 - fx) * (1.0 - fy) + g(x0 + 1, y0) * fx * (1.0 - fy) + g(x0, y0 + 1) * (1.0 - fx) * fy + g(x0 + 1, y0 + 1) * fx * fy;
                v.round().clamp(0.0, 255.0) as u8
            });
            out.put_pixel(x, y, image::Rgba(px));
        }
    }
    Some(out)
}

/// Finds the four dark square markers nearest the photo's corners. Returns their centres (photo
/// pixels): top-left, top-right, bottom-right, bottom-left.
pub fn find_markers(photo: &image::RgbaImage) -> Option<[Pt; 4]> {
    // Work on a smaller grey copy.
    let scale = (900.0 / photo.width().max(photo.height()) as f64).min(1.0);
    let small = image::imageops::resize(photo, ((photo.width() as f64 * scale) as u32).max(1), ((photo.height() as f64 * scale) as u32).max(1), image::imageops::FilterType::Triangle);
    let (w, h) = (small.width() as usize, small.height() as usize);
    let lum: Vec<f32> = small.pixels().map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).collect();
    // Dark: below halfway between the darkest 2 % and the middle shade.
    let mut sorted = lum.clone();
    sorted.sort_by(f32::total_cmp);
    let threshold = (sorted[sorted.len() / 50] + sorted[sorted.len() / 2]) / 2.0;
    let dark: Vec<bool> = lum.iter().map(|l| *l < threshold).collect();
    let mut seen = vec![false; w * h];
    let mut blobs: Vec<Pt> = Vec::new();
    let min_area = (w * h) as f64 * 0.0002;
    for start in 0..w * h {
        if seen[start] || !dark[start] {
            continue;
        }
        let mut stack = vec![start];
        seen[start] = true;
        let (mut n, mut sx, mut sy) = (0usize, 0.0, 0.0);
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            n += 1;
            sx += x as f64;
            sy += y as f64;
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            let mut push = |j: usize| {
                if !seen[j] && dark[j] {
                    seen[j] = true;
                    stack.push(j);
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
        let (bw, bh) = ((x1 - x0 + 1) as f64, (y1 - y0 + 1) as f64);
        // Solid, squarish blobs only.
        if n as f64 >= min_area && n as f64 / (bw * bh) > 0.75 && (0.6..1.7).contains(&(bw / bh)) {
            blobs.push((sx / n as f64, sy / n as f64));
        }
    }
    if blobs.len() < 4 {
        return None;
    }
    let corners = [(0.0, 0.0), (w as f64, 0.0), (w as f64, h as f64), (0.0, h as f64)];
    let mut out = [(0.0, 0.0); 4];
    let mut used = vec![false; blobs.len()];
    for (k, c) in corners.iter().enumerate() {
        let (i, _) = blobs.iter().enumerate().filter(|(i, _)| !used[*i]).map(|(i, b)| (i, (b.0 - c.0).hypot(b.1 - c.1))).min_by(|a, b| a.1.total_cmp(&b.1))?;
        used[i] = true;
        out[k] = (blobs[i].0 / scale, blobs[i].1 / scale);
    }
    // The four must enclose a good part of the photo.
    let area = (0..4).map(|i| out[i].0 * out[(i + 1) % 4].1 - out[(i + 1) % 4].0 * out[i].1).sum::<f64>().abs() / 2.0;
    (area >= photo.width() as f64 * photo.height() as f64 * 0.1).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homography_maps_the_corners() {
        let src = [(10.0, 20.0), (300.0, 5.0), (320.0, 260.0), (0.0, 240.0)];
        let dst = [(0.0, 0.0), (100.0, 0.0), (100.0, 80.0), (0.0, 80.0)];
        let h = homography(&src, &dst).unwrap();
        for i in 0..4 {
            let p = apply(&h, src[i]);
            assert!((p.0 - dst[i].0).abs() < 1e-6 && (p.1 - dst[i].1).abs() < 1e-6);
        }
    }

    #[test]
    fn finds_markers_and_straightens() {
        let mut photo = image::RgbaImage::from_pixel(400, 300, image::Rgba([240, 235, 230, 255]));
        let centres = [(30u32, 30u32), (370, 40), (360, 270), (40, 260)];
        for (cx, cy) in centres {
            for y in cy - 8..cy + 8 {
                for x in cx - 8..cx + 8 {
                    photo.put_pixel(x, y, image::Rgba([10, 10, 10, 255]));
                }
            }
        }
        let m = find_markers(&photo).expect("markers");
        for (k, (cx, cy)) in centres.iter().enumerate() {
            assert!((m[k].0 - (*cx as f64 - 0.5)).abs() < 2.0 && (m[k].1 - (*cy as f64 - 0.5)).abs() < 2.0, "{k}: {:?}", m[k]);
        }
        let r = rectify(&photo, &m, 100.0, 80.0, 2.0).unwrap();
        assert_eq!((r.width(), r.height()), (200, 160));
    }

    #[test]
    fn the_sheet_has_four_markers() {
        let svg = MarkerSheet::default().svg(210.0, 297.0);
        assert_eq!(svg.matches("fill=\"black\"").count(), 4);
        assert_eq!(MarkerSheet::default().span(210.0, 297.0), (186.0, 273.0));
    }
}
