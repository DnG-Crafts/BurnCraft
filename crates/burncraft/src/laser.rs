//! Laser control: the Laser panel, Move and Console tabs, the Devices window, and job submission.

use crate::device::{Device, DeviceCmd, MachineLayer, RuidaTarget};
use crate::device_wizard::{self, Finder, FinderOutcome, Wizard, WizardOutcome};
use crate::units::{Speed, SpeedUnit, Units};
use bc_doc::Layer;
use bc_galvo::{GalvoSettings, Source};
use bc_geom::Point;
use bc_grbl::{CustomGcode, Dialect, GcodeOptions};
use bc_job::{BuildOptions, Job, MachineConfig, OriginCorner};
use bc_ruida::RuidaModel;
use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

/// Controller families. G-code covers GRBL, Marlin, Smoothieware and custom dialects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ControllerKind {
    #[default]
    Ruida,
    /// Serialised as "Grbl" for settings saved by earlier versions.
    Grbl,
    Galvo,
    /// LaserPecker 2, which has its own USB protocol (other LaserPecker models speak G-code).
    Lp2,
}

impl ControllerKind {
    pub const ALL: [ControllerKind; 4] = [ControllerKind::Ruida, ControllerKind::Grbl, ControllerKind::Galvo, ControllerKind::Lp2];

    pub fn label(self) -> &'static str {
        match self {
            ControllerKind::Ruida => "Ruida (DSP)",
            ControllerKind::Grbl => "G-code (GRBL, Marlin, Smoothieware…)",
            ControllerKind::Galvo => "Galvo (BJJCZ / EZCad2 board)",
            ControllerKind::Lp2 => "LaserPecker 2",
        }
    }

    /// The usual origin corner for machines with this controller.
    pub fn default_origin(self) -> OriginCorner {
        match self {
            ControllerKind::Ruida => OriginCorner::TopRight,
            ControllerKind::Grbl => OriginCorner::BottomLeft,
            ControllerKind::Galvo | ControllerKind::Lp2 => OriginCorner::TopLeft,
        }
    }

    /// The usual speed unit: per minute for G-code machines (their feed rates are per minute),
    /// per second for the rest.
    /// A fixed marking field (mm), for machines whose work area can't be changed.
    pub fn fixed_field(self) -> Option<(f64, f64)> {
        (self == ControllerKind::Lp2).then_some(bc_lp2::FIELD)
    }

    pub fn default_speed_unit(self) -> SpeedUnit {
        match self {
            ControllerKind::Grbl => SpeedUnit::PerMinute,
            ControllerKind::Ruida | ControllerKind::Galvo | ControllerKind::Lp2 => SpeedUnit::PerSecond,
        }
    }
}

/// Machine settings, remembered between runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LaserSettings {
    pub controller: ControllerKind,
    pub ruida_model: RuidaModel,
    /// Connect to the Ruida over USB (its serial port) instead of Ethernet.
    pub ruida_usb: bool,
    pub ip: String,
    pub serial_port: String,
    pub baud: u32,
    /// G-code: the S value for full power (`$30` on GRBL).
    pub s_max: f64,
    pub dialect: Dialect,
    pub custom_gcode: CustomGcode,
    pub galvo: GalvoSettings,
    pub origin: OriginCorner,
    /// Rapid speed in mm/s, for time estimates, jogging and framing.
    pub rapid_speed: f64,
    pub jog_step: f64,
    pub cut_selected: bool,
    /// Power (%) used by "Frame with laser on" (outline traced at low power).
    pub frame_power: f64,
    /// Work area in mm (new designs use it; a Ruida or galvo reports its own on connect).
    pub bed: (f64, f64),
    /// G-code: lines sent at the start and end of every job.
    pub start_gcode: String,
    pub end_gcode: String,
    /// G-code: air assist with M7 instead of M8.
    pub air_m7: bool,
    /// G-code: raise DTR when opening the port (resets Arduino-style boards).
    pub dtr: bool,
    /// Home the machine as soon as it connects.
    pub home_on_connect: bool,
    /// One-click command buttons (G-code machines).
    pub macros: Vec<Macro>,
    pub rotary: Rotary,
    /// Where jobs run: where they sit on the bed, or relative to the head or a saved origin.
    pub start_from: StartFrom,
    /// Which point of the job goes to the head / user origin (0..9, row by row from top-left).
    pub job_origin: usize,
    /// The saved user origin, in machine mm.
    pub user_origin: Option<(f64, f64)>,
    /// How speeds are shown for this machine (stored in mm/s regardless).
    pub speed_unit: SpeedUnit,
}

/// Where a job is placed on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StartFrom {
    /// Exactly where it sits on the bed in the design.
    #[default]
    Absolute,
    /// With its job origin point at the laser head's current position.
    CurrentPosition,
    /// With its job origin point at the saved user origin.
    UserOrigin,
}

impl StartFrom {
    pub const ALL: [StartFrom; 3] = [StartFrom::Absolute, StartFrom::CurrentPosition, StartFrom::UserOrigin];

    pub fn label(self) -> &'static str {
        match self {
            StartFrom::Absolute => "Absolute Coords",
            StartFrom::CurrentPosition => "Current Position",
            StartFrom::UserOrigin => "User Origin",
        }
    }
}

/// A compact 3 × 3 picker for an origin point (0..9, row by row from the top-left).
fn origin_picker(ui: &mut egui::Ui, value: &mut usize, enabled: bool) {
    const CELL: f32 = 11.0;
    const GAP: f32 = 4.0;
    let side = 3.0 * CELL + 2.0 * GAP;
    let (block, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let ink = if enabled { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
    for i in 0..9 {
        let (row, col) = ((i / 3) as f32, (i % 3) as f32);
        let cell = egui::Rect::from_min_size(block.min + egui::vec2(col * (CELL + GAP), row * (CELL + GAP)), egui::vec2(CELL, CELL));
        let r = ui.interact(cell, ui.id().with(("job-origin", i)), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
        if i == *value {
            let fill = if enabled { ui.visuals().selection.stroke.color } else { ink };
            ui.painter().circle_filled(cell.center(), 4.0, fill);
        } else {
            let stroke = if r.hovered() && enabled { ink } else { ink.gamma_multiply(0.6) };
            ui.painter().circle_stroke(cell.center(), 3.5, egui::Stroke::new(1.0, stroke));
        }
        if r.on_hover_text("The point of the job that goes to the head or user origin").clicked() {
            *value = i;
        }
    }
}

/// The point of `bounds` picked by a 3 × 3 origin index (row by row from the top-left).
fn origin_point(min: Point, max: Point, index: usize) -> Point {
    let f = [0.0, 0.5, 1.0];
    Point::new(min.x + (max.x - min.x) * f[index % 3], min.y + (max.y - min.y) * f[(index / 3).min(2)])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RotaryKind {
    /// The object sits on driven rollers.
    #[default]
    Roller,
    /// The object is clamped in a chuck that turns it.
    Chuck,
}

/// A rotary attachment in place of the Y axis (or on a 4th "A" axis).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rotary {
    pub enabled: bool,
    pub kind: RotaryKind,
    /// How far the controller moves "Y" for one full turn of the rotary (mm). Calibrate with "Rotate once".
    pub travel_per_rotation: f64,
    /// Diameter of the driven roller (roller type), mm.
    pub roller_diameter: f64,
    /// Diameter of the object (chuck type), mm.
    pub object_diameter: f64,
    /// G-code: the rotary is a 4th axis, so send A instead of Y.
    pub a_axis: bool,
}

impl Default for Rotary {
    fn default() -> Self {
        Self { enabled: false, kind: RotaryKind::Roller, travel_per_rotation: 100.0, roller_diameter: 30.0, object_diameter: 80.0, a_axis: false }
    }
}

impl Rotary {
    /// Surface length of one turn (mm): the roller's circumference for rollers, the object's for chucks.
    pub fn surface_per_turn(&self) -> f64 {
        std::f64::consts::PI * match self.kind {
            RotaryKind::Roller => self.roller_diameter,
            RotaryKind::Chuck => self.object_diameter,
        }
    }

    /// Machine "Y" mm per design mm around the object.
    pub fn scale(&self) -> f64 {
        self.travel_per_rotation / self.surface_per_turn().max(0.01)
    }
}

/// A labelled button that sends G-code lines to the machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Macro {
    pub label: String,
    pub gcode: String,
}

/// A named machine in the device list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceProfile {
    pub name: String,
    pub settings: LaserSettings,
}

/// The device list as saved between runs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceStore {
    pub profiles: Vec<DeviceProfile>,
    pub active: usize,
}

/// BurnCraft's own device file (`.bcdev`): one or more machines exactly as BurnCraft keeps them,
/// as JSON, for moving machines between computers or keeping a backup.
pub const DEVICE_EXTENSION: &str = "bcdev";
const DEVICE_FORMAT: &str = "BurnCraft device file";

#[derive(Serialize, Deserialize)]
struct DeviceFile {
    format: String,
    version: u32,
    /// The BurnCraft version that wrote it.
    written_by: String,
    devices: Vec<DeviceProfile>,
}

/// A device file holding `profiles`.
pub fn device_file(profiles: &[DeviceProfile]) -> String {
    let file = DeviceFile { format: DEVICE_FORMAT.into(), version: 1, written_by: format!("BurnCraft {}", env!("CARGO_PKG_VERSION")), devices: profiles.to_vec() };
    serde_json::to_string_pretty(&file).expect("device profiles serialize")
}

/// Machines from a device file: BurnCraft's `.bcdev`, a LightBurn `.lbdev`, or a LightBurn
/// `.lbzip` bundle (every device file inside). Returns the machines and notes about any that
/// couldn't be used.
pub fn devices_from_file(file_name: &str, bytes: &[u8]) -> Result<(Vec<DeviceProfile>, Vec<String>), String> {
    let ext = std::path::Path::new(file_name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let is_zip = bytes.starts_with(b"PK");
    let mut problems = Vec::new();
    let mut from_lightburn = |data: &[u8], out: &mut Vec<DeviceProfile>| -> Result<(), String> {
        for d in bc_io::lbdev::parse(data)? {
            match DeviceProfile::from_lbdev(&d) {
                Ok(p) => out.push(p),
                Err(e) => problems.push(e),
            }
        }
        Ok(())
    };
    let mut devices = Vec::new();
    if ext == bc_io::lbzip::EXTENSION || is_zip {
        for (_, data) in bc_io::lbzip::device_files(bytes)? {
            from_lightburn(&data, &mut devices)?;
        }
    } else if ext == DEVICE_EXTENSION || (ext != bc_io::lbdev::EXTENSION && serde_json::from_slice::<DeviceFile>(bytes).is_ok()) {
        let file: DeviceFile = serde_json::from_slice(bytes).map_err(|e| format!("not a BurnCraft device file: {e}"))?;
        if file.format != DEVICE_FORMAT {
            return Err("not a BurnCraft device file".into());
        }
        devices = file.devices;
    } else {
        from_lightburn(bytes, &mut devices)?;
    }
    Ok((devices, problems))
}

impl DeviceProfile {
    /// Converts a LightBurn device-file entry.
    pub fn from_lbdev(d: &bc_io::lbdev::LbDevice) -> Result<Self, String> {
        let mut s = LaserSettings::default();
        let driver = d.driver.to_ascii_lowercase();
        let unsupported = || format!("\"{}\" uses LightBurn's {} driver, which BurnCraft doesn't support", d.name, d.driver);
        if driver.contains("ruida") {
            s.controller = ControllerKind::Ruida;
            // Network connections are called "Ethernet" in older files and "UDP" in newer ones.
            let network = ["ethernet", "udp", "tcp", "network"].iter().any(|n| d.connection.eq_ignore_ascii_case(n));
            s.ruida_usb = !network && d.address.is_none();
            if let Some(ip) = &d.address {
                s.ip = ip.clone();
            }
        } else if driver.contains("ezcad") || driver.contains("galvo") {
            s.controller = ControllerKind::Galvo;
            if d.width > 0.0 {
                s.galvo.lens_size = d.width;
            }
        } else {
            s.controller = ControllerKind::Grbl;
            s.dialect = if driver.contains("grbl-m3") {
                Dialect::GrblM3
            } else if driver.starts_with("grbl") {
                Dialect::Grbl
            } else if driver.contains("smoothie") {
                Dialect::Smoothieware
            } else if driver.contains("marlin") {
                Dialect::Marlin
            } else if driver.contains("gcode") || driver.contains("custom") {
                Dialect::Custom
            } else {
                return Err(unsupported());
            };
            s.s_max = d.s_max.unwrap_or(s.dialect.default_s_max());
        }
        if d.width > 0.0 && d.height > 0.0 {
            s.bed = (d.width, d.height);
        }
        // LightBurn's mirror flags pick the origin corner.
        s.origin = match (d.mirror_x, d.mirror_y) {
            (false, false) => OriginCorner::BottomLeft,
            (false, true) => OriginCorner::TopLeft,
            (true, false) => OriginCorner::BottomRight,
            (true, true) => OriginCorner::TopRight,
        };
        s.serial_port = d.port.clone().unwrap_or_default();
        s.baud = d.baud.unwrap_or(s.baud);
        s.start_gcode = d.start_gcode.clone();
        s.end_gcode = d.end_gcode.clone();
        s.air_m7 = d.air_m7;
        s.dtr = d.dtr.unwrap_or(true);
        s.home_on_connect = d.home_on_startup;
        s.speed_unit = s.controller.default_speed_unit();
        s.macros = d.macros.iter().map(|(label, gcode)| Macro { label: label.clone(), gcode: gcode.clone() }).collect();
        Ok(Self { name: d.name.clone(), settings: s })
    }
}

impl Default for LaserSettings {
    fn default() -> Self {
        Self {
            controller: ControllerKind::Ruida,
            ruida_model: RuidaModel::default(),
            ruida_usb: false,
            ip: "192.168.1.100".into(),
            serial_port: String::new(),
            baud: 115_200,
            s_max: 1000.0,
            dialect: Dialect::Grbl,
            custom_gcode: CustomGcode::default(),
            galvo: GalvoSettings::default(),
            origin: OriginCorner::TopRight,
            rapid_speed: 300.0,
            jog_step: 10.0,
            cut_selected: false,
            frame_power: 1.0,
            bed: (600.0, 400.0),
            start_gcode: String::new(),
            end_gcode: String::new(),
            air_m7: false,
            dtr: true,
            home_on_connect: false,
            macros: Vec::new(),
            rotary: Rotary::default(),
            start_from: StartFrom::Absolute,
            job_origin: 4,
            user_origin: None,
            speed_unit: SpeedUnit::PerSecond,
        }
    }
}

/// What the laser panels ask the app to do.
pub enum LaserAction {
    Frame,
    /// Trace the job's outline (convex box) with the laser on at very low power.
    FrameOutline,
    Start,
    Preview,
}

pub struct Laser {
    /// The active machine's settings (a working copy of `profiles[active]`).
    pub settings: LaserSettings,
    profiles: Vec<DeviceProfile>,
    active: usize,
    /// Work area of a newly selected machine, for the app to apply.
    pending_bed: Option<(f64, f64)>,
    /// Estimated duration (s) of the last job sent.
    pub job_estimate: std::cell::Cell<Option<f64>>,
    pub device: Device,
    ports: Vec<String>,
    console_input: String,
    goto: (f64, f64),
    /// Devices window: the detailed settings of the selected machine are shown.
    editing: bool,
    wizard: Option<Wizard>,
    finder: Option<Finder>,
    /// Display units, for the device summaries.
    units: Units,
    /// The Machine Settings (GRBL) window.
    pub grbl_config: crate::grbl_config::GrblConfig,
}

impl Laser {
    /// `legacy` is the single machine saved by versions before the device list. On a first run
    /// (neither saved) the list starts empty, for the user to add their own machine.
    pub fn new(store: Option<DeviceStore>, legacy: Option<LaserSettings>, device: Device) -> Self {
        let store = store.unwrap_or_else(|| DeviceStore { profiles: legacy.into_iter().map(|settings| DeviceProfile { name: "My laser".into(), settings }).collect(), active: 0 });
        let active = store.active.min(store.profiles.len().saturating_sub(1));
        let settings = store.profiles.get(active).map(|p| p.settings.clone()).unwrap_or_default();
        Self { settings, profiles: store.profiles, active, pending_bed: None, job_estimate: Default::default(), device, ports: bc_grbl::list_ports(), console_input: String::new(), goto: (0.0, 0.0), editing: false, wizard: None, finder: None, units: Units::default(), grbl_config: Default::default() }
    }

    /// The device list for saving.
    pub fn store(&mut self) -> DeviceStore {
        self.keep_settings();
        DeviceStore { profiles: self.profiles.clone(), active: self.active }
    }

    pub fn profile_name(&self) -> &str {
        self.profiles.get(self.active).map_or("No machine", |p| &p.name)
    }

    /// Whether any machine has been added.
    pub fn has_machine(&self) -> bool {
        !self.profiles.is_empty()
    }

    /// Copies the working settings back into the selected machine.
    fn keep_settings(&mut self) {
        if let Some(p) = self.profiles.get_mut(self.active) {
            p.settings = self.settings.clone();
        }
    }

    /// Switches to another machine (only while disconnected).
    fn select_profile(&mut self, index: usize) {
        if index == self.active || index >= self.profiles.len() || self.device.connected {
            return;
        }
        self.keep_settings();
        self.active = index;
        self.settings = self.profiles[index].settings.clone();
        self.pending_bed = Some(self.settings.bed);
        self.ports = bc_grbl::list_ports();
        self.device.message = Some(format!("Selected {}", self.profiles[index].name));
    }

    fn add_profile(&mut self, profile: DeviceProfile) {
        self.keep_settings();
        self.profiles.push(profile);
        let last = self.profiles.len() - 1;
        self.active = last;
        self.settings = self.profiles[last].settings.clone();
        self.pending_bed = Some(self.settings.bed);
        self.ports = bc_grbl::list_ports();
    }

    /// Imports machines from a device file (BurnCraft or LightBurn); returns a message.
    fn import_devices(&mut self) -> String {
        let all = [DEVICE_EXTENSION, bc_io::lbdev::EXTENSION, bc_io::lbzip::EXTENSION];
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Device files", &all)
            .add_filter("BurnCraft device", &[DEVICE_EXTENSION])
            .add_filter("LightBurn device or bundle", &[bc_io::lbdev::EXTENSION, bc_io::lbzip::EXTENSION])
            .pick_file()
        else {
            return String::new();
        };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (devices, problems) = match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| devices_from_file(&name, &b)) {
            Ok(d) => d,
            Err(e) => return format!("Could not import {}: {e}", path.display()),
        };
        let mut added = Vec::new();
        let mut needs_ip = false;
        for p in devices {
            // LightBurn files only sometimes carry the IP address (in "Info"); otherwise the default is a guess.
            needs_ip |= p.settings.controller == ControllerKind::Ruida && !p.settings.ruida_usb && p.settings.ip == LaserSettings::default().ip;
            added.push(p.name.clone());
            self.add_profile(p);
        }
        let mut msg = if added.is_empty() { "No devices added".to_string() } else { format!("Added {}", added.join(", ")) };
        if needs_ip {
            msg += ". The file has no IP address for the controller: enter it under Connection";
        }
        if !problems.is_empty() {
            msg += &format!(". {}", problems.join("; "));
        }
        msg
    }

    /// Saves the selected machine (or all of them) as a BurnCraft device file; returns a message.
    fn export_devices(&mut self, all: bool) -> String {
        if !self.has_machine() {
            return String::new();
        }
        self.keep_settings();
        let profiles: Vec<DeviceProfile> = if all { self.profiles.clone() } else { vec![self.profiles[self.active].clone()] };
        let suggested: String = if all { "BurnCraft devices".into() } else { self.profile_name().chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect() };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("BurnCraft device", &[DEVICE_EXTENSION])
            .set_file_name(format!("{}.{DEVICE_EXTENSION}", suggested.trim()))
            .save_file()
        else {
            return String::new();
        };
        match std::fs::write(&path, device_file(&profiles)) {
            Ok(()) => format!("Saved {} machine(s) to {}", profiles.len(), path.display()),
            Err(e) => format!("Could not save {}: {e}", path.display()),
        }
    }

    /// Applies controller events. Returns a new work area: reported by the controller, or from a newly selected machine.
    pub fn poll(&mut self) -> Option<(f64, f64)> {
        let reported = self.device.poll();
        if std::mem::take(&mut self.device.just_connected) && self.settings.home_on_connect {
            self.device.send(DeviceCmd::Home);
        }
        if let Some(b) = reported {
            self.settings.bed = b;
        }
        reported.or(self.pending_bed.take())
    }

    /// Sends a macro's lines to the machine.
    fn run_macro(&self, m: &Macro) {
        for line in m.gcode.lines().map(str::trim).filter(|l| !l.is_empty()) {
            self.device.send(DeviceCmd::Console(line.to_string()));
        }
    }

    /// The active machine's macro buttons (G-code machines).
    pub fn macro_buttons(&self, ui: &mut egui::Ui) {
        if self.settings.controller != ControllerKind::Grbl || self.settings.macros.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for m in &self.settings.macros {
                if ui.add_enabled(self.device.connected, egui::Button::new(&m.label)).on_hover_text(&m.gcode).clicked() {
                    self.run_macro(m);
                }
            }
        });
    }

    pub fn machine(&self, bed: (f64, f64)) -> MachineConfig {
        if self.settings.controller == ControllerKind::Galvo {
            // Galvo coordinates are the lens field, measured from its top-left corner.
            let lens = self.settings.galvo.lens_size;
            return MachineConfig { bed: (lens, lens), origin: OriginCorner::TopLeft, rapid_speed: self.settings.galvo.jump_speed };
        }
        if let Some(field) = self.settings.controller.fixed_field() {
            return MachineConfig { bed: field, origin: OriginCorner::TopLeft, rapid_speed: bc_lp2::MAX_SPEED };
        }
        MachineConfig { bed, origin: self.settings.origin, rapid_speed: self.settings.rapid_speed }
    }

    /// How jobs should be built for the selected controller.
    pub fn build_options(&self) -> BuildOptions {
        BuildOptions { grayscale: self.settings.controller == ControllerKind::Grbl }
    }

    pub fn gcode_options(&self) -> GcodeOptions {
        let s = &self.settings;
        GcodeOptions {
            s_max: s.s_max,
            dialect: s.dialect,
            custom: s.custom_gcode.clone(),
            start_gcode: s.start_gcode.clone(),
            end_gcode: s.end_gcode.clone(),
            air_m7: s.air_m7,
            y_as_a: self.rotary_on_a(),
        }
    }

    /// The rotary's Y scale, when a rotary is in use (galvos and the LP2 have no rotary support).
    pub fn rotary_scale(&self) -> Option<f64> {
        let r = &self.settings.rotary;
        (r.enabled && matches!(self.settings.controller, ControllerKind::Ruida | ControllerKind::Grbl)).then(|| r.scale())
    }

    fn rotary_on_a(&self) -> bool {
        self.settings.controller == ControllerKind::Grbl && self.rotary_scale().is_some() && self.settings.rotary.a_axis
    }

    /// Design → machine coordinates for `job`, including the rotary stretch about the job's top edge.
    /// Design point → machine position for a job, including where Start From puts it.
    fn job_mapper(&self, job: &Job, bed: (f64, f64)) -> Result<impl Fn(Point) -> (f64, f64) + use<>, String> {
        let cfg = self.machine(bed);
        let rotary = self.rotary_scale().zip(job.bounds()).map(|(k, (top_left, _))| (k, cfg.to_machine(top_left).1));
        let base = move |p: Point| {
            let (x, y) = cfg.to_machine(p);
            match rotary {
                Some((k, y_ref)) => (x, y_ref + (y - y_ref) * k),
                None => (x, y),
            }
        };
        // Relative modes: slide the job so its origin point lands on the head or the user origin.
        let target = match self.settings.start_from {
            StartFrom::Absolute => None,
            StartFrom::CurrentPosition => Some(
                self.device.state.as_ref().map(|s| (s.x, s.y)).ok_or("Start From is Current Position, but the head position isn't known yet. Connect to the laser first.")?,
            ),
            StartFrom::UserOrigin => Some(self.settings.user_origin.ok_or("Start From is User Origin, but no user origin is set. Move the head there and click Set User Origin (Move tab).")?),
        };
        let offset = match (target, job.bounds()) {
            (Some(t), Some((min, max))) => {
                let at = base(origin_point(min, max, self.settings.job_origin));
                (t.0 - at.0, t.1 - at.1)
            }
            _ => (0.0, 0.0),
        };
        Ok(move |p: Point| {
            let (x, y) = base(p);
            (x + offset.0, y + offset.1)
        })
    }

    /// Checks a job can run: not empty, and inside the work area where Start From puts it.
    fn check_placed(&self, job: &Job, bed: (f64, f64)) -> Result<(), String> {
        Self::check(job, bed).or_else(|e| if self.settings.start_from == StartFrom::Absolute || job.is_empty() { Err(e) } else { Ok(()) })?;
        if self.settings.start_from == StartFrom::Absolute {
            return Ok(());
        }
        let map = self.job_mapper(job, bed)?;
        let (min, max) = job.bounds().expect("non-empty job");
        let corners = [map(min), map(max), map(Point::new(min.x, max.y)), map(Point::new(max.x, min.y))];
        let (w, h) = self.machine(bed).bed;
        if corners.iter().any(|(x, y)| *x < -1e-6 || *y < -1e-6 || *x > w + 1e-6 || *y > h + 1e-6) {
            return Err(format!(
                "Placed at the {}, the job would run outside the {w:.0} × {h:.0} mm work area. Move the head, or pick another job origin point.",
                if self.settings.start_from == StartFrom::CurrentPosition { "head's current position" } else { "user origin" }
            ));
        }
        Ok(())
    }

    /// The job as G-code for the selected dialect (for File > Save G-code).
    pub fn gcode(&self, job: &Job, bed: (f64, f64)) -> Result<String, String> {
        self.check_placed(job, bed)?;
        let layers = crate::device::to_gcode_layers(to_machine_job(job, self.job_mapper(job, bed)?));
        let mut text = bc_grbl::generate(&layers, &self.gcode_options()).join("\n");
        text.push('\n');
        Ok(text)
    }

    fn select_controller(&mut self, kind: ControllerKind) {
        self.settings.controller = kind;
        self.settings.origin = kind.default_origin();
        self.settings.speed_unit = kind.default_speed_unit();
    }

    /// How speeds are shown for the selected machine.
    pub fn speed(&self, units: Units) -> Speed {
        let speed = Speed::new(units, self.settings.speed_unit);
        // The LP2 sets speed as a "depth", as its own app does.
        if self.settings.controller == ControllerKind::Lp2 { speed.as_depth() } else { speed }
    }

    /// Head position in design coordinates, if known.
    pub fn head_position(&self, bed: (f64, f64)) -> Option<Point> {
        self.device.state.as_ref().map(|s| self.machine(bed).to_design((s.x, s.y)))
    }

    /// Checks that a job is non-empty and inside the bed.
    pub fn check(job: &Job, bed: (f64, f64)) -> Result<(), String> {
        if job.is_empty() {
            let mut msg = "Nothing to cut.".to_string();
            for (i, why) in &job.skipped {
                msg += &format!("\nLayer {}: {why}", bc_doc::layer_label(*i));
            }
            return Err(msg);
        }
        let (min, max) = job.bounds().expect("non-empty job");
        let (w, h) = bed;
        if min.x < -1e-6 || min.y < -1e-6 || max.x > w + 1e-6 || max.y > h + 1e-6 {
            return Err(format!(
                "The job (including any overscan) extends outside the {w:.0} × {h:.0} mm work area. Move it inside the bed outline."
            ));
        }
        Ok(())
    }

    /// Traces the job's bounding box with the laser off.
    pub fn frame(&self, job: &Job, bed: (f64, f64)) -> Result<(), String> {
        self.check_placed(job, bed)?;
        let (a, b) = job.bounds().expect("checked");
        let map = self.job_mapper(job, bed)?;
        let (p, q) = (map(a), map(b));
        let (min, max) = ((p.0.min(q.0), p.1.min(q.1)), (p.0.max(q.0), p.1.max(q.1)));
        if self.rotary_on_a() {
            // Jogging moves X/Y only, so trace the frame as a laser-off job on X/A.
            let ring = vec![min, (max.0, min.1), max, (min.0, max.1), min];
            let layer = MachineLayer { speed: self.settings.rapid_speed, min_power: 0.0, max_power: 0.0, air_assist: false, frequency: 30.0, color: [0, 0, 0], raster: false, passes: 1, paths: vec![(ring, 0.0)] };
            self.device.send(DeviceCmd::Run(vec![layer]));
        } else {
            self.device.send(DeviceCmd::Frame(min, max, self.settings.rapid_speed));
        }
        Ok(())
    }

    /// Traces the job's bounding box with the laser on at the (very low) frame power, as a job.
    pub fn frame_with_laser(&self, job: &Job, bed: (f64, f64)) -> Result<bool, String> {
        self.check_placed(job, bed)?;
        let (a, b) = job.bounds().expect("checked");
        let mut settings = Layer::new(0);
        settings.speed = self.settings.rapid_speed.min(100.0);
        settings.min_power = self.settings.frame_power;
        settings.max_power = self.settings.frame_power;
        settings.air_assist = false;
        let ring = vec![a, Point::new(b.x, a.y), b, Point::new(a.x, b.y), a];
        let frame = Job { layers: vec![bc_job::JobLayer { index: 0, settings, paths: vec![bc_job::JobPath::burn(ring)] }], skipped: Vec::new() };
        self.start(&frame, bed, &format!("Trace the job outline with the laser ON at {:.1}% power?", self.settings.frame_power))
    }

    /// Asks for confirmation, then sends the job. Returns whether it was sent.
    pub fn start(&self, job: &Job, bed: (f64, f64), title: &str) -> Result<bool, String> {
        self.check_placed(job, bed)?;
        let map = self.job_mapper(job, bed)?;
        let cfg = self.machine(bed);
        let start = self.head_position(bed).unwrap_or(Point::ORIGIN);
        let stats = job.stats(start, cfg.rapid_speed);
        let mut summary = format!(
            "{title}\n\n{} layer(s), {:.0} mm of burning, {:.0} mm of travel, about {}.",
            job.layers.len(),
            stats.cut_mm,
            stats.travel_mm,
            crate::dialogs::format_duration(stats.seconds)
        );
        for (i, why) in &job.skipped {
            summary += &format!("\nLayer {}: {why}", bc_doc::layer_label(*i));
        }
        if self.rotary_scale().is_some() {
            let r = &self.settings.rotary;
            summary += &match r.kind {
                RotaryKind::Roller => format!("\n\nRotary ON (roller, Ø {:.1} mm).", r.roller_diameter),
                RotaryKind::Chuck => format!("\n\nRotary ON (chuck, object Ø {:.1} mm, {:.1} mm around).", r.object_diameter, r.surface_per_turn()),
            };
            if let Some((a, b)) = job.bounds()
                && r.kind == RotaryKind::Chuck
                && b.y - a.y > r.surface_per_turn()
            {
                summary += " The design is taller than the object's circumference, so it will overlap itself.";
            }
        }
        match self.settings.start_from {
            StartFrom::Absolute => {}
            StartFrom::CurrentPosition => summary += "\n\nStarting from the head's current position.",
            StartFrom::UserOrigin => summary += "\n\nStarting from the user origin.",
        }
        summary += "\n\nMake sure the lid is closed, the exhaust and air assist are on, and the material is in place.";
        let confirmed = rfd::MessageDialog::new().set_title("Start job").set_description(&summary).set_buttons(rfd::MessageButtons::OkCancel).show()
            == rfd::MessageDialogResult::Ok;
        if confirmed {
            self.device.send(DeviceCmd::Run(to_machine_job(job, map)));
            self.job_estimate.set(Some(stats.seconds));
        }
        Ok(confirmed)
    }

    /// Moves the head (laser off) to a design point.
    pub fn move_to_design(&self, bed: (f64, f64), p: Point) {
        let (x, y) = self.machine(bed).to_machine(p);
        self.device.send(DeviceCmd::MoveTo(x, y, self.settings.rapid_speed));
    }

    /// Moves the head to the machine origin (0, 0).
    pub fn go_to_origin(&self) {
        // With Start From = User Origin, "the origin" is the user's.
        let (x, y) = match (self.settings.start_from, self.settings.user_origin) {
            (StartFrom::UserOrigin, Some(o)) => o,
            _ => (0.0, 0.0),
        };
        self.device.send(DeviceCmd::MoveTo(x, y, self.settings.rapid_speed));
    }

    /// Runs `f` with Start From set to Absolute: for jobs that must land exactly where they are on
    /// the bed (the camera target, sections of an oversized job), whatever the user has chosen.
    pub fn absolute<R>(&mut self, f: impl FnOnce(&Self) -> R) -> R {
        let chosen = std::mem::replace(&mut self.settings.start_from, StartFrom::Absolute);
        let r = f(self);
        self.settings.start_from = chosen;
        r
    }

    /// Start From and the job origin point, for the Laser tab.
    fn start_from_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Start from");
            egui::ComboBox::from_id_salt("start-from")
                .selected_text(self.settings.start_from.label())
                .show_ui(ui, |ui| {
                    for s in StartFrom::ALL {
                        ui.selectable_value(&mut self.settings.start_from, s, s.label());
                    }
                })
                .response
                .on_hover_text("Absolute: where the job sits on the bed. Current Position / User Origin: the job's origin point (right) goes to the head or to your saved origin.");
            ui.add_space(8.0);
            let relative = self.settings.start_from != StartFrom::Absolute;
            ui.add_enabled(relative, egui::Label::new("Job origin"));
            origin_picker(ui, &mut self.settings.job_origin, relative);
        });
        if self.settings.start_from == StartFrom::UserOrigin && self.settings.user_origin.is_none() {
            ui.colored_label(ui.visuals().warn_fg_color, "No user origin yet: move the head there and click Set User Origin in the Move tab.");
        }
    }

    /// Jog by a design-space offset (mm), converted to the machine's axis directions.
    fn jog(&self, bed: (f64, f64), dx: f64, dy: f64) {
        let cfg = self.machine(bed);
        let (a, b) = (cfg.to_machine(Point::ORIGIN), cfg.to_machine(Point::new(dx, dy)));
        self.device.send(DeviceCmd::JogBy(b.0 - a.0, b.1 - a.1, self.settings.rapid_speed));
    }

    pub fn connect(&mut self) {
        if !self.has_machine() {
            self.device.message = Some("Add your machine first: Devices → Find My Laser or Create Manually".into());
            return;
        }
        let s = &self.settings;
        let needs_port = matches!(s.controller, ControllerKind::Grbl | ControllerKind::Lp2) || (s.controller == ControllerKind::Ruida && s.ruida_usb);
        if needs_port && s.serial_port.is_empty() {
            self.device.message = Some("Choose the serial port in the drop-down next to the machine".into());
            return;
        }
        match s.controller {
            ControllerKind::Ruida if s.ruida_usb => {
                self.device.send(DeviceCmd::ConnectRuida { target: RuidaTarget::Usb(s.serial_port.clone()), model: s.ruida_model });
                self.device.message = Some(format!("Connecting on {}…", s.serial_port));
            }
            ControllerKind::Ruida => match s.ip.trim().parse::<Ipv4Addr>() {
                Ok(ip) => {
                    self.device.send(DeviceCmd::ConnectRuida { target: RuidaTarget::Ethernet(ip), model: s.ruida_model });
                    self.device.message = Some(format!("Connecting to {ip}…"));
                }
                Err(_) => self.device.message = Some("Enter an IPv4 address like 192.168.1.100 (Devices…)".into()),
            },
            ControllerKind::Grbl => {
                self.device.send(DeviceCmd::ConnectGcode { port: s.serial_port.clone(), baud: s.baud, dtr: s.dtr, options: Box::new(self.gcode_options()) });
                self.device.message = Some(format!("Connecting on {}…", self.settings.serial_port));
            }
            ControllerKind::Galvo => {
                self.device.send(DeviceCmd::ConnectGalvo(Box::new(s.galvo.clone())));
                self.device.message = Some("Connecting to the galvo over USB…".into());
            }
            ControllerKind::Lp2 => {
                self.device.send(DeviceCmd::ConnectLp2 { port: s.serial_port.clone() });
                self.device.message = Some(format!("Connecting on {}…", s.serial_port));
            }
        }
    }

    fn device_name(&self) -> String {
        let s = &self.settings;
        let port = if s.serial_port.is_empty() { "(no port)" } else { &s.serial_port };
        match s.controller {
            ControllerKind::Ruida if s.ruida_usb => format!("Ruida {port}"),
            ControllerKind::Ruida => format!("Ruida {}", s.ip),
            ControllerKind::Grbl => format!("{} {port}", s.dialect.label()),
            ControllerKind::Galvo => format!("Galvo {} {:.0} mm", s.galvo.source.label(), s.galvo.lens_size),
            ControllerKind::Lp2 => format!("LaserPecker 2 {port}"),
        }
    }

    /// The main Laser tab. Returns an action for the app, and whether "Devices…" was clicked.
    pub fn panel(&mut self, ui: &mut egui::Ui) -> (Option<LaserAction>, bool) {
        let mut action = None;
        let mut open_devices = false;
        let connected = self.device.connected;
        let status = if connected { format!("Ready: {}", self.device.description) } else { "Disconnected".into() };
        ui.label(egui::RichText::new(status).strong());
        if let Some(msg) = &self.device.message {
            ui.weak(msg);
        }
        if let Some((done, total, what)) = self.device.progress {
            ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(format!("{what} {:.0}%", 100.0 * done as f32 / total.max(1) as f32)));
        }
        let w = (ui.available_width() - 8.0) / 2.0;
        let big = |label: &str| egui::Button::new(label).min_size(egui::vec2(w, 30.0));
        ui.horizontal(|ui| {
            let pause_label = if self.device.paused { "▶ Resume" } else { "⏸ Pause" };
            if ui.add_enabled(connected, big(pause_label)).clicked() {
                let paused = !self.device.paused;
                self.device.set_paused(paused);
            }
            if ui.add_enabled(connected, big("■ Stop").fill(Color32::from_rgb(180, 40, 40))).clicked() {
                self.device.stop();
            }
        });
        ui.horizontal(|ui| {
            // The full row, lining up with the button pair above; a muted green beside Stop's red.
            let start = egui::Button::new(egui::RichText::new("▶ Start").strong()).fill(Color32::from_rgb(46, 120, 62)).min_size(egui::vec2(ui.available_width(), 30.0));
            if ui.add_enabled(connected, start).clicked() {
                action = Some(LaserAction::Start);
            }
        });
        ui.horizontal(|ui| {
            if self.settings.controller == ControllerKind::Lp2 {
                // The LP2's preview already traces the outline with its low-power light.
                let frame = egui::Button::new("Frame").min_size(egui::vec2(ui.available_width(), 30.0));
                if ui.add_enabled(connected, frame).on_hover_text("Trace the job's bounding box with the preview light").clicked() {
                    action = Some(LaserAction::Frame);
                }
                return;
            }
            if ui.add_enabled(connected, big("Frame")).on_hover_text("Trace the job's bounding box with the laser off").clicked() {
                action = Some(LaserAction::Frame);
            }
            if ui.add_enabled(connected, big("Frame (laser on)")).on_hover_text("Trace the bounding box with the laser on at very low power").clicked() {
                action = Some(LaserAction::FrameOutline);
            }
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(connected, big("Home")).clicked() {
                self.device.send(DeviceCmd::Home);
            }
            if ui.add_enabled(connected, big("Go to Origin")).clicked() {
                self.go_to_origin();
            }
        });
        self.start_from_ui(ui);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.settings.cut_selected, "Cut Selected Graphics");
            if matches!(self.settings.controller, ControllerKind::Ruida | ControllerKind::Grbl) {
                ui.checkbox(&mut self.settings.rotary.enabled, "Rotary").on_hover_text("Wrap jobs around a roller or chuck (set it up under Laser > Rotary Setup)");
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Preview").clicked() {
                action = Some(LaserAction::Preview);
            }
            if self.settings.controller != ControllerKind::Lp2 {
                ui.label("Frame power");
                ui.add(egui::DragValue::new(&mut self.settings.frame_power).range(0.0..=20.0).speed(0.1).suffix(" %"));
            }
        });
        self.macro_buttons(ui);
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Devices").clicked() {
                open_devices = true;
            }
            if !self.has_machine() {
                if ui.button("Add your laser…").on_hover_text("Find it over USB, or set it up step by step").clicked() {
                    open_devices = true;
                }
                return;
            }
            let mut pick = None;
            // USB machines get a port picker beside the machine picker, as the port can change
            // each time the laser is plugged in.
            let serial = self.uses_serial_port();
            let port_width = if serial { 110.0 } else { 0.0 };
            // Each picker gets a box of fixed width and long names or ports are cut short inside
            // it. The row is sized from the panel's width, so anything wider would widen the
            // panel, which widens the row again, and so on. (A combo box truncates its text at
            // the space left in the row, not at its own width, hence the boxes.)
            let width = (ui.available_width() - 100.0 - port_width).max(80.0);
            let name = self.profile_name().to_string();
            let tip = self.device_name();
            ui.scope(|ui| {
                ui.set_max_width(width);
                egui::ComboBox::from_id_salt("device-pick")
                    .selected_text(name)
                    .truncate()
                    .width(width)
                    .show_ui(ui, |ui| {
                        for (i, p) in self.profiles.iter().enumerate() {
                            if ui.add_enabled(!connected, egui::Button::selectable(i == self.active, &p.name)).clicked() {
                                pick = Some(i);
                            }
                        }
                    })
                    .response
                    .on_hover_text(tip);
            });
            if let Some(i) = pick {
                self.select_profile(i);
            }
            if serial {
                self.port_combo(ui, "panel-port", port_width - 8.0, !connected);
            }
            if connected {
                if ui.button("Disconnect").clicked() {
                    self.device.send(DeviceCmd::Disconnect);
                }
            } else if ui.button("Connect").clicked() {
                self.connect();
            }
        });
        (action, open_devices)
    }

    /// The Move tab: jogging, position readout and go-to.
    pub fn move_panel(&mut self, ui: &mut egui::Ui, bed: (f64, f64), units: Units, y_up: bool) {
        let connected = self.device.connected;
        if let Some(s) = &self.device.state {
            let p = self.machine(bed).to_design((s.x, s.y));
            let y = if y_up { bed.1 - p.y } else { p.y };
            ui.monospace(format!("X {}  Y {}   {}", units.format(p.x), units.format(y), s.state));
        } else {
            ui.weak("Position unknown");
        }
        ui.add_enabled_ui(connected, |ui| {
            let step = self.settings.jog_step;
            egui::Grid::new("jog").show(ui, |ui| {
                let b = |t: &str| egui::Button::new(t).min_size(egui::vec2(34.0, 30.0));
                ui.label("");
                if ui.add(b("⏶")).clicked() { self.jog(bed, 0.0, -step); }
                ui.label("");
                ui.end_row();
                if ui.add(b("◀")).clicked() { self.jog(bed, -step, 0.0); }
                if ui.add(b("⌂")).on_hover_text("Home").clicked() { self.device.send(DeviceCmd::Home); }
                if ui.add(b("▶")).clicked() { self.jog(bed, step, 0.0); }
                ui.end_row();
                ui.label("");
                if ui.add(b("⏷")).clicked() { self.jog(bed, 0.0, step); }
                ui.label("");
                ui.end_row();
            });
            ui.horizontal(|ui| {
                ui.label("Distance");
                units.drag(ui, &mut self.settings.jog_step, 0.1..=500.0);
                ui.label("Speed");
                ui.add(Speed::new(units, self.settings.speed_unit).edit(&mut self.settings.rapid_speed, 1.0..=2000.0));
            });
            ui.horizontal(|ui| {
                ui.label("Move to X");
                units.drag(ui, &mut self.goto.0, -5000.0..=5000.0);
                ui.label("Y");
                units.drag(ui, &mut self.goto.1, -5000.0..=5000.0);
                if ui.button("Go").clicked() {
                    let y = if y_up { bed.1 - self.goto.1 } else { self.goto.1 };
                    self.move_to_design(bed, Point::new(self.goto.0, y));
                }
            });
            ui.horizontal(|ui| {
                let here = self.device.state.as_ref().map(|s| (s.x, s.y));
                if ui.add_enabled(here.is_some(), egui::Button::new("Set User Origin")).on_hover_text("Remember the head's current position, for Start From: User Origin").clicked() {
                    self.settings.user_origin = here;
                    self.device.message = Some("User origin set at the head's position".into());
                }
                if ui.add_enabled(self.settings.user_origin.is_some(), egui::Button::new("Go to User Origin")).clicked()
                    && let Some((x, y)) = self.settings.user_origin
                {
                    self.device.send(DeviceCmd::MoveTo(x, y, self.settings.rapid_speed));
                }
                if let Some((x, y)) = self.settings.user_origin {
                    let p = self.machine(bed).to_design((x, y));
                    let yd = if y_up { bed.1 - p.y } else { p.y };
                    ui.weak(format!("at X {} Y {}", units.format(p.x), units.format(yd)));
                }
            });
            if self.settings.controller == ControllerKind::Grbl && ui.button("Unlock ($X)").on_hover_text("Clear an alarm lock").clicked() {
                self.device.send(DeviceCmd::Unlock);
            }
        });
    }

    /// The Console tab (GRBL): send raw commands and see replies.
    pub fn console_panel(&mut self, ui: &mut egui::Ui) {
        if self.settings.controller == ControllerKind::Grbl && self.settings.dialect.is_grbl() && ui.button("Machine Settings…").on_hover_text("Read, change, export and import the controller's own settings ($$)").clicked() {
            self.grbl_config.open = true;
        }
        self.macro_buttons(ui);
        let height = (ui.available_height() - 30.0).max(80.0);
        egui::ScrollArea::vertical().max_height(height).stick_to_bottom(true).show(ui, |ui| {
            for line in &self.device.console {
                ui.monospace(line);
            }
            if self.device.console.is_empty() {
                ui.weak("Type a controller command below (e.g. $$ or ?) and press Enter.");
            }
        });
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.console_input).desired_width(ui.available_width() - 60.0).hint_text("Command"));
            let send = (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) | ui.button("Send").clicked();
            if send && !self.console_input.trim().is_empty() && self.device.connected {
                self.device.send(DeviceCmd::Console(self.console_input.trim().to_string()));
                self.console_input.clear();
                r.request_focus();
            }
        });
    }

    /// The Machine Settings (GRBL) window, when open.
    pub fn grbl_config_window(&mut self, ctx: &egui::Context) {
        let name = self.profile_name().to_string();
        self.grbl_config.show(ctx, &mut self.device, &name, &mut self.settings.s_max);
    }

    /// The Devices window: controller, connection, origin, speed and work area.
    pub fn devices_window(&mut self, ctx: &egui::Context, open: &mut bool, bed: &mut (f64, f64), units: Units) -> bool {
        let mut bed_changed = false;
        let connected = self.device.connected;
        self.units = units;
        crate::dialogs::centered(ctx, "Devices").open(open).resizable(false).auto_sized().show(ctx, |ui| {
            // A set width: separators fill whatever width the window has, so without it the window
            // stays as wide as it ever was (after editing, or a long message).
            ui.set_width(if self.editing { 560.0 } else { 460.0 });
            self.device_list(ui, connected);
            if !self.has_machine() {
                self.editing = false;
            }
            if self.editing {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Name");
                    if let Some(p) = self.profiles.get_mut(self.active) {
                        ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(220.0));
                    }
                });
                ui.add_enabled_ui(!connected, |ui| {
                    egui::Grid::new("device-grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                        ui.label("Controller");
                        let mut kind = self.settings.controller;
                        egui::ComboBox::from_id_salt("controller").selected_text(kind.label()).show_ui(ui, |ui| {
                            for k in ControllerKind::ALL {
                                ui.selectable_value(&mut kind, k, k.label());
                            }
                        });
                        if kind != self.settings.controller {
                            self.select_controller(kind);
                        }
                        ui.end_row();
                        match self.settings.controller {
                            ControllerKind::Ruida => {
                                ui.label("Model");
                                egui::ComboBox::from_id_salt("ruida-model").selected_text(self.settings.ruida_model.label()).show_ui(ui, |ui| {
                                    for m in RuidaModel::ALL {
                                        ui.selectable_value(&mut self.settings.ruida_model, m, m.label());
                                    }
                                });
                                ui.end_row();
                                ui.label("Connection");
                                ui.horizontal(|ui| {
                                    ui.radio_value(&mut self.settings.ruida_usb, false, "Ethernet");
                                    ui.radio_value(&mut self.settings.ruida_usb, true, "USB");
                                });
                                ui.end_row();
                                if self.settings.ruida_usb {
                                    self.port_row(ui);
                                } else {
                                    ui.label("IP address");
                                    ui.add(egui::TextEdit::singleline(&mut self.settings.ip).desired_width(140.0));
                                    ui.end_row();
                                }
                            }
                            ControllerKind::Grbl => {
                                ui.label("Firmware");
                                let mut dialect = self.settings.dialect;
                                egui::ComboBox::from_id_salt("dialect").selected_text(dialect.label()).show_ui(ui, |ui| {
                                    for d in Dialect::ALL {
                                        ui.selectable_value(&mut dialect, d, d.label());
                                    }
                                });
                                if dialect != self.settings.dialect {
                                    self.settings.dialect = dialect;
                                    self.settings.s_max = dialect.default_s_max();
                                }
                                ui.end_row();
                                self.port_row(ui);
                                ui.label("Baud rate");
                                egui::ComboBox::from_id_salt("baud").selected_text(self.settings.baud.to_string()).show_ui(ui, |ui| {
                                    for b in [57_600, 115_200, 230_400, 250_000, 460_800, 921_600] {
                                        ui.selectable_value(&mut self.settings.baud, b, b.to_string());
                                    }
                                });
                                ui.end_row();
                                ui.label("Full-power S value").on_hover_text("GRBL: $30 (usually 1000). Marlin: 255. Smoothieware: 1.");
                                ui.add(egui::DragValue::new(&mut self.settings.s_max).range(0.01..=100_000.0));
                                ui.end_row();
                            }
                            ControllerKind::Galvo => {
                                let g = &mut self.settings.galvo;
                                ui.label("Laser source");
                                egui::ComboBox::from_id_salt("galvo-source").selected_text(g.source.label()).show_ui(ui, |ui| {
                                    for s in [Source::Fiber, Source::Co2, Source::Uv] {
                                        ui.selectable_value(&mut g.source, s, s.label());
                                    }
                                });
                                ui.end_row();
                                ui.label("Lens field size");
                                if units.drag(ui, &mut g.lens_size, 10.0..=600.0) {
                                    *bed = (g.lens_size, g.lens_size);
                                    bed_changed = true;
                                }
                                ui.end_row();
                                ui.label("Orientation");
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut g.flip_x, "Flip X");
                                    ui.checkbox(&mut g.flip_y, "Flip Y");
                                    ui.checkbox(&mut g.swap_xy, "Swap X/Y");
                                });
                                ui.end_row();
                            }
                            ControllerKind::Lp2 => self.port_row(ui),
                        }
                    });
                    if self.settings.controller == ControllerKind::Grbl && self.settings.dialect == Dialect::Custom {
                        ui.collapsing("Custom G-code commands", |ui| self.custom_gcode_editor(ui));
                    }
                    if self.settings.controller == ControllerKind::Grbl {
                        ui.collapsing("Start / end G-code, macros", |ui| self.machine_gcode_editor(ui));
                    }
                    if self.settings.controller != ControllerKind::Lp2 {
                        ui.checkbox(&mut self.settings.home_on_connect, "Home when connecting");
                    }
                });
                egui::Grid::new("device-grid-2").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    if self.settings.controller == ControllerKind::Lp2 {
                        // A fixed field, and speeds set as depth: nothing more to set.
                        return;
                    }
                    ui.label("Speed units");
                    ui.horizontal(|ui| {
                        for per in SpeedUnit::ALL {
                            ui.radio_value(&mut self.settings.speed_unit, per, Speed::new(units, per).unit());
                        }
                    });
                    ui.end_row();
                    let speed = Speed::new(units, self.settings.speed_unit);
                    if self.settings.controller == ControllerKind::Galvo {
                        let g = &mut self.settings.galvo;
                        let us = |ui: &mut egui::Ui, v: &mut f64| ui.add(egui::DragValue::new(v).range(-10_000.0..=10_000.0).suffix(" µs"));
                        ui.label("Travel speed");
                        ui.add(speed.edit(&mut g.jump_speed, 10.0..=20_000.0));
                        ui.end_row();
                        ui.label("Framing (red light) speed");
                        ui.add(speed.edit(&mut g.light_speed, 10.0..=20_000.0));
                        ui.end_row();
                        ui.label("Laser on / off delay");
                        ui.horizontal(|ui| {
                            us(ui, &mut g.laser_on_delay);
                            us(ui, &mut g.laser_off_delay);
                        });
                        ui.end_row();
                        ui.label("Polygon delay");
                        us(ui, &mut g.polygon_delay);
                        ui.end_row();
                        ui.label("Jump delay short / long");
                        ui.horizontal(|ui| {
                            us(ui, &mut g.jump_delay_short);
                            us(ui, &mut g.jump_delay_long);
                        });
                        ui.end_row();
                        if g.source == Source::Fiber {
                            ui.label("MOPA pulse width");
                            ui.add(egui::DragValue::new(&mut g.pulse_width).range(0..=500).suffix(" ns")).on_hover_text("0 = don't set (Q-switched sources)");
                            ui.end_row();
                        }
                        return;
                    }
                    ui.label("Origin");
                    egui::ComboBox::from_id_salt("origin").selected_text(self.settings.origin.label()).show_ui(ui, |ui| {
                        for o in OriginCorner::ALL {
                            ui.selectable_value(&mut self.settings.origin, o, o.label());
                        }
                    });
                    ui.end_row();
                    ui.label("Work area");
                    ui.horizontal(|ui| {
                        bed_changed |= units.drag(ui, &mut bed.0, 10.0..=5000.0);
                        ui.label("×");
                        bed_changed |= units.drag(ui, &mut bed.1, 10.0..=5000.0);
                    });
                    ui.end_row();
                    ui.label("Rapid speed");
                    ui.add(speed.edit(&mut self.settings.rapid_speed, 10.0..=2000.0));
                    ui.end_row();
                });
            }
            if self.settings.controller == ControllerKind::Galvo {
                ui.weak("On Windows the board needs the WinUSB driver (install it with Zadig) instead of the EZCad driver.");
                let lens = self.settings.galvo.lens_size;
                if *bed != (lens, lens) {
                    *bed = (lens, lens);
                    bed_changed = true;
                }
            }
            if self.settings.controller == ControllerKind::Lp2 {
                ui.weak("The LaserPecker 2 marks a 100 × 100 mm field. Speeds are set as depth (1-100 %), as in its own app. On Windows it needs the CH340 driver.");
                if *bed != bc_lp2::FIELD {
                    *bed = bc_lp2::FIELD;
                    bed_changed = true;
                }
            }
            if bed_changed {
                self.settings.bed = *bed;
            }
            ui.separator();
            ui.horizontal(|ui| {
                if connected {
                    if ui.button("Disconnect").clicked() {
                        self.device.send(DeviceCmd::Disconnect);
                    }
                } else if self.has_machine() && ui.button("Connect").clicked() {
                    self.connect();
                }
                if let Some(msg) = &self.device.message {
                    ui.add(egui::Label::new(egui::RichText::new(msg).weak()).wrap());
                }
            });
        });
        if let Some(w) = &mut self.wizard {
            match w.show(ctx, &self.ports, units) {
                WizardOutcome::Open => {}
                WizardOutcome::Cancel => self.wizard = None,
                WizardOutcome::Finish(p) => {
                    self.device.message = Some(format!("Added {}", p.name));
                    self.add_profile(*p);
                    self.wizard = None;
                }
            }
        }
        if let Some(f) = &mut self.finder {
            match f.show(ctx) {
                FinderOutcome::Open => {}
                FinderOutcome::Close => self.finder = None,
                FinderOutcome::Add(p) => {
                    self.device.message = Some(format!("Added {}", p.name));
                    self.add_profile(*p);
                    self.finder = None;
                }
            }
        }
        bed_changed
    }

    /// The top of the Devices window: the machine list with a summary of the selected one, and
    /// the buttons that add, edit and remove machines.
    fn device_list(&mut self, ui: &mut egui::Ui, connected: bool) {
        self.keep_settings();
        let any = self.has_machine();
        let mut pick = None;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(310.0);
                egui::Frame::group(ui.style()).inner_margin(4.0).show(ui, |ui| {
                    ui.set_width(300.0);
                    egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                        if !any {
                            ui.add_space(6.0);
                            ui.label("No machines yet.");
                            ui.weak("Plug in a USB laser, switch it on and click Find My Laser, or set one up with Create Manually.");
                            ui.add_space(6.0);
                        }
                        for (i, p) in self.profiles.iter().enumerate() {
                            let selected = i == self.active;
                            let row = egui::Frame::NONE
                                .fill(if selected { ui.visuals().selection.bg_fill.gamma_multiply(0.5) } else { Color32::TRANSPARENT })
                                .corner_radius(3.0)
                                .inner_margin(egui::Margin::symmetric(6, 3))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(egui::RichText::new(&p.name).strong());
                                    ui.weak(device_wizard::subtitle(&p.settings));
                                })
                                .response;
                            let sense = if connected { egui::Sense::hover() } else { egui::Sense::click() };
                            let r = ui.interact(row.rect, ui.id().with(("device-row", i)), sense);
                            if r.clicked() {
                                pick = Some(i);
                            }
                            if r.double_clicked() {
                                self.editing = true;
                            }
                        }
                    });
                });
                ui.add_space(2.0);
                if let Some(p) = self.profiles.get(self.active) {
                    for line in device_wizard::summary(p, self.units) {
                        ui.weak(line);
                    }
                }
                if connected {
                    ui.weak("Disconnect to change machines.");
                }
            });
            ui.vertical(|ui| {
                let full = egui::vec2(140.0, 0.0);
                let button = |ui: &mut egui::Ui, enabled: bool, text: &str, tip: &str| ui.add_enabled(enabled, egui::Button::new(text).min_size(full)).on_hover_text(tip).clicked();
                if button(ui, !connected && self.finder.is_none(), "Find My Laser", "Look for lasers plugged in over USB") {
                    self.wizard = None;
                    self.finder = Some(Finder::start(None));
                }
                if button(ui, !connected && self.wizard.is_none(), "Create Manually", "Add a machine step by step: controller, connection, size and origin") {
                    self.finder = None;
                    self.wizard = Some(Wizard::new());
                }
                if button(ui, !connected, "Import…", "Add machines from a BurnCraft device file (.bcdev), or a LightBurn device file (.lbdev) or bundle (.lbzip)") {
                    let msg = self.import_devices();
                    if !msg.is_empty() {
                        self.device.message = Some(msg);
                    }
                }
                ui.add_space(6.0);
                let edit_label = if self.editing { "Done Editing" } else { "Edit" };
                if button(ui, !connected && any, edit_label, "Change the selected machine's settings") {
                    self.editing = !self.editing;
                }
                if button(ui, !connected && any, "Duplicate", "Add a copy of the selected machine") {
                    let copy = DeviceProfile { name: format!("{} copy", self.profile_name()), settings: self.settings.clone() };
                    self.add_profile(copy);
                }
                if button(ui, !connected && any, "Remove", "Delete the selected machine from the list") {
                    self.profiles.remove(self.active);
                    self.active = self.active.min(self.profiles.len().saturating_sub(1));
                    self.settings = self.profiles.get(self.active).map(|p| p.settings.clone()).unwrap_or_default();
                    self.pending_bed = Some(self.settings.bed);
                }
                ui.add_space(6.0);
                if button(ui, any, "Export…", "Save the selected machine's settings to a file, to use on another computer or keep as a backup") {
                    let msg = self.export_devices(false);
                    if !msg.is_empty() {
                        self.device.message = Some(msg);
                    }
                }
                if button(ui, self.profiles.len() > 1, "Export All…", "Save every machine in the list to one file") {
                    let msg = self.export_devices(true);
                    if !msg.is_empty() {
                        self.device.message = Some(msg);
                    }
                }
            });
        });
        if let Some(i) = pick {
            self.select_profile(i);
        }
    }

    /// G-code machines: lines around every job, air command, DTR, and macro buttons.
    fn machine_gcode_editor(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        egui::Grid::new("machine-gcode").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("Start G-code");
            ui.add(egui::TextEdit::multiline(&mut s.start_gcode).desired_rows(2).code_editor());
            ui.end_row();
            ui.label("End G-code");
            ui.add(egui::TextEdit::multiline(&mut s.end_gcode).desired_rows(2).code_editor());
            ui.end_row();
            ui.label("Options");
            ui.vertical(|ui| {
                ui.checkbox(&mut s.air_m7, "Air assist uses M7 (instead of M8)");
                ui.checkbox(&mut s.dtr, "Raise DTR when connecting").on_hover_text("Resets Arduino-based boards. Some machines need this off.");
            });
            ui.end_row();
        });
        ui.label("Macros");
        let mut remove = None;
        for (i, m) in s.macros.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut m.label).desired_width(140.0).hint_text("Label"));
                ui.add(egui::TextEdit::multiline(&mut m.gcode).desired_rows(1).desired_width(160.0).code_editor().hint_text("G-code"));
                if ui.small_button("🗙").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            s.macros.remove(i);
        }
        if ui.button("Add macro").clicked() {
            s.macros.push(Macro { label: format!("Macro {}", s.macros.len() + 1), gcode: String::new() });
        }
    }

    /// Rotary Setup: type, calibration, object size and a one-turn test.
    pub fn rotary_window(&mut self, ctx: &egui::Context, open: &mut bool) {
        let galvo = matches!(self.settings.controller, ControllerKind::Galvo | ControllerKind::Lp2);
        let gcode = self.settings.controller == ControllerKind::Grbl;
        let connected = self.device.connected;
        crate::dialogs::centered(ctx, "Rotary Setup").open(open).resizable(false).show(ctx, |ui| {
            if galvo {
                ui.label("Rotary isn't supported for galvo lasers or the LaserPecker 2 yet.");
                return;
            }
            let r = &mut self.settings.rotary;
            ui.checkbox(&mut r.enabled, egui::RichText::new("Enable rotary").strong());
            egui::Grid::new("rotary-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Type");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut r.kind, RotaryKind::Roller, "Roller");
                    ui.radio_value(&mut r.kind, RotaryKind::Chuck, "Chuck");
                });
                ui.end_row();
                ui.label("Travel per rotation");
                ui.add(egui::DragValue::new(&mut r.travel_per_rotation).range(1.0..=10_000.0).speed(0.1).suffix(" mm"))
                    .on_hover_text("How far the controller moves the rotary's axis for one full turn. Use \"Rotate once\" to check: adjust until it turns exactly 360°.");
                ui.end_row();
                match r.kind {
                    RotaryKind::Roller => {
                        ui.label("Roller diameter");
                        ui.add(egui::DragValue::new(&mut r.roller_diameter).range(1.0..=500.0).speed(0.1).suffix(" mm")).on_hover_text("The driven roller, not the object");
                        ui.end_row();
                    }
                    RotaryKind::Chuck => {
                        ui.label("Object diameter");
                        ui.add(egui::DragValue::new(&mut r.object_diameter).range(1.0..=1000.0).speed(0.1).suffix(" mm"));
                        ui.end_row();
                        ui.label("Circumference");
                        ui.label(format!("{:.1} mm", r.surface_per_turn()));
                        ui.end_row();
                    }
                }
                if gcode {
                    ui.label("Axis");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut r.a_axis, false, "Replaces Y");
                        ui.radio_value(&mut r.a_axis, true, "A axis (4th axis)");
                    });
                    ui.end_row();
                }
            });
            ui.weak(match r.kind {
                RotaryKind::Roller => "Roller: the design's height wraps around the object; the object size doesn't matter.",
                RotaryKind::Chuck => "Chuck: set the object's diameter at the spot being engraved.",
            });
            ui.separator();
            let (travel, a_axis) = (r.travel_per_rotation, r.a_axis && gcode);
            ui.horizontal(|ui| {
                if ui.add_enabled(connected, egui::Button::new("Rotate once")).on_hover_text("Turns the rotary one full revolution (laser off). It should end exactly where it started.").clicked() {
                    if a_axis {
                        for line in ["G91", &format!("G0 A{travel:.3}"), "G90"] {
                            self.device.send(DeviceCmd::Console(line.to_string()));
                        }
                    } else {
                        self.device.send(DeviceCmd::JogBy(0.0, travel, self.settings.rapid_speed));
                    }
                }
                ui.weak("Mark the object with a pen line first, then check it comes back round to it.");
            });
        });
    }

    /// Whether the selected machine connects through a serial (USB) port.
    fn uses_serial_port(&self) -> bool {
        match self.settings.controller {
            ControllerKind::Grbl | ControllerKind::Lp2 => true,
            ControllerKind::Ruida => self.settings.ruida_usb,
            ControllerKind::Galvo => false,
        }
    }

    /// A drop-down of serial ports for the selected machine. The list is read again each time it's
    /// opened, so a laser plugged in after BurnCraft started shows up.
    fn port_combo(&mut self, ui: &mut egui::Ui, id: &str, width: f32, enabled: bool) {
        let current = self.settings.serial_port.clone();
        let missing = !current.is_empty() && !self.ports.contains(&current);
        let shown = if current.is_empty() { "(Choose port)".to_string() } else if missing { format!("{current} (not found)") } else { current.clone() };
        let mut chosen = None;
        let mut refresh = false;
        ui.add_enabled_ui(enabled, |ui| {
            // Truncate within `width`, not within the rest of the row (see `panel`).
            ui.set_max_width(width);
            let r = egui::ComboBox::from_id_salt(id).selected_text(shown.clone()).width(width).truncate().show_ui(ui, |ui| {
                if self.ports.is_empty() {
                    ui.weak("No serial ports found. Is the laser plugged in and switched on?");
                }
                for p in &self.ports {
                    if ui.selectable_label(*p == current, p).clicked() {
                        chosen = Some(p.clone());
                    }
                }
                ui.separator();
                refresh = ui.button("⟳ Look again").clicked();
            });
            refresh |= r.response.clicked();
            r.response.on_hover_text(format!("{shown}: the USB (serial) port this laser is plugged into"));
        });
        if refresh {
            self.ports = bc_grbl::list_ports();
        }
        if let Some(p) = chosen {
            self.settings.serial_port = p;
        }
    }

    /// A "Serial port" grid row (Devices window).
    fn port_row(&mut self, ui: &mut egui::Ui) {
        ui.label("Serial port");
        self.port_combo(ui, "port", 160.0, true);
        ui.end_row();
    }

    fn custom_gcode_editor(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.settings.custom_gcode;
        egui::Grid::new("custom-gcode").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("Job start");
            ui.add(egui::TextEdit::multiline(&mut c.start).desired_rows(2).code_editor());
            ui.end_row();
            ui.label("Job end");
            ui.add(egui::TextEdit::multiline(&mut c.end).desired_rows(2).code_editor());
            ui.end_row();
            ui.label("Power");
            ui.checkbox(&mut c.inline_power, "S value on each move");
            ui.end_row();
            if !c.inline_power {
                ui.label("Laser on / off");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut c.laser_on).desired_width(70.0));
                    ui.add(egui::TextEdit::singleline(&mut c.laser_off).desired_width(70.0));
                });
                ui.end_row();
            }
            ui.label("Air on / off");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut c.air_on).desired_width(70.0));
                ui.add(egui::TextEdit::singleline(&mut c.air_off).desired_width(70.0));
            });
            ui.end_row();
            ui.label("Home");
            ui.add(egui::TextEdit::singleline(&mut c.home).desired_width(150.0));
            ui.end_row();
        });
    }
}

/// Converts a job to machine coordinates (mm) for the device worker.
pub fn to_machine_job(job: &Job, to_machine: impl Fn(Point) -> (f64, f64)) -> Vec<MachineLayer> {
    job.layers
        .iter()
        .map(|l| MachineLayer {
            speed: l.settings.speed,
            min_power: l.settings.min_power,
            max_power: l.settings.max_power,
            air_assist: l.settings.air_assist,
            frequency: l.settings.frequency,
            color: l.settings.rgb(l.index),
            raster: matches!(l.settings.mode, bc_doc::LayerMode::Fill | bc_doc::LayerMode::Image),
            passes: l.settings.passes.max(1),
            paths: l.paths.iter().map(|p| (p.points.iter().map(|q| to_machine(*q)).collect(), p.power)).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set `BC_LBDEV` to a LightBurn device file to check it converts (e.g. LaserPecker's LP2 Plus file).
    #[test]
    #[ignore = "needs a device file"]
    fn sample_device_file() {
        let data = std::fs::read(std::env::var("BC_LBDEV").unwrap()).unwrap();
        for d in bc_io::lbdev::parse(&data).unwrap() {
            let p = DeviceProfile::from_lbdev(&d).unwrap();
            let s = &p.settings;
            println!("{}: {:?} {:?} bed {:?} origin {:?} {} baud on {:?} S{} dtr {} home {} macros {:?}", p.name, s.controller, s.dialect, s.bed, s.origin, s.baud, s.serial_port, s.s_max, s.dtr, s.home_on_connect, s.macros.iter().map(|m| (&m.label, &m.gcode)).collect::<Vec<_>>());
        }
    }

    #[test]
    fn long_port_names_do_not_widen_the_panel() {
        let settings = LaserSettings { controller: ControllerKind::Grbl, serial_port: "/dev/cu.usbmodemNEJE_3_0_1234567890_ABCDEFGH".into(), ..LaserSettings::default() };
        let mut laser = Laser::new(None, Some(settings), Device::spawn(egui::Context::default()));
        laser.profiles[0].name = "A machine with a rather long name indeed".into();
        let ctx = egui::Context::default();
        let mut widths = Vec::new();
        for _ in 0..30 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let r = egui::Panel::right("laser-test").default_size(320.0).show(ui, |ui| {
                    laser.panel(ui);
                });
                widths.push(r.response.rect.width());
            });
            out.textures_delta.clear();
        }
        let (first, last) = (widths[2], *widths.last().unwrap());
        assert!(last <= first + 1.0 && last < 400.0, "the panel grew: {widths:?}");
    }

    #[test]
    fn first_run_has_no_machine() {
        let mut laser = Laser::new(None, None, Device::spawn(egui::Context::default()));
        assert!(!laser.has_machine());
        assert_eq!(laser.profile_name(), "No machine");
        assert!(laser.store().profiles.is_empty(), "saving an empty list works");
        // A saved empty list stays empty, even with old single-machine settings around.
        let again = Laser::new(Some(laser.store()), Some(LaserSettings::default()), Device::spawn(egui::Context::default()));
        assert!(!again.has_machine());
        // Older versions' single machine still comes across.
        assert!(Laser::new(None, Some(LaserSettings::default()), Device::spawn(egui::Context::default())).has_machine());
    }

    fn laser(settings: LaserSettings) -> Laser {
        Laser::new(None, Some(settings), Device::spawn(egui::Context::default()))
    }

    fn square_job(y0: f64) -> Job {
        let pts = vec![Point::new(10.0, y0), Point::new(20.0, y0), Point::new(20.0, y0 + 10.0), Point::new(10.0, y0 + 10.0), Point::new(10.0, y0)];
        Job { layers: vec![bc_job::JobLayer { index: 0, settings: Layer::new(0), paths: vec![bc_job::JobPath::burn(pts)] }], skipped: Vec::new() }
    }

    #[test]
    fn start_from_places_the_job_origin() {
        let bed = (300.0, 200.0);
        let job = square_job(30.0); // design x 10..20, y 30..40
        let corner = |l: &Laser| {
            let map = l.job_mapper(&job, bed).unwrap();
            let pts = &to_machine_job(&job, map)[0].paths[0].0;
            let x0 = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
            let y0 = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
            (x0, y0)
        };
        let s = LaserSettings { origin: OriginCorner::TopLeft, ..Default::default() };
        // Absolute: where it is on the bed.
        assert_eq!(corner(&laser(s.clone())), (10.0, 30.0));
        // User origin with the top-left job origin: the job's top-left goes there.
        let user = LaserSettings { start_from: StartFrom::UserOrigin, user_origin: Some((100.0, 50.0)), job_origin: 0, ..s.clone() };
        assert_eq!(corner(&laser(user.clone())), (100.0, 50.0));
        // Centre origin: the job's centre goes there.
        assert_eq!(corner(&laser(LaserSettings { job_origin: 4, ..user.clone() })), (95.0, 45.0));
        // Fits the bed there, but not near the far corner.
        assert!(laser(user.clone()).check_placed(&job, bed).is_ok());
        assert!(laser(LaserSettings { user_origin: Some((295.0, 195.0)), ..user.clone() }).check_placed(&job, bed).is_err());
        // No user origin, or no known head position: a clear error rather than a guess.
        assert!(laser(LaserSettings { user_origin: None, ..user }).job_mapper(&job, bed).is_err());
        let current = laser(LaserSettings { start_from: StartFrom::CurrentPosition, ..s.clone() });
        assert!(current.job_mapper(&job, bed).is_err());
        let mut current = current;
        current.device.state = Some(crate::device::MachineStatus { x: 150.0, y: 120.0, state: String::new() });
        current.settings.job_origin = 8; // bottom-right
        let map = current.job_mapper(&job, bed).unwrap();
        assert_eq!(map(Point::new(20.0, 40.0)), (150.0, 120.0));
        // A design placed off the bed is fine in a relative mode, as long as it lands on the bed.
        let off_bed = square_job(-50.0);
        assert!(laser(s.clone()).check_placed(&off_bed, bed).is_err());
        assert!(current.check_placed(&off_bed, bed).is_ok());
    }

    #[test]
    fn rotary_stretches_y_about_the_top_of_the_job() {
        let mut s = LaserSettings { origin: OriginCorner::TopLeft, ..Default::default() };
        // Roller whose circumference is half the travel per turn: Y doubles.
        s.rotary = Rotary { enabled: true, kind: RotaryKind::Roller, travel_per_rotation: 100.0, roller_diameter: 50.0 / std::f64::consts::PI, ..Default::default() };
        let l = laser(s.clone());
        assert!((l.rotary_scale().unwrap() - 2.0).abs() < 1e-9);
        let job = square_job(30.0);
        let ys: Vec<f64> = to_machine_job(&job, l.job_mapper(&job, (300.0, 200.0)).unwrap())[0].paths[0].0.iter().map(|p| p.1).collect();
        let (lo, hi) = (ys.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MIN, f64::max));
        assert!((hi - lo - 20.0).abs() < 1e-9, "10 mm of design → 20 mm of Y: {ys:?}");
        // The top edge stays put, so the job starts where it was placed.
        let plain = to_machine_job(&job, laser(LaserSettings { rotary: Rotary::default(), ..s.clone() }).job_mapper(&job, (300.0, 200.0)).unwrap());
        assert!(ys.contains(&plain[0].paths[0].0[0].1));
        // Off for galvos, and when disabled.
        assert!(laser(LaserSettings { controller: ControllerKind::Galvo, ..s.clone() }).rotary_scale().is_none());
        s.rotary.enabled = false;
        assert!(laser(s).rotary_scale().is_none());
    }

    #[test]
    fn chuck_scale_uses_the_object() {
        let r = Rotary { kind: RotaryKind::Chuck, travel_per_rotation: 360.0, object_diameter: 120.0 / std::f64::consts::PI, ..Default::default() };
        assert!((r.surface_per_turn() - 120.0).abs() < 1e-9 && (r.scale() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn lbdev_driver_and_origin_mapping() {
        let d = |driver: &str, mx, my| bc_io::lbdev::LbDevice { name: "x".into(), driver: driver.into(), width: 300.0, height: 200.0, mirror_x: mx, mirror_y: my, ..Default::default() };
        let p = DeviceProfile::from_lbdev(&d("GRBL", false, true)).unwrap().settings;
        assert_eq!((p.controller, p.dialect, p.origin, p.bed, p.s_max), (ControllerKind::Grbl, Dialect::Grbl, OriginCorner::TopLeft, (300.0, 200.0), 1000.0));
        assert_eq!(DeviceProfile::from_lbdev(&d("Marlin", false, false)).unwrap().settings.origin, OriginCorner::BottomLeft);
        assert_eq!(DeviceProfile::from_lbdev(&d("Ruida", true, true)).unwrap().settings.controller, ControllerKind::Ruida);
        assert_eq!(DeviceProfile::from_lbdev(&d("Smoothieware", false, false)).unwrap().settings.s_max, 1.0);
        assert!(DeviceProfile::from_lbdev(&d("TopWisdom", false, false)).is_err());
        // Newer LightBurn files call the network connection "UDP".
        let udp = bc_io::lbdev::LbDevice { connection: "UDP".into(), ..d("Ruida", true, true) };
        assert!(!DeviceProfile::from_lbdev(&udp).unwrap().settings.ruida_usb);
        let usb = bc_io::lbdev::LbDevice { connection: "USB".into(), ..d("Ruida", true, true) };
        assert!(DeviceProfile::from_lbdev(&usb).unwrap().settings.ruida_usb);
    }

    #[test]
    fn device_files_round_trip_every_setting() {
        let mut s = LaserSettings { controller: ControllerKind::Ruida, ip: "10.0.0.7".into(), rapid_speed: 123.0, bed: (1255.0, 900.0), ..Default::default() };
        s.macros.push(Macro { label: "Fan".into(), gcode: "M8".into() });
        let profiles = vec![DeviceProfile { name: "Shop CO2".into(), settings: s }, DeviceProfile { name: "Diode".into(), settings: LaserSettings::default() }];
        let text = device_file(&profiles);
        let (back, problems) = devices_from_file("machines.bcdev", text.as_bytes()).unwrap();
        assert!(problems.is_empty());
        assert_eq!(back.len(), 2);
        assert_eq!(serde_json::to_value(&back[0]).unwrap(), serde_json::to_value(&profiles[0]).unwrap(), "nothing is lost");
        // Recognised even with another extension.
        assert_eq!(devices_from_file("copy.json", text.as_bytes()).unwrap().0.len(), 2);
        assert!(devices_from_file("x.bcdev", b"{\"format\":\"other\",\"version\":1,\"written_by\":\"\",\"devices\":[]}").is_err());
    }

    /// Prints what the device files listed in `BC_DEVFILES` (separated by `;`) import as.
    #[test]
    #[ignore = "diagnostic"]
    fn device_file_probe() {
        for path in std::env::var("BC_DEVFILES").unwrap().split(';') {
            let bytes = std::fs::read(path).unwrap();
            let (devices, problems) = devices_from_file(path, &bytes).unwrap();
            for d in devices {
                let s = &d.settings;
                println!(
                    "{path}: \"{}\" {:?} usb={} ip={} port={:?} baud={} bed={:?} origin={:?}",
                    d.name, s.controller, s.ruida_usb, s.ip, s.serial_port, s.baud, s.bed, s.origin
                );
            }
            println!("  problems: {problems:?}");
        }
    }

    #[test]
    fn lightburn_device_files_and_bundles_import() {
        let lbdev = br#"{"DeviceList":[{"DisplayName":"Big Ruida","Name":"Ruida","Type":"UDP","Width":1255,"Height":900,"MirrorX":true,"MirrorY":true}]}"#;
        let (d, _) = devices_from_file("x.lbdev", lbdev).unwrap();
        assert_eq!((d[0].name.as_str(), d[0].settings.controller, d[0].settings.bed), ("Big Ruida", ControllerKind::Ruida, (1255.0, 900.0)));
        // The same file inside a zip bundle.
        let mut zipped = std::io::Cursor::new(Vec::new());
        {
            use std::io::Write as _;
            let mut zip = zip::ZipWriter::new(&mut zipped);
            zip.start_file("LightBurnSoftware.json", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b"{}").unwrap();
            zip.start_file("abc.lbdev", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(lbdev).unwrap();
            zip.finish().unwrap();
        }
        let (d, _) = devices_from_file("export.lbzip", zipped.get_ref()).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].name, "Big Ruida");
    }
}
