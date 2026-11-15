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

/// An object's diameter (mm) as sent: hundredths of a mm (used with the roller).
fn diameter(mm: f64) -> [u8; 2] {
    ((mm.max(0.0) * 100.0).floor().min(u16::MAX as f64) as u16).to_be_bytes()
}

/// How the preview light shows a rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    /// Traced on a flat surface.
    Rectangle = 2,
    /// With the electric roller, held still (the app's roller preview).
    Roller = 4,
    /// With the electric roller turning, to show the whole length.
    Rolling = 5,
}

/// Traces a rectangle with the preview light until [`idle`] is sent. Sizes are in 0.1 mm;
/// `power` is the light's power, `object_mm` the object's diameter (for the roller).
pub fn preview(kind: PreviewKind, w: u16, h: u16, x: u16, y: u16, power: u8, object_mm: f64) -> Vec<u8> {
    let mut p = vec![0x02, kind as u8];
    for v in [w, h, x, y] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    // Custom (0), then the pixel size code (4 = 0.1 mm).
    p.extend_from_slice(&[0x00, 0x04, power.max(1)]);
    p.extend_from_slice(&diameter(object_mm));
    p
}

/// What the start command asks for: start a job, or act on the one running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Start = 1,
    Continue = 2,
    End = 3,
    Hold = 4,
}

/// Starts the uploaded job: power and speed in percent, the job's id (as in its file header), its
/// top-left corner in whole mm, how many passes to run, and the object's diameter (mm, used with
/// the roller). The machine echoes `01 01`.
pub fn start(job: u32, power: u8, speed: u8, at_mm: (u16, u16), passes: u16, object_mm: f64) -> Vec<u8> {
    let mut p = vec![0x01, JobState::Start as u8, power, speed];
    p.extend_from_slice(&job.to_be_bytes());
    p.extend_from_slice(&at_mm.0.to_be_bytes());
    p.extend_from_slice(&at_mm.1.to_be_bytes());
    // Custom (0), then the passes.
    p.extend_from_slice(&[0x00, passes.clamp(1, 255) as u8]);
    // Laser type 0 (the LP2's 450 nm diode), then the object's diameter.
    p.push(0x00);
    p.extend_from_slice(&diameter(object_mm));
    // Precision (1), fan (0), laser frequency (60 kHz), speed in mm/s (0: set by the speed
    // percentage), and where the file is kept (1: the machine's SD card).
    p.extend_from_slice(&[0x01, 0x00, 0x3C, 0x00, 0x00, 0x01]);
    p
}

/// Pauses (Hold), resumes (Continue) or ends the running job: the start command with only its
/// state set, as the LP2 app sends it.
pub fn job_state(state: JobState) -> Vec<u8> {
    let mut p = vec![0x01, state as u8, 0x00, 101];
    p.resize(22, 0);
    p.push(0x01);
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
    /// What the job is doing (3 once a pass has finished, in the captures).
    pub work_state: u8,
    /// The error byte: the LP2 app treats only 3 as an error (the laser's temperature alarm).
    pub error: u8,
    /// The electric roller is plugged in.
    pub roller_connected: bool,
    /// The pass being run (1, 2…; 0 when none is).
    pub pass: u8,
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
        Some(Self {
            state,
            progress: p[3],
            power: p[4],
            speed: p[5],
            job: u32::from_be_bytes([p[8], p[9], p[10], p[11]]),
            work_state: p[2],
            error: p[6],
            roller_connected: p[14] != 0,
            pass: p[15],
        })
    }
}
