//! QR codes and Code 128 barcodes as filled-rectangle outlines.

use bc_doc::Symbology;
use bc_geom::{Affine, BezPath, Point};

/// Adds one rectangle per horizontal run of dark modules in each row.
fn runs_to_path(rows: impl Iterator<Item = Vec<bool>>, module_w: f64, module_h: f64) -> BezPath {
    let mut path = BezPath::new();
    for (r, row) in rows.enumerate() {
        let mut c = 0;
        while c < row.len() {
            if !row[c] {
                c += 1;
                continue;
            }
            let start = c;
            while c < row.len() && row[c] {
                c += 1;
            }
            let (x0, x1, y0, y1) = (start as f64 * module_w, c as f64 * module_w, r as f64 * module_h, (r + 1) as f64 * module_h);
            path.extend(bc_geom::polyline_path(&[Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)], true));
        }
    }
    path
}

/// The modules cleared for a logo in a QR code `width` modules wide: a centred square about a
/// quarter of the width (about 6 % of the code, well within what the strongest error correction
/// can recover), as a module range used for both rows and columns.
fn logo_space(width: usize) -> (usize, usize) {
    let mut side = (width as f64 * 0.26).round() as usize;
    if side % 2 != width % 2 {
        side += 1;
    }
    let lo = (width - side) / 2;
    (lo, lo + side)
}

/// The side of the empty logo square as a fraction of a QR code with logo space (for drawing a
/// guide or sizing the logo).
pub fn logo_fraction(content: &str) -> Option<f64> {
    let code = qrcode::QrCode::with_error_correction_level(content.as_bytes(), qrcode::EcLevel::H).ok()?;
    let (lo, hi) = logo_space(code.width());
    Some((hi - lo) as f64 / code.width() as f64)
}

/// The barcode outline, centred on the origin. `size` is the QR side length, or the 1D bar height
/// (1D bars are 0.33 mm per module, or narrower to keep the width within 5× the height).
pub fn outline(symbology: Symbology, content: &str, size: f64) -> Result<BezPath, String> {
    if content.is_empty() {
        return Err("enter some content".into());
    }
    let path = match symbology {
        Symbology::Qr => {
            let code = qrcode::QrCode::new(content.as_bytes()).map_err(|e| format!("can't make a QR code: {e}"))?;
            let w = code.width();
            // One module of quiet zone is left to the material; the code itself fills `size`.
            let module = size / w as f64;
            let colors = code.to_colors();
            runs_to_path((0..w).map(|r| colors[r * w..(r + 1) * w].iter().map(|c| *c == qrcode::Color::Dark).collect()), module, module)
        }
        Symbology::QrLogo => {
            let code = qrcode::QrCode::with_error_correction_level(content.as_bytes(), qrcode::EcLevel::H).map_err(|e| format!("can't make a QR code: {e}"))?;
            let w = code.width();
            let module = size / w as f64;
            let (lo, hi) = logo_space(w);
            let colors = code.to_colors();
            runs_to_path((0..w).map(|r| (0..w).map(|c| colors[r * w + c] == qrcode::Color::Dark && !((lo..hi).contains(&r) && (lo..hi).contains(&c))).collect()), module, module)
        }
        Symbology::Code128 => {
            // Character set B covers printable ASCII.
            if !content.chars().all(|c| (' '..='~').contains(&c)) {
                return Err("Code 128 here supports plain ASCII letters, digits and symbols".into());
            }
            let code = barcoders::sym::code128::Code128::new(format!("\u{0181}{content}")).map_err(|e| format!("can't make a Code 128 barcode: {e}"))?;
            let bars: Vec<bool> = code.encode().into_iter().map(|b| b == 1).collect();
            let module = (0.33f64).min(size * 5.0 / bars.len() as f64);
            runs_to_path(std::iter::once(bars), module, size)
        }
    };
    let b = bc_geom::bounds(&path).ok_or("empty barcode")?;
    Ok(Affine::translate(-b.center().to_vec2()) * path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_fills_requested_size() {
        let p = outline(Symbology::Qr, "https://example.com", 20.0).unwrap();
        let b = bc_geom::bounds(&p).unwrap();
        assert!((b.width() - 20.0).abs() < 1e-6 && (b.height() - 20.0).abs() < 1e-6);
        assert!(b.center().distance(Point::ORIGIN) < 1e-9);
        assert!(bc_geom::flatten(&p, 0.01).iter().all(|l| l.closed));
    }

    #[test]
    fn qr_with_logo_space_is_empty_in_the_middle() {
        let p = outline(Symbology::QrLogo, "https://example.com/some/longer/path", 30.0).unwrap();
        let b = bc_geom::bounds(&p).unwrap();
        assert!((b.width() - 30.0).abs() < 1e-6);
        let f = logo_fraction("https://example.com/some/longer/path").unwrap();
        assert!((0.2..0.32).contains(&f), "{f}");
        // Nothing dark inside the logo square.
        let half = 30.0 * f / 2.0 - 0.01;
        let inside = bc_geom::flatten(&p, 0.01).iter().flat_map(|l| l.points.clone()).any(|q| q.x.abs() < half && q.y.abs() < half);
        assert!(!inside);
    }

    #[test]
    fn code128_bars() {
        let p = outline(Symbology::Code128, "ABC-123", 10.0).unwrap();
        let b = bc_geom::bounds(&p).unwrap();
        assert!((b.height() - 10.0).abs() < 1e-9 && b.width() > 10.0);
        assert!(outline(Symbology::Code128, "naïve", 10.0).is_err());
        assert!(outline(Symbology::Qr, "", 10.0).is_err());
    }
}
