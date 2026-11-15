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
