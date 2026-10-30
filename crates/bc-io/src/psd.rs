//! Photoshop documents (PSD and PSB): the merged picture every file carries after its layers,
//! as a PNG, so a PSD can be imported and engraved like any picture. Layers aren't read.
//!
//! Layout (Adobe Photoshop File Formats Specification): a 26-byte header (signature `8BPS`,
//! version 1 for PSD or 2 for PSB, channels, height, width, bits per channel, colour mode), then
//! three length-prefixed sections (colour mode data, image resources, layer and mask information;
//! the last has an 8-byte length in a PSB), then the merged image: a compression code and each
//! channel's rows, raw or PackBits run-length encoded.

/// File extensions read as Photoshop documents.
pub const EXTENSIONS: [&str; 2] = ["psd", "psb"];

/// Most pixels a picture may have.
const MAX_PIXELS: u64 = 1 << 28;

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.data.len()).ok_or("the file ends early")?;
        let s = &self.data[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let hi = self.u32()? as u64;
        Ok(hi << 32 | self.u32()? as u64)
    }
    fn skip(&mut self, n: u64) -> Result<(), String> {
        self.take(usize::try_from(n).map_err(|_| "the file is damaged")?).map(|_| ())
    }
}

/// PackBits: a header byte n; 0..=127 copies n + 1 bytes, -1..=-127 repeats the next byte
/// 1 - n times, -128 does nothing.
fn unpack(src: &[u8], out: &mut Vec<u8>, want: usize) -> Result<(), String> {
    let mut i = 0;
    let end = out.len() + want;
    while out.len() < end {
        let n = *src.get(i).ok_or("a row is damaged")? as i8;
        i += 1;
        if n >= 0 {
            let len = n as usize + 1;
            out.extend_from_slice(src.get(i..i + len).ok_or("a row is damaged")?);
            i += len;
        } else if n != -128 {
            let b = *src.get(i).ok_or("a row is damaged")?;
            i += 1;
            out.extend(std::iter::repeat_n(b, (1 - n as isize) as usize));
        }
    }
    out.truncate(end);
    Ok(())
}

/// The merged picture of a PSD or PSB as PNG bytes.
pub fn to_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut r = Reader { data: bytes, at: 0 };
    if r.take(4)? != b"8BPS" {
        return Err("not a Photoshop file".into());
    }
    let version = r.u16()?;
    if !matches!(version, 1 | 2) {
        return Err("an unknown Photoshop file version".into());
    }
    let big = version == 2;
    r.skip(6)?;
    let channels = r.u16()? as usize;
    let (height, width) = (r.u32()? as usize, r.u32()? as usize);
    let depth = r.u16()?;
    let mode = r.u16()?;
    if width == 0 || height == 0 || (width as u64) * (height as u64) > MAX_PIXELS {
        return Err("the picture is empty or too big".into());
    }
    if !matches!(depth, 8 | 16) {
        return Err(format!("{depth}-bit Photoshop pictures aren't read (save as 8 or 16 bits per channel)"));
    }
    // 1 greyscale, 3 RGB, 4 CMYK (others: bitmap, indexed, multichannel, duotone, Lab).
    if !matches!(mode, 1 | 3 | 4) {
        return Err("only greyscale, RGB and CMYK Photoshop pictures are read".into());
    }
    let len = r.u32()? as u64;
    r.skip(len)?; // colour mode data
    let len = r.u32()? as u64;
    r.skip(len)?; // image resources
    let len = if big { r.u64()? } else { r.u32()? as u64 };
    r.skip(len)?; // layers and masks
    let compression = r.u16()?;
    let bpc = depth as usize / 8;
    let row = width * bpc;
    let plane = row * height;
    let mut data: Vec<u8> = Vec::with_capacity(plane * channels.min(5));
    match compression {
        0 => data.extend_from_slice(r.take(plane * channels)?),
        1 => {
            // Each row's packed length (2 bytes, 4 in a PSB), for every row of every channel.
            let lens: Vec<usize> = (0..height * channels).map(|_| if big { r.u32().map(|v| v as usize) } else { r.u16().map(|v| v as usize) }).collect::<Result<_, _>>()?;
            for l in lens {
                let packed = r.take(l)?;
                unpack(packed, &mut data, row)?;
            }
        }
        _ => return Err("this Photoshop compression (ZIP) isn't read: save the file uncompressed or with RLE".into()),
    }
    let sample = |c: usize, i: usize| -> u8 { data.get(c * plane + i * bpc).copied().unwrap_or(255) };
    let colours = match mode {
        1 => 1,
        3 => 3,
        _ => 4,
    };
    let alpha = channels > colours;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for i in 0..width * height {
        let [r, g, b] = match mode {
            1 => [sample(0, i); 3],
            3 => [sample(0, i), sample(1, i), sample(2, i)],
            // Photoshop stores CMYK inverted: 255 is no ink.
            _ => {
                let k = sample(3, i) as u32;
                [0, 1, 2].map(|c| (sample(c, i) as u32 * k / 255) as u8)
            }
        };
        rgba.extend_from_slice(&[r, g, b, if alpha { sample(colours, i) } else { 255 }]);
    }
    let img = image::RgbaImage::from_raw(width as u32, height as u32, rgba).ok_or("the picture's pixels don't match its size")?;
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 × 1 RGB PSD, rows packed: a red pixel and a blue one.
    fn tiny_psd() -> Vec<u8> {
        let mut f = b"8BPS".to_vec();
        f.extend(1u16.to_be_bytes());
        f.extend([0; 6]);
        f.extend(3u16.to_be_bytes()); // channels
        f.extend(1u32.to_be_bytes()); // height
        f.extend(2u32.to_be_bytes()); // width
        f.extend(8u16.to_be_bytes());
        f.extend(3u16.to_be_bytes()); // RGB
        for _ in 0..3 {
            f.extend(0u32.to_be_bytes()); // empty sections
        }
        f.extend(1u16.to_be_bytes()); // RLE
        let rows: [&[u8]; 3] = [&[1, 255, 0], &[0xff, 0], &[1, 0, 255]]; // R: 255 0; G: 0 0 (repeat); B: 0 255
        for r in rows {
            f.extend((r.len() as u16).to_be_bytes());
        }
        for r in rows {
            f.extend(r);
        }
        f
    }

    #[test]
    fn the_merged_picture_comes_out_as_a_png() {
        let png = to_png(&tiny_psd()).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (2, 1));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 0, 255, 255]);
    }

    #[test]
    fn other_files_are_refused() {
        assert!(to_png(b"GIF89a").is_err());
    }
}
