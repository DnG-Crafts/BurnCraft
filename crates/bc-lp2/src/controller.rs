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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::gcode_file;
    use std::sync::{Arc, Mutex};

    /// A pretend LP2: answers each framed command as the real one did in the captures.
    #[derive(Clone, Default)]
    struct FakeLp2 {
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
        out: Arc<Mutex<Vec<u8>>>,
        /// Bytes of an upload still expected.
        uploading: Arc<Mutex<usize>>,
    }

    impl FakeLp2 {
        fn commands(&self) -> Vec<Vec<u8>> {
            self.sent.lock().unwrap().iter().filter(|w| w.starts_with(&protocol::SYNC)).map(|w| w[3..w.len() - 2].to_vec()).collect()
        }

        fn reply(&self, payload: &[u8]) {
            self.out.lock().unwrap().extend(protocol::frame(payload));
        }
    }

    impl Port for FakeLp2 {
        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            self.sent.lock().unwrap().push(data.to_vec());
            let mut left = self.uploading.lock().unwrap();
            if *left > 0 {
                *left -= data.len();
                if *left == 0 {
                    self.reply(&[0x05, 0x02, 0x01, 0, 0, 0]);
                    self.reply(&[0xFF, 0x01, 0, 0, 0, 0xDA]);
                }
                return Ok(());
            }
            let p = &data[3..data.len() - 2];
            match (p[0], p[1]) {
                (0x00, 0x00) => self.reply(&[0x00, 0x06, 0xFF, 0x63, 0x64, 0x47, 0, 0, 0x20, 0xDF, 0x2F, 0x44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x02, 0, 0x01, 0, 0, 0, 0, 0]),
                (0x00, 0x01) => self.reply(&[0x00; 128]),
                (0x00, 0x08) => self.reply(&[]),
                // The settings reply from a capture: free mode, beep and path optimisation on.
                (0x00, 0x02) => self.reply(&[0x00, 0x01, 0x01, 0x01, 0x00, 0x01, 0x00, 0x00, 0x01, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0a, 0x02]),
                (0x00, _) => self.reply(&[0x00, p[1], 0, 0, 0, 0, 0, 0, p[1]]),
                (0x05, 0x01) => {
                    *left = u32::from_be_bytes([p[2], p[3], p[4], p[5]]) as usize;
                    self.reply(&[0x05, 0x01, 0x01, 0, 0, 0]);
                }
                                (a, b) => self.reply(&[a, b, 0, 0, 0, 0]),
            }
            Ok(())
        }

        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut out = self.out.lock().unwrap();
            // Hand the replies over a few bytes at a time, as USB packets split them.
            let n = out.len().min(buf.len()).min(7);
            buf[..n].copy_from_slice(&out[..n]);
            out.drain(..n);
            Ok(n)
        }
    }

    #[test]
    fn connects_and_runs_a_job_like_the_app() {
        let lp2 = FakeLp2::default();
        let mut c = Controller::with_port(Box::new(lp2.clone()), &Settings::default(), false).unwrap();
        let firsts: Vec<(u8, u8)> = lp2.commands().iter().map(|p| (p[0], p[1])).collect();
        assert_eq!(firsts, [(0x00, 0x03), (0x00, 0x08), (0x00, 0x05), (0x00, 0x02), (0x06, 0x01), (0x00, 0x00)], "the settings are read, then written back");
        let reply = [0x00, 0x01, 0x01, 0x01, 0x00, 0x01, 0x00, 0x00, 0x01, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0a, 0x02];
        assert_eq!(lp2.commands()[4], Settings::machine_command(&reply).unwrap().to_vec(), "exactly as read");
        assert!(c.settings.free_mode, "the machine's own");

        // Changed while disconnected: the user's switches are written, the rest stay the machine's.
        let other = FakeLp2::default();
        let mine = Settings { beep: false, ..Settings::default() };
        let chosen = Controller::with_port(Box::new(other.clone()), &mine, true).unwrap();
        let written = &other.commands()[4];
        let machine = Settings::machine_command(&reply).unwrap();
        assert_eq!((written[2], written[3]), (0, 0), "free mode and beep as chosen");
        assert_eq!(written[8], machine[8], "a setting BurnCraft doesn't show is the machine's");
        assert!(!chosen.settings.beep);
        assert_eq!(c.status.unwrap().state, crate::State::Idle);

        lp2.sent.lock().unwrap().clear();
        let ring = vec![(10.763, 15.576), (48.386, 15.576), (48.386, 42.639), (10.763, 42.639), (10.763, 15.576)];
        let file = gcode_file(&[(ring, 1.0)], 0x547F8A85, "Untitled").unwrap();
        let mut last = (0, 0);
        c.run(&file, 100, 0x62, 1, 20.0, |done, total| last = (done, total)).unwrap();
        assert_eq!(last, (0xDA, 0xDA));
        let firsts: Vec<(u8, u8)> = lp2.commands().iter().map(|p| (p[0], p[1])).collect();
        assert_eq!(firsts, [(0x00, 0x00), (0x00, 0x01), (0xFF, 0x00), (0x05, 0x01), (0xFF, 0x00), (0x01, 0x01)]);
        let uploaded: Vec<u8> = lp2.sent.lock().unwrap().iter().filter(|w| !w.starts_with(&protocol::SYNC)).flatten().copied().collect();
        assert_eq!(uploaded, file.bytes);
        assert_eq!(lp2.commands().last().unwrap(), &protocol::start(0x547F8A85, 100, 0x62, (10, 15), 1, 20.0));
    }

    #[test]
    fn silence_is_an_error() {
        struct Silent;
        impl Port for Silent {
            fn write_all(&mut self, _: &[u8]) -> io::Result<()> {
                Ok(())
            }
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                std::thread::sleep(Duration::from_millis(5));
                Ok(0)
            }
        }
        assert!(matches!(Controller::with_port(Box::new(Silent), &Settings::default(), false), Err(Error::NoReply("version"))));
    }
}
