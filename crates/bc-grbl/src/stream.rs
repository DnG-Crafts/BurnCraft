//! GRBL connection: line streaming with character-counting flow control.

use crate::gcode::{CustomGcode, Dialect};
use crate::protocol::{alarm_text, parse_line, Response, Status};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

/// GRBL's serial receive buffer (bytes). Keeping the in-flight total under this keeps the planner full.
const RX_BUFFER: usize = 127;

/// Real-time commands (sent outside the line stream, acted on immediately).
const STATUS_QUERY: u8 = b'?';
const FEED_HOLD: u8 = b'!';
const CYCLE_START: u8 = b'~';
const SOFT_RESET: u8 = 0x18;
const JOG_CANCEL: u8 = 0x85;

/// After a read that brought nothing: wait a moment before reading again. The port is opened with
/// a read timeout, but some USB-serial drivers (seen on Windows 10) return at once instead of
/// waiting, and every loop that waits for a reply then spins, using a whole processor core.
pub(crate) fn nothing_read() {
    std::thread::sleep(Duration::from_millis(2));
}

#[derive(Debug)]
pub enum GrblError {
    Io(io::Error),
    Timeout(&'static str),
    /// GRBL rejected a line.
    Error { code: u32, line: String },
    Alarm(u32),
}

impl std::fmt::Display for GrblError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrblError::Io(e) => write!(f, "serial port error: {e}"),
            GrblError::Timeout(what) => write!(f, "no reply from the controller ({what})"),
            GrblError::Error { code: 5, line } if line.trim().eq_ignore_ascii_case("$H") => write!(
                f,
                "the controller can't home: homing is turned off in its settings ($22 = 0), usually because the machine has no limit switches.                  Move the head to the origin corner by jogging or by hand before a job. If the machine has limit switches,                  turn homing on by sending $22=1 in the Console (error 5)"
            ),
            GrblError::Error { code, line } => write!(f, "the controller rejected \"{line}\": {} (error {code})", crate::protocol::error_text(*code)),
            GrblError::Alarm(code) => write!(f, "alarm {code}: {}", alarm_text(*code)),
        }
    }
}

impl std::error::Error for GrblError {}

impl From<io::Error> for GrblError {
    fn from(e: io::Error) -> Self {
        GrblError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, GrblError>;

/// Anything that moves bytes to and from the controller (a serial port, or a test double).
pub trait Link: Read + Write + Send {}
impl<T: Read + Write + Send> Link for T {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamProgress {
    pub acked: usize,
    pub total: usize,
}

impl StreamProgress {
    pub fn done(&self) -> bool {
        self.acked >= self.total
    }
}

pub struct Grbl {
    link: Box<dyn Link>,
    read_buf: Vec<u8>,
    /// Lengths (incl. newline) of lines sent but not yet acknowledged, with the line text.
    in_flight: VecDeque<(usize, String)>,
    /// Lines of the running job not yet sent.
    queue: VecDeque<String>,
    total: usize,
    acked: usize,
    pub last_status: Option<Status>,
    /// Work coordinate offset, remembered from the last status that included it.
    wco: (f64, f64, f64),
    dialect: Dialect,
    /// Homing and laser-off commands for the custom dialect.
    custom: CustomGcode,
    /// Line-by-line streaming is paused (non-GRBL dialects have no real-time feed hold).
    held: bool,
}

impl Grbl {
    /// Opens a serial port and checks the controller answers. With `dtr`, the DTR line is raised,
    /// which resets most Arduino-style boards; some machines must be opened without it.
    pub fn open(port: &str, baud: u32, dialect: Dialect, custom: CustomGcode, dtr: bool) -> Result<Self> {
        let mut serial = serialport::new(port, baud).timeout(Duration::from_millis(20)).open().map_err(|e| GrblError::Io(e.into()))?;
        let _ = serial.write_data_terminal_ready(dtr);
        let mut grbl = Self::with_link(Box::new(serial)).with_dialect(dialect, custom);
        if dialect.is_grbl() {
            grbl.wait_for_banner(Duration::from_secs(3))?;
        } else {
            // Let the board finish its reset chatter, then check it acknowledges a harmless command.
            let until = Instant::now() + Duration::from_millis(2500);
            while Instant::now() < until {
                grbl.read_responses()?;
            }
            grbl.command("G90", Duration::from_secs(3))?;
        }
        Ok(grbl)
    }

    pub fn with_link(link: Box<dyn Link>) -> Self {
        Self {
            link,
            read_buf: Vec::new(),
            in_flight: VecDeque::new(),
            queue: VecDeque::new(),
            total: 0,
            acked: 0,
            last_status: None,
            wco: (0.0, 0.0, 0.0),
            dialect: Dialect::Grbl,
            custom: CustomGcode::default(),
            held: false,
        }
    }

    pub fn with_dialect(mut self, dialect: Dialect, custom: CustomGcode) -> Self {
        self.dialect = dialect;
        self.custom = custom;
        self
    }

    /// GRBL and Smoothieware answer the real-time `?` status query.
    fn has_realtime_status(&self) -> bool {
        matches!(self.dialect, Dialect::Grbl | Dialect::GrblM3 | Dialect::Smoothieware)
    }

    /// Waits for the banner; if none arrives (board didn't reset), checks it answers a status query.
    fn wait_for_banner(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            for r in self.read_responses()? {
                if matches!(r, Response::Welcome(_)) {
                    return Ok(());
                }
            }
        }
        self.query_status(Duration::from_secs(1)).map(|_| ())
    }

    /// Reads whatever has arrived and parses complete lines. Status reports update `last_status`.
    pub fn read_responses(&mut self) -> Result<Vec<Response>> {
        let mut buf = [0u8; 512];
        match self.link.read(&mut buf) {
            Ok(0) => nothing_read(),
            Ok(n) => self.read_buf.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => nothing_read(),
            Err(e) => return Err(e.into()),
        }
        let mut out = Vec::new();
        while let Some(i) = self.read_buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.read_buf.drain(..=i).collect();
            let text = String::from_utf8_lossy(&line);
            if text.trim().is_empty() {
                continue;
            }
            let r = parse_line(&text);
            if let Response::Status(s) = &r {
                if let Some(wco) = s.wco {
                    self.wco = wco;
                }
                self.last_status = Some(s.clone());
            }
            out.push(r);
        }
        Ok(out)
    }

    fn realtime(&mut self, byte: u8) -> Result<()> {
        self.link.write_all(&[byte])?;
        self.link.flush()?;
        Ok(())
    }

    /// Sends a status query and waits for the report. Dialects without `?` use `M114` (position only).
    pub fn query_status(&mut self, timeout: Duration) -> Result<Status> {
        if !self.has_realtime_status() {
            let lines = self.console("M114", timeout)?;
            let pos = lines.iter().find_map(|l| parse_m114(l)).ok_or(GrblError::Timeout("position"))?;
            let status = Status { state: if self.held { "Hold".into() } else { "Idle".into() }, mpos: None, wpos: Some((pos.0, pos.1, 0.0)), wco: None };
            self.last_status = Some(status.clone());
            return Ok(status);
        }
        self.realtime(STATUS_QUERY)?;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            for r in self.read_responses()? {
                match r {
                    Response::Status(s) => return Ok(s),
                    Response::Ok => {
                        self.in_flight.pop_front();
                        self.acked += 1;
                    }
                    _ => {}
                }
            }
        }
        Err(GrblError::Timeout("status"))
    }

    /// Asks for a status report without waiting; it arrives through [`Grbl::pump`] or [`Grbl::read_responses`].
    pub fn request_status(&mut self) -> Result<()> {
        if self.has_realtime_status() { self.realtime(STATUS_QUERY) } else { Ok(()) }
    }

    /// Work position (x, y) from the last status report.
    pub fn work_position(&self) -> Option<(f64, f64)> {
        let s = self.last_status.as_ref()?;
        s.wpos.map(|(x, y, _)| (x, y)).or_else(|| s.mpos.map(|(x, y, _)| (x - self.wco.0, y - self.wco.1)))
    }

    /// Sends one command line and waits for `ok` (only when no job is streaming).
    pub fn command(&mut self, line: &str, timeout: Duration) -> Result<()> {
        self.link.write_all(format!("{line}\n").as_bytes())?;
        self.link.flush()?;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            for r in self.read_responses()? {
                match r {
                    Response::Ok => return Ok(()),
                    Response::Error(code) => return Err(GrblError::Error { code, line: line.to_string() }),
                    Response::Alarm(code) => return Err(GrblError::Alarm(code)),
                    _ => {}
                }
            }
        }
        Err(GrblError::Timeout("command"))
    }

    /// Sends one line and returns every reply line up to and including `ok` or `error:` (for the console).
    pub fn console(&mut self, line: &str, timeout: Duration) -> Result<Vec<String>> {
        self.link.write_all(format!("{line}\n").as_bytes())?;
        self.link.flush()?;
        let deadline = Instant::now() + timeout;
        let mut lines = Vec::new();
        while Instant::now() < deadline {
            let mut buf = [0u8; 512];
            match self.link.read(&mut buf) {
                Ok(0) => nothing_read(),
                Ok(n) => self.read_buf.extend_from_slice(&buf[..n]),
                Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => nothing_read(),
                Err(e) => return Err(e.into()),
            }
            while let Some(i) = self.read_buf.iter().position(|&b| b == b'\n') {
                let raw: Vec<u8> = self.read_buf.drain(..=i).collect();
                let text = String::from_utf8_lossy(&raw).trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let done = text == "ok" || text.starts_with("error:") || text.starts_with("ALARM:");
                lines.push(text);
                if done {
                    return Ok(lines);
                }
            }
        }
        lines.push("(no reply)".into());
        Ok(lines)
    }

    /// The controller's settings (`$$`), in the order it lists them, and every line it sent (for
    /// the console). Replies still waiting from earlier commands are cleared first, and an "ok"
    /// that arrives before any setting is taken as one of those, not the end of the list.
    pub fn read_settings(&mut self) -> (Result<Vec<(u32, String)>>, Vec<String>) {
        let mut lines = Vec::new();
        let result = self.read_settings_into(&mut lines);
        (result, lines)
    }

    fn read_settings_into(&mut self, lines: &mut Vec<String>) -> Result<Vec<(u32, String)>> {
        self.discard_input(Duration::from_millis(150))?;
        self.link.write_all(b"$$\n")?;
        self.link.flush()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut settings: Vec<(u32, String)> = Vec::new();
        while Instant::now() < deadline {
            for text in self.read_lines()? {
                lines.push(text.clone());
                if let Some(s) = crate::settings::parse_line(&text) {
                    settings.push(s);
                } else if text == "ok" && !settings.is_empty() {
                    return Ok(settings);
                } else if let Some(code) = text.strip_prefix("error:").and_then(|c| c.trim().parse().ok()) {
                    return Err(GrblError::Error { code, line: "$$".into() });
                } else if let Some(code) = text.strip_prefix("ALARM:").and_then(|c| c.trim().parse().ok()) {
                    return Err(GrblError::Alarm(code));
                }
            }
        }
        // No closing "ok": use what arrived, if anything did.
        if settings.is_empty() {
            return Err(GrblError::Timeout("the settings list"));
        }
        Ok(settings)
    }

    /// Reads whatever is waiting for up to `time` and throws it away.
    fn discard_input(&mut self, time: Duration) -> Result<()> {
        let until = Instant::now() + time;
        while Instant::now() < until {
            self.read_lines()?;
        }
        self.read_buf.clear();
        Ok(())
    }

    /// Complete lines received so far (trimmed, empty ones skipped), after one read.
    fn read_lines(&mut self) -> Result<Vec<String>> {
        let mut buf = [0u8; 512];
        match self.link.read(&mut buf) {
            Ok(0) => nothing_read(),
            Ok(n) => self.read_buf.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => nothing_read(),
            Err(e) => return Err(e.into()),
        }
        let mut out = Vec::new();
        while let Some(i) = self.read_buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.read_buf.drain(..=i).collect();
            let text = String::from_utf8_lossy(&raw).trim().to_string();
            if !text.is_empty() {
                out.push(text);
            }
        }
        Ok(out)
    }

    /// Changes one setting (`$N=value`). The controller stores it in its memory at once.
    pub fn write_setting(&mut self, id: u32, value: &str) -> Result<()> {
        self.command(&format!("${id}={}", value.trim()), Duration::from_secs(3))
    }

    pub fn home(&mut self) -> Result<()> {
        let cmd = match self.dialect {
            Dialect::Grbl | Dialect::GrblM3 => "$H".to_string(),
            Dialect::Smoothieware => "G28.2".to_string(),
            Dialect::Marlin => "G28".to_string(),
            Dialect::Custom => self.custom.home.clone(),
        };
        self.command(&cmd, Duration::from_secs(120))
    }

    pub fn unlock(&mut self) -> Result<()> {
        match self.dialect {
            Dialect::Grbl | Dialect::GrblM3 => self.command("$X", Duration::from_secs(2)),
            Dialect::Smoothieware => self.command("M999", Duration::from_secs(2)),
            Dialect::Marlin | Dialect::Custom => Ok(()),
        }
    }

    /// Relative jog in mm at `feed` mm/min.
    pub fn jog_by(&mut self, dx: f64, dy: f64, feed: f64) -> Result<()> {
        if self.dialect.is_grbl() {
            return self.command(&format!("$J=G91 G21 X{dx:.3} Y{dy:.3} F{feed:.0}"), Duration::from_secs(2));
        }
        self.command("G91", Duration::from_secs(2))?;
        let moved = self.command(&format!("G0 X{dx:.3} Y{dy:.3} F{feed:.0}"), Duration::from_secs(10));
        self.command("G90", Duration::from_secs(2))?;
        moved
    }

    /// Relative Z move in mm at `feed` mm/min, laser off.
    pub fn jog_z(&mut self, dz: f64, feed: f64) -> Result<()> {
        if self.dialect.is_grbl() {
            return self.command(&format!("$J=G91 G21 Z{dz:.3} F{feed:.0}"), Duration::from_secs(2));
        }
        self.command("G91", Duration::from_secs(2))?;
        let moved = self.command(&format!("G0 Z{dz:.3} F{feed:.0}"), Duration::from_secs(10));
        self.command("G90", Duration::from_secs(2))?;
        moved
    }

    /// Absolute move (work coordinates) in mm at `feed` mm/min, laser off.
    pub fn jog_to(&mut self, x: f64, y: f64, feed: f64) -> Result<()> {
        if self.dialect.is_grbl() {
            return self.command(&format!("$J=G90 G21 X{x:.3} Y{y:.3} F{feed:.0}"), Duration::from_secs(2));
        }
        self.command(&format!("G0 X{x:.3} Y{y:.3} F{feed:.0}"), Duration::from_secs(10))
    }

    pub fn cancel_jog(&mut self) -> Result<()> {
        if self.dialect.is_grbl() { self.realtime(JOG_CANCEL) } else { Ok(()) }
    }

    /// GRBL: real-time feed hold. Others: stop sending lines (moves already queued still run).
    pub fn pause(&mut self) -> Result<()> {
        if self.dialect.is_grbl() {
            return self.realtime(FEED_HOLD);
        }
        self.held = true;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        if self.dialect.is_grbl() {
            return self.realtime(CYCLE_START);
        }
        self.held = false;
        Ok(())
    }

    /// Stops motion and the laser, and drops the rest of the job.
    /// GRBL: feed hold then soft reset. Marlin: quick stop. Others: stop sending and turn the laser off.
    pub fn stop(&mut self) -> Result<()> {
        self.queue.clear();
        self.in_flight.clear();
        self.total = 0;
        self.acked = 0;
        self.held = false;
        if self.dialect.is_grbl() {
            self.realtime(FEED_HOLD)?;
            std::thread::sleep(Duration::from_millis(100));
            return self.realtime(SOFT_RESET);
        }
        let mut lines = Vec::new();
        if self.dialect == Dialect::Marlin {
            lines.push("M410".to_string());
        }
        lines.push(if self.dialect == Dialect::Custom { self.custom.laser_off.clone() } else { "M5".into() });
        for l in lines.iter().filter(|l| !l.is_empty()) {
            self.link.write_all(format!("{l}\n").as_bytes())?;
        }
        self.link.flush()?;
        Ok(())
    }

    /// Starts streaming a program. Call [`Grbl::pump`] regularly until it reports done.
    pub fn start_stream(&mut self, lines: Vec<String>) {
        self.total = lines.len();
        self.acked = 0;
        self.in_flight.clear();
        self.queue = lines.into();
    }

    pub fn is_streaming(&self) -> bool {
        !self.queue.is_empty() || !self.in_flight.is_empty()
    }

    /// Sends as many queued lines as fit in GRBL's buffer and processes replies.
    pub fn pump(&mut self) -> Result<StreamProgress> {
        for r in self.read_responses()? {
            match r {
                Response::Ok => {
                    if self.in_flight.pop_front().is_some() {
                        self.acked += 1;
                    }
                }
                Response::Error(code) => {
                    let line = self.in_flight.pop_front().map(|(_, l)| l).unwrap_or_default();
                    self.stop()?;
                    return Err(GrblError::Error { code, line });
                }
                Response::Alarm(code) => {
                    self.queue.clear();
                    self.in_flight.clear();
                    return Err(GrblError::Alarm(code));
                }
                _ => {}
            }
        }
        if self.held {
            return Ok(StreamProgress { acked: self.acked, total: self.total });
        }
        // GRBL: fill its serial buffer (character counting). Others: one line at a time, wait for ok.
        let line_by_line = !self.dialect.is_grbl();
        let mut used: usize = self.in_flight.iter().map(|(n, _)| n).sum();
        let mut batch = String::new();
        while let Some(line) = self.queue.front() {
            let len = line.len() + 1;
            if !self.in_flight.is_empty() && (line_by_line || used + len > RX_BUFFER) {
                break;
            }
            let line = self.queue.pop_front().unwrap();
            batch.push_str(&line);
            batch.push('\n');
            used += len;
            self.in_flight.push_back((len, line));
        }
        if !batch.is_empty() {
            self.link.write_all(batch.as_bytes())?;
            self.link.flush()?;
        }
        Ok(StreamProgress { acked: self.acked, total: self.total })
    }
}

/// Parses Marlin's `M114` reply, e.g. `X:10.00 Y:20.00 Z:0.00 E:0.00 Count X:...`.
fn parse_m114(line: &str) -> Option<(f64, f64)> {
    let head = line.split("Count").next()?;
    let field = |name: &str| head.split_whitespace().find_map(|t| t.strip_prefix(name)).and_then(|v| v.parse::<f64>().ok());
    Some((field("X:")?, field("Y:")?))
}
