//! Talking to the machine: connecting, preview, uploading and starting jobs, and status.

use crate::job::File;
use crate::protocol::{self, Reader, Status};
use crate::settings::Settings;
use std::fmt;
use std::io;
use std::time::{Duration, Instant};

/// How the app paces an upload: 2048-byte writes, about 100 ms apart.
const CHUNK: usize = 2048;
const CHUNK_INTERVAL: Duration = Duration::from_millis(100);
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// The machine stores the file before confirming it (about 3 s in the captures).
const STORE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// No answer to a command, named here.
    NoReply(&'static str),
    /// The machine answered but said no.
    Refused(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::NoReply(what) => write!(f, "the LaserPecker didn't answer ({what})"),
            Error::Refused(what) => write!(f, "the LaserPecker refused {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// The serial link (a trait so tests can stand in for the machine).
pub trait Port: Send {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    /// Reads what has arrived, waiting briefly; 0 when nothing has.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

struct Serial(Box<dyn serialport::SerialPort>);

impl Drop for Serial {
    /// Lowers DTR and RTS, which is all the app does to disconnect.
    fn drop(&mut self) {
        let _ = self.0.write_data_terminal_ready(false);
        let _ = self.0.write_request_to_send(false);
    }
}

impl Port for Serial {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        io::Write::write_all(&mut self.0, data)?;
        io::Write::flush(&mut self.0)
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match io::Read::read(&mut self.0, buf) {
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
            r => r,
        }
    }
}

pub struct Controller {
    port: Box<dyn Port>,
    reader: Reader,
    /// The last status the machine sent.
    pub status: Option<Status>,
    /// The settings in force: the machine's own, as read when connecting.
    pub settings: Settings,
}

impl Controller {
    /// Opens the serial port and introduces itself as the LP2's app does (see [`Controller::with_port`]).
    pub fn connect(port: &str, settings: &Settings, changed: bool) -> Result<Self> {
        let mut serial = serialport::new(port, crate::BAUD).timeout(Duration::from_millis(20)).open().map_err(|e| Error::Io(e.into()))?;
        serial.write_data_terminal_ready(true).map_err(|e| Error::Io(e.into()))?;
        serial.write_request_to_send(true).map_err(|e| Error::Io(e.into()))?;
        Self::with_port(Box::new(Serial(serial)), settings, changed)
    }

    /// Whether a LaserPecker 2 answers on `port`. Only asks its firmware version, which changes nothing.
    pub fn probe(port: &str) -> Result<bool> {
        let mut serial = serialport::new(port, crate::BAUD).timeout(Duration::from_millis(20)).open().map_err(|e| Error::Io(e.into()))?;
        serial.write_data_terminal_ready(true).map_err(|e| Error::Io(e.into()))?;
        serial.write_request_to_send(true).map_err(|e| Error::Io(e.into()))?;
        let mut c = Self { port: Box::new(Serial(serial)), reader: Reader::default(), status: None, settings: Settings::default() };
        match c.request(&protocol::query(protocol::QUERY_VERSION), "version", Duration::from_millis(600), |p| p.first() == Some(&0x00)) {
            Ok(_) => Ok(true),
            Err(Error::NoReply(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Introduces itself, reads the machine's settings and writes them back, as the app does (the
    /// LP2 beeps). If `changed`, the switches in `settings` were set by the user while disconnected
    /// and are written in place of the machine's; otherwise the machine's are kept and `settings`
    /// only supplies what BurnCraft keeps itself, such as the preview light's power. Settings BurnCraft
    /// doesn't show always stay as the machine had them.
    pub fn with_port(port: Box<dyn Port>, settings: &Settings, changed: bool) -> Result<Self> {
        let mut c = Self { port, reader: Reader::default(), status: None, settings: *settings };
        for (what, name) in [(protocol::QUERY_VERSION, "version"), (protocol::QUERY_08, "hello"), (protocol::QUERY_SERIAL, "serial number")] {
            c.request(&protocol::query(what), name, REPLY_TIMEOUT, |p| p.is_empty() || p[0] == 0x00)?;
        }
        let reply = c.request(&protocol::query(protocol::QUERY_SETTINGS), "settings", REPLY_TIMEOUT, |p| p.is_empty() || p[0] == 0x00)?;
        if let Some(machine) = Settings::machine_command(&reply) {
            let chosen = if changed { Settings { machine: Some(machine), ..*settings } } else { settings.with_machine(machine) };
            c.write_settings(&chosen)?;
        }
        c.query_status()?;
        Ok(c)
    }

    /// Writes the machine's settings (beep, touch buttons, work mode and so on).
    pub fn write_settings(&mut self, settings: &Settings) -> Result<()> {
        // Keep the machine's own values for what `settings` doesn't cover.
        let settings = Settings { machine: settings.machine.or(self.settings.machine), ..*settings };
        self.request(&settings.command(), "settings", REPLY_TIMEOUT, |p| p.starts_with(&[0x06, 0x01]))?;
        self.settings = settings;
        Ok(())
    }

    fn send(&mut self, payload: &[u8]) -> Result<()> {
        self.port.write_all(&protocol::frame(payload))?;
        Ok(())
    }

    /// Reads frames until one satisfies `wanted`, or `timeout` passes. Status replies seen on the
    /// way are kept; other frames (such as the `FF 00` sent when a job ends) are passed over.
    fn wait_for(&mut self, what: &'static str, timeout: Duration, wanted: impl Fn(&[u8]) -> bool) -> Result<Vec<u8>> {
        let end = Instant::now() + timeout;
        let mut buf = [0u8; 256];
        loop {
            while let Some(p) = self.reader.next_frame() {
                if let Some(s) = Status::parse(&p) {
                    self.status = Some(s);
                }
                if wanted(&p) {
                    return Ok(p);
                }
            }
            if Instant::now() >= end {
                return Err(Error::NoReply(what));
            }
            let n = self.port.read(&mut buf)?;
            self.reader.push(&buf[..n]);
        }
    }

    fn request(&mut self, payload: &[u8], what: &'static str, timeout: Duration, wanted: impl Fn(&[u8]) -> bool) -> Result<Vec<u8>> {
        self.send(payload)?;
        self.wait_for(what, timeout, wanted)
    }

    /// Asks for the machine's state.
    pub fn query_status(&mut self) -> Result<Status> {
        let p = self.request(&protocol::query(protocol::QUERY_STATUS), "status", REPLY_TIMEOUT, |p| Status::parse(p).is_some())?;
        Ok(Status::parse(&p).expect("checked"))
    }

    /// Ends a preview (or anything else the machine is doing that the app would end this way).
    pub fn idle(&mut self) -> Result<()> {
        self.request(&protocol::idle(), "stop", REPLY_TIMEOUT, |p| p.starts_with(&[0xFF, 0x00])).map(|_| ())
    }

    /// Traces a rectangle (mm) with the preview light at `power` until [`Controller::idle`], on an
    /// object `object_mm` across (with the roller).
    pub fn preview(&mut self, kind: protocol::PreviewKind, min: (f64, f64), max: (f64, f64), power: u8, object_mm: f64) -> Result<()> {
        let cmd = crate::job::preview(kind, min, max, power, object_mm).map_err(|_| Error::Refused("a frame outside the marking field"))?;
        self.request(&cmd, "preview", REPLY_TIMEOUT, |p| p.first() == Some(&0x02)).map(|_| ())
    }

    /// Pauses, resumes or ends the running job. The app doesn't wait for an answer; any that
    /// comes is read and passed over.
    pub fn job_state(&mut self, state: protocol::JobState) -> Result<()> {
        self.send(&protocol::job_state(state))?;
        let _ = self.wait_for("job", Duration::from_millis(500), |p| p.first() == Some(&0x01));
        Ok(())
    }

    /// Uploads a job and starts it at `power` and `speed` (percent), `passes` times over. `progress`
    /// hears bytes sent and the total. Afterwards, [`Controller::query_status`] follows the job.
    pub fn run(&mut self, file: &File, power: u8, speed: u8, passes: u16, object_mm: f64, mut progress: impl FnMut(usize, usize)) -> Result<()> {
        self.query_status()?;
        // The app asks for this block before every upload.
        self.request(&protocol::query(protocol::QUERY_BLOCK), "upload", REPLY_TIMEOUT, |p| p.len() >= 64 && p[0] == 0x00)?;
        self.idle()?;
        let size = file.bytes.len() as u32;
        let ready = self.request(&protocol::begin_file(size), "upload", REPLY_TIMEOUT, |p| p.starts_with(&[0x05, 0x01]))?;
        if ready.get(2) != Some(&0x01) {
            return Err(Error::Refused("the file"));
        }
        // The header goes on its own, then the body in paced chunks.
        let (header, body) = file.bytes.split_at(64.min(file.bytes.len()));
        self.port.write_all(header)?;
        progress(header.len(), file.bytes.len());
        for (i, chunk) in body.chunks(CHUNK).enumerate() {
            let started = Instant::now();
            self.port.write_all(chunk)?;
            progress(header.len() + i * CHUNK + chunk.len(), file.bytes.len());
            if let Some(rest) = CHUNK_INTERVAL.checked_sub(started.elapsed())
                && body.len() > CHUNK
            {
                std::thread::sleep(rest);
            }
        }
        let stored = self.wait_for("upload", STORE_TIMEOUT, |p| p.starts_with(&[0x05, 0x02]))?;
        if stored.get(2) != Some(&0x01) {
            return Err(Error::Refused("the file"));
        }
        // Followed by `FF 01` and the size; the app doesn't seem to wait for it.
        let _ = self.wait_for("upload", Duration::from_millis(500), |p| p.starts_with(&[0xFF, 0x01]));
        self.idle()?;
        self.request(&protocol::start(file.job, power, speed, file.at_mm, passes, object_mm), "start", REPLY_TIMEOUT, |p| p.starts_with(&[0x01, 0x01]))?;
        Ok(())
    }
}
