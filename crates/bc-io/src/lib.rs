//! File import/export and fonts for BurnCraft.

pub mod barcode;
pub mod dxf_export;
pub mod dxf_import;
pub mod eps;
pub mod fonts;
pub mod gcode_import;
pub mod hpgl_import;
pub mod lbdev;
pub mod lbzip;
pub mod lightburn;
pub mod metafile;
pub mod pdf_import;
pub mod psd;
pub mod single_line;
pub mod svg;

/// Reads a CSV file (first row = column names) for variable text.
pub fn read_csv(data: &[u8]) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).trim(csv::Trim::All).from_reader(data);
    let headers = reader.headers().map_err(|e| e.to_string())?.iter().map(str::to_string).collect();
    let rows = reader.records().map(|r| r.map(|r| r.iter().map(str::to_string).collect())).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    Ok((headers, rows))
}

use bc_geom::BezPath;

/// A vector imported from a file, in mm with Y pointing down.
#[derive(Debug, Clone)]
pub struct Imported {
    pub path: BezPath,
    /// Stroke (or fill) colour from the file, used to pick a layer.
    pub color: Option<[u8; 3]>,
}

/// Result of an import: the shapes, plus notes about anything left out.
#[derive(Debug, Default)]
pub struct ImportResult {
    pub shapes: Vec<Imported>,
    pub warnings: Vec<String>,
}

/// Pixel dimensions of an image file, if it can be decoded.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()
}

/// File extensions accepted by the importer.
pub const VECTOR_EXTENSIONS: [&str; 8] = ["svg", "dxf", "pdf", "ai", "eps", "ps", "emf", "wmf"];
/// Machine and plotter files that import as vectors: G-code, HPGL, Ruida job files.
pub const TOOLPATH_EXTENSIONS: [&str; 6] = ["gcode", "nc", "gc", "hpgl", "plt", "rd"];
pub const IMAGE_EXTENSIONS: [&str; 11] = ["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "tga", "webp", "psd", "psb"];

/// Index of the standard palette colour (the first 30 layers) closest to `color`.
pub fn nearest_layer(color: [u8; 3]) -> usize {
    let d = |c: [u8; 3]| -> i32 { (0..3).map(|i| (c[i] as i32 - color[i] as i32).pow(2)).sum() };
    (0..bc_doc::LEGACY_CUT_LAYERS).min_by_key(|&i| d(bc_doc::Layer::color(i))).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn csv_with_quotes_and_short_rows() {
        let (h, rows) = super::read_csv(b"Name, Size\n\"Smith, Jo\",M\nAda\n").unwrap();
        assert_eq!(h, vec!["Name", "Size"]);
        assert_eq!(rows, vec![vec!["Smith, Jo".to_string(), "M".to_string()], vec!["Ada".to_string()]]);
    }
}
