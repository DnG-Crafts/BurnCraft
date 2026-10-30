//! Framing and commands.
//!
//! A frame is `AA BB len payload sum`: `len` counts the payload plus the two checksum bytes, and
//! `sum` is the 16-bit sum of the payload bytes, big-endian. Commands are at least six bytes; the
//! first byte picks the command family and the machine answers with the same first two bytes.

pub const SYNC: [u8; 2] = [0xAA, 0xBB];

/// Query (`00 nn`): the machine's state and the job's progress.
pub const QUERY_STATUS: u8 = 0x00;
/// Query: a 128-byte block the app asks for before every upload (its meaning isn't known).
pub const QUERY_BLOCK: u8 = 0x01;
/// Query: the machine's settings.
pub const QUERY_SETTINGS: u8 = 0x02;
/// Query: the firmware version.
pub const QUERY_VERSION: u8 = 0x03;
/// Query: the serial number.
pub const QUERY_SERIAL: u8 = 0x05;
/// Query: asked on connecting; the reply is empty.
pub const QUERY_08: u8 = 0x08;

/// What the app writes to the machine's settings each time it connects.
pub const CONNECT_SETTINGS: [u8; 21] = [0x06, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x01, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// 16-bit sum of the payload bytes.
pub fn checksum(payload: &[u8]) -> u16 {
    payload.iter().fold(0u16, |s, &b| s.wrapping_add(b as u16))
}

/// A framed command.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() <= 253, "payload too long for one frame");
    let mut f = Vec::with_capacity(payload.len() + 5);
    f.extend_from_slice(&SYNC);
    f.push(payload.len() as u8 + 2);
    f.extend_from_slice(payload);
    f.extend_from_slice(&checksum(payload).to_be_bytes());
    f
}

/// A query (`00 nn 00 00 00 00`).
pub fn query(what: u8) -> Vec<u8> {
    vec![0x00, what, 0, 0, 0, 0]
}

/// Leaves preview (and is sent before uploading and starting a job). The machine echoes it, and
/// sends it on its own when a job ends.
pub fn idle() -> Vec<u8> {
    vec![0xFF, 0x00, 0, 0, 0, 0]
}

/// Announces a file of `size` bytes. The machine answers `05 01 01` when it's ready for it.
pub fn begin_file(size: u32) -> Vec<u8> {
    let mut p = vec![0x05, 0x01];
    p.extend_from_slice(&size.to_be_bytes());
    p.push(0);
    p
}

/// Traces a rectangle with the low-power light until [`idle`] is sent. Sizes are in 0.1 mm.
pub fn preview(w: u16, h: u16, x: u16, y: u16) -> Vec<u8> {
    let mut p = vec![0x02, 0x02];
    for v in [w, h, x, y] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(&[0x00, 0x04, 0x01, 0x07, 0xD0]);
    p
}

/// Starts the uploaded job: power and speed in percent, the job's id (as in its file header) and
/// its top-left corner in whole mm. The machine echoes `01 01`.
pub fn start(job: u32, power: u8, speed: u8, at_mm: (u16, u16)) -> Vec<u8> {
    let mut p = vec![0x01, 0x01, power, speed];
    p.extend_from_slice(&job.to_be_bytes());
    p.extend_from_slice(&at_mm.0.to_be_bytes());
    p.extend_from_slice(&at_mm.1.to_be_bytes());
    // Always the same in the captures.
    p.extend_from_slice(&[0x00, 0x01, 0x00, 0x07, 0xD0, 0x01, 0x00, 0x3C, 0x00, 0x00, 0x01]);
    p
}

/// Splits frames out of the bytes the machine sends, as they arrive.
#[derive(Debug, Default)]
pub struct Reader {
    buf: Vec<u8>,
}

impl Reader {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame's payload. Stray bytes and frames with a bad checksum are dropped.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        loop {
            let start = self.buf.windows(2).position(|w| w == SYNC);
            let Some(start) = start else {
                // Keep a trailing AA, which may be the start of the next frame.
                let keep = usize::from(self.buf.last() == Some(&SYNC[0]));
                self.buf.drain(..self.buf.len() - keep);
                return None;
            };
            self.buf.drain(..start);
            let len = *self.buf.get(2)? as usize;
            if len < 2 {
                self.buf.drain(..2);
                continue;
            }
            if self.buf.len() < 3 + len {
                return None;
            }
            let payload = self.buf[3..3 + len - 2].to_vec();
            let sum = u16::from_be_bytes([self.buf[1 + len], self.buf[2 + len]]);
            if sum == checksum(&payload) {
                self.buf.drain(..3 + len);
                return Some(payload);
            }
            self.buf.drain(..2);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Engraving,
    Preview,
    Other(u8),
}

/// The reply to [`QUERY_STATUS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// Percent done while engraving.
    pub progress: u8,
    /// The power and speed of the current (or last) job.
    pub power: u8,
    pub speed: u8,
    pub job: u32,
}

impl Status {
    /// The reply's payload is 29 bytes; the settings reply, which also starts with 00, is shorter.
    pub fn parse(p: &[u8]) -> Option<Self> {
        if !(28..64).contains(&p.len()) || p[0] != 0x00 {
            return None;
        }
        let state = match p[1] {
            0x01 => State::Engraving,
            0x02 => State::Preview,
            0x06 => State::Idle,
            s => State::Other(s),
        };
        Some(Self { state, progress: p[3], power: p[4], speed: p[5], job: u32::from_be_bytes([p[8], p[9], p[10], p[11]]) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn commands_match_the_captures() {
        assert_eq!(frame(&query(QUERY_VERSION)), hex("aa bb 08 00 03 00 00 00 00 00 03"));
        assert_eq!(frame(&idle()), hex("aa bb 08 ff 00 00 00 00 00 00 ff"));
        assert_eq!(frame(&CONNECT_SETTINGS), hex("aa bb 17 06 01 01 01 01 00 00 00 01 01 00 00 00 00 00 00 00 00 00 00 00 00 0c"));
        assert_eq!(frame(&begin_file(0xDA)), hex("aa bb 09 05 01 00 00 00 da 00 00 e0"));
        assert_eq!(frame(&begin_file(0x4901)), hex("aa bb 09 05 01 00 00 49 01 00 00 50"));
        assert_eq!(frame(&preview(0x178, 0x10E, 0x6B, 0x91)), hex("aa bb 11 02 02 01 78 01 0e 00 6b 00 91 00 04 01 07 d0 02 64"));
        assert_eq!(frame(&start(0x547F8A85, 100, 0x62, (10, 15))), hex("aa bb 19 01 01 64 62 54 7f 8a 85 00 0a 00 0f 00 01 00 07 d0 01 00 3c 00 00 01 03 d9"));
        assert_eq!(frame(&start(0xA756D19E, 100, 0x47, (45, 44))), hex("aa bb 19 01 01 64 47 a7 56 d1 9e 00 2d 00 2c 00 01 00 07 d0 01 00 3c 00 00 01 04 88"));
    }

    #[test]
    fn reader_splits_replies_across_packets() {
        let mut r = Reader::default();
        // A status reply whose checksum arrives in the next USB packet, then two short replies in one.
        r.push(&hex("aa bb 1f 00 01 01 04 64 47 00 00 a7 56 d1 9e 00 00 00 01 00 00 00 00 00 02 00 01 00 00 00 00 00"));
        assert_eq!(r.next_frame(), None);
        r.push(&hex("03 21 aa bb 08 05 02 01 00 00 00 00 08 aa bb 08 ff 01 00 00 49 01 01 4a"));
        let s = Status::parse(&r.next_frame().unwrap()).unwrap();
        assert_eq!(s, Status { state: State::Engraving, progress: 4, power: 100, speed: 0x47, job: 0xA756D19E });
        assert_eq!(r.next_frame().unwrap(), hex("05 02 01 00 00 00"));
        assert_eq!(r.next_frame().unwrap(), hex("ff 01 00 00 49 01"));
        assert_eq!(r.next_frame(), None);
        // An empty reply, and noise with a bad checksum before it.
        r.push(&hex("13 aa bb 08 00 00 00 00 00 00 00 99 aa bb 02 00 00"));
        assert_eq!(r.next_frame().unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn idle_status() {
        let s = Status::parse(&hex("00 06 ff 63 64 47 00 00 20 df 2f 44 00 00 00 00 00 00 00 00 00 02 00 01 00 00 00 00 00")).unwrap();
        assert_eq!((s.state, s.job), (State::Idle, 0x20DF2F44));
    }
}
