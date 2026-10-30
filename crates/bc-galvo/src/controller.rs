//! Galvo machine control: initialisation, jobs, red-light framing, pause/resume/stop.

use crate::job::{encode_job, encode_light, GalvoLayer, GalvoSettings, Source};
use crate::protocol::*;
use crate::usb::{RusbIo, UsbError, UsbIo};
use std::time::{Duration, Instant};

/// Port bit that enables the laser output, and the one that turns on the red pointer.
const PORT_LASER: u16 = 0x0001;
const PORT_LIGHT: u16 = 0x0100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GalvoStatus {
    pub busy: bool,
    pub ready: bool,
    pub paused: bool,
}

pub struct Controller {
    io: Box<dyn UsbIo>,
    pub settings: GalvoSettings,
    port: u16,
    /// A job (or light loop) has been started and not yet cleaned up.
    running: bool,
    pub serial: Option<[u16; 4]>,
}

pub type Result<T> = std::result::Result<T, UsbError>;

impl Controller {
    /// Opens the first board on USB and initialises it.
    pub fn connect(settings: GalvoSettings) -> Result<Self> {
        Self::with_io(Box::new(RusbIo::open(0)?), settings)
    }

    pub fn with_io(io: Box<dyn UsbIo>, settings: GalvoSettings) -> Result<Self> {
        let mut c = Self { io, settings, port: 0, running: false, serial: None };
        c.init()?;
        Ok(c)
    }

    /// Sends one immediate command and returns the four reply words.
    fn single(&mut self, op: u16, params: &[u16]) -> Result<[u16; 4]> {
        self.io.write(&encode(&command(op, params)))?;
        let r = self.io.read()?;
        Ok([0, 1, 2, 3].map(|i| u16::from_le_bytes([r[i * 2], r[i * 2 + 1]])))
    }

    fn init(&mut self) -> Result<()> {
        self.serial = Some(self.single(GET_SERIAL_NO, &[])?);
        self.single(GET_VERSION, &[])?;
        self.single(RESET, &[1])?;
        // Identity correction table (65 × 65 zero offsets).
        self.single(WRITE_COR_TABLE, &[1])?;
        for i in 0..65 * 65 {
            self.single(WRITE_COR_LINE, &[0, 0, if i == 0 { 0 } else { 1 }])?;
        }
        let s = self.settings.clone();
        self.single(ENABLE_LASER, &[])?;
        self.single(SET_CONTROL_MODE, &[s.control_mode])?;
        self.single(SET_LASER_MODE, &[s.laser_mode])?;
        self.single(SET_DELAY_MODE, &[s.delay_mode])?;
        self.single(SET_TIMING, &[s.timing_mode])?;
        self.single(SET_STANDBY, &[s.standby_1, s.standby_2])?;
        self.single(SET_FIRST_PULSE_KILLER, &[s.first_pulse_killer])?;
        self.single(SET_PWM_HALF_PERIOD, &[s.pwm_half_period])?;
        self.single(SET_PWM_PULSE_WIDTH, &[s.pwm_pulse_width])?;
        self.single(FIBER_SET_MO, &[0])?;
        self.single(SET_FPK_PARAM_2, &s.fpk2)?;
        self.single(SET_FLY_RES, &s.fly_res)?;
        self.single(ENABLE_Z, &[])?;
        self.single(WRITE_ANALOG_PORT_1, &[0x07FF])?;
        self.single(ENABLE_Z, &[])?;
        self.port = 0;
        self.single(WRITE_PORT, &[self.port])?;
        Ok(())
    }

    pub fn status(&mut self) -> Result<GalvoStatus> {
        let v = self.single(GET_VERSION, &[])?;
        let l = self.single(GET_LIST_STATUS, &[])?;
        Ok(GalvoStatus { busy: v[3] & STATUS_BUSY != 0, ready: v[3] & STATUS_READY != 0, paused: l[3] & 0x08 != 0 })
    }

    /// Waits (up to a minute) until the board can take another list packet.
    fn wait_ready(&mut self) -> Result<()> {
        let start = Instant::now();
        loop {
            let v = self.single(GET_VERSION, &[])?;
            if v[3] & STATUS_READY != 0 {
                return Ok(());
            }
            if start.elapsed() > Duration::from_secs(60) {
                return Err(UsbError::Usb("controller not ready".into()));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Uploads a list and starts it. Execution begins once more than two packets are queued,
    /// or after the last one, so the board never runs dry while we're still sending.
    fn run_list(&mut self, port: u16, commands: &[Command], progress: &mut dyn FnMut(usize, usize)) -> Result<()> {
        if self.running {
            self.abort()?;
        }
        self.single(RESET_LIST, &[])?;
        self.port = port;
        self.single(WRITE_PORT, &[self.port])?;
        let packets = list_packets(commands);
        let mut executing = false;
        for (i, p) in packets.iter().enumerate() {
            self.wait_ready()?;
            self.io.write(p)?;
            progress(i + 1, packets.len());
            if i + 1 > 2 && !executing {
                self.single(EXECUTE_LIST, &[])?;
                executing = true;
            }
        }
        self.single(SET_END_OF_LIST, &[0])?;
        if !executing {
            self.single(EXECUTE_LIST, &[])?;
        }
        self.running = true;
        Ok(())
    }

    /// Sends a job and starts it. Returns how many points were outside the field and skipped.
    /// Call [`Controller::poll`] afterwards to detect the end of the job.
    pub fn run_job(&mut self, layers: &[GalvoLayer], mut progress: impl FnMut(usize, usize)) -> Result<usize> {
        let port = (self.port | PORT_LASER) & !PORT_LIGHT;
        let (commands, skipped) = encode_job(&self.settings, port, layers);
        if !commands.iter().any(|c| c[0] == LIST_MARK_TO) {
            return Err(UsbError::Usb("the job is empty".into()));
        }
        if self.settings.source == Source::Fiber {
            // Master oscillator on; `poll` turns it off when the job ends.
            self.single(FIBER_SET_MO, &[1])?;
        }
        self.run_list(port, &commands, &mut progress)?;
        Ok(skipped)
    }

    /// Traces `outline` (field mm) with the red pointer `loops` times.
    pub fn light(&mut self, outline: &[(f64, f64)], loops: usize) -> Result<()> {
        let port = (self.port & !PORT_LASER) | PORT_LIGHT;
        let commands = encode_light(&self.settings, port, outline, loops.max(1));
        self.run_list(port, &commands, &mut |_, _| {})
    }

    /// Checks on a running job; when it has finished, turns the source off. Returns `true` while running.
    pub fn poll(&mut self) -> Result<bool> {
        if !self.running {
            return Ok(false);
        }
        let s = self.status()?;
        if s.busy || s.paused {
            return Ok(true);
        }
        self.finish()?;
        Ok(false)
    }

    fn finish(&mut self) -> Result<()> {
        self.running = false;
        self.single(FIBER_SET_MO, &[0])?;
        self.port &= !(PORT_LASER | PORT_LIGHT);
        self.single(WRITE_PORT, &[self.port])?;
        Ok(())
    }

    pub fn pause(&mut self) -> Result<()> {
        self.single(STOP_LIST, &[]).map(|_| ())
    }

    pub fn resume(&mut self) -> Result<()> {
        self.single(RESTART_LIST, &[]).map(|_| ())
    }

    /// Stops immediately and turns the source and pointer off.
    pub fn abort(&mut self) -> Result<()> {
        self.single(STOP_EXECUTE, &[])?;
        self.single(RESET_LIST, &[])?;
        self.finish()
    }

    /// Points the galvo at (x, y) field mm with the laser off.
    pub fn goto(&mut self, p: (f64, f64)) -> Result<()> {
        let (x, y) = self.settings.to_galvo(p).ok_or_else(|| UsbError::Usb("point outside the field".into()))?;
        self.single(GOTO_XY, &[x, y]).map(|_| ())
    }

    pub fn red_pointer(&mut self, on: bool) -> Result<()> {
        if on {
            self.port |= PORT_LIGHT;
        } else {
            self.port &= !PORT_LIGHT;
        }
        self.single(WRITE_PORT, &[self.port]).map(|_| ())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::job::MarkSettings;
    use std::sync::{Arc, Mutex};

    /// A simulated board: records writes; reports READY, and BUSY while `busy_polls` > 0.
    #[derive(Clone, Default)]
    pub struct FakeBoard {
        pub writes: Arc<Mutex<Vec<Vec<u8>>>>,
        pub busy_polls: Arc<Mutex<usize>>,
    }

    impl UsbIo for FakeBoard {
        fn write(&mut self, data: &[u8]) -> Result<()> {
            self.writes.lock().unwrap().push(data.to_vec());
            Ok(())
        }

        fn read(&mut self) -> Result<[u8; 8]> {
            let last = self.writes.lock().unwrap().last().cloned().unwrap_or_default();
            let mut status = STATUS_READY;
            if last.len() == 12 && last[..2] == GET_VERSION.to_le_bytes() {
                let mut b = self.busy_polls.lock().unwrap();
                if *b > 0 {
                    *b -= 1;
                    status |= STATUS_BUSY;
                }
            }
            let mut r = [0u8; 8];
            r[6..8].copy_from_slice(&status.to_le_bytes());
            Ok(r)
        }
    }

    fn ops(board: &FakeBoard) -> Vec<u16> {
        board.writes.lock().unwrap().iter().filter(|w| w.len() == 12).map(|w| u16::from_le_bytes([w[0], w[1]])).collect()
    }

    #[test]
    fn init_writes_identity_correction_table() {
        let board = FakeBoard::default();
        Controller::with_io(Box::new(board.clone()), GalvoSettings::default()).unwrap();
        let ops = ops(&board);
        assert_eq!(ops.iter().filter(|&&o| o == WRITE_COR_LINE).count(), 65 * 65);
        assert!(ops.contains(&ENABLE_LASER) && ops.contains(&SET_FPK_PARAM_2));
    }

    #[test]
    fn job_runs_and_cleans_up() {
        let board = FakeBoard::default();
        let mut c = Controller::with_io(Box::new(board.clone()), GalvoSettings::default()).unwrap();
        board.writes.lock().unwrap().clear();
        // Long enough for several packets.
        let points: Vec<(f64, f64)> = (0..2000).map(|i| (10.0 + (i % 50) as f64, 10.0 + (i / 50) as f64)).collect();
        let layer = GalvoLayer { settings: MarkSettings { speed: 500.0, power: 30.0, frequency: 30.0 }, paths: vec![(points, true)] };
        let mut seen = 0;
        c.run_job(&[layer], |done, _| seen = done).unwrap();
        let writes = board.writes.lock().unwrap().clone();
        let lists: Vec<&Vec<u8>> = writes.iter().filter(|w| w.len() == LIST_BYTES).collect();
        assert!(lists.len() > 3);
        assert_eq!(seen, lists.len());
        // Execution starts once the third packet is queued, exactly once.
        let execs: Vec<usize> = writes.iter().enumerate().filter(|(_, w)| w.len() == 12 && w[..2] == EXECUTE_LIST.to_le_bytes()).map(|(i, _)| i).collect();
        assert_eq!(execs.len(), 1);
        let third = writes.iter().enumerate().filter(|(_, w)| w.len() == LIST_BYTES).nth(2).unwrap().0;
        assert!(execs[0] > third);
        // Busy for two polls, then finished: MO off and port cleared.
        *board.busy_polls.lock().unwrap() = 2;
        assert!(c.poll().unwrap());
        assert!(c.poll().unwrap());
        assert!(!c.poll().unwrap());
        let tail = ops(&board);
        assert_eq!(&tail[tail.len() - 2..], &[FIBER_SET_MO, WRITE_PORT]);
        assert_eq!(c.port, 0);
    }

    #[test]
    fn light_uses_pointer_port() {
        let board = FakeBoard::default();
        let mut c = Controller::with_io(Box::new(board.clone()), GalvoSettings::default()).unwrap();
        c.light(&[(10.0, 10.0), (20.0, 10.0), (20.0, 20.0), (10.0, 10.0)], 2).unwrap();
        assert_eq!(c.port & PORT_LIGHT, PORT_LIGHT);
        assert_eq!(c.port & PORT_LASER, 0);
        c.abort().unwrap();
        assert_eq!(c.port, 0);
    }
}
