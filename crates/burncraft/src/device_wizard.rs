//! Adding machines: the step-by-step "Create Manually" wizard and the "Find My Laser" USB scan.

use crate::laser::{ControllerKind, DeviceProfile, LaserSettings};
use crate::units::{Speed, SpeedUnit, Units};
use bc_grbl::Dialect;
use bc_job::OriginCorner;
use bc_ruida::RuidaModel;
use eframe::egui;
use std::net::Ipv4Addr;
use std::sync::mpsc;

/// The controllers offered in the wizard: a family, the G-code flavour, and a short description.
const CHOICES: [(ControllerKind, Dialect, &str, &str); 8] = [
    (ControllerKind::Grbl, Dialect::Grbl, "GRBL", "Most diode lasers and hobby CNC boards (GRBL 1.1 or later, laser mode)"),
    (ControllerKind::Grbl, Dialect::GrblM3, "GRBL-M3", "GRBL boards without variable-power laser mode, or older GRBL"),
    (ControllerKind::Grbl, Dialect::Marlin, "Marlin", "3D-printer style boards running Marlin firmware"),
    (ControllerKind::Grbl, Dialect::Smoothieware, "Smoothieware", "Smoothieboard and other boards running Smoothieware"),
    (ControllerKind::Grbl, Dialect::Custom, "Custom G-code", "Any other G-code controller: you set the commands"),
    (ControllerKind::Ruida, Dialect::Grbl, "Ruida", "CO₂ lasers with a Ruida DSP controller (RDC644x, 654x, 634x…), over Ethernet or USB"),
    (ControllerKind::Galvo, Dialect::Grbl, "Galvo (EZCad2 board)", "Fiber, UV and CO₂ galvo markers with a BJJCZ / EZCad2 USB board"),
    (ControllerKind::Lp2, Dialect::Grbl, "LaserPecker 2", "The LaserPecker LP2 over USB. Other LaserPecker models use GRBL (or import their LightBurn device file)."),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Controller,
    Connection,
    Address,
    Name,
    Origin,
    Summary,
}

/// The "Create Manually" wizard.
pub struct Wizard {
    step: Step,
    choice: usize,
    pub settings: LaserSettings,
    pub name: String,
    /// The result of "Check connection" (running while the receiver is set).
    check: Option<mpsc::Receiver<Result<String, String>>>,
    check_result: Option<Result<String, String>>,
}

/// What the wizard asks of the Devices window this frame.
pub enum WizardOutcome {
    Open,
    Cancel,
    Finish(Box<DeviceProfile>),
}

impl Wizard {
    pub fn new() -> Self {
        let mut w = Self { step: Step::Controller, choice: 0, settings: LaserSettings::default(), name: String::new(), check: None, check_result: None };
        w.apply_choice();
        w
    }

    /// Puts the chosen controller's usual settings in place.
    fn apply_choice(&mut self) {
        let (kind, dialect, label, _) = CHOICES[self.choice];
        let s = &mut self.settings;
        s.controller = kind;
        s.dialect = dialect;
        s.s_max = dialect.default_s_max();
        s.origin = kind.default_origin();
        s.speed_unit = kind.default_speed_unit();
        match kind {
            ControllerKind::Grbl => s.bed = (400.0, 400.0),
            ControllerKind::Ruida => s.bed = (600.0, 400.0),
            ControllerKind::Galvo => s.bed = (s.galvo.lens_size, s.galvo.lens_size),
            ControllerKind::Lp2 => s.bed = bc_lp2::FIELD,
        }
        self.name = label.to_string();
    }

    /// The steps for the chosen controller, in order.
    fn steps(&self) -> Vec<Step> {
        let mut steps = vec![Step::Controller];
        if self.settings.controller == ControllerKind::Ruida {
            steps.push(Step::Connection);
        }
        if self.settings.controller != ControllerKind::Galvo {
            steps.push(Step::Address);
        }
        steps.push(Step::Name);
        if matches!(self.settings.controller, ControllerKind::Ruida | ControllerKind::Grbl) {
            steps.push(Step::Origin);
        }
        steps.push(Step::Summary);
        steps
    }

    /// Why the current step can't be left yet, if it can't.
    fn blocker(&self) -> Option<&'static str> {
        match self.step {
            Step::Address if self.settings.controller == ControllerKind::Ruida && !self.settings.ruida_usb && self.settings.ip.trim().parse::<Ipv4Addr>().is_err() => {
                Some("Enter the controller's IP address, e.g. 192.168.1.100")
            }
            Step::Name if self.name.trim().is_empty() => Some("Give the machine a name"),
            _ => None,
        }
    }

    /// The finished machine.
    pub fn profile(&self) -> DeviceProfile {
        let mut settings = self.settings.clone();
        settings.ip = settings.ip.trim().to_string();
        if settings.controller == ControllerKind::Galvo {
            settings.bed = (settings.galvo.lens_size, settings.galvo.lens_size);
        }
        if let Some(field) = settings.controller.fixed_field() {
            settings.bed = field;
        }
        DeviceProfile { name: self.name.trim().to_string(), settings }
    }

    /// Shows the wizard window. `ports` lists the serial ports present.
    pub fn show(&mut self, ctx: &egui::Context, ports: &[String], units: Units) -> WizardOutcome {
        if let Some(rx) = &self.check
            && let Ok(r) = rx.try_recv()
        {
            self.check_result = Some(r);
            self.check = None;
        }
        if self.check.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut outcome = WizardOutcome::Open;
        let mut open = true;
        let steps = self.steps();
        let at = steps.iter().position(|s| *s == self.step).unwrap_or(0);
        crate::dialogs::centered(ctx, "New Machine").open(&mut open).resizable(false).auto_sized().collapsible(false).show(ctx, |ui| {
            // A set width, so long text wraps instead of widening the window.
            ui.set_width(440.0);
            ui.weak(format!("Step {} of {}", at + 1, steps.len()));
            ui.add_space(4.0);
            match self.step {
                Step::Controller => self.controller_step(ui),
                Step::Connection => {
                    ui.heading("How is it connected?");
                    ui.radio_value(&mut self.settings.ruida_usb, false, "Ethernet (network cable or Wi-Fi bridge)");
                    ui.radio_value(&mut self.settings.ruida_usb, true, "USB cable");
                    ui.add_space(4.0);
                    ui.weak("Ethernet is more reliable for long jobs. Over USB the controller shows up as a serial port.");
                }
                Step::Address => self.address_step(ui, ports),
                Step::Name => {
                    ui.heading("Name, size and units");
                    egui::Grid::new("wizard-name").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(220.0));
                        ui.end_row();
                        if self.settings.controller == ControllerKind::Galvo {
                            ui.label("Lens field size");
                            units.drag(ui, &mut self.settings.galvo.lens_size, 10.0..=600.0);
                            ui.end_row();
                        } else if let Some((w, h)) = self.settings.controller.fixed_field() {
                            ui.label("Marking field");
                            ui.label(format!("{} × {}", units.format(w), units.format(h)));
                            ui.end_row();
                        } else {
                            ui.label("Work area");
                            ui.horizontal(|ui| {
                                units.drag(ui, &mut self.settings.bed.0, 10.0..=5000.0);
                                ui.label("×");
                                units.drag(ui, &mut self.settings.bed.1, 10.0..=5000.0);
                            });
                            ui.end_row();
                        }
                        if self.settings.controller == ControllerKind::Lp2 {
                            return;
                        }
                        ui.label("Speeds in");
                        ui.horizontal(|ui| {
                            for (per, usual) in [(SpeedUnit::PerSecond, "usual for CO₂ and galvo"), (SpeedUnit::PerMinute, "usual for diode")] {
                                ui.radio_value(&mut self.settings.speed_unit, per, Speed::new(units, per).unit()).on_hover_text(usual);
                            }
                        });
                        ui.end_row();
                    });
                    ui.add_space(4.0);
                    if self.settings.controller != ControllerKind::Lp2 {
                        ui.weak("Speeds can be shown per second or per minute; it only changes the display. Millimetres or inches is set under View.");
                    }
                    ui.weak(match self.settings.controller {
                        ControllerKind::Galvo => "The size of the square the lens can mark, as printed on the lens (e.g. 110 mm).",
                        ControllerKind::Ruida => "Width × height the head can travel. A Ruida reports its own size when it connects.",
                        ControllerKind::Grbl => "Width × height the head can travel (GRBL: $130 × $131).",
                        ControllerKind::Lp2 => "Speeds are set as depth (1-100 %), as in the LaserPecker app: deeper is slower.",
                    });
                }
                Step::Origin => {
                    ui.heading("Where is the origin?");
                    ui.label("The corner the head goes to when it homes (where X and Y are 0):");
                    ui.add_space(4.0);
                    egui::Grid::new("wizard-origin").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                        for row in OriginCorner::ALL.chunks(2) {
                            for &o in row {
                                ui.radio_value(&mut self.settings.origin, o, o.label());
                            }
                            ui.end_row();
                        }
                    });
                    ui.add_space(6.0);
                    ui.checkbox(&mut self.settings.home_on_connect, "Home the machine when connecting");
                    ui.weak("Most diode lasers have the origin at the front left; most CO₂ lasers at the back right.");
                }
                Step::Summary => {
                    ui.heading("Ready to add");
                    for line in summary(&self.profile(), units) {
                        ui.label(line);
                    }
                    ui.add_space(4.0);
                    ui.weak("Everything can be changed later with Edit in the Devices window.");
                }
            }
            ui.add_space(8.0);
            ui.separator();
            let blocker = self.blocker();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    outcome = WizardOutcome::Cancel;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.step == Step::Summary {
                        if ui.button(egui::RichText::new("Finish").strong()).clicked() {
                            outcome = WizardOutcome::Finish(Box::new(self.profile()));
                        }
                    } else if ui.add_enabled(blocker.is_none(), egui::Button::new("Next  ›")).clicked() {
                        self.step = steps[(at + 1).min(steps.len() - 1)];
                    }
                    if ui.add_enabled(at > 0, egui::Button::new("‹  Back")).clicked() {
                        self.step = steps[at - 1];
                    }
                    if let Some(why) = blocker {
                        ui.weak(why);
                    }
                });
            });
        });
        if !open {
            outcome = WizardOutcome::Cancel;
        }
        outcome
    }

    fn controller_step(&mut self, ui: &mut egui::Ui) {
        ui.heading("What controller does the laser have?");
        ui.add_space(4.0);
        let mut picked = None;
        for (i, (_, _, label, about)) in CHOICES.iter().enumerate() {
            let r = ui.radio(self.choice == i, egui::RichText::new(*label).strong());
            ui.indent(("about", i), |ui| ui.weak(*about));
            if r.clicked() {
                picked = Some(i);
            }
        }
        if let Some(i) = picked
            && i != self.choice
        {
            self.choice = i;
            self.apply_choice();
        }
    }

    fn address_step(&mut self, ui: &mut egui::Ui, ports: &[String]) {
        let s = &mut self.settings;
        let ethernet = s.controller == ControllerKind::Ruida && !s.ruida_usb;
        if ethernet {
            ui.heading("Controller IP address");
            egui::Grid::new("wizard-ip").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("IP address");
                ui.add(egui::TextEdit::singleline(&mut s.ip).desired_width(140.0));
                ui.end_row();
                ui.label("Model");
                egui::ComboBox::from_id_salt("wizard-model").selected_text(s.ruida_model.label()).show_ui(ui, |ui| {
                    for m in RuidaModel::ALL {
                        ui.selectable_value(&mut s.ruida_model, m, m.label());
                    }
                });
                ui.end_row();
            });
            ui.weak("On the controller's panel: Menu → System Settings → Network shows the address. The computer must be on the same network.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let ip = s.ip.trim().parse::<Ipv4Addr>();
                if ui.add_enabled(ip.is_ok() && self.check.is_none(), egui::Button::new("Check connection")).clicked()
                    && let Ok(ip) = ip
                {
                    let model = s.ruida_model;
                    let (tx, rx) = mpsc::channel();
                    std::thread::spawn(move || {
                        let r = bc_ruida::Controller::connect(ip, model).and_then(|c| c.card_id()).map(|id| format!("The controller answered (card ID {id:X})")).map_err(|e| format!("No answer: {e}"));
                        let _ = tx.send(r);
                    });
                    self.check = Some(rx);
                    self.check_result = None;
                }
                if self.check.is_some() {
                    ui.spinner();
                    ui.weak("Checking…");
                }
                match &self.check_result {
                    Some(Ok(m)) => {
                        ui.colored_label(egui::Color32::from_rgb(60, 170, 80), m);
                    }
                    Some(Err(m)) => {
                        ui.colored_label(ui.visuals().warn_fg_color, m);
                    }
                    None => {}
                }
            });
            return;
        }
        ui.heading("Which port?");
        egui::Grid::new("wizard-port").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Serial port");
            let shown = if s.serial_port.is_empty() { "(Choose later)".to_string() } else { s.serial_port.clone() };
            egui::ComboBox::from_id_salt("wizard-port").selected_text(shown).width(160.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.serial_port, String::new(), "(Choose later)");
                for p in ports {
                    ui.selectable_value(&mut s.serial_port, p.clone(), p);
                }
            });
            ui.end_row();
            if s.controller == ControllerKind::Grbl {
                ui.label("Baud rate");
                egui::ComboBox::from_id_salt("wizard-baud").selected_text(s.baud.to_string()).show_ui(ui, |ui| {
                    for b in [57_600, 115_200, 230_400, 250_000, 460_800, 921_600] {
                        ui.selectable_value(&mut s.baud, b, b.to_string());
                    }
                });
                ui.end_row();
            } else if s.controller == ControllerKind::Ruida {
                ui.label("Model");
                egui::ComboBox::from_id_salt("wizard-model").selected_text(s.ruida_model.label()).show_ui(ui, |ui| {
                    for m in RuidaModel::ALL {
                        ui.selectable_value(&mut s.ruida_model, m, m.label());
                    }
                });
                ui.end_row();
            }
        });
        ui.weak(if s.controller == ControllerKind::Lp2 {
            "The port can also be picked in the Laser panel each time you connect. The LaserPecker 2 shows up as a CH340 serial port."
        } else {
            "The port can also be picked in the Laser panel each time you connect. Most GRBL boards use 115200 baud."
        });
    }
}

/// One-line descriptions of a machine, for the device list and the wizard's last step.
pub fn summary(p: &DeviceProfile, units: Units) -> Vec<String> {
    let s = &p.settings;
    let port = |p: &str| if p.is_empty() { "port chosen when connecting".to_string() } else { p.to_string() };
    let connection = match s.controller {
        ControllerKind::Ruida if s.ruida_usb => format!("Ruida ({}) over USB, {}", s.ruida_model.label(), port(&s.serial_port)),
        ControllerKind::Ruida => format!("Ruida ({}) over Ethernet at {}", s.ruida_model.label(), s.ip),
        ControllerKind::Grbl => format!("{} over USB, {} at {} baud", s.dialect.label(), port(&s.serial_port), s.baud),
        ControllerKind::Galvo => format!("Galvo ({}) over USB", s.galvo.source.label()),
        ControllerKind::Lp2 => format!("LaserPecker 2 over USB, {}", port(&s.serial_port)),
    };
    let size = if s.controller == ControllerKind::Galvo {
        format!("{} lens field", units.format(s.galvo.lens_size))
    } else if let Some((w, h)) = s.controller.fixed_field() {
        format!("{} × {} marking field", units.format(w), units.format(h))
    } else {
        format!("{} × {} work area, origin at {}", units.format(s.bed.0), units.format(s.bed.1), s.origin.label().to_lowercase())
    };
    let speeds = if s.controller == ControllerKind::Lp2 { "Speeds set as depth (%)".to_string() } else { format!("Speeds in {}", Speed::new(units, s.speed_unit).unit()) };
    let mut lines = vec![connection, size, speeds];
    if s.home_on_connect {
        lines.push("Homes when connecting".into());
    }
    lines
}

/// The family and type shown under a machine's name in the list, e.g. "Ruida · DSP".
pub fn subtitle(s: &LaserSettings) -> String {
    match s.controller {
        ControllerKind::Ruida => format!("Ruida · {}", if s.ruida_usb { "USB" } else { "Ethernet" }),
        ControllerKind::Grbl => format!("{} · G-code", s.dialect.label()),
        ControllerKind::Galvo => "Galvo · EZCad2 board".into(),
        ControllerKind::Lp2 => "LaserPecker 2 · USB".into(),
    }
}

/// A controller found by the scan.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub port: String,
    pub controller: ControllerKind,
    pub dialect: Dialect,
    pub baud: u32,
    pub ruida_model: RuidaModel,
    /// What the controller called itself.
    pub info: String,
}

impl Found {
    /// A new machine for this controller, with its usual settings.
    pub fn profile(&self) -> DeviceProfile {
        let mut s = LaserSettings { controller: self.controller, dialect: self.dialect, serial_port: self.port.clone(), baud: self.baud, ruida_model: self.ruida_model, ..LaserSettings::default() };
        s.origin = self.controller.default_origin();
        s.s_max = self.dialect.default_s_max();
        s.speed_unit = self.controller.default_speed_unit();
        if self.controller == ControllerKind::Ruida {
            s.ruida_usb = true;
        } else if let Some(field) = self.controller.fixed_field() {
            s.bed = field;
        } else {
            s.bed = (400.0, 400.0);
        }
        let name = match self.controller {
            ControllerKind::Ruida => "Ruida (USB)".to_string(),
            ControllerKind::Lp2 => "LaserPecker 2".to_string(),
            _ => self.dialect.label().to_string(),
        };
        DeviceProfile { name, settings: s }
    }

    pub fn kind_label(&self) -> &'static str {
        match self.controller {
            ControllerKind::Ruida => "Ruida",
            ControllerKind::Lp2 => "LaserPecker 2",
            _ => self.dialect.label(),
        }
    }
}

enum ScanEvent {
    Checking(String),
    Found(Found),
    /// A port that couldn't be opened, and why.
    Skipped(String, String),
    Done(usize),
}

/// "Find My Laser": looks at every serial port in the background.
pub struct Finder {
    rx: mpsc::Receiver<ScanEvent>,
    found: Vec<Found>,
    skipped: Vec<(String, String)>,
    status: String,
    done: bool,
    selected: Option<usize>,
}

/// What the scan window asks of the Devices window this frame.
pub enum FinderOutcome {
    Open,
    Close,
    Add(Box<DeviceProfile>),
}

impl Finder {
    /// Starts scanning. `skip` is a port already in use by the connected machine.
    pub fn start(skip: Option<String>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Lasers connect over USB; built-in serial ports (/dev/ttyS* on Linux) are skipped.
            let ports: Vec<_> = bc_grbl::list_port_info().into_iter().filter(|p| Some(&p.name) != skip.as_ref() && !p.name.starts_with("/dev/ttyS")).collect();
            let count = ports.len();
            for p in ports {
                let label = if p.description.is_empty() { p.name.clone() } else { format!("{} ({})", p.name, p.description) };
                let _ = tx.send(ScanEvent::Checking(label));
                // Ruida controllers use FTDI USB chips; ask them first, since their replies are
                // binary and G-code text would mean nothing to them.
                if p.is_ftdi()
                    && let Some(found) = probe_ruida(&p.name)
                {
                    let _ = tx.send(ScanEvent::Found(found));
                    continue;
                }
                // The LaserPecker 2 (CH340) answers binary frames, not G-code.
                if p.is_ch340() && bc_lp2::Controller::probe(&p.name).unwrap_or(false) {
                    let found = Found { port: p.name.clone(), controller: ControllerKind::Lp2, dialect: Dialect::Grbl, baud: bc_lp2::BAUD, ruida_model: RuidaModel::default(), info: "LaserPecker 2".into() };
                    let _ = tx.send(ScanEvent::Found(found));
                    continue;
                }
                match bc_grbl::probe::probe(&p.name) {
                    Ok(Some(f)) => {
                        let _ = tx.send(ScanEvent::Found(Found { port: f.port, controller: ControllerKind::Grbl, dialect: f.dialect, baud: f.baud, ruida_model: RuidaModel::default(), info: f.info }));
                    }
                    Ok(None) => {}
                    Err(why) => {
                        let _ = tx.send(ScanEvent::Skipped(p.name.clone(), why));
                    }
                }
            }
            let _ = tx.send(ScanEvent::Done(count));
        });
        Self { rx, found: Vec::new(), skipped: Vec::new(), status: "Looking for serial ports…".into(), done: false, selected: None }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> FinderOutcome {
        while let Ok(e) = self.rx.try_recv() {
            match e {
                ScanEvent::Checking(port) => self.status = format!("Checking {port}…"),
                ScanEvent::Found(f) => {
                    self.found.push(f);
                    self.selected.get_or_insert(0);
                }
                ScanEvent::Skipped(port, why) => self.skipped.push((port, why)),
                ScanEvent::Done(0) => {
                    self.status = if cfg!(target_os = "linux") {
                        "No USB serial ports found. Is the laser plugged in and switched on? In a virtual machine, pass the USB device through to it.".into()
                    } else {
                        "No serial ports found. Is the laser plugged in and switched on?".into()
                    };
                    self.done = true;
                }
                ScanEvent::Done(n) => {
                    self.status = format!("Checked {n} port{}.", if n == 1 { "" } else { "s" });
                    self.done = true;
                }
            }
        }
        if !self.done {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut outcome = FinderOutcome::Open;
        let mut open = true;
        crate::dialogs::centered(ctx, "Find My Laser").open(&mut open).resizable(false).auto_sized().collapsible(false).show(ctx, |ui| {
            // A set width, so long messages wrap instead of widening the window.
            ui.set_width(460.0);
            ui.label("Looking for lasers plugged in over USB.");
            ui.weak("Network (Ethernet) controllers aren't found this way: use Create Manually and enter the IP address.");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if !self.done {
                    ui.spinner();
                }
                ui.add(egui::Label::new(&self.status).wrap());
            });
            ui.add_space(4.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_height(90.0);
                ui.set_width(ui.available_width());
                if self.found.is_empty() {
                    ui.weak(if self.done { "No lasers found." } else { "Nothing found yet." });
                    return;
                }
                egui::Grid::new("found-grid").num_columns(3).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
                    ui.strong("Controller");
                    ui.strong("Type");
                    ui.strong("Connection");
                    ui.end_row();
                    for (i, f) in self.found.iter().enumerate() {
                        if ui.selectable_label(self.selected == Some(i), &f.info).clicked() {
                            self.selected = Some(i);
                        }
                        ui.label(f.kind_label());
                        ui.label(format!("{} @ {}", f.port, f.baud));
                        ui.end_row();
                    }
                });
            });
            for (port, why) in &self.skipped {
                ui.colored_label(ui.visuals().warn_fg_color, format!("Couldn't check {port}: {why}"));
            }
            if self.done && self.found.is_empty() {
                ui.weak("Check the cable and that the laser is on. Close other programs using the port, then look again. If the board needs a driver (CH340, CP210x), install it first.");
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Close").clicked() {
                    outcome = FinderOutcome::Close;
                }
                if ui.add_enabled(self.done, egui::Button::new("Look again")).clicked() {
                    *self = Finder::start(None);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pick = self.selected.and_then(|i| self.found.get(i));
                    if ui.add_enabled(pick.is_some(), egui::Button::new(egui::RichText::new("Add Machine").strong())).clicked()
                        && let Some(f) = pick
                    {
                        outcome = FinderOutcome::Add(Box::new(f.profile()));
                    }
                });
            });
        });
        if !open {
            outcome = FinderOutcome::Close;
        }
        outcome
    }
}

/// Asks a USB port for a Ruida controller's card ID, under each model's scrambling key.
fn probe_ruida(port: &str) -> Option<Found> {
    RuidaModel::ALL.into_iter().find_map(|model| {
        let id = bc_ruida::Controller::connect_usb(port, model).ok()?.card_id().ok()?;
        Some(Found { port: port.to_string(), controller: ControllerKind::Ruida, dialect: Dialect::Grbl, baud: 115_200, ruida_model: model, info: format!("Ruida {}", model.label().split(' ').next().unwrap_or("")).trim().to_string() + &format!(" (card {id:X})") })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk_to_summary(w: &mut Wizard) -> Vec<Step> {
        let steps = w.steps();
        w.step = Step::Summary;
        steps
    }

    #[test]
    fn wizard_steps_follow_the_controller() {
        let mut w = Wizard::new();
        assert_eq!(w.steps(), vec![Step::Controller, Step::Address, Step::Name, Step::Origin, Step::Summary], "GRBL: serial port, no connection choice");
        w.choice = CHOICES.iter().position(|c| c.0 == ControllerKind::Ruida).unwrap();
        w.apply_choice();
        assert_eq!(w.steps()[1], Step::Connection);
        w.choice = CHOICES.iter().position(|c| c.0 == ControllerKind::Galvo).unwrap();
        w.apply_choice();
        assert_eq!(w.steps(), vec![Step::Controller, Step::Name, Step::Summary], "galvo: no port, no origin");
        w.choice = CHOICES.iter().position(|c| c.0 == ControllerKind::Lp2).unwrap();
        w.apply_choice();
        assert_eq!(w.steps(), vec![Step::Controller, Step::Address, Step::Name, Step::Summary], "LP2: a port, but no origin");
        assert_eq!(w.profile().settings.bed, bc_lp2::FIELD);
        assert_eq!(w.profile().settings.origin, OriginCorner::TopLeft);
    }

    #[test]
    fn wizard_builds_the_machine() {
        let mut w = Wizard::new();
        w.choice = CHOICES.iter().position(|c| c.1 == Dialect::Marlin).unwrap();
        w.apply_choice();
        w.settings.serial_port = "COM7".into();
        w.name = "  Shop diode ".into();
        walk_to_summary(&mut w);
        let p = w.profile();
        assert_eq!(p.name, "Shop diode");
        assert_eq!((p.settings.controller, p.settings.dialect, p.settings.s_max), (ControllerKind::Grbl, Dialect::Marlin, 255.0));
        assert_eq!(p.settings.origin, OriginCorner::BottomLeft);
        assert_eq!(p.settings.serial_port, "COM7");
        assert_eq!(p.settings.speed_unit, SpeedUnit::PerMinute, "G-code machines show speeds per minute");

        let mut w = Wizard::new();
        w.choice = CHOICES.iter().position(|c| c.0 == ControllerKind::Ruida).unwrap();
        w.apply_choice();
        w.step = Step::Address;
        w.settings.ip = "10.1.1.x".into();
        assert!(w.blocker().is_some(), "a bad IP address can't go on");
        w.settings.ip = " 10.1.1.90 ".into();
        assert!(w.blocker().is_none());
        let p = w.profile();
        assert_eq!((p.settings.controller, p.settings.ruida_usb, p.settings.ip.as_str()), (ControllerKind::Ruida, false, "10.1.1.90"));
        assert_eq!(p.settings.origin, OriginCorner::TopRight);
        assert_eq!(p.settings.speed_unit, SpeedUnit::PerSecond);
        w.step = Step::Name;
        w.name = " ".into();
        assert!(w.blocker().is_some(), "a name is needed");
    }

    #[test]
    fn found_controllers_become_machines() {
        let f = Found { port: "COM4".into(), controller: ControllerKind::Grbl, dialect: Dialect::Grbl, baud: 460_800, ruida_model: RuidaModel::default(), info: "Grbl 1.1h".into() };
        let p = f.profile();
        assert_eq!((p.settings.serial_port.as_str(), p.settings.baud, p.settings.controller), ("COM4", 460_800, ControllerKind::Grbl));
        let r = Found { controller: ControllerKind::Ruida, ..f };
        assert!(r.profile().settings.ruida_usb);
        assert_eq!(subtitle(&r.profile().settings), "Ruida · USB");
        assert!(summary(&r.profile(), Units::Millimetres)[0].contains("COM4"));
    }
}
