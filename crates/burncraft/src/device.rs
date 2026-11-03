//! Background worker that owns the laser connection (Ruida, G-code, galvo or LaserPecker 2).
//!
//! The UI sends [`DeviceCmd`]s; the worker runs them on its own thread, polls
//! machine state, streams G-code jobs, and reports back with [`DeviceEvent`]s.

use bc_galvo::{GalvoLayer, GalvoSettings, MarkSettings};
use bc_grbl::{GcodeOptions, Grbl, GrblLayer, GrblPath};
use bc_ruida::{Controller, RdLayer, RdPath, RuidaModel};
use eframe::egui;
use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(400);
/// How often the G-code stream is pumped while a job runs.
const STREAM_INTERVAL: Duration = Duration::from_millis(5);
/// Consecutive failed polls before the connection is considered lost. Ruida controllers can be
/// slow to answer while the head is moving, so they get longer.
const MAX_POLL_FAILURES: u32 = 3;
const MAX_POLL_FAILURES_RUIDA: u32 = 8;
/// How long a galvo red-light frame keeps tracing (unless stopped).
const GALVO_FRAME_SECONDS: f64 = 15.0;
/// How long a LaserPecker 2 preview keeps tracing (unless stopped).
pub const LP2_PREVIEW: Duration = Duration::from_secs(15);
/// After starting a LaserPecker 2 job, how long it may still report idle before the job is taken
/// to have ended (it reports engraving within about a second).
const LP2_START_GRACE: Duration = Duration::from_secs(10);

/// A job in machine coordinates (mm), independent of controller.
#[derive(Debug, Clone)]
pub struct MachineLayer {
    pub speed: f64,
    pub min_power: f64,
    pub max_power: f64,
    pub air_assist: bool,
    /// Pulse frequency in kHz (galvo).
    pub frequency: f64,
    pub color: [u8; 3],
    /// Raster scan lines (Fill and Image layers), which the LaserPecker 2 takes as a bitmap.
    pub raster: bool,
    /// The layer's pass count. `paths` already repeats each pass.
    pub passes: u32,
    /// Raster line interval (mm), for raster layers.
    pub interval: f64,
    /// Z axis (G-code machines): the layer's offset and per-pass step, in mm.
    pub z_offset: f64,
    pub z_step: f64,
    /// G-code: laser held on at the start and end of each burn (ms).
    pub delay_on: f64,
    pub delay_off: f64,
    /// Polylines with a power fraction (0 = laser off).
    pub paths: Vec<(Vec<(f64, f64)>, f32)>,
}

/// How to reach a Ruida controller.
pub enum RuidaTarget {
    Ethernet(Ipv4Addr),
    Usb(String),
}

pub enum DeviceCmd {
    ConnectRuida { target: RuidaTarget, model: RuidaModel },
    ConnectGcode { port: String, baud: u32, dtr: bool, options: Box<GcodeOptions> },
    ConnectGalvo(Box<GalvoSettings>),
    /// `changed`: the settings were changed while disconnected, so they replace the machine's.
    ConnectLp2 { port: String, settings: bc_lp2::Settings, changed: bool },
    /// LaserPecker 2: write the machine's own settings (beep, touch buttons, work mode…).
    Lp2Settings(bc_lp2::Settings),
    Disconnect,
    Home,
    /// G-code: clear an alarm lock.
    Unlock,
    /// Relative move in machine mm, at `feed` mm/s where the controller supports it.
    JogBy(f64, f64, f64),
    /// Absolute rapid move in machine mm, at `feed` mm/s where supported.
    MoveTo(f64, f64, f64),
    /// G-code: relative Z move in mm (positive raises the laser), at `feed` mm/s.
    JogZ(f64, f64),
    /// Ruida: press (true) or release a keypad direction key; the axis moves while it's held.
    JogKey(bc_ruida::JogKey, bool),
    /// A raw controller command typed in the console (G-code).
    Console(String),
    /// GRBL: read the controller's own settings (`$$`).
    ReadSettings,
    /// GRBL: change settings (`$N=value`), then read them all back.
    WriteSettings(Vec<(u32, String)>),
    /// Trace a rectangle (machine mm) with the laser off, at `feed` mm/s (galvo: red light).
    Frame((f64, f64), (f64, f64), f64),
    /// LaserPecker 2: show a rectangle (mm) with the preview light, in the given way.
    Lp2Frame((f64, f64), (f64, f64), bc_lp2::PreviewKind),
    Run(Vec<MachineLayer>),
    Pause,
    Resume,
    Stop,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MachineStatus {
    pub x: f64,
    pub y: f64,
    pub state: String,
}

pub enum DeviceEvent {
    Connected { description: String, bed: Option<(f64, f64)> },
    Disconnected(Option<String>),
    State(MachineStatus),
    /// Progress of sending (Ruida, galvo) or running (G-code): done, total.
    Progress(usize, usize, &'static str),
    JobStarted,
    JobFinished,
    Error(String),
    /// Console output lines.
    Console(Vec<String>),
    /// GRBL: the controller's settings as read, or why they couldn't be; and the lines the
    /// controller sent, for the console.
    Settings(Result<Vec<(u32, String)>, String>, Vec<String>),
    /// GRBL: how many settings were written, why writing stopped (if it did), and the settings
    /// read back afterwards.
    SettingsWritten { written: usize, error: Option<String>, settings: Option<Vec<(u32, String)>> },
    /// LaserPecker 2: the machine's own settings, read when connecting.
    Lp2Settings(bc_lp2::Settings),
}

/// UI-side handle to the worker, plus the latest known machine state.
pub struct Device {
    tx: Sender<DeviceCmd>,
    rx: Receiver<DeviceEvent>,
    pub connected: bool,
    pub description: String,
    pub state: Option<MachineStatus>,
    pub progress: Option<(usize, usize, &'static str)>,
    pub paused: bool,
    pub message: Option<String>,
    /// Console transcript, newest last.
    pub console: Vec<String>,
    /// Set when a connection is made; cleared by whoever acts on it.
    pub just_connected: bool,
    /// GRBL: the controller's settings as last read, and a count that goes up with each reading.
    pub grbl_settings: Option<Vec<(u32, String)>>,
    pub grbl_settings_read: u64,
    /// GRBL: the outcome of the last settings write.
    pub grbl_settings_note: Option<String>,
    /// LaserPecker 2: the machine's settings as read on connecting, for the app to take on.
    pub lp2_settings: Option<bc_lp2::Settings>,
    /// Jobs that ran to the end (not stopped or failed), counting up from 0.
    pub jobs_completed: u64,
    /// When the current (or last) job started, and when it finished.
    pub job_started: Option<Instant>,
    pub job_finished: Option<Instant>,
    /// Set by Stop so a frame or move in progress on the worker ends at once (the Stop command
    /// itself waits in the queue behind it).
    abort: Arc<AtomicBool>,
}

impl Device {
    /// Stops the machine and ends the current job.
    pub fn stop(&mut self) {
        self.abort.store(true, Ordering::SeqCst);
        self.send(DeviceCmd::Stop);
        self.paused = false;
        if self.job_running() {
            self.job_finished = Some(Instant::now());
        }
    }

    /// Pauses or resumes.
    pub fn set_paused(&mut self, paused: bool) {
        self.send(if paused { DeviceCmd::Pause } else { DeviceCmd::Resume });
        self.paused = paused;
    }

    /// A job has started and hasn't finished, failed or been cut off.
    pub fn job_running(&self) -> bool {
        self.connected && self.job_started.is_some() && self.job_finished.is_none()
    }
}

impl Device {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (tx, worker_rx) = mpsc::channel();
        let (worker_tx, rx) = mpsc::channel();
        let abort = Arc::new(AtomicBool::new(false));
        let worker_abort = abort.clone();
        thread::Builder::new()
            .name("laser".into())
            .spawn(move || Worker { rx: worker_rx, tx: worker_tx, ctx, backend: None, poll_failures: 0, abort: worker_abort, bed: None, last_state: None }.run())
            .expect("spawn laser worker thread");
        Self {
            tx,
            rx,
            connected: false,
            description: String::new(),
            state: None,
            progress: None,
            paused: false,
            message: None,
            console: Vec::new(),
            just_connected: false,
            grbl_settings: None,
            grbl_settings_read: 0,
            grbl_settings_note: None,
            lp2_settings: None,
            jobs_completed: 0,
            job_started: None,
            job_finished: None,
            abort,
        }
    }

    pub fn send(&self, cmd: DeviceCmd) {
        // The worker only exits when this handle is dropped, so sending cannot fail while we exist.
        let _ = self.tx.send(cmd);
    }

    /// Applies pending events. Returns the reported bed size, if a connection was just made.
    pub fn poll(&mut self) -> Option<(f64, f64)> {
        let mut bed = None;
        while let Ok(event) = self.rx.try_recv() {
            match event {
                DeviceEvent::Connected { description, bed: b } => {
                    self.connected = true;
                    self.just_connected = true;
                    self.message = Some(format!("Connected: {description}"));
                    self.description = description;
                    bed = b;
                }
                DeviceEvent::Disconnected(reason) => {
                    if self.job_running() {
                        self.job_finished = Some(Instant::now());
                    }
                    self.connected = false;
                    self.state = None;
                    self.progress = None;
                    self.paused = false;
                    self.message = Some(reason.unwrap_or_else(|| "Disconnected".into()));
                }
                DeviceEvent::State(s) => self.state = Some(s),
                DeviceEvent::Progress(done, total, what) => self.progress = Some((done, total, what)),
                DeviceEvent::JobStarted => {
                    self.paused = false;
                    self.message = Some("Job started".into());
                    self.job_started = Some(Instant::now());
                    self.job_finished = None;
                }
                DeviceEvent::JobFinished => {
                    self.jobs_completed += 1;
                    self.progress = None;
                    self.paused = false;
                    self.message = Some("Job finished".into());
                    self.job_finished = Some(Instant::now());
                }
                DeviceEvent::Console(lines) => {
                    self.console.extend(lines);
                    let excess = self.console.len().saturating_sub(500);
                    self.console.drain(..excess);
                }
                DeviceEvent::Settings(result, lines) => {
                    self.console.push("> $$".into());
                    self.console.extend(lines);
                    match result {
                        Ok(s) => {
                            self.grbl_settings = Some(s);
                            self.grbl_settings_read += 1;
                        }
                        Err(e) => self.grbl_settings_note = Some(e),
                    }
                }
                DeviceEvent::Lp2Settings(s) => self.lp2_settings = Some(s),
                DeviceEvent::SettingsWritten { written, error, settings } => {
                    self.grbl_settings_note = Some(match error {
                        None => format!("Wrote {written} setting{} to the controller", if written == 1 { "" } else { "s" }),
                        Some(e) => format!("Wrote {written}, then stopped at {e}"),
                    });
                    if let Some(s) = settings {
                        self.grbl_settings = Some(s);
                        self.grbl_settings_read += 1;
                    }
                }
                DeviceEvent::Error(e) => {
                    self.progress = None;
                    self.message = Some(e);
                    if self.job_running() {
                        self.job_finished = Some(Instant::now());
                    }
                }
            }
        }
        bed
    }
}

/// A connected galvo, plus where the beam was last pointed (the board doesn't report it).
struct Galvo {
    controller: bc_galvo::Controller,
    at: (f64, f64),
    /// A marking job (not a red-light frame) is running.
    job: bool,
}

/// A connected LaserPecker 2. It takes one file at a time, so a job of several layers (or passes
/// of an engraving) is a queue of files, each sent when the one before has finished.
struct Lp2 {
    controller: bc_lp2::Controller,
    /// Files still to run.
    queue: VecDeque<Lp2File>,
    /// Files in the whole job, and how many have been started.
    files: usize,
    started: usize,
    /// When the running file was started, and whether the machine has reported engraving it.
    running: Option<Instant>,
    engraving: bool,
    /// How far through the running file the job is.
    progress: ShapeProgress,
    preview_until: Option<Instant>,
    /// The machine's settings as last written (roller, preview power).
    settings: bc_lp2::Settings,
    /// The error the machine last reported during this job (reported once).
    reported_error: u8,
}

enum Backend {
    Ruida(Box<Controller>),
    Gcode { grbl: Box<Grbl>, options: Box<GcodeOptions> },
    Galvo(Box<Galvo>),
    Lp2(Box<Lp2>),
}

struct Worker {
    rx: Receiver<DeviceCmd>,
    tx: Sender<DeviceEvent>,
    ctx: egui::Context,
    backend: Option<Backend>,
    poll_failures: u32,
    abort: Arc<AtomicBool>,
    /// Work area reported by the controller (mm), to keep manual moves inside it.
    bed: Option<(f64, f64)>,
    /// The machine state last sent to the window. A state is only sent when it changes, since
    /// each one redraws the window (costly where drawing is done in software).
    last_state: Option<MachineStatus>,
}

impl Worker {
    fn send(&self, e: DeviceEvent) {
        let _ = self.tx.send(e);
        self.ctx.request_repaint();
    }

    fn streaming(&self) -> bool {
        matches!(&self.backend, Some(Backend::Gcode { grbl, .. }) if grbl.is_streaming())
    }

    fn run(mut self) {
        let mut last_poll = Instant::now();
        loop {
            let wait = if self.streaming() { STREAM_INTERVAL } else { POLL_INTERVAL };
            match self.rx.recv_timeout(wait) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if self.streaming() {
                self.pump();
            }
            if last_poll.elapsed() >= POLL_INTERVAL {
                last_poll = Instant::now();
                self.poll_state();
            }
        }
    }

    fn pump(&mut self) {
        let Some(Backend::Gcode { grbl, .. }) = &mut self.backend else { return };
        match grbl.pump() {
            Ok(p) if p.done() => self.send(DeviceEvent::JobFinished),
            Ok(p) => self.send(DeviceEvent::Progress(p.acked, p.total, "Running")),
            Err(e) => self.send(DeviceEvent::Error(format!("Job stopped: {e}"))),
        }
    }

    fn poll_state(&mut self) {
        let mut finished = false;
        let result: Result<Option<MachineStatus>, String> = match &mut self.backend {
            None => return,
            Some(Backend::Ruida(c)) => c.state().map(|s| Some(MachineStatus { x: s.x, y: s.y, state: format!("0x{:X}", s.status) })).map_err(|e| e.to_string()),
            Some(Backend::Gcode { grbl, .. }) => {
                // While streaming, the reply arrives through the stream pump; report the latest known state.
                let asked = if grbl.is_streaming() { grbl.request_status() } else { grbl.query_status(Duration::from_millis(300)).map(|_| ()) };
                asked.map_err(|e| e.to_string()).map(|()| {
                    let position = grbl.work_position();
                    grbl.last_status.as_ref().zip(position).map(|(s, (x, y))| MachineStatus { x, y, state: s.state.clone() })
                })
            }
            Some(Backend::Galvo(g)) => g.controller.poll().map_err(|e| e.to_string()).map(|running| {
                if !running && g.job {
                    g.job = false;
                    finished = true;
                }
                Some(MachineStatus { x: g.at.0, y: g.at.1, state: if running { "Busy" } else { "Idle" }.into() })
            }),
            Some(Backend::Lp2(l)) => lp2_poll(l, &self.tx, &self.ctx).map(|(state, done)| {
                finished = done;
                Some(state)
            }),
        };
        match result {
            Ok(state) => {
                self.poll_failures = 0;
                if let Some(s) = state
                    && self.last_state.as_ref() != Some(&s)
                {
                    self.last_state = Some(s.clone());
                    self.send(DeviceEvent::State(s));
                }
                if finished {
                    self.send(DeviceEvent::JobFinished);
                }
            }
            Err(e) => {
                self.poll_failures += 1;
                let limit = if matches!(self.backend, Some(Backend::Ruida(_))) { MAX_POLL_FAILURES_RUIDA } else { MAX_POLL_FAILURES };
                if self.poll_failures >= limit {
                    // Never leave a head moving on its own: try to stop it before letting go.
                    if let Some(Backend::Ruida(c)) = &self.backend {
                        let _ = c.stop();
                    }
                    self.backend = None;
                    self.send(DeviceEvent::Disconnected(Some(format!("Connection lost: {e}"))));
                }
            }
        }
    }

    fn connected(&mut self, backend: Backend, description: String, bed: Option<(f64, f64)>) {
        self.poll_failures = 0;
        self.last_state = None;
        self.backend = Some(backend);
        self.send(DeviceEvent::Connected { description, bed });
    }

    fn handle(&mut self, cmd: DeviceCmd) {
        match cmd {
            DeviceCmd::ConnectRuida { target, model } => {
                self.backend = None;
                let (result, name) = match &target {
                    RuidaTarget::Ethernet(ip) => (Controller::connect(*ip, model), ip.to_string()),
                    RuidaTarget::Usb(port) => (Controller::connect_usb(port, model), port.clone()),
                };
                match result {
                    Ok(c) => {
                        let bed = c.bed_size().ok().filter(|(w, h)| *w > 0.0 && *h > 0.0);
                        self.bed = bed;
                        self.connected(Backend::Ruida(Box::new(c)), format!("Ruida on {name}"), bed);
                    }
                    Err(e) => self.send(DeviceEvent::Disconnected(Some(format!("Could not connect to {name}: {e}")))),
                }
            }
            DeviceCmd::ConnectGcode { port, baud, dtr, options } => {
                self.backend = None;
                match Grbl::open(&port, baud, options.dialect, options.custom.clone(), dtr) {
                    Ok(grbl) => {
                        let name = format!("{} on {port}", options.dialect.label());
                        self.connected(Backend::Gcode { grbl: Box::new(grbl), options }, name, None);
                    }
                    Err(e) => self.send(DeviceEvent::Disconnected(Some(format!("Could not connect on {port}: {e}")))),
                }
            }
            DeviceCmd::ConnectGalvo(settings) => {
                self.backend = None;
                let lens = settings.lens_size;
                match bc_galvo::Controller::connect(*settings) {
                    Ok(c) => {
                        let centre = (lens / 2.0, lens / 2.0);
                        self.connected(Backend::Galvo(Box::new(Galvo { controller: c, at: centre, job: false })), "Galvo (USB)".into(), Some((lens, lens)));
                    }
                    Err(e) => self.send(DeviceEvent::Disconnected(Some(format!("Could not connect to the galvo: {e}")))),
                }
            }
            DeviceCmd::ConnectLp2 { port, settings, changed } => {
                self.backend = None;
                match bc_lp2::Controller::connect(&port, &settings, changed) {
                    Ok(controller) => {
                        // The machine's own settings, as read: shown in Devices and kept.
                        let settings = controller.settings;
                        let lp2 = Lp2 { controller, queue: VecDeque::new(), files: 0, started: 0, running: None, engraving: false, progress: ShapeProgress::default(), preview_until: None, settings, reported_error: 0 };
                        let field = settings.field();
                        self.bed = Some(field);
                        self.connected(Backend::Lp2(Box::new(lp2)), format!("LaserPecker 2 on {port}"), Some(field));
                        self.send(DeviceEvent::Lp2Settings(settings));
                    }
                    Err(e) => self.send(DeviceEvent::Disconnected(Some(format!("Could not connect on {port}: {e}")))),
                }
            }
            DeviceCmd::Disconnect => {
                self.backend = None;
                self.send(DeviceEvent::Disconnected(None));
            }
            DeviceCmd::Lp2Settings(settings) => {
                let result = match &mut self.backend {
                    Some(Backend::Lp2(l)) if l.running.is_some() => Err("The LaserPecker's settings can't change during a job; they're sent when it next connects".to_string()),
                    Some(Backend::Lp2(l)) => {
                        let written = l.controller.write_settings(&settings).map_err(|e| format!("Couldn't change the LaserPecker's settings: {e}"));
                        if written.is_ok() {
                            l.settings = l.controller.settings;
                        }
                        written
                    }
                    _ => Ok(()),
                };
                if let Err(e) = result {
                    self.send(DeviceEvent::Error(e));
                }
            }
            other => {
                if matches!(other, DeviceCmd::MoveTo(..) | DeviceCmd::JogBy(..) | DeviceCmd::Frame(..)) {
                    self.abort.store(false, Ordering::SeqCst);
                }
                let result = match &mut self.backend {
                    None => Err("Not connected".to_string()),
                    Some(Backend::Ruida(c)) => ruida_command(c, other, &self.tx, &self.ctx, &self.abort, self.bed),
                    Some(Backend::Gcode { grbl, options }) => gcode_command(grbl, options, other),
                    Some(Backend::Galvo(g)) => galvo_command(g, other, &self.tx, &self.ctx),
                    Some(Backend::Lp2(l)) => lp2_command(l, other, &self.tx, &self.ctx),
                };
                match result {
                    Ok(Some(event)) => self.send(event),
                    Ok(None) => {}
                    Err(e) => self.send(DeviceEvent::Error(e)),
                }
            }
        }
    }
}

/// Keeps a manual move inside the controller's work area, if it reported one.
fn clamp_to_bed(bed: Option<(f64, f64)>, (x, y): (f64, f64)) -> (f64, f64) {
    match bed {
        Some((w, h)) => (x.clamp(0.0, w), y.clamp(0.0, h)),
        None => (x, y),
    }
}

fn ruida_command(c: &Controller, cmd: DeviceCmd, tx: &Sender<DeviceEvent>, ctx: &egui::Context, abort: &AtomicBool, bed: Option<(f64, f64)>) -> Result<Option<DeviceEvent>, String> {
    let report = |s: bc_ruida::MachineState| {
        let _ = tx.send(DeviceEvent::State(MachineStatus { x: s.x, y: s.y, state: format!("0x{:X}", s.status) }));
        ctx.request_repaint();
    };
    let stopped = || abort.load(Ordering::SeqCst);
    let r = match cmd {
        DeviceCmd::Home => c.home(),
        DeviceCmd::Unlock => return Ok(None),
        DeviceCmd::JogKey(key, pressed) => c.jog_key(key, pressed),
        DeviceCmd::JogZ(..) => return Err("Hold the Z buttons to move a Ruida's Z axis".into()),
        DeviceCmd::JogBy(dx, dy, speed) => c.state().and_then(|s| {
            let (x, y) = clamp_to_bed(bed, (s.x + dx, s.y + dy));
            c.move_to(x, y, speed)
        }),
        DeviceCmd::MoveTo(x, y, speed) => {
            let (x, y) = clamp_to_bed(bed, (x, y));
            c.move_to(x, y, speed)
        }
        DeviceCmd::Console(_) => return Ok(Some(DeviceEvent::Console(vec!["The console is only available for G-code controllers.".into()]))),
        DeviceCmd::ReadSettings | DeviceCmd::WriteSettings(_) => return Err("Machine settings can only be read from GRBL controllers".into()),
        DeviceCmd::Frame(min, max, speed) => {
            let (min, max) = (clamp_to_bed(bed, min), clamp_to_bed(bed, max));
            return c.frame(min, max, speed, &stopped, report).map(|()| None).map_err(|e| format!("Framing stopped: {e}"));
        }
        DeviceCmd::Pause => c.pause(),
        DeviceCmd::Resume => c.resume(),
        DeviceCmd::Stop => c.stop(),
        DeviceCmd::Run(layers) => {
            let rd: Vec<RdLayer> = layers.iter().map(to_ruida).collect();
            return c
                .run_job(&rd, |sent, total| {
                    let _ = tx.send(DeviceEvent::Progress(sent, total, "Sending"));
                    ctx.request_repaint();
                })
                // The controller runs the job on its own once it has the whole file.
                .map(|()| Some(DeviceEvent::JobStarted))
                .map_err(|e| e.to_string());
        }
        DeviceCmd::Lp2Frame(..) => return Err("Only the LaserPecker 2 has a preview light".into()),
        DeviceCmd::ConnectRuida { .. } | DeviceCmd::ConnectGcode { .. } | DeviceCmd::ConnectGalvo(_) | DeviceCmd::ConnectLp2 { .. } | DeviceCmd::Lp2Settings(_) | DeviceCmd::Disconnect => unreachable!("handled by the worker"),
    };
    r.map(|()| None).map_err(|e| e.to_string())
}

fn gcode_command(g: &mut Grbl, options: &GcodeOptions, cmd: DeviceCmd) -> Result<Option<DeviceEvent>, String> {
    if g.is_streaming() && !matches!(cmd, DeviceCmd::Pause | DeviceCmd::Resume | DeviceCmd::Stop) {
        return Err("A job is running; pause or stop it first".into());
    }
    let r = match cmd {
        DeviceCmd::Home => g.home(),
        DeviceCmd::Unlock => g.unlock(),
        DeviceCmd::JogBy(dx, dy, feed) => g.jog_by(dx, dy, feed * 60.0),
        DeviceCmd::JogZ(dz, feed) => g.jog_z(dz, feed * 60.0),
        DeviceCmd::JogKey(..) => return Ok(None),
        DeviceCmd::MoveTo(x, y, feed) => g.jog_to(x, y, feed * 60.0),
        DeviceCmd::Console(line) => {
            let mut out = vec![format!("> {line}")];
            out.extend(g.console(&line, Duration::from_secs(5)).map_err(|e| e.to_string())?);
            return Ok(Some(DeviceEvent::Console(out)));
        }
        DeviceCmd::ReadSettings => {
            if !options.dialect.is_grbl() {
                return Err("Machine settings can only be read from GRBL controllers".into());
            }
            let (result, lines) = g.read_settings();
            let result = result.map_err(|e| format!("Couldn't read the settings: {e}. What the controller sent is in the Console tab."));
            return Ok(Some(DeviceEvent::Settings(result, lines)));
        }
        DeviceCmd::WriteSettings(list) => {
            let mut written = 0;
            let mut error = None;
            for (id, value) in &list {
                match g.write_setting(*id, value) {
                    Ok(()) => written += 1,
                    Err(e) => {
                        error = Some(format!("${id}={value}: {e}"));
                        break;
                    }
                }
            }
            let settings = g.read_settings().0.ok();
            return Ok(Some(DeviceEvent::SettingsWritten { written, error, settings }));
        }
        DeviceCmd::Frame(min, max, feed) => [min, (max.0, min.1), max, (min.0, max.1), min].into_iter().try_for_each(|(x, y)| g.jog_to(x, y, feed * 60.0)),
        DeviceCmd::Pause => g.pause(),
        DeviceCmd::Resume => g.resume(),
        DeviceCmd::Stop => g.stop(),
        DeviceCmd::Run(layers) => {
            g.start_stream(bc_grbl::generate(&to_gcode_layers(layers), options));
            return Ok(Some(DeviceEvent::JobStarted));
        }
        DeviceCmd::Lp2Frame(..) => return Err("Only the LaserPecker 2 has a preview light".into()),
        DeviceCmd::ConnectRuida { .. } | DeviceCmd::ConnectGcode { .. } | DeviceCmd::ConnectGalvo(_) | DeviceCmd::ConnectLp2 { .. } | DeviceCmd::Lp2Settings(_) | DeviceCmd::Disconnect => unreachable!("handled by the worker"),
    };
    r.map(|()| None).map_err(|e| e.to_string())
}

fn galvo_command(g: &mut Galvo, cmd: DeviceCmd, tx: &Sender<DeviceEvent>, ctx: &egui::Context) -> Result<Option<DeviceEvent>, String> {
    let c = &mut g.controller;
    let r = match cmd {
        // There's no head to home: centre the beam.
        DeviceCmd::Home => {
            let half = c.settings.lens_size / 2.0;
            g.at = (half, half);
            c.goto(g.at)
        }
        DeviceCmd::Unlock => return Ok(None),
        DeviceCmd::JogZ(..) | DeviceCmd::JogKey(..) => return Err("This machine has no Z or U axis to move".into()),
        DeviceCmd::JogBy(dx, dy, _) => {
            g.at = (g.at.0 + dx, g.at.1 + dy);
            c.goto(g.at)
        }
        DeviceCmd::MoveTo(x, y, _) => {
            g.at = (x, y);
            c.goto(g.at)
        }
        DeviceCmd::Console(_) => return Ok(Some(DeviceEvent::Console(vec!["The console is only available for G-code controllers.".into()]))),
        DeviceCmd::ReadSettings | DeviceCmd::WriteSettings(_) => return Err("Machine settings can only be read from GRBL controllers".into()),
        DeviceCmd::Frame(min, max, _) => {
            let ring = [min, (max.0, min.1), max, (min.0, max.1), min];
            let perimeter = 2.0 * ((max.0 - min.0) + (max.1 - min.1));
            let loops = (GALVO_FRAME_SECONDS * c.settings.light_speed / perimeter.max(1.0)).clamp(1.0, 2000.0) as usize;
            g.job = false;
            c.light(&ring, loops)
        }
        DeviceCmd::Pause => c.pause(),
        DeviceCmd::Resume => c.resume(),
        DeviceCmd::Stop => c.abort(),
        DeviceCmd::Run(layers) => {
            let galvo_layers: Vec<GalvoLayer> = layers
                .into_iter()
                .map(|l| GalvoLayer {
                    settings: MarkSettings { speed: l.speed, power: l.max_power, frequency: l.frequency },
                    paths: l.paths.into_iter().map(|(points, power)| (points, power > 0.0)).collect(),
                })
                .collect();
            let skipped = c
                .run_job(&galvo_layers, |sent, total| {
                    let _ = tx.send(DeviceEvent::Progress(sent, total, "Sending"));
                    ctx.request_repaint();
                })
                .map_err(|e| e.to_string())?;
            g.job = true;
            if skipped > 0 {
                return Ok(Some(DeviceEvent::Error(format!("Job started; {skipped} point(s) outside the lens field were left out"))));
            }
            return Ok(Some(DeviceEvent::JobStarted));
        }
        DeviceCmd::Lp2Frame(..) => return Err("Only the LaserPecker 2 has a preview light".into()),
        DeviceCmd::ConnectRuida { .. } | DeviceCmd::ConnectGcode { .. } | DeviceCmd::ConnectGalvo(_) | DeviceCmd::ConnectLp2 { .. } | DeviceCmd::Lp2Settings(_) | DeviceCmd::Disconnect => unreachable!("handled by the worker"),
    };
    r.map(|()| None).map_err(|e| e.to_string())
}

/// A file for the LaserPecker 2, with its power and speed (percent), and for G-code files the
/// length of each burning path in order.
#[derive(Clone)]
struct Lp2File {
    file: bc_lp2::File,
    power: u8,
    speed: u8,
    passes: u16,
    shapes: Vec<f64>,
}

/// Progress through one LaserPecker 2 file. On a G-code file the machine reports how far through
/// the current shape it is, starting from 0 again for each shape; a drop means the next shape has
/// begun. Shapes are weighted by length. Bitmaps report the whole file. Never goes backwards.
#[derive(Debug, Default)]
struct ShapeProgress {
    shapes: Vec<f64>,
    shape: usize,
    last: u8,
    shown: f64,
    /// Passes in the file (`shapes` repeats one pass's shapes this many times).
    passes: usize,
}

impl ShapeProgress {
    fn new(shapes: Vec<f64>, passes: usize) -> Self {
        Self { shapes, passes: passes.max(1), ..Self::default() }
    }

    /// Takes the machine's percentage and the pass it reports running (1, 2…; anything else is
    /// ignored); returns the fraction of the file done (below 1 until it ends).
    fn update(&mut self, percent: u8, pass: u8) -> f64 {
        let p = percent.min(100) as f64 / 100.0;
        let total: f64 = self.shapes.iter().sum();
        // A later pass starts at its first shape, however many drops in progress were missed.
        let per_pass = self.shapes.len() / self.passes;
        if (1..=self.passes).contains(&(pass as usize)) && per_pass > 0 {
            let first = (pass as usize - 1) * per_pass;
            if first > self.shape {
                self.shape = first;
                self.last = 0;
            }
        }
        let fraction = if self.shapes.len() < 2 || total <= 0.0 {
            p
        } else {
            if percent + 10 < self.last {
                self.shape = (self.shape + 1).min(self.shapes.len() - 1);
            }
            (self.shapes[..self.shape].iter().sum::<f64>() + p * self.shapes[self.shape]) / total
        };
        self.last = percent;
        self.shown = self.shown.max(fraction.min(0.99));
        self.shown
    }
}

/// The LaserPecker 2's files for a job: a G-code file per line layer, and a bitmap per engraving
/// layer at the resolution nearest its line interval.
fn lp2_files(layers: &[MachineLayer], roller: bool) -> Result<VecDeque<Lp2File>, String> {
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u32).unwrap_or(1);
    let mut files = VecDeque::new();
    for (i, l) in layers.iter().enumerate() {
        // A fresh id for each file, as the app uses.
        let job = seed.wrapping_mul(2_654_435_761).wrapping_add(i as u32 * 7919) | 1;
        let (power, speed) = (bc_lp2::power_percent(l.max_power), bc_lp2::speed_percent(l.speed));
        // The machine runs the passes itself: send one pass of the paths (the job repeats them).
        let passes = l.passes.clamp(1, u16::MAX as u32) as usize;
        let n = l.paths.len() / passes;
        let repeated = passes > 1 && n > 0 && l.paths.len() % passes == 0 && l.paths.chunks(n).all(|c| c == &l.paths[..n]);
        let one_pass = if repeated { &l.paths[..n] } else { &l.paths[..] };
        let passes = if one_pass.len() < l.paths.len() { passes } else { 1 };
        // Progress starts again for each shape, and is assumed to for each pass.
        let shapes: Vec<f64> = if l.raster || roller {
            vec![1.0; passes]
        } else {
            // The same paths, in the same order, as the G-code file burns.
            let lengths: Vec<f64> = one_pass.iter().filter(|(p, power)| *power > 0.0 && p.len() >= 2).map(|(p, _)| p.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum()).collect();
            lengths.repeat(passes)
        };
        // The LP2 only engraves pictures with the electric roller (the app allows only Picture and
        // Fill there), so line work is sent as a picture of its lines.
        let file = if l.raster || roller {
            bc_lp2::job::image_file(one_pass, job, "BurnCraft", bc_lp2::Resolution::for_interval(l.interval), roller)?
        } else {
            bc_lp2::job::gcode_file(one_pass, job, "BurnCraft")?
        };
        files.push_back(Lp2File { file, power, speed, passes: passes as u16, shapes });
    }
    if files.is_empty() {
        return Err("Nothing to engrave".into());
    }
    if roller && files.len() > 1 {
        return Err("With the electric roller, the LaserPecker 2 engraves one layer at a time. Put the design on one layer, or hide the other layers' shapes.".into());
    }
    Ok(files)
}

/// Uploads and starts the next file in the queue.
fn lp2_next(l: &mut Lp2, tx: &Sender<DeviceEvent>, ctx: &egui::Context) -> Result<(), String> {
    let Some(Lp2File { file, power, speed, passes, shapes }) = l.queue.pop_front() else { return Ok(()) };
    l.started += 1;
    let what = if l.files > 1 { "Sending part" } else { "Sending" };
    l.controller
        .run(&file, power, speed, passes, bc_lp2::OBJECT_DIAMETER, |sent, total| {
            let _ = tx.send(DeviceEvent::Progress(sent, total, what));
            ctx.request_repaint();
        })
        .map_err(|e| format!("Couldn't start the job: {e}"))?;
    l.running = Some(Instant::now());
    l.engraving = false;
    l.progress = ShapeProgress::new(shapes, passes as usize);
    l.reported_error = 0;
    Ok(())
}

/// Polls the LaserPecker 2: ends an expired preview, follows the running file and starts the next
/// one when it ends. Returns the state, and whether the whole job has just finished.
fn lp2_poll(l: &mut Lp2, tx: &Sender<DeviceEvent>, ctx: &egui::Context) -> Result<(MachineStatus, bool), String> {
    if l.preview_until.is_some_and(|t| Instant::now() >= t) {
        l.preview_until = None;
        l.controller.idle().map_err(|e| e.to_string())?;
    }
    let s = l.controller.query_status().map_err(|e| e.to_string())?;
    let mut finished = false;
    if let Some(since) = l.running {
        match s.state {
            bc_lp2::State::Engraving => {
                l.engraving = true;
                let done = ((l.started - 1) as f64 + l.progress.update(s.progress, s.pass)) * 1000.0;
                let _ = tx.send(DeviceEvent::Progress(done as usize, l.files * 1000, "Engraving"));
                ctx.request_repaint();
            }
            bc_lp2::State::Idle if l.engraving || since.elapsed() > LP2_START_GRACE => {
                l.running = None;
                if l.queue.is_empty() {
                    finished = true;
                } else {
                    lp2_next(l, tx, ctx)?;
                }
            }
            _ => {}
        }
        // Like the LP2 app: error 3 is the laser's temperature alarm, and mode 5 a failed job.
        // Other values in the error byte turn up during normal jobs and mean nothing to the app.
        let alarm = match (s.error, s.state) {
            (3, _) => Some("The LaserPecker's laser is too hot (temperature alarm). Let it cool down."),
            (_, bc_lp2::State::Other(5)) => Some("The LaserPecker reported that the job failed."),
            _ => None,
        };
        let code = if s.error == 3 { 3 } else { 5 };
        if let Some(text) = alarm
            && l.reported_error != code
        {
            l.reported_error = code;
            let _ = tx.send(DeviceEvent::Console(vec![text.to_string()]));
            let _ = tx.send(DeviceEvent::Error(text.to_string()));
            ctx.request_repaint();
        }
    }
    let state = match s.state {
        bc_lp2::State::Idle => "Idle".to_string(),
        bc_lp2::State::Engraving => format!("Engraving {}%", s.progress.min(100)),
        bc_lp2::State::Preview => "Preview".to_string(),
        bc_lp2::State::Other(n) => format!("State 0x{n:02X}"),
    };
    // There's no head to show: put the marker in the middle of the field.
    Ok((MachineStatus { x: bc_lp2::FIELD.0 / 2.0, y: bc_lp2::FIELD.0 / 2.0, state }, finished))
}

fn lp2_command(l: &mut Lp2, cmd: DeviceCmd, tx: &Sender<DeviceEvent>, ctx: &egui::Context) -> Result<Option<DeviceEvent>, String> {
    let busy = l.running.is_some() && !matches!(cmd, DeviceCmd::Stop | DeviceCmd::Pause | DeviceCmd::Resume);
    if busy {
        return Err("A job is running; stop it first".into());
    }
    let r = match cmd {
        DeviceCmd::Home | DeviceCmd::JogBy(..) | DeviceCmd::MoveTo(..) | DeviceCmd::JogZ(..) | DeviceCmd::JogKey(..) => return Err("The LaserPecker 2 has no head to move. Use Frame to see where the job will go.".into()),
        DeviceCmd::Unlock => return Ok(None),
        DeviceCmd::Console(_) => return Ok(Some(DeviceEvent::Console(vec!["The console is only available for G-code controllers.".into()]))),
        DeviceCmd::ReadSettings | DeviceCmd::WriteSettings(_) => return Err("Machine settings can only be read from GRBL controllers".into()),
        DeviceCmd::Pause | DeviceCmd::Resume if l.running.is_none() => return Err("No job is running".into()),
        DeviceCmd::Pause => l.controller.job_state(bc_lp2::JobState::Hold).map_err(|e| e.to_string()),
        DeviceCmd::Resume => l.controller.job_state(bc_lp2::JobState::Continue).map_err(|e| e.to_string()),
        DeviceCmd::Frame(min, max, _) => lp2_command(l, DeviceCmd::Lp2Frame(min, max, bc_lp2::PreviewKind::Rectangle), tx, ctx).map(|_| ()),
        DeviceCmd::Lp2Frame(min, max, kind) => {
            l.controller.preview(kind, min, max, l.settings.preview_power, bc_lp2::OBJECT_DIAMETER).map_err(|e| format!("Couldn't preview: {e}"))?;
            l.preview_until = Some(Instant::now() + LP2_PREVIEW);
            return Ok(None);
        }
        DeviceCmd::Stop => {
            let was_running = l.running.take().is_some();
            l.queue.clear();
            l.preview_until = None;
            if was_running {
                // End the job, as the LP2 app does, then leave whatever mode it's in.
                l.controller.job_state(bc_lp2::JobState::End).map_err(|e| e.to_string())?;
            }
            l.controller.idle().map_err(|e| e.to_string())?;
            if was_running {
                return Ok(Some(DeviceEvent::Error("Job stopped".into())));
            }
            Ok(())
        }
        DeviceCmd::Run(layers) => {
            // Like the LP2 app, only a warning: the LP2 doesn't always report the roller as connected.
            if l.settings.roller() && !l.controller.query_status().map_err(|e| e.to_string())?.roller_connected {
                let _ = tx.send(DeviceEvent::Console(vec!["The electric roller is switched on in Devices, but the LaserPecker doesn't report it as connected. Check it's plugged in.".into()]));
            }
            l.queue = lp2_files(&layers, l.settings.roller())?;
            l.files = l.queue.len();
            l.started = 0;
            l.preview_until = None;
            lp2_next(l, tx, ctx)?;
            return Ok(Some(DeviceEvent::JobStarted));
        }
        DeviceCmd::ConnectRuida { .. } | DeviceCmd::ConnectGcode { .. } | DeviceCmd::ConnectGalvo(_) | DeviceCmd::ConnectLp2 { .. } | DeviceCmd::Lp2Settings(_) | DeviceCmd::Disconnect => unreachable!("handled by the worker"),
    };
    r.map(|()| None)
}

/// G-code layers for streaming or saving to a file.
pub fn to_gcode_layers(layers: Vec<MachineLayer>) -> Vec<GrblLayer> {
    layers
        .into_iter()
        .map(|l| GrblLayer {
            speed: l.speed,
            min_power: l.min_power,
            max_power: l.max_power,
            air_assist: l.air_assist,
            passes: l.passes,
            z_offset: l.z_offset,
            z_step: l.z_step,
            delay_on: l.delay_on,
            delay_off: l.delay_off,
            paths: l.paths.into_iter().map(|(points, power)| GrblPath { points, power }).collect(),
        })
        .collect()
}

fn to_ruida(l: &MachineLayer) -> RdLayer {
    let um = |v: f64| bc_ruida::controller::mm_to_um(v);
    RdLayer {
        speed: l.speed,
        min_power: l.min_power,
        max_power: l.max_power,
        color: l.color,
        air_assist: l.air_assist,
        paths: l.paths.iter().map(|(pts, power)| RdPath { points: pts.iter().map(|&(x, y)| (um(x), um(y))).collect(), laser: *power > 0.0 }).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(raster: bool, passes: u32, interval: f64) -> MachineLayer {
        let ring = vec![(10.0, 10.0), (20.0, 10.0), (20.0, 15.0), (10.0, 10.0)];
        let line = vec![(10.0, 12.0), (20.0, 12.0)];
        let one: Vec<(Vec<(f64, f64)>, f32)> = vec![(ring, 1.0), (line, 1.0)];
        MachineLayer { speed: 300.0, min_power: 0.0, max_power: 50.0, air_assist: false, frequency: 30.0, color: [0; 3], raster, passes, interval, z_offset: 0.0, z_step: 0.0, delay_on: 0.0, delay_off: 0.0, paths: (0..passes).flat_map(|_| one.clone()).collect() }
    }

    #[test]
    fn lp2_jobs_leave_the_passes_and_resolution_to_the_machine() {
        let files = lp2_files(&[layer(false, 2, 0.1), layer(true, 3, 0.05)], false).unwrap();
        assert_eq!(files.len(), 2);
        let (lines, image) = (&files[0], &files[1]);
        assert_eq!(lines.passes, 2);
        let gcode = String::from_utf8_lossy(&lines.file.bytes[64..]).to_string();
        assert_eq!(gcode.matches("M03").count(), 2, "one pass of the two shapes: {gcode}");
        assert_eq!(lines.shapes.len(), 4, "progress follows both shapes, twice");
        assert_eq!(image.passes, 3);
        assert_eq!(image.file.bytes[9], 0x02, "0.05 mm lines go out at 2K");
        // Paths that don't simply repeat are sent as they are, once.
        let mut odd = layer(false, 2, 0.1);
        odd.paths.pop();
        assert_eq!(lp2_files(&[odd], false).unwrap()[0].passes, 1);
        // With the roller, lines go as a picture, and only one layer at a time.
        let roller = lp2_files(&[layer(false, 2, 0.1)], true).unwrap();
        assert_eq!(roller[0].file.kind, bc_lp2::Kind::Image);
        assert_eq!(roller[0].file.bytes[21], 1, "marked for the roller");
        assert!(lp2_files(&[layer(false, 2, 0.1), layer(true, 3, 0.05)], true).is_err());
    }

    #[test]
    fn lp2_progress_runs_through_the_shapes() {
        // Three shapes, the middle one twice as long; the machine counts 0-100 % for each.
        let mut p = ShapeProgress::new(vec![10.0, 20.0, 10.0], 1);
        assert!((p.update(50, 1) - 0.125).abs() < 1e-9);
        assert!((p.update(100, 1) - 0.25).abs() < 1e-9);
        assert!((p.update(25, 1) - 0.375).abs() < 1e-9, "a drop means the second shape has begun");
        assert!((p.update(90, 1) - 0.7).abs() < 1e-9);
        assert!((p.update(40, 1) - 0.85).abs() < 1e-9);
        assert_eq!(p.update(100, 1), 0.99, "short of done until the machine says it's finished");
        // A drop past the last shape, or a small wobble, never sends the bar backwards.
        assert_eq!(p.update(5, 1), 0.99);
        // A bitmap reports the whole file.
        let mut image = ShapeProgress::new(Vec::new(), 1);
        assert_eq!(image.update(42, 1), 0.42);
        assert_eq!(image.update(40, 1), 0.42);
        // Two passes of two equal shapes: the pass the machine reports puts the bar at its start,
        // even if the drop between passes was missed.
        let mut two = ShapeProgress::new(vec![1.0, 1.0, 1.0, 1.0], 2);
        assert!((two.update(50, 1) - 0.125).abs() < 1e-9);
        assert!((two.update(30, 2) - 0.575).abs() < 1e-9, "pass 2, first shape, 30%");
        // A pass number that makes no sense is ignored.
        assert!((two.update(40, 9) - 0.6).abs() < 1e-9);
    }
}
