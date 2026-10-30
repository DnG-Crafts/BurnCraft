//! Planar perspective mapping (homography) between camera pixels and bed millimetres.

/// A 3×3 projective transform, row-major, normalised so the last element is 1.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    /// Maps `src[i]` to `dst[i]` exactly for four point pairs (no three collinear).
    pub fn from_points(src: [(f64, f64); 4], dst: [(f64, f64); 4]) -> Option<Self> {
        // Unknowns h0..h7 (h8 = 1). Each pair gives two equations:
        // x' = (h0 x + h1 y + h2) / (h6 x + h7 y + 1),  y' = (h3 x + h4 y + h5) / (h6 x + h7 y + 1)
        let mut a = [[0.0f64; 9]; 8];
        for i in 0..4 {
            let ((x, y), (u, v)) = (src[i], dst[i]);
            a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
        }
        let h = solve8(a)?;
        Some(Self([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0]))
    }

    pub fn apply(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let m = &self.0;
        let w = m[6] * x + m[7] * y + m[8];
        ((m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w)
    }

    pub fn inverse(&self) -> Option<Self> {
        let m = &self.0;
        let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6]);
        if det.abs() < 1e-15 {
            return None;
        }
        let inv = [
            m[4] * m[8] - m[5] * m[7],
            m[2] * m[7] - m[1] * m[8],
            m[1] * m[5] - m[2] * m[4],
            m[5] * m[6] - m[3] * m[8],
            m[0] * m[8] - m[2] * m[6],
            m[2] * m[3] - m[0] * m[5],
            m[3] * m[7] - m[4] * m[6],
            m[1] * m[6] - m[0] * m[7],
            m[0] * m[4] - m[1] * m[3],
        ];
        let s = inv[8];
        if s.abs() < 1e-15 {
            return None;
        }
        Some(Self(inv.map(|v| v / s)))
    }
}

/// Solves an 8×8 linear system given as an augmented 8×9 matrix (Gaussian elimination, partial pivoting).
fn solve8(mut a: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let pivot = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        for row in 0..8 {
            if row != col {
                let f = a[row][col] / a[col][col];
                let pivot_row = a[col];
                for (x, p) in a[row].iter_mut().zip(pivot_row).skip(col) {
                    *x -= f * p;
                }
            }
        }
    }
    let mut x = [0.0; 8];
    for i in 0..8 {
        x[i] = a[i][8] / a[i][i];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn maps_the_four_points_and_inverts() {
        // A tilted, perspective-distorted view of a 400 × 300 mm rectangle.
        let img = [(102.0, 87.0), (1180.0, 120.0), (1105.0, 900.0), (150.0, 860.0)];
        let bed = [(50.0, 50.0), (450.0, 50.0), (450.0, 350.0), (50.0, 350.0)];
        let h = Homography::from_points(img, bed).unwrap();
        for i in 0..4 {
            assert!(close(h.apply(img[i]), bed[i]));
        }
        let inv = h.inverse().unwrap();
        for p in [(200.0, 100.0), (420.0, 330.0)] {
            assert!(close(h.apply(inv.apply(p)), p));
        }
    }

    #[test]
    fn identity_and_degenerate_input() {
        let sq = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let h = Homography::from_points(sq, sq).unwrap();
        assert!(close(h.apply((0.3, 0.7)), (0.3, 0.7)));
        let line = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0)];
        assert!(Homography::from_points(line, sq).is_none());
    }
}
