//! Reading Ruida job files (`.rd`), as saved by RDWorks, BurnCraft and other programs: the moves and
//! cuts of each layer, and the layer colours.
//!
//! A file is the controller's command stream, scrambled with the controller's key. Only command
//! bytes have the top bit set, so commands this reader doesn't know are skipped safely.

use crate::codec::{decode14, decode35, RuidaModel, Swizzle};

/// One layer of a job file: its colour (if the file names one) and the lines cut, in machine µm.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RdFileLayer {
    pub color: Option<[u8; 3]>,
    pub paths: Vec<Vec<(i64, i64)>>,
}

/// A polyline in machine µm.
type Polyline = Vec<(i64, i64)>;

/// Commands this reader recognises, used to tell which scrambling key a file uses.
const KNOWN: [u8; 21] = [0x88, 0x89, 0x8A, 0x8B, 0xA8, 0xA9, 0xAA, 0xAB, 0xC6, 0xC9, 0xCA, 0xD7, 0xD8, 0xD9, 0xDA, 0xE5, 0xE6, 0xE7, 0xEA, 0xF0, 0xF1];

/// Unscrambles a file, picking the key under which it reads as the most known commands.
fn unscramble(bytes: &[u8]) -> Vec<u8> {
    let score = |data: &[u8]| data.iter().filter(|b| KNOWN.contains(b)).count() + if data.last() == Some(&0xD7) { 1000 } else { 0 };
    let mut best: (usize, Vec<u8>) = (score(bytes), bytes.to_vec());
    for model in RuidaModel::ALL {
        let plain = Swizzle::new(model.magic()).decode(bytes);
        let s = score(&plain);
        if s > best.0 {
            best = (s, plain);
        }
    }
    best.1
}

/// Reads a job file into layers. Layers come out in the order the file cuts them.
pub fn read(bytes: &[u8]) -> Result<Vec<RdFileLayer>, String> {
    let data = unscramble(bytes);
    // Split into commands: a byte with the top bit set, then its data bytes.
    let mut commands: Vec<&[u8]> = Vec::new();
    let mut start = None;
    for (i, &b) in data.iter().enumerate() {
        if b & 0x80 != 0 {
            if let Some(s) = start {
                commands.push(&data[s..i]);
            }
            start = Some(i);
        }
    }
    if let Some(s) = start {
        commands.push(&data[s..]);
    }
    let mut colors: std::collections::BTreeMap<u8, [u8; 3]> = Default::default();
    let mut layers: Vec<(u8, Vec<Polyline>)> = Vec::new();
    let mut layer = 0u8;
    let mut pos = (0i64, 0i64);
    let mut current: Polyline = Vec::new();
    let finish = |current: &mut Polyline, layers: &mut Vec<(u8, Vec<Polyline>)>, layer: u8| {
        if current.len() >= 2 {
            match layers.iter_mut().find(|(l, _)| *l == layer) {
                Some((_, paths)) => paths.push(std::mem::take(current)),
                None => layers.push((layer, vec![std::mem::take(current)])),
            }
        }
        current.clear();
    };
    let abs = |d: &[u8]| -> Option<(i64, i64)> {
        (d.len() >= 10).then(|| (decode35(d[0..5].try_into().expect("5 bytes")), decode35(d[5..10].try_into().expect("5 bytes"))))
    };
    let rel = |d: &[u8]| -> Option<i64> { (d.len() >= 2).then(|| decode14([d[0], d[1]])) };
    let mut recognised = 0usize;
    for cmd in commands {
        let (op, args) = (cmd[0], &cmd[1..]);
        // New position, and whether the laser is on for the move.
        let step: Option<((i64, i64), bool)> = match op {
            0x88 | 0xA8 => abs(args).map(|p| (p, op == 0xA8)),
            0x89 | 0xA9 => rel(args).zip(rel(args.get(2..).unwrap_or(&[]))).map(|(dx, dy)| ((pos.0 + dx, pos.1 + dy), op == 0xA9)),
            0x8A | 0xAA => rel(args).map(|dx| ((pos.0 + dx, pos.1), op == 0xAA)),
            0x8B | 0xAB => rel(args).map(|dy| ((pos.0, pos.1 + dy), op == 0xAB)),
            _ => None,
        };
        if let Some((to, cut)) = step {
            recognised += 1;
            if cut {
                if current.is_empty() {
                    current.push(pos);
                }
                current.push(to);
            } else {
                finish(&mut current, &mut layers, layer);
            }
            pos = to;
            continue;
        }
        match (op, args) {
            // Layer colour (settings table): CA 06 <layer> <colour>.
            (0xCA, [0x06, part, rest @ ..]) if rest.len() >= 5 => {
                let c = decode35(rest[0..5].try_into().expect("5 bytes"));
                colors.insert(*part, [(c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, ((c >> 16) & 0xFF) as u8]);
            }
            // Start of a layer's moves: CA 02 <layer>.
            (0xCA, [0x02, part, ..]) => {
                finish(&mut current, &mut layers, layer);
                layer = *part;
            }
            (0xD7, _) => break,
            _ => {}
        }
    }
    finish(&mut current, &mut layers, layer);
    if layers.is_empty() {
        return Err(if recognised == 0 { "not a Ruida job file (no moves found)".into() } else { "the job has no cuts".into() });
    }
    Ok(layers.into_iter().map(|(part, paths)| RdFileLayer { color: colors.get(&part).copied(), paths }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::MAGIC_644X;
    use crate::job::{encode_job, RdLayer, RdPath};

    #[test]
    fn reads_back_what_the_writer_writes() {
        let square = vec![(10_000, 10_000), (60_000, 10_000), (60_000, 40_000), (10_000, 40_000), (10_000, 10_000)];
        // A long line forces absolute moves; short steps use relative ones.
        let line = vec![(100_000, 5_000), (100_500, 5_000), (100_500, 5_800), (300_000, 200_000)];
        let layers = vec![
            RdLayer { speed: 20.0, min_power: 50.0, max_power: 60.0, color: [255, 0, 0], air_assist: true, paths: vec![RdPath::cut(square.clone())] },
            RdLayer { speed: 100.0, min_power: 10.0, max_power: 10.0, color: [0, 0, 255], air_assist: false, paths: vec![RdPath::cut(line.clone())] },
        ];
        let file: Vec<u8> = Swizzle::new(MAGIC_644X).encode(&encode_job(&layers).unwrap().concat());
        let read = read(&file).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].color, Some([255, 0, 0]));
        assert_eq!(read[1].color, Some([0, 0, 255]));
        assert_eq!(read[0].paths, vec![square]);
        assert_eq!(read[1].paths, vec![line]);
    }

    #[test]
    fn other_files_are_rejected() {
        assert!(read(b"this is not a job file").is_err());
        assert!(read(&[]).is_err());
    }
}
