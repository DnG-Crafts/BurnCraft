//! High-level machine control for Ruida controllers.

use crate::codec::{encode_coord, encode_speed, RuidaModel};
use std::time::{Duration, Instant};
use crate::job::{encode_job, RdLayer};
use crate::link::{LinkError, Result, SerialLink, Transport, UdpLink};
use std::net::Ipv4Addr;

/// Controller parameter addresses (public; see MeerK40t `rdjob.py`).
mod mem {
    pub const CARD_ID: [u8; 2] = [0x05, 0x7E];
    pub const MACHINE_STATUS: [u8; 2] = [0x04, 0x00];
    pub const CURRENT_X: [u8; 2] = [0x04, 0x21];
    pub const CURRENT_Y: [u8; 2] = [0x04, 0x31];
    pub const BED_SIZE_X: [u8; 2] = [0x00, 0x26];
    pub const BED_SIZE_Y: [u8; 2] = [0x00, 0x36];
}

/// Direction keys for continuous jogging, as on the controller's keypad: the axis moves from the
/// key press until its release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JogKey {
    XLeft,
    XRight,
    YTop,
    YBottom,
    ZUp,
    ZDown,
    UForward,
    UBackward,
}

impl JogKey {
    fn code(self) -> u8 {
        match self {
            JogKey::XLeft => 0x20,
            JogKey::XRight => 0x21,
            JogKey::YTop => 0x22,
            JogKey::YBottom => 0x23,
            JogKey::ZUp => 0x24,
            JogKey::ZDown => 0x25,
            JogKey::UForward => 0x26,
            JogKey::UBackward => 0x27,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MachineState {
    /// Raw status bits from the controller.
    pub status: i64,
    /// Head position in mm from the machine origin.
    pub x: f64,
    pub y: f64,
}

pub struct Controller {
    link: Box<dyn Transport>,
}

impl Controller {
    /// Connects over Ethernet to the controller at `ip` and checks that it answers.
    pub fn connect(ip: Ipv4Addr, model: RuidaModel) -> Result<Self> {
        let controller = Self { link: Box::new(UdpLink::connect(ip, model.magic())?) };
        controller.card_id()?;
        Ok(controller)
    }

    /// Connects over USB (the controller's serial port) and checks that it answers.
    pub fn connect_usb(port: &str, model: RuidaModel) -> Result<Self> {
        let controller = Self { link: Box::new(SerialLink::open(port, model.magic())?) };
        controller.card_id()?;
        Ok(controller)
    }

    pub fn card_id(&self) -> Result<i64> {
        self.link.read_param(mem::CARD_ID)
    }

    pub fn state(&self) -> Result<MachineState> {
        Ok(MachineState {
            status: self.link.read_param(mem::MACHINE_STATUS)?,
            x: self.link.read_param(mem::CURRENT_X)? as f64 / 1000.0,
            y: self.link.read_param(mem::CURRENT_Y)? as f64 / 1000.0,
        })
    }

    /// Work area size reported by the controller, in mm.
    pub fn bed_size(&self) -> Result<(f64, f64)> {
        Ok((self.link.read_param(mem::BED_SIZE_X)? as f64 / 1000.0, self.link.read_param(mem::BED_SIZE_Y)? as f64 / 1000.0))
    }

    fn command(&self, bytes: &[u8]) -> Result<()> {
        self.link.send_packet(bytes).map(|_| ())
    }

    pub fn home(&self) -> Result<()> {
        self.command(&[0xD8, 0x2A])
    }

    /// Rapid move with the laser off to the absolute position (x, y) mm in machine coordinates,
    /// at `speed` mm/s.
    ///
    /// The speed is set first (C9 02); without it the controller moves at whatever speed it last
    /// had, which can be very slow. The move is D9 10 with option 0x00, which takes an absolute
    /// position (as the open-source MeerK40t driver uses on real controllers). Option 0x02 moves
    /// *by* the given amount, so an absolute position sent with it runs the head towards the
    /// limits.
    pub fn move_to(&self, x: f64, y: f64, speed: f64) -> Result<()> {
        self.command(&move_command(x, y, speed))
    }

    /// Relative jog by (dx, dy) mm from the current position, at `speed` mm/s.
    pub fn jog_by(&self, dx: f64, dy: f64, speed: f64) -> Result<()> {
        let s = self.state()?;
        self.move_to(s.x + dx, s.y + dy, speed)
    }

    /// Waits until the head reaches (x, y) mm (within 0.1 mm). Gives up, and stops the machine,
    /// if `abort` says so, if it takes longer than `limit`, or if the head stops short.
    pub fn wait_until_at(&self, x: f64, y: f64, limit: Duration, abort: &dyn Fn() -> bool, mut report: impl FnMut(MachineState)) -> Result<()> {
        let start = Instant::now();
        let mut last: Option<(f64, f64, Instant)> = None;
        loop {
            if abort() {
                let _ = self.stop();
                return Err(LinkError::Rejected("stopped"));
            }
            // A slow reply while moving is normal; only give up on the overall time limit.
            if let Ok(s) = self.state() {
                report(s);
                if (s.x - x).abs() < 0.1 && (s.y - y).abs() < 0.1 {
                    return Ok(());
                }
                match last {
                    Some((lx, ly, since)) if (lx - s.x).abs() < 0.01 && (ly - s.y).abs() < 0.01 => {
                        if since.elapsed() > STALL {
                            let _ = self.stop();
                            return Err(LinkError::Rejected("the head stopped short of the target (outside the machine limits?)"));
                        }
                    }
                    _ => last = Some((s.x, s.y, Instant::now())),
                }
            }
            if start.elapsed() > limit {
                let _ = self.stop();
                return Err(LinkError::Rejected("the move took too long, so it was stopped"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Traces the rectangle (machine mm) with the laser off at `speed` mm/s, ending at the start
    /// corner. Each corner is reached before the next move is sent; `abort` stops it.
    pub fn frame(&self, min: (f64, f64), max: (f64, f64), speed: f64, abort: &dyn Fn() -> bool, mut report: impl FnMut(MachineState)) -> Result<()> {
        let mut at = self.state().map(|s| (s.x, s.y)).unwrap_or(min);
        for (x, y) in [min, (max.0, min.1), max, (min.0, max.1), min] {
            self.move_to(x, y, speed)?;
            self.wait_until_at(x, y, move_time_limit(at, (x, y), speed), abort, &mut report)?;
            at = (x, y);
        }
        Ok(())
    }

    pub fn jog_key(&self, key: JogKey, pressed: bool) -> Result<()> {
        let base = if pressed { 0 } else { 0x10 };
        self.command(&[0xD8, key.code() + base])
    }

    /// Encodes and sends a job; the controller starts it immediately.
    pub fn run_job(&self, layers: &[RdLayer], progress: impl FnMut(usize, usize)) -> Result<()> {
        let commands = encode_job(layers).ok_or(LinkError::Rejected("the job is empty"))?;
        let mut progress = progress;
        self.link.send_stream(&commands, &mut progress)
    }

    pub fn stop(&self) -> Result<()> {
        self.command(&[0xD8, 0x01])
    }

    pub fn pause(&self) -> Result<()> {
        self.command(&[0xD8, 0x02])
    }

    pub fn resume(&self) -> Result<()> {
        self.command(&[0xD8, 0x03])
    }
}

pub fn mm_to_um(mm: f64) -> i64 {
    (mm * 1000.0).round() as i64
}

/// How long the head may stand still short of its target before it's treated as stuck.
const STALL: Duration = Duration::from_secs(3);

/// Speed command then absolute rapid move (see [`Controller::move_to`]), in one packet.
fn move_command(x: f64, y: f64, speed: f64) -> Vec<u8> {
    let mut cmd = vec![0xC9, 0x02];
    cmd.extend_from_slice(&encode_speed(speed.clamp(1.0, 2000.0)));
    cmd.extend_from_slice(&[0xD9, 0x10, 0x00]);
    cmd.extend_from_slice(&encode_coord(mm_to_um(x)));
    cmd.extend_from_slice(&encode_coord(mm_to_um(y)));
    cmd
}

/// Generous time for a move from `a` to `b` at `speed` mm/s: three times the travel time, plus
/// time to accelerate and for slow replies.
pub fn move_time_limit(a: (f64, f64), b: (f64, f64), speed: f64) -> Duration {
    let distance = (b.0 - a.0).hypot(b.1 - a.1);
    Duration::from_secs_f64(3.0 * distance / speed.max(1.0) + 5.0)
}
