//! Encodes a vector job as a Ruida command stream.
//!
//! Structure follows the open-source MeerK40t `rdjob` writer: file header with
//! bounds, per-layer settings table, then per-layer bodies of travel and cut
//! moves, and a footer with the file checksum. The output is a list of
//! individual commands (unscrambled), so the transport can split packets on
//! command boundaries.

use crate::codec::{encode14, encode35, encode_color, encode_coord, encode_power, encode_speed};

/// One layer of a job, already in machine coordinates (µm from the machine origin).
#[derive(Debug, Clone, PartialEq)]
pub struct RdLayer {
    /// Head speed in mm/s.
    pub speed: f64,
    /// Power in percent (laser 1).
    pub min_power: f64,
    pub max_power: f64,
    pub color: [u8; 3],
    pub air_assist: bool,
    /// Polylines in order. Closed shapes must repeat their first point at the end.
    pub paths: Vec<RdPath>,
}

/// A polyline, cut with the laser on, or traversed with it off (e.g. overscan run-ups).
#[derive(Debug, Clone, PartialEq)]
pub struct RdPath {
    pub points: Vec<(i64, i64)>,
    pub laser: bool,
}

impl RdPath {
    pub fn cut(points: Vec<(i64, i64)>) -> Self {
        Self { points, laser: true }
    }
}

/// Largest relative move, in µm. Larger moves use absolute coordinates.
const MAX_RELATIVE: i64 = 8191;

#[derive(Default)]
struct Writer {
    cmds: Vec<Vec<u8>>,
    pos: Option<(i64, i64)>,
}

impl Writer {
    fn cmd(&mut self, parts: &[&[u8]]) {
        self.cmds.push(parts.concat());
    }

    fn xy(&mut self, op_abs: u8, op_rel: u8, op_rel_x: u8, op_rel_y: u8, to: (i64, i64)) {
        let (dx, dy) = match self.pos {
            Some((x, y)) => (to.0 - x, to.1 - y),
            None => (i64::MAX, i64::MAX),
        };
        if dx == 0 && dy == 0 {
            return;
        }
        if dx.abs() > MAX_RELATIVE || dy.abs() > MAX_RELATIVE {
            self.cmd(&[&[op_abs], &encode_coord(to.0), &encode_coord(to.1)]);
        } else if dy == 0 {
            self.cmd(&[&[op_rel_x], &encode14(dx)]);
        } else if dx == 0 {
            self.cmd(&[&[op_rel_y], &encode14(dy)]);
        } else {
            self.cmd(&[&[op_rel], &encode14(dx), &encode14(dy)]);
        }
        self.pos = Some(to);
    }

    fn travel(&mut self, to: (i64, i64)) {
        self.xy(0x88, 0x89, 0x8A, 0x8B, to);
    }

    fn cut(&mut self, to: (i64, i64)) {
        self.xy(0xA8, 0xA9, 0xAA, 0xAB, to);
    }

    fn layer_end(&mut self) {
        self.cmd(&[&[0xE7, 0x00]]);
        self.cmd(&[&[0xCA, 0x01, 0x00]]);
        self.cmd(&[&[0xCA, 0x01, 0x30]]);
    }
}

type Bounds = ((i64, i64), (i64, i64));

fn bounds<'a>(points: impl Iterator<Item = &'a (i64, i64)>) -> Option<Bounds> {
    points.fold(None, |acc, &(x, y)| match acc {
        None => Some(((x, y), (x, y))),
        Some(((x0, y0), (x1, y1))) => Some(((x0.min(x), y0.min(y)), (x1.max(x), y1.max(y)))),
    })
}

/// Builds the command stream for `layers`. Returns `None` if there is nothing to cut.
pub fn encode_job(layers: &[RdLayer]) -> Option<Vec<Vec<u8>>> {
    let layers: Vec<&RdLayer> = layers.iter().filter(|l| l.paths.iter().any(|p| p.laser && p.points.len() >= 2)).collect();
    let ((min_x, min_y), (max_x, max_y)) = bounds(layers.iter().flat_map(|l| l.paths.iter().flat_map(|p| &p.points)))?;
    let mut w = Writer::default();
    let pair = |a: i64, b: i64| [encode_coord(a), encode_coord(b)].concat();

    // --- File header.
    w.cmd(&[&[0xD8, 0x10]]); // reference point mode (as written by MeerK40t)
    w.cmd(&[&[0xE6, 0x01]]); // set absolute
    w.cmd(&[&[0xF0]]); // reference point set
    w.cmd(&[&[0xF1, 0x02, 0x00]]); // block cutting off
    w.cmd(&[&[0xD8, 0x00]]); // start process
    w.cmd(&[&[0xE7, 0x06], &pair(0, 0)]); // feed repeat
    w.cmd(&[&[0xE7, 0x38, 0x00]]); // feed auto-pause off
    // Job extents. Corner order matches MeerK40t's writer (X reversed relative to the design view).
    w.cmd(&[&[0xE7, 0x03], &pair(max_x, min_y)]); // process top-left
    w.cmd(&[&[0xE7, 0x07], &pair(min_x, max_y)]); // process bottom-right
    w.cmd(&[&[0xE7, 0x50], &pair(max_x, min_y)]); // document min point
    w.cmd(&[&[0xE7, 0x51], &pair(min_x, max_y)]); // document max point
    w.cmd(&[&[0xE7, 0x04], &[1, 1, 0, 0, 0, 0, 0].map(encode14).concat()]); // process repeat: once
    w.cmd(&[&[0xE7, 0x05, 0x00]]); // array direction

    // --- Layer settings table.
    for (part, layer) in layers.iter().enumerate() {
        let p = part as u8;
        let ((lx0, ly0), (lx1, ly1)) = bounds(layer.paths.iter().flat_map(|p| &p.points)).expect("non-empty layer");
        w.cmd(&[&[0xC9, 0x04, p], &encode_speed(layer.speed)]);
        w.cmd(&[&[0xC6, 0x31, p], &encode_power(layer.min_power)]);
        w.cmd(&[&[0xC6, 0x32, p], &encode_power(layer.max_power)]);
        w.cmd(&[&[0xC6, 0x41, p], &encode_power(0.0)]);
        w.cmd(&[&[0xC6, 0x42, p], &encode_power(0.0)]);
        w.cmd(&[&[0xCA, 0x06, p], &encode_color(layer.color)]);
        w.cmd(&[&[0xCA, 0x41, p, 0x00]]); // work mode: vector
        w.cmd(&[&[0xE7, 0x52, p], &pair(lx0, ly0)]);
        w.cmd(&[&[0xE7, 0x53, p], &pair(lx1, ly1)]);
        w.cmd(&[&[0xE7, 0x61, p], &pair(lx0, ly0)]);
        w.cmd(&[&[0xE7, 0x62, p], &pair(lx1, ly1)]);
    }
    w.cmd(&[&[0xCA, 0x22, (layers.len() - 1) as u8]]); // highest layer index
    for axis in [0u8, 1] {
        w.cmd(&[&[0xE7, 0x54, axis], &encode_coord(0)]); // pen offset
    }
    for axis in [0u8, 1] {
        w.cmd(&[&[0xE7, 0x55, axis], &encode_coord(0)]); // layer offset
    }
    w.cmd(&[&[0xF1, 0x03], &pair(0, 0)]); // display offset
    w.cmd(&[&[0xE7, 0x0A], &encode_coord(0)]); // feed info

    // --- Single-element array wrapper.
    w.cmd(&[&[0xEA, 0x00]]);
    w.cmd(&[&[0xE7, 0x60, 0x00]]);
    w.cmd(&[&[0xE7, 0x0B, 0x00]]);
    w.cmd(&[&[0xE7, 0x13], &pair(min_x, min_y)]);
    w.cmd(&[&[0xE7, 0x17], &pair(max_x, max_y)]);
    w.cmd(&[&[0xE7, 0x23], &pair(0, 0)]);
    w.cmd(&[&[0xE7, 0x24, 0x00]]);
    // Array repeat: one element. The trailing values are copied from MeerK40t's known-working output.
    w.cmd(&[&[0xE7, 0x08], &[1, 1, 0, 1123, -3328, 4, 3480].map(encode14).concat()]);

    // --- Layer bodies.
    for (part, layer) in layers.iter().enumerate() {
        if part > 0 {
            w.layer_end();
        }
        w.cmd(&[&[0xCA, 0x02, part as u8]]); // layer number
        w.cmd(&[&[0xCA, 0x01, 0x10]]); // laser device 0
        w.cmd(&[&[0xCA, 0x01, if layer.air_assist { 0x13 } else { 0x12 }]]);
        w.cmd(&[&[0xC9, 0x02], &encode_speed(layer.speed)]);
        w.cmd(&[&[0xC6, 0x12], &encode35(0)]); // laser on delay
        w.cmd(&[&[0xC6, 0x13], &encode35(0)]); // laser off delay
        w.cmd(&[&[0xC6, 0x01], &encode_power(layer.min_power)]);
        w.cmd(&[&[0xC6, 0x02], &encode_power(layer.max_power)]);
        w.cmd(&[&[0xC6, 0x21], &encode_power(0.0)]);
        w.cmd(&[&[0xC6, 0x22], &encode_power(0.0)]);
        w.cmd(&[&[0xCA, 0x03, 0x01]]); // enable laser tube
        for path in layer.paths.iter().filter(|p| p.points.len() >= 2) {
            w.travel(path.points[0]);
            for &pt in &path.points[1..] {
                if path.laser {
                    w.cut(pt);
                } else {
                    w.travel(pt);
                }
            }
        }
    }

    // --- Footer: layer end, file checksum (including the EOF byte), EOF.
    w.layer_end();
    let sum: i64 = w.cmds.iter().flatten().map(|&b| b as i64).sum::<i64>() + 0xD7;
    w.cmd(&[&[0xE5, 0x05], &encode35(sum)]);
    w.cmd(&[&[0xD7]]);
    Some(w.cmds)
}
