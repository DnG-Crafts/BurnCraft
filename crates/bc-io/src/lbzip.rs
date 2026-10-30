//! LightBurn bundles (`.lbzip`): an ordinary zip holding a manifest (`LightBurnSoftware.json`)
//! and the exported files, such as device files (`.lbdev`). Newer LightBurn versions export
//! devices this way.

use std::io::Read;

pub const EXTENSION: &str = "lbzip";

/// Largest bundled file read (bytes); device files are a few kB.
const MAX_ENTRY: u64 = 16 * 1024 * 1024;

/// The device files in a bundle, as (name, contents). An error if it isn't a zip or holds none.
pub fn device_files(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| format!("not a LightBurn bundle: {e}"))?;
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|e| format!("damaged bundle: {e}"))?;
        let name = entry.name().to_string();
        if !entry.is_file() || !name.to_ascii_lowercase().ends_with(&format!(".{}", crate::lbdev::EXTENSION)) {
            continue;
        }
        if entry.size() > MAX_ENTRY {
            return Err(format!("{name} is too large to be a device file"));
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.take(MAX_ENTRY).read_to_end(&mut bytes).map_err(|e| format!("couldn't read {name}: {e}"))?;
        out.push((name, bytes));
    }
    if out.is_empty() {
        return Err("the bundle contains no device files (.lbdev)".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn bundle(files: &[(&str, &str)]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, text) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn finds_the_device_files() {
        let device = r#"{"DeviceList":[{"DisplayName":"Ruida","Name":"Ruida","Type":"Ethernet","Width":900,"Height":600}]}"#;
        let data = bundle(&[("LightBurnSoftware.json", r#"{"LightBurnSoftware":{}}"#), ("abc.lbdev", device)]);
        let files = device_files(&data).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "abc.lbdev");
        let d = &crate::lbdev::parse(&files[0].1).unwrap()[0];
        assert_eq!((d.name.as_str(), d.driver.as_str(), d.width), ("Ruida", "Ruida", 900.0));
    }

    #[test]
    fn rejects_bundles_without_devices_and_non_zips() {
        assert!(device_files(&bundle(&[("notes.txt", "hi")])).unwrap_err().contains("no device files"));
        assert!(device_files(b"not a zip").is_err());
    }

    /// Lists the devices in the bundle at `BC_LBZIP` (names and drivers only).
    #[test]
    #[ignore = "diagnostic"]
    fn probe() {
        let data = std::fs::read(std::env::var("BC_LBZIP").unwrap()).unwrap();
        for (name, bytes) in device_files(&data).unwrap() {
            for d in crate::lbdev::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}")) {
                println!("{name}: \"{}\" driver {} via {} {}×{} mm", d.name, d.driver, d.connection, d.width, d.height);
            }
        }
    }
}
