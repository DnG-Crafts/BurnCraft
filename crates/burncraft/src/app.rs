//! Application shell: state, menus, file handling, shortcuts and dialogs.

use crate::camera::{CameraAction, CameraSettings, CameraUi};
use crate::canvas::{Canvas, CanvasAction, Tool};
use crate::commands::Command;
use crate::device::Device;
use crate::dialogs::{BarcodeDialog, CircularArrayDialog, GridArrayDialog, MaterialTest, NestDialog, OffsetDialog, Outcome, Preview, PreviewAction, TestAction, TextDialog, TraceDialog};
use crate::laser::{DeviceStore, Laser, LaserAction, LaserSettings};
use crate::panels::{CutsTab, LaserTab, Preset};
use crate::units::Units;
use bc_doc::edit::{self, Side};
use bc_doc::{Document, History, LayerMode, Shape, ShapeKind, TextStyle, CUT_LAYER_COUNT, PROJECT_EXTENSION};
use bc_geom::ops::BooleanOp;
use bc_geom::{Affine, Point, Vec2};
use bc_io::fonts::FontLibrary;
use eframe::egui::{self, Color32, Key};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Single-machine settings from before the device list (read once to migrate).
const LASER_KEY: &str = "laser_settings";
const DEVICES_KEY: &str = "devices";
const REMOTE_KEY: &str = "remote";
const LIBRARY_KEY: &str = "cut_library";
const CAMERA_KEY: &str = "camera";
const VIEW_KEY: &str = "view";

/// Colour of the design area, independent of the window theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BoardTheme {
    #[default]
    MatchTheme,
    Light,
    Dark,
}

/// Editor preferences, remembered between runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewPrefs {
    pub board: BoardTheme,
    /// Snap moving shapes into line with other shapes, the bed and guides.
    pub smart_guides: bool,
    /// File > Open Recent, newest first.
    pub recent: Vec<PathBuf>,
    /// Draw images as the laser will burn them.
    pub burn_preview: bool,
    /// The folder shown in the Art tab.
    pub art_folder: Option<PathBuf>,
    pub units: Units,
    pub snap_to_grid: bool,
    pub snap_to_objects: bool,
    pub grid_mm: f64,
    pub show_rulers: bool,
    /// Show Y counted up from the bottom of the bed.
    pub y_up: bool,
    /// Selection point used by the edit bar (0..9, row-major from the top-left; 4 = centre).
    pub anchor: usize,
    pub allow_move: bool,
    pub allow_size: bool,
    pub allow_rotate: bool,
    pub allow_shear: bool,
}

impl Default for ViewPrefs {
    fn default() -> Self {
        Self {
            board: BoardTheme::MatchTheme,
            smart_guides: true,
            recent: Vec::new(),
            burn_preview: false,
            art_folder: None,
            units: Units::Millimetres,
            snap_to_grid: false,
            snap_to_objects: true,
            grid_mm: 10.0,
            show_rulers: true,
            y_up: true,
            anchor: 4,
            allow_move: true,
            allow_size: true,
            allow_rotate: true,
            allow_shear: true,
        }
    }
}

/// The one tool window open at a time (the preview has its own slot).
pub(crate) enum Dialog {
    Text(TextDialog),
    GridArray(GridArrayDialog),
    CircularArray(CircularArrayDialog),
    Offset(OffsetDialog),
    MaterialTest(MaterialTest),
    Barcode(BarcodeDialog),
    Nest(NestDialog),
    Trace(TraceDialog),
    Box(Box<crate::dialogs::BoxDialog>),
    ShapeEdit(crate::dialogs::ShapeEditDialog),
    CopyAlong(crate::dialogs::CopyAlongDialog),
    Sections(crate::dialogs::SectionsDialog),
    PlaceSlots(crate::dialogs::PlaceSlotsDialog),
    Tool(crate::tools::ToolDialog),
}

/// Current local date and time for `{date}` / `{time}` fields.
pub(crate) fn clock() -> bc_doc::vars::Clock {
    let now = chrono::Local::now();
    bc_doc::vars::Clock { date: now.format("%Y-%m-%d").to_string(), time: now.format("%H:%M").to_string() }
}

pub struct BurnCraftApp {
    pub(crate) doc: Document,
    pub(crate) history: History,
    pub(crate) selection: BTreeSet<u64>,
    pub(crate) canvas: Canvas,
    /// Layer new shapes are drawn on.
    pub(crate) current_layer: usize,
    file: Option<PathBuf>,
    /// Bumped on every edit; compared with `saved_revision` for the unsaved-changes marker.
    pub(crate) revision: u64,
    saved_revision: u64,
    pub(crate) status: String,
    pub(crate) cursor: Option<Point>,
    pub(crate) laser: Laser,
    pub(crate) fonts: FontLibrary,
    pub(crate) library: Vec<Preset>,
    pub(crate) camera: CameraUi,
    pub(crate) preset_name: String,
    pub(crate) lock_aspect: bool,
    pub(crate) dialog: Option<Dialog>,
    preview: Option<Preview>,
    pub(crate) last_text: (String, f64, bool, bool, TextStyle),
    pub(crate) clipboard: Vec<Shape>,
    pub(crate) prefs: ViewPrefs,
    pub(crate) cuts_tab: CutsTab,
    pub(crate) laser_tab: LaserTab,
    pub(crate) show_settings: bool,
    pub(crate) show_devices: bool,
    pub(crate) show_rotary: bool,
    /// The Parameters & Constraints window is open.
    pub(crate) show_params: bool,
    /// Constraints that couldn't be met, from the last solve.
    pub(crate) constraint_errors: Vec<String>,
    pub(crate) art: crate::art::ArtBrowser,
    autosaver: crate::recovery::AutoSaver,
    /// Whether the startup check for a crash-recovery file has run.
    recovery_checked: bool,
    /// Set once the user has agreed to close, so the next close request goes through.
    closing: bool,
    remote_settings: crate::remote::RemoteSettings,
    remote: Option<crate::remote::RemoteServer>,
    remote_error: Option<String>,
    /// When status and camera were last published (egui time, s).
    remote_published: f64,
    remote_camera: f64,
    /// Layer open in the Cut Settings Editor.
    pub(crate) cut_editor: Option<usize>,
    /// A layer with shapes on it being removed: (layer, where its shapes go, or None to delete them).
    pub(crate) removing_layer: Option<(usize, Option<usize>)>,
    /// Shape Builder: the areas of the selected shapes, for (revision, selection) it was made from.
    builder: Option<(u64, BTreeSet<u64>, Option<bc_geom::curveops::Builder>)>,
    /// Radius used by the Round Corners tool (mm).
    pub(crate) corner_radius: f64,
    /// Corner tool: round or chamfer.
    pub(crate) corner_style: bc_geom::CornerStyle,
    /// Rotation typed into the edit bar, applied on Enter.
    pub(crate) rotate_field: f64,
    /// Logs continuous redrawing while idle (see diagnostics.rs).
    frame_watch: crate::diagnostics::FrameWatch,
    /// The window title last set.
    window_title: String,
    /// Production queue (Laser menu).
    pub(crate) queue: crate::queue::ProductionQueue,
    /// PixScan window (Tools menu).
    pub(crate) pixscan: crate::pixscan::PixScan,
    /// Pictures being imported, waiting for "how will you use it?" (the first is shown).
    uploads: Vec<crate::upload::Upload>,
    /// A file named on the command line ("Open with BurnCraft"), opened on the first frame.
    startup_file: Option<PathBuf>,
}

/// Clearer text than egui's defaults in both themes: labels and hints nearly as strong as the
/// widgets they describe, as in other desktop design programs. Backgrounds are unchanged.
fn readable_text(ctx: &egui::Context) {
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        let w = &mut s.visuals.widgets;
        w.noninteractive.fg_stroke.color = Color32::from_gray(218);
        w.inactive.fg_stroke.color = Color32::from_gray(228);
        w.hovered.fg_stroke.color = Color32::from_gray(245);
        s.visuals.weak_text_color = Some(Color32::from_gray(160));
        s.visuals.disabled_alpha = 0.6;
    });
    ctx.style_mut_of(egui::Theme::Light, |s| {
        let w = &mut s.visuals.widgets;
        w.noninteractive.fg_stroke.color = Color32::from_gray(25);
        w.inactive.fg_stroke.color = Color32::from_gray(15);
        w.hovered.fg_stroke.color = Color32::BLACK;
        s.visuals.weak_text_color = Some(Color32::from_gray(90));
        s.visuals.disabled_alpha = 0.6;
        // A deeper selection blue, so white text on it (panel headers) and selected items read well.
        s.visuals.selection.bg_fill = Color32::from_rgb(120, 180, 235);
        s.visuals.selection.stroke.color = Color32::from_rgb(0, 40, 85);
    });
}

impl BurnCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        readable_text(&cc.egui_ctx);
        crate::diagnostics::log_renderer(cc);
        let settings: Option<LaserSettings> = cc.storage.and_then(|s| eframe::get_value(s, LASER_KEY));
        let devices: Option<DeviceStore> = cc.storage.and_then(|s| eframe::get_value(s, DEVICES_KEY));
        let laser = Laser::new(devices, settings, Device::spawn(cc.egui_ctx.clone()));
        let laser_has_machine = laser.has_machine();
        let mut doc = Document::default();
        doc.bed_size = laser.settings.bed;
        let library: Vec<Preset> = cc.storage.and_then(|s| eframe::get_value(s, LIBRARY_KEY)).unwrap_or_default();
        let prefs: ViewPrefs = cc.storage.and_then(|s| eframe::get_value(s, VIEW_KEY)).unwrap_or_default();
        let fonts = FontLibrary::load_system();
        let default_font = fonts.default_family();
        Self {
            doc,
            history: History::default(),
            selection: BTreeSet::new(),
            canvas: Canvas::default(),
            current_layer: 0,
            file: None,
            revision: 0,
            saved_revision: 0,
            status: "Ready".into(),
            cursor: None,
            laser,
            fonts,
            library,
            camera: CameraUi::new(cc.storage.and_then(|s| eframe::get_value::<CameraSettings>(s, CAMERA_KEY)).unwrap_or_default()),
            preset_name: String::new(),
            lock_aspect: true,
            dialog: None,
            preview: None,
            last_text: (default_font, 10.0, false, false, TextStyle::default()),
            clipboard: Vec::new(),
            prefs,
            cuts_tab: CutsTab::Layers,
            laser_tab: LaserTab::Laser,
            show_settings: false,
            // A first run has no machine yet: open Devices so the user can add theirs.
            show_devices: !laser_has_machine,
            show_rotary: false,
            show_params: false,
            constraint_errors: Vec::new(),
            art: Default::default(),
            autosaver: crate::recovery::AutoSaver::new(),
            recovery_checked: false,
            closing: false,
            remote_settings: cc.storage.and_then(|s| eframe::get_value(s, REMOTE_KEY)).unwrap_or_default(),
            remote: None,
            remote_error: None,
            remote_published: f64::NEG_INFINITY,
            remote_camera: f64::NEG_INFINITY,
            cut_editor: None,
            removing_layer: None,
            builder: None,
            corner_radius: 5.0,
            corner_style: bc_geom::CornerStyle::Round,
            rotate_field: 0.0,
            frame_watch: Default::default(),
            window_title: String::new(),
            queue: Default::default(),
            pixscan: Default::default(),
            uploads: Vec::new(),
            startup_file: std::env::args_os().nth(1).map(PathBuf::from).filter(|p| p.is_file()),
        }
    }

    fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    fn title(&self) -> String {
        let name = self.file.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        format!("{}{name} - BurnCraft", if self.is_dirty() { "*" } else { "" })
    }

    // --- Edits. Every change to the document goes through a checkpoint first.

    pub(crate) fn checkpoint_named(&mut self, label: &str) {
        self.history.checkpoint_named(&self.doc, label);
        self.revision += 1;
    }

    /// Records one merged undo step for repeated edits (slider drags) under `key`.
    pub(crate) fn checkpoint_merged(&mut self, key: &str, now: f64) {
        self.history.checkpoint_merged(&self.doc, key, now);
        self.revision += 1;
    }

    pub(crate) fn assign_layer(&mut self, layer: usize) {
        self.current_layer = layer;
        if self.selection.is_empty() {
            return;
        }
        self.edit_named(&format!("Move to layer {}", bc_doc::layer_label(layer)), |doc, sel| {
            for s in doc.shapes.iter_mut().filter(|s| sel.contains(&s.id)) {
                s.layer = layer;
            }
        });
    }

    fn set_selection(&mut self, sel: BTreeSet<u64>) {
        self.selection = self.doc.expand_groups(&sel);
    }

    pub(crate) fn undo(&mut self) {
        if self.history.undo(&mut self.doc) {
            self.revision += 1;
            self.prune_selection();
        }
    }

    pub(crate) fn redo(&mut self) {
        if self.history.redo(&mut self.doc) {
            self.revision += 1;
            self.prune_selection();
        }
    }

    pub(crate) fn prune_selection(&mut self) {
        let doc = &self.doc;
        self.selection.retain(|id| doc.shape(*id).is_some());
    }

    // --- Files.

    fn confirm_discard(&self) -> bool {
        !self.is_dirty()
            || rfd::MessageDialog::new()
                .set_title("BurnCraft")
                .set_description("Discard unsaved changes?")
                .set_buttons(rfd::MessageButtons::YesNo)
                .show()
                == rfd::MessageDialogResult::Yes
    }

    pub(crate) fn new_project(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        let bed = self.doc.bed_size;
        self.doc = Document::default();
        self.doc.bed_size = bed;
        self.after_load(None);
    }

    pub(crate) fn open(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("BurnCraft or LightBurn project", &[PROJECT_EXTENSION, "lbrn2", "lbrn"])
            .add_filter("BurnCraft project", &[PROJECT_EXTENSION])
            .add_filter("LightBurn project", &bc_io::lightburn::EXTENSIONS)
            .pick_file()
        else {
            return;
        };
        self.open_path(path);
    }

    /// Opens a project from File > Open Recent.
    pub(crate) fn open_recent(&mut self, path: PathBuf) {
        if !self.confirm_discard() {
            return;
        }
        if !path.exists() {
            self.prefs.recent.retain(|p| p != &path);
            return self.error(format!("{} no longer exists", path.display()));
        }
        self.open_path(path);
    }

    fn open_path(&mut self, path: PathBuf) {
        crate::recovery::remember(&mut self.prefs.recent, &path);
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if bc_io::lightburn::EXTENSIONS.contains(&ext.as_str()) {
            return self.open_lightburn(&path);
        }
        match Document::load(&path) {
            Ok(doc) => {
                self.doc = doc;
                self.after_load(Some(path));
            }
            Err(e) => self.error(format!("Could not open {}: {e}", path.display())),
        }
    }

    /// Opens a LightBurn project as a new, unsaved BurnCraft document with its layer settings.
    fn open_lightburn(&mut self, path: &Path) {
        let project = match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| bc_io::lightburn::import(&b, self.doc.bed_size.1, &self.fonts)) {
            Ok(p) => p,
            Err(e) => return self.error(format!("Could not open {}: {e}", path.display())),
        };
        let bed = self.doc.bed_size;
        self.doc = Document::default();
        self.doc.bed_size = bed;
        let mut warnings = project.warnings.clone();
        let ids = self.add_lightburn(project, true);
        // LightBurn measures from the bottom of its own bed; if that was taller than ours, slide the design on.
        if let Some(b) = self.doc.bounds_of(ids.iter().copied()) {
            let (w, h) = bed;
            let shift = |lo: f64, hi: f64, max: f64| if hi - lo > max { 0.0 } else if lo < 0.0 { -lo } else if hi > max { max - hi } else { 0.0 };
            let (dx, dy) = (shift(b.x0, b.x1, w), shift(b.y0, b.y1, h));
            if dx != 0.0 || dy != 0.0 {
                edit::move_to(&mut self.doc, &ids, Point::new(b.x0 + dx, b.y0 + dy));
                warnings.push(format!("moved {:.1} mm to fit the work area", dx.hypot(dy)));
            } else if b.width() > w || b.height() > h {
                warnings.push("the design is larger than the work area".into());
            }
        }
        // Not a BurnCraft file, so Save asks where to save it, and it counts as unsaved.
        self.after_load(None);
        self.revision += 1;
        self.status = format!("Opened {}", path.display());
        if !warnings.is_empty() {
            self.status += &format!(" ({})", warnings.join("; "));
        }
    }

    /// Adds a LightBurn project's shapes. Layer settings are applied to all its layers
    /// (`all_layers`) or only to layers not yet used in this design.
    fn add_lightburn(&mut self, project: bc_io::lightburn::LbProject, all_layers: bool) -> BTreeSet<u64> {
        for (index, layer) in project.layers {
            if all_layers || !self.doc.shapes.iter().any(|s| s.layer == index) {
                self.doc.layers[index] = layer;
            }
        }
        self.doc.tool_layers_off();
        let mut ids = BTreeSet::new();
        let mut groups: std::collections::BTreeMap<usize, BTreeSet<u64>> = Default::default();
        for s in project.shapes {
            let id = self.doc.add_shape(s.kind, s.transform, s.layer);
            ids.insert(id);
            if let Some(g) = s.group {
                groups.entry(g).or_default().insert(id);
            }
        }
        for members in groups.values() {
            self.doc.group(members);
        }
        ids
    }

    fn after_load(&mut self, path: Option<PathBuf>) {
        self.history.clear();
        self.selection.clear();
        self.revision += 1;
        self.saved_revision = self.revision;
        self.status = match &path {
            Some(p) => format!("Opened {}", p.display()),
            None => "New project".into(),
        };
        self.file = path;
        self.canvas.request_fit();
    }

    pub(crate) fn save(&mut self, save_as: bool) {
        let path = match (&self.file, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let Some(p) = rfd::FileDialog::new().add_filter("BurnCraft project", &[PROJECT_EXTENSION]).set_file_name(format!("design.{PROJECT_EXTENSION}")).save_file() else { return };
                p
            }
        };
        match self.doc.save(&path) {
            Ok(()) => {
                self.saved_revision = self.revision;
                self.status = format!("Saved {}", path.display());
                crate::recovery::remember(&mut self.prefs.recent, &path);
                self.file = Some(path);
            }
            Err(e) => self.error(format!("Could not save {}: {e}", path.display())),
        }
    }

    pub(crate) fn import(&mut self) {
        let mut all: Vec<&str> = vec![PROJECT_EXTENSION];
        all.extend(bc_io::VECTOR_EXTENSIONS);
        all.extend(bc_io::TOOLPATH_EXTENSIONS);
        all.extend(bc_io::lightburn::EXTENSIONS);
        all.extend(bc_io::IMAGE_EXTENSIONS);
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("All supported", &all)
            .add_filter("BurnCraft project", &[PROJECT_EXTENSION])
            .add_filter("Vector (SVG, DXF, PDF, AI, EPS, EMF, WMF)", &bc_io::VECTOR_EXTENSIONS)
            .add_filter("G-code, HPGL, Ruida job", &bc_io::TOOLPATH_EXTENSIONS)
            .add_filter("LightBurn project", &bc_io::lightburn::EXTENSIONS)
            .add_filter("Images", &bc_io::IMAGE_EXTENSIONS)
            .pick_files()
        else {
            return;
        };
        for path in paths {
            if let Err(e) = self.import_file(&path) {
                self.error(format!("Could not import {}: {e}", path.display()));
            }
        }
    }

    pub(crate) fn import_file(&mut self, path: &Path) -> Result<(), String> {
        let mut bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if bc_io::psd::EXTENSIONS.contains(&ext.as_str()) {
            // A Photoshop file comes in as its merged picture.
            bytes = bc_io::psd::to_png(&bytes)?;
        }
        if bc_io::IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            // Asks how the picture will be used first (see `upload.rs`).
            let size = self.picture_size(&bytes)?;
            self.uploads.push(crate::upload::Upload::new(name, bytes, size));
            return Ok(());
        }
        if ext == PROJECT_EXTENSION {
            let text = std::str::from_utf8(&bytes).map_err(|_| "not a BurnCraft project".to_string())?;
            let other = Document::from_json(text).map_err(|e| e.to_string())?;
            if other.shapes.is_empty() {
                return Err("the project is empty".into());
            }
            self.checkpoint_named(&format!("Import {name}"));
            let ids = edit::merge_project(&mut self.doc, &other);
            self.place_on_bed(&ids);
            self.status = format!("Imported {} shape(s) from {name}", ids.len());
            self.selection = ids;
            return Ok(());
        }
        if bc_io::lightburn::EXTENSIONS.contains(&ext.as_str()) {
            let project = bc_io::lightburn::import(&bytes, self.doc.bed_size.1, &self.fonts)?;
            if project.shapes.is_empty() {
                return Err("no shapes found".into());
            }
            self.checkpoint_named(&format!("Import {name}"));
            let warnings = project.warnings.clone();
            let ids = self.add_lightburn(project, false);
            self.place_on_bed(&ids);
            self.selection = ids;
            self.status = format!("Imported {name}");
            if !warnings.is_empty() {
                self.status += &format!(" ({})", warnings.join("; "));
            }
            return Ok(());
        }
        let result = match ext.as_str() {
            "svg" => bc_io::svg::import(&bytes, self.fonts.database())?,
            "dxf" => bc_io::dxf_import::import(&bytes)?,
            // An .ai is PDF inside (Illustrator 9 on), or PostScript (the legacy format).
            "pdf" | "ai" => match bc_io::pdf_import::import(&bytes) {
                Err(_) if bytes.starts_with(b"%!") => bc_io::eps::import(&bytes)?,
                r => r?,
            },
            "eps" | "ps" => bc_io::eps::import(&bytes)?,
            "emf" | "wmf" => bc_io::metafile::import(&bytes)?,
            "gcode" | "nc" | "gc" => bc_io::gcode_import::import(&bytes)?,
            "hpgl" | "plt" => bc_io::hpgl_import::import(&bytes)?,
            "rd" => self.import_ruida_file(&bytes)?,
            _ => return Err(format!("unsupported file type .{ext}")),
        };
        if result.shapes.is_empty() {
            return Err("no shapes found".into());
        }
        // Formats whose colours (or pens / power levels / layers) say which layer a line is on.
        let use_colors = matches!(ext.as_str(), "svg" | "pdf" | "ai" | "eps" | "ps" | "emf" | "wmf" | "gcode" | "nc" | "gc" | "hpgl" | "plt" | "rd");
        self.checkpoint_named(&format!("Import {name}"));
        let mut ids = BTreeSet::new();
        for s in result.shapes {
            let layer = if use_colors { s.color.map(bc_io::nearest_layer).unwrap_or(self.current_layer) } else { self.current_layer };
            ids.insert(self.doc.add_shape(ShapeKind::Path(s.path), Affine::IDENTITY, layer));
        }
        self.doc.group(&ids);
        self.place_on_bed(&ids);
        self.selection = ids;
        self.status = format!("Imported {name}");
        if !result.warnings.is_empty() {
            self.status += &format!(" ({})", result.warnings.join("; "));
        }
        Ok(())
    }

    /// Keeps imported shapes where they are if they fit on the bed; otherwise centres them.
    /// A Ruida job file as vectors: machine positions are turned back into bed positions with this
    /// machine's origin corner, so a file made for it lands where it would burn.
    fn import_ruida_file(&self, bytes: &[u8]) -> Result<bc_io::ImportResult, String> {
        let layers = bc_ruida::rd_file::read(bytes)?;
        let cfg = self.laser.machine(self.doc.bed_size);
        let mut result = bc_io::ImportResult::default();
        for l in layers {
            for path in l.paths {
                let pts: Vec<Point> = path.iter().map(|&(x, y)| cfg.to_design((x as f64 / 1000.0, y as f64 / 1000.0))).collect();
                let closed = pts.len() > 2 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.distance(*b) < 1e-6);
                let pts = if closed { &pts[..pts.len() - 1] } else { &pts[..] };
                result.shapes.push(bc_io::Imported { path: bc_geom::polyline_path(pts, closed), color: l.color });
            }
        }
        Ok(result)
    }

    fn place_on_bed(&mut self, ids: &BTreeSet<u64>) {
        let Some(b) = self.doc.bounds_of(ids.iter().copied()) else { return };
        let (w, h) = self.doc.bed_size;
        if b.x0 < 0.0 || b.y0 < 0.0 || b.x1 > w || b.y1 > h {
            let to = Point::new((w - b.width()) / 2.0, (h - b.height()) / 2.0);
            edit::move_to(&mut self.doc, ids, to);
        }
    }

    /// The size (mm) a picture is placed at: 150 DPI, at most 200 mm on its long side, and
    /// shrunk to fit 90 % of the bed if needed.
    fn picture_size(&self, bytes: &[u8]) -> Result<(f64, f64), String> {
        let (pw, ph) = bc_io::image_dimensions(bytes).ok_or("not a readable image")?;
        let (bw, bh) = self.doc.bed_size;
        let (w, h) = (pw as f64 * 25.4 / 150.0, ph as f64 * 25.4 / 150.0);
        let fit = (200.0 / w.max(h)).min(bw * 0.9 / w).min(bh * 0.9 / h).min(1.0);
        Ok((w * fit, h * fit))
    }

    /// The layer a picture goes on: the current one if it engraves images, else one that already
    /// does, else the first unused layer (set up for images).
    fn image_layer(&mut self) -> usize {
        let used = self.doc.used_layers();
        let layer = if self.doc.layers[self.current_layer].mode == LayerMode::Image {
            self.current_layer
        } else {
            (0..CUT_LAYER_COUNT).find(|i| self.doc.layers[*i].mode == LayerMode::Image && used.contains(i)).or_else(|| self.doc.spare_layer()).unwrap_or(self.current_layer)
        };
        if self.doc.layers[layer].mode != LayerMode::Image {
            let l = &mut self.doc.layers[layer];
            l.mode = LayerMode::Image;
            l.interval = 0.1;
            l.speed = 200.0;
        }
        layer
    }

    /// Adds an imported picture as chosen in the import window, in the middle of the bed: the
    /// picture, the picture with a cut line round it (grouped), or just the cut line.
    fn place_upload(&mut self, u: crate::upload::Upload) {
        use crate::upload::UseAs;
        let (w, h) = u.size;
        // Top-left 10 mm in from the bed's top-left corner.
        let corner = bc_geom::Vec2::new(10.0, 10.0);
        self.checkpoint_named(&format!("Import {}", u.name));
        let mut ids = BTreeSet::new();
        let mut layers = Vec::new();
        if u.use_as == UseAs::Engrave || u.use_as == UseAs::EngraveAndCut || (u.use_as == UseAs::Trace && u.keep_picture) {
            let layer = self.image_layer();
            ids.insert(self.doc.add_shape(ShapeKind::image(&u.png, w, h), Affine::translate(corner + bc_geom::Vec2::new(w / 2.0, h / 2.0)), layer));
            layers.push(layer);
        }
        if u.use_as != UseAs::Engrave && !u.outline.elements().is_empty() {
            let layer = if self.doc.layers[self.current_layer].mode == LayerMode::Line { self.current_layer } else { self.spare_layer(LayerMode::Line, "Cut") };
            let at = Affine::translate(corner);
            ids.insert(self.doc.add_shape(ShapeKind::Path(at * u.outline), Affine::IDENTITY, layer));
            layers.push(layer);
        }
        if ids.len() > 1 {
            self.doc.group(&ids);
        }
        let on: Vec<String> = layers.iter().map(|l| bc_doc::layer_label(*l)).collect();
        self.status = match u.use_as {
            UseAs::Engrave => format!("Imported {} ({w:.1} × {h:.1} mm) on layer {}", u.name, on.join("")),
            UseAs::EngraveAndCut => format!("Imported {}: the picture on layer {}, the cut line round it on layer {}", u.name, on[0], on.get(1).cloned().unwrap_or_default()),
            UseAs::Cut => format!("Imported the outline of {} on layer {}", u.name, on.join("")),
            UseAs::Trace => format!("Traced {} into cut lines on layer {}", u.name, on.last().cloned().unwrap_or_default()),
        };
        self.selection = ids;
    }

    pub(crate) fn export_svg(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("SVG", &["svg"]).set_file_name("design.svg").save_file() else { return };
        match std::fs::write(&path, bc_io::svg::export(&self.doc)) {
            Ok(()) => self.status = format!("Exported {}", path.display()),
            Err(e) => self.error(format!("Could not write {}: {e}", path.display())),
        }
    }

    /// Writes the current job as G-code for the selected dialect.
    pub(crate) fn save_gcode(&mut self) {
        let text = match self.design_job().and_then(|job| self.laser.gcode(&job, self.doc.bed_size)) {
            Ok(t) => t,
            Err(e) => return self.error(e),
        };
        let Some(path) = rfd::FileDialog::new().add_filter("G-code", &["gcode", "nc", "gc"]).set_file_name("job.gcode").save_file() else { return };
        match std::fs::write(&path, text) {
            Ok(()) => self.status = format!("Saved {}", path.display()),
            Err(e) => self.error(format!("Could not write {}: {e}", path.display())),
        }
    }

    /// A dimension from `a` to `b` on tool layer Tool1 (never cut): the line with arrowheads and end
    /// ticks, and the length as text beside it, grouped.
    fn add_dimension(&mut self, a: Point, b: Point) {
        let d = b - a;
        let len = d.hypot();
        if len < 1e-6 {
            return;
        }
        let dir = d / len;
        let n = Vec2::new(-dir.y, dir.x);
        let mut path = bc_geom::BezPath::new();
        path.move_to(a);
        path.line_to(b);
        let arrow = 2.0f64.min(len / 4.0);
        for (tip, back) in [(a, dir), (b, -dir)] {
            path.move_to(tip + back * arrow + n * arrow * 0.4);
            path.line_to(tip);
            path.line_to(tip + back * arrow - n * arrow * 0.4);
            path.move_to(tip + n * 2.0);
            path.line_to(tip - n * 2.0);
        }
        let layer = bc_doc::CUT_LAYER_COUNT; // Tool1
        self.checkpoint_named("Dimension");
        let mut ids = BTreeSet::from([self.doc.add_shape(ShapeKind::Path(path), Affine::IDENTITY, layer)]);
        let label = self.prefs.units.format(len);
        let font = self.last_text.0.clone();
        let style = TextStyle::default();
        if let Some(outline) = self.fonts.outline(&label, &font, 3.0, false, false, &style) {
            // Beside the middle of the line, upright as far as possible.
            let mut angle = dir.atan2();
            if angle > std::f64::consts::FRAC_PI_2 || angle <= -std::f64::consts::FRAC_PI_2 {
                angle += std::f64::consts::PI;
            }
            let side = Vec2::new(angle.sin(), -angle.cos()); // "up" for the text
            let at = a.midpoint(b) + side * 3.5;
            let kind = ShapeKind::Text { text: label.clone(), font, height: 3.0, bold: false, italic: false, style, outline };
            ids.insert(self.doc.add_shape(kind, Affine::translate(at.to_vec2()) * Affine::rotate(angle), layer));
        }
        self.doc.group(&ids);
        self.selection = ids;
        self.status = format!("Dimension {label} (on tool layer Tool1, never cut)");
    }
    /// Offers to restore a backup left by a crash (once, at startup), backs up unsaved changes
    /// every minute, and asks about unsaved changes when the window is closed.
    fn crash_safety(&mut self, ctx: &egui::Context) {
        if !self.recovery_checked {
            self.recovery_checked = true;
            if let Some((backup, info)) = self.autosaver.pending() {
                let from = info.file.as_ref().and_then(|f| f.file_name()).map(|n| format!(" (\"{}\")", n.to_string_lossy())).unwrap_or_default();
                let restore = rfd::MessageDialog::new()
                    .set_title("Restore your work?")
                    .set_description(format!("BurnCraft didn't close properly last time. Restore the design you were working on{from}, backed up {}?", info.age()))
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    == rfd::MessageDialogResult::Yes;
                match restore.then(|| Document::load(&backup)) {
                    Some(Ok(doc)) => {
                        self.doc = doc;
                        self.after_load(info.file.clone());
                        // Restored changes aren't saved yet.
                        self.revision += 1;
                        self.status = format!("Restored your design from {}. Save it to keep it.", info.age());
                    }
                    Some(Err(e)) => {
                        self.error(format!("The backup couldn't be read: {e}"));
                        self.autosaver.clear();
                    }
                    None => self.autosaver.clear(),
                }
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            if self.is_dirty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                let name = self.file.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "this design".into());
                let answer = rfd::MessageDialog::new()
                    .set_title("BurnCraft")
                    .set_description(format!("Save changes to {name} before closing?"))
                    .set_buttons(rfd::MessageButtons::YesNoCancel)
                    .show();
                let close = match answer {
                    rfd::MessageDialogResult::Yes => {
                        self.save(false);
                        !self.is_dirty()
                    }
                    rfd::MessageDialogResult::No => true,
                    _ => false,
                };
                if close {
                    self.closing = true;
                    self.autosaver.clear();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            } else {
                self.autosaver.clear();
            }
        }
        self.autosaver.tick(&self.doc, self.revision, self.is_dirty(), self.file.as_deref());
    }
    /// Opens a live warp or simplify on the selected vector shapes (one undo step).
    pub(crate) fn open_shape_edit(&mut self, edit: crate::dialogs::ShapeEdit) {
        let originals: Vec<bc_doc::Shape> = self
            .doc
            .shapes
            .iter()
            .filter(|s| self.selection.contains(&s.id) && !s.locked && !matches!(s.kind, ShapeKind::Image { .. }))
            .cloned()
            .collect();
        if originals.is_empty() {
            return self.status = "Select shapes to change (images can't be warped)".into();
        }
        self.checkpoint_named(match edit {
            crate::dialogs::ShapeEdit::Warp { .. } => "Warp",
            crate::dialogs::ShapeEdit::Simplify { .. } => "Simplify",
        });
        self.dialog = Some(Dialog::ShapeEdit(crate::dialogs::ShapeEditDialog::new(edit, originals)));
    }

    /// With exactly one text and one vector shape selected: (text id, guide shape id).
    pub(crate) fn text_and_guide(&self) -> Option<(u64, u64)> {
        if self.selection.len() != 2 {
            return None;
        }
        let shapes: Vec<&bc_doc::Shape> = self.selection.iter().filter_map(|id| self.doc.shape(*id)).collect();
        let text = shapes.iter().find(|s| matches!(s.kind, ShapeKind::Text { .. }))?;
        let guide = shapes.iter().find(|s| s.id != text.id && matches!(s.kind, ShapeKind::Rect { .. } | ShapeKind::Ellipse { .. } | ShapeKind::Polygon { .. } | ShapeKind::Star { .. } | ShapeKind::Path(_)))?;
        Some((text.id, guide.id))
    }

    /// Lays the selected text along the selected shape. The shape stays (move it to a tool layer to
    /// keep it as a guide only); the text keeps its own copy of the path.
    pub(crate) fn text_on_path(&mut self) {
        let Some((text_id, guide_id)) = self.text_and_guide() else { return };
        let (Some(text), Some(guide)) = (self.doc.shape(text_id).cloned(), self.doc.shape(guide_id)) else { return };
        let ShapeKind::Text { text: content, font, height, bold, italic, style, .. } = &text.kind else { return };
        // The guide in the text's own coordinates, so moving the text later moves both together.
        let local = text.transform.inverse() * guide.world_path();
        let length: f64 = bc_geom::flatten(&local, 0.05).first().map(|l| l.points.windows(2).map(|w| w[0].distance(w[1])).sum()).unwrap_or(0.0);
        let closed = guide.is_closed();
        // Centred text sits in the middle of an open path, or at the start of a closed one.
        let offset = if closed || style.align != bc_doc::TextAlign::Center { 0.0 } else { length / 2.0 };
        let style = bc_doc::TextStyle { curve: Some(bc_doc::TextCurve::Path { path: local, offset, flip: false }), ..style.clone() };
        let Some(outline) = self.fonts.outline(content, font, *height, *bold, *italic, &style) else {
            return self.error(format!("Font \"{font}\" can't render this text"));
        };
        self.checkpoint_named("Text on path");
        if let Some(s) = self.doc.shape_mut(text_id) {
            s.kind = ShapeKind::Text { text: content.clone(), font: font.clone(), height: *height, bold: *bold, italic: *italic, style, outline };
        }
        self.selection = BTreeSet::from([text_id]);
        self.status = "Text placed on the path. Double-click it to change the position or flip it.".into();
    }
    /// Writes the visible vector shapes as a DXF (mm, one DXF layer per BurnCraft layer).
    pub(crate) fn export_dxf(&mut self) {
        let (bytes, notes) = match bc_io::dxf_export::export(&self.doc) {
            Ok(r) => r,
            Err(e) => return self.error(format!("Could not make the DXF: {e}")),
        };
        let Some(path) = rfd::FileDialog::new().add_filter("DXF", &["dxf"]).set_file_name("design.dxf").save_file() else { return };
        match std::fs::write(&path, bytes) {
            Ok(()) => {
                self.status = format!("Exported {}", path.display());
                if !notes.is_empty() {
                    self.status += &format!(" ({})", notes.join("; "));
                }
            }
            Err(e) => self.error(format!("Could not write {}: {e}", path.display())),
        }
    }

    pub(crate) fn error(&mut self, message: String) {
        rfd::MessageDialog::new().set_title("BurnCraft").set_level(rfd::MessageLevel::Error).set_description(&message).show();
        self.status = message;
    }

    // --- Text and barcodes.

    /// Opens the text dialog (or the barcode dialog, for a barcode shape).
    pub(crate) fn open_text_dialog(&mut self, target: Option<u64>, at: Point) {
        let d = match target.and_then(|id| self.doc.shape(id)).map(|s| &s.kind) {
            Some(ShapeKind::Text { text, font, height, bold, italic, style, .. }) => TextDialog::new(target, at, text.clone(), font.clone(), *height, *bold, *italic, style.clone()),
            Some(ShapeKind::Barcode { symbology, content, size, .. }) => {
                self.dialog = Some(Dialog::Barcode(BarcodeDialog { target, at, symbology: *symbology, content: content.clone(), size: *size }));
                return;
            }
            _ => {
                let (font, height, bold, italic, style) = self.last_text.clone();
                TextDialog::new(None, at, String::new(), font, height, bold, italic, style)
            }
        };
        self.dialog = Some(Dialog::Text(d));
    }

    fn apply_barcode(&mut self, d: &BarcodeDialog) {
        let outline = match bc_io::barcode::outline(d.symbology, &d.content, d.size) {
            Ok(o) => o,
            Err(e) => return self.error(e),
        };
        let kind = ShapeKind::Barcode { symbology: d.symbology, content: d.content.clone(), size: d.size, outline };
        self.checkpoint_named(if d.target.is_some() { "Edit barcode" } else { "Add barcode" });
        match d.target.and_then(|id| self.doc.shape_mut(id)) {
            Some(shape) => shape.kind = kind,
            None => {
                // Barcodes need to be filled to be readable: use a Fill layer if the current one isn't.
                let layer = self.current_layer;
                if self.doc.layers[layer].mode == LayerMode::Line && !self.doc.used_layers().contains(&layer) {
                    self.doc.layers[layer].mode = LayerMode::Fill;
                }
                let id = self.doc.add_shape(kind, Affine::translate(d.at.to_vec2()), layer);
                self.selection = BTreeSet::from([id]);
            }
        }
    }

    /// A copy of the document with variable-text fields filled in (text and barcodes regenerated).
    fn evaluated_doc(&self) -> Result<Document, String> {
        self.evaluate(&self.doc)
    }

    /// `doc` with its variable text and barcodes filled in (serial numbers, CSV rows, dates).
    pub(crate) fn evaluate(&self, doc: &Document) -> Result<Document, String> {
        let clock = clock();
        let vars = &doc.variables;
        let mut doc = doc.clone();
        for s in &mut doc.shapes {
            match &mut s.kind {
                ShapeKind::Text { text, font, height, bold, italic, style, outline } if bc_doc::vars::has_fields(text) => {
                    let value = vars.evaluate(text, &clock);
                    *outline = self.fonts.outline(&value, font, *height, *bold, *italic, style).ok_or_else(|| format!("Could not render \"{value}\" with font \"{font}\""))?;
                }
                ShapeKind::Barcode { symbology, content, size, outline } if bc_doc::vars::has_fields(content) => {
                    *outline = bc_io::barcode::outline(*symbology, &vars.evaluate(content, &clock), *size)?;
                }
                _ => {}
            }
        }
        Ok(doc)
    }

    fn apply_text(&mut self, d: &TextDialog) {
        let Some(outline) = self.fonts.outline(&d.text, &d.font, d.height, d.bold, d.italic, &d.style) else {
            self.error(format!("Could not render the text with font \"{}\"", d.font));
            return;
        };
        self.last_text = (d.font.clone(), d.height, d.bold, d.italic, bc_doc::TextStyle { curve: None, ..d.style.clone() });
        // New straight text goes with its alignment point (e.g. top-left) where it was clicked.
        let placed = match (&d.style.curve, bc_geom::bounds(&outline)) {
            (None, Some(b)) => d.at - d.style.anchor(b).to_vec2(),
            _ => d.at,
        };
        let kind = ShapeKind::Text { text: d.text.clone(), font: d.font.clone(), height: d.height, bold: d.bold, italic: d.italic, style: d.style.clone(), outline };
        self.checkpoint_named(if d.target.is_some() { "Edit text" } else { "Add text" });
        match d.target.filter(|id| self.doc.shape(*id).is_some()) {
            // Edited text keeps its alignment point where it was.
            Some(id) => {
                edit::replace_text(&mut self.doc, id, kind);
            }
            None => {
                let id = self.doc.add_shape(kind, Affine::translate(placed.to_vec2()), self.current_layer);
                self.selection = BTreeSet::from([id]);
            }
        }
    }

    // --- Jobs.

    fn design_job(&self) -> Result<bc_job::Job, String> {
        let only = self.laser.settings.cut_selected.then_some(&self.selection);
        if only.is_some_and(|s| s.is_empty()) {
            return Err("\"Cut selected only\" is on, but nothing is selected.".into());
        }
        Ok(bc_job::build(&self.evaluated_doc()?, only, self.laser.build_options()))
    }

    /// Outlines items found by the camera as rectangles on tool layer Tool1 (jig slots), selected.
    fn add_slots(&mut self, items: &[bc_geom::fit::OrientedRect]) {
        if items.is_empty() {
            self.status = "No items found. Try a lower contrast setting, or a background that differs more from the items.".into();
            return;
        }
        self.checkpoint_named("Detect items");
        let t1 = bc_doc::CUT_LAYER_COUNT;
        self.doc.layers[t1].visible = true;
        self.selection = items
            .iter()
            .map(|r| self.doc.add_shape(ShapeKind::Rect { width: r.width, height: r.height, corner_radius: 0.0 }, r.placement(), t1))
            .collect();
        self.status = format!("Found {} item(s), outlined on Tool1. Add your design to the selection, then Arrange > Place on Each Slot.", items.len());
    }

    /// Eraser tool: rubs out the brushed parts of images (the selected images, or else any visible,
    /// unlocked image the brush touches).
    fn erase_images(&mut self, points: &[Point], radius: f64) {
        let Some(reach) = points.iter().map(|p| bc_geom::Rect::from_center_size(*p, (2.0 * radius, 2.0 * radius))).reduce(|a, b| a.union(b)) else { return };
        let selected_images: Vec<u64> = self.selection.iter().copied().filter(|id| matches!(self.doc.shape(*id).map(|s| &s.kind), Some(ShapeKind::Image { .. }))).collect();
        let targets: Vec<u64> = self
            .doc
            .shapes
            .iter()
            .filter(|s| matches!(s.kind, ShapeKind::Image { .. }) && !s.locked && self.doc.layers[s.layer].visible)
            .filter(|s| selected_images.is_empty() || selected_images.contains(&s.id))
            .filter(|s| s.bounds().is_some_and(|b| b.intersect(reach).area() > 0.0))
            .map(|s| s.id)
            .collect();
        let mut first = true;
        for id in targets {
            let Some(shape) = self.doc.shape(id) else { continue };
            let ShapeKind::Image { width, height, .. } = shape.kind else { continue };
            let Some(bytes) = shape.image_bytes() else { continue };
            // Brush points in the picture's own millimetres from its top-left corner.
            let inverse = shape.transform.inverse();
            let local: Vec<(f64, f64)> = points.iter().map(|p| inverse * *p).map(|q| (q.x + width / 2.0, q.y + height / 2.0)).collect();
            let scale = shape.transform.determinant().abs().sqrt().max(1e-9);
            if let Ok(png) = bc_raster::erase(&bytes, &local, radius / scale, width, height) {
                if first {
                    self.checkpoint_named("Erase");
                    first = false;
                }
                let fresh = ShapeKind::image(&png, width, height);
                if let (Some(s), ShapeKind::Image { data, .. }) = (self.doc.shape_mut(id), fresh)
                    && let ShapeKind::Image { data: old, .. } = &mut s.kind
                {
                    *old = data;
                }
            }
        }
    }

    /// Eraser tool by colour: clears the area of colour clicked in the topmost image under the
    /// pointer (a selected one if any is there).
    fn erase_colour(&mut self, at: Point) {
        let selected = |id: u64| self.selection.contains(&id);
        let mut under: Vec<&bc_doc::Shape> = self
            .doc
            .shapes
            .iter()
            .filter(|s| matches!(s.kind, ShapeKind::Image { .. }) && !s.locked && self.doc.layers[s.layer].visible && s.bounds().is_some_and(|b| b.contains(at)))
            .collect();
        // Topmost last; a selected image wins.
        under.sort_by_key(|s| selected(s.id));
        let Some(shape) = under.last() else {
            self.status = "Click on a picture to clear a colour from it".into();
            return;
        };
        let ShapeKind::Image { width, height, .. } = shape.kind else { return };
        let Some(bytes) = shape.image_bytes() else { return };
        let local = shape.transform.inverse() * at;
        let (id, c) = (shape.id, &self.canvas);
        match bc_raster::erase_colour(&bytes, (local.x + width / 2.0, local.y + height / 2.0), width, height, c.eraser_tolerance, c.eraser_everywhere) {
            Ok(png) => {
                self.checkpoint_named("Erase colour");
                if let (Some(s), ShapeKind::Image { data, .. }) = (self.doc.shape_mut(id), ShapeKind::image(&png, width, height))
                    && let ShapeKind::Image { data: old, .. } = &mut s.kind
                {
                    *old = data;
                }
                self.status = "Cleared".into();
            }
            Err(e) => self.status = format!("Nothing cleared: {e}"),
        }
    }

    pub(crate) fn open_sections(&mut self) {
        match self.design_job() {
            Ok(job) if job.is_empty() => self.error("There is nothing to burn.".to_string()),
            Ok(job) => self.dialog = Some(Dialog::Sections(crate::dialogs::SectionsDialog::new(job, self.doc.bed_size.1))),
            Err(e) => self.error(e),
        }
    }

    /// The production queue: follows the running piece, and handles the window's buttons.
    fn queue_tick(&mut self, ctx: &egui::Context) {
        let device = &self.laser.device;
        self.queue.track(device.jobs_completed, device.job_running());
        let Some(action) = self.queue.ui(ctx, device.connected) else { return };
        use crate::queue::QueueAction;
        match action {
            QueueAction::AddCurrent => {
                let name = self.file.as_ref().and_then(|p| p.file_stem()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| format!("Design {}", self.queue.items.len() + 1));
                self.queue.add(name, self.doc.clone());
            }
            QueueAction::AddFiles => {
                let Some(paths) = rfd::FileDialog::new().add_filter("BurnCraft project", &[PROJECT_EXTENSION]).pick_files() else { return };
                for path in paths {
                    match Document::load(&path) {
                        Ok(doc) => self.queue.add(path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), doc),
                        Err(e) => self.error(format!("Could not add {}: {e}", path.display())),
                    }
                }
            }
            QueueAction::Run(i) | QueueAction::Frame(i) => {
                let bed = self.doc.bed_size;
                let item = &self.queue.items[i];
                let title = format!("Burn {} (copy {} of {})?\n\nLoad the material first.", item.name, item.done + 1, item.copies);
                let job = match self.evaluate(&item.doc) {
                    Ok(doc) => bc_job::build(&doc, None, self.laser.build_options()),
                    Err(e) => return self.error(e),
                };
                if job.is_empty() {
                    return self.error(format!("There is nothing to burn in {}.", item.name));
                }
                let result = if let QueueAction::Frame(_) = action {
                    self.laser.frame(&job, bed).map(|()| self.status = "Framing (laser off)".into())
                } else {
                    let completed = self.laser.device.jobs_completed;
                    self.laser.start(&job, bed, &title).map(|sent| {
                        if sent {
                            self.queue.started(i, completed);
                            // Serial numbers and CSV rows move on for each piece.
                            let doc = &mut self.queue.items[i].doc;
                            if doc.variables.auto_advance {
                                doc.variables.advance();
                            }
                        }
                    })
                };
                if let Err(e) = result {
                    self.error(e);
                }
            }
        }
    }

    /// Adds a PixScan photo at true size, top-left 10 mm in from the bed's corner, on an image
    /// layer that isn't burned: it's for placing designs on, or tracing.
    fn add_pixscan(&mut self, p: crate::pixscan::Placed) {
        self.checkpoint_named("PixScan photo");
        let layer = self.spare_layer(LayerMode::Image, "PixScan photo");
        self.doc.layers[layer].output = false;
        let at = Affine::translate((10.0 + p.width / 2.0, 10.0 + p.height / 2.0));
        let id = self.doc.add_shape(ShapeKind::image(&p.png, p.width, p.height), at, layer);
        self.selection = BTreeSet::from([id]);
        self.status = format!("Photo placed at true size ({:.0} × {:.0} mm) on layer {}, which isn't burned. Place your design on it, or trace it.", p.width, p.height, bc_doc::layer_label(layer));
    }

    /// Splits the selected text into one text per letter, each exactly where it was, so letters can
    /// be nudged or turned one at a time. Text along a curve becomes outlines (each letter keeps
    /// its own angle).
    pub(crate) fn split_text(&mut self) {
        let texts: Vec<bc_doc::Shape> = self.doc.shapes.iter().filter(|s| self.selection.contains(&s.id) && !s.locked && matches!(s.kind, ShapeKind::Text { .. })).cloned().collect();
        let mut made = BTreeSet::new();
        let mut split = BTreeSet::new();
        let mut pieces: Vec<(ShapeKind, Affine, usize)> = Vec::new();
        for shape in &texts {
            let ShapeKind::Text { text, font, height, bold, italic, style, .. } = &shape.kind else { continue };
            let Some(letters) = self.fonts.letters(text, font, *height, *bold, *italic, style) else { continue };
            if letters.len() < 2 {
                continue;
            }
            for (c, glyph) in letters {
                let Some(b) = bc_geom::bounds(&glyph) else { continue };
                if style.curve.is_some() {
                    pieces.push((ShapeKind::Path(shape.transform * glyph), Affine::IDENTITY, shape.layer));
                    continue;
                }
                let letter_style = bc_doc::TextStyle { upper_case: false, weld: false, curve: None, kerning: Vec::new(), ..style.clone() };
                let Some(outline) = self.fonts.outline(&c.to_string(), font, *height, *bold, *italic, &letter_style) else { continue };
                let kind = ShapeKind::Text { text: c.to_string(), font: font.clone(), height: *height, bold: *bold, italic: *italic, style: letter_style, outline };
                // A one-letter outline is centred on itself: put that centre where the letter was.
                pieces.push((kind, shape.transform * Affine::translate(b.center().to_vec2()), shape.layer));
            }
            split.insert(shape.id);
        }
        if split.is_empty() {
            self.status = "Select text with at least two letters".into();
            return;
        }
        self.checkpoint_named("Split text into letters");
        self.doc.remove_where(|id| split.contains(&id));
        for (kind, transform, layer) in pieces {
            made.insert(self.doc.add_shape(kind, transform, layer));
        }
        self.status = format!("Split into {} letters", made.len());
        self.selection = made;
    }

    /// After a job is sent: move variable text on to the next serial number / CSV row.
    fn after_job_sent(&mut self) {
        let uses_vars = self.doc.shapes.iter().any(|s| match &s.kind {
            ShapeKind::Text { text, .. } => bc_doc::vars::has_fields(text),
            ShapeKind::Barcode { content, .. } => bc_doc::vars::has_fields(content),
            _ => false,
        });
        if uses_vars && self.doc.variables.auto_advance {
            self.checkpoint_named("Advance serial");
            self.doc.variables.advance();
        }
    }

    pub(crate) fn laser_action(&mut self, action: LaserAction) {
        let bed = self.doc.bed_size;
        let result = self.design_job().and_then(|job| match action {
            LaserAction::Preview => {
                self.preview = Some(Preview::new(job, "design".into(), self.laser.settings.rapid_speed));
                Ok(())
            }
            LaserAction::Frame => self.laser.frame(&job, bed).map(|()| self.status = "Framing (laser off)".into()),
            LaserAction::FrameOutline => self.laser.frame_with_laser(&job, bed).map(|_| ()),
            LaserAction::Start => self.laser.start(&job, bed, "Start this job on the laser?").map(|sent| {
                if sent {
                    self.status = "Sending job…".into();
                    self.after_job_sent();
                }
            }),
        });
        if let Err(e) = result {
            self.error(e);
        }
    }

    // --- UI.

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        for cmd in Command::with_shortcuts() {
            if (typing || self.dialog.is_some()) && cmd.needs_canvas_focus() {
                continue;
            }
            let shortcut = cmd.shortcut().expect("listed commands have shortcuts");
            if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                self.run(cmd);
            }
        }
        // Plain keys only when no text field has focus and no dialog is open.
        if typing || self.dialog.is_some() {
            return;
        }
        let (del, esc, tools) = ctx.input(|i| {
            let plain = i.modifiers.is_none();
            let tool_key = Tool::ALL.into_iter().chain([Tool::ShapeBuilder]).find(|t| t.key().is_some_and(|k| plain && i.key_pressed(k)));
            (i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace), i.key_pressed(Key::Escape), tool_key)
        });
        // With a node selected, Delete removes the node (handled by the canvas), not the shape.
        if del && !self.canvas.node_selected() {
            self.run(Command::Delete);
        }
        if esc && !self.canvas.cancel() {
            if self.canvas.tool == Tool::Select {
                self.selection.clear();
            }
            self.canvas.set_tool(Tool::Select);
        }
        if let Some(tool) = tools {
            self.canvas.set_tool(tool);
        }

        // Arrow keys nudge the selection: 1 mm, Shift = 10 mm, Ctrl = 0.1 mm.
        let nudge = ctx.input(|i| {
            let step = if i.modifiers.shift { 10.0 } else if i.modifiers.command { 0.1 } else { 1.0 };
            let mut d = Vec2::ZERO;
            if i.key_pressed(Key::ArrowLeft) { d.x -= step; }
            if i.key_pressed(Key::ArrowRight) { d.x += step; }
            if i.key_pressed(Key::ArrowUp) { d.y -= step; }
            if i.key_pressed(Key::ArrowDown) { d.y += step; }
            d
        });
        if nudge != Vec2::ZERO && !self.selection.is_empty() {
            let now = ctx.input(|i| i.time);
            self.checkpoint_merged("Nudge", now);
            edit::translate(&mut self.doc, &self.selection, nudge);
        }
    }

    /// A menu entry for `cmd`, greyed out when it can't run.
    pub(crate) fn command_item(&mut self, ui: &mut egui::Ui, cmd: Command) {
        let mut button = egui::Button::new(cmd.label());
        if let Some(s) = cmd.shortcut() {
            button = button.shortcut_text(ui.ctx().format_shortcut(&s));
        }
        if ui.add_enabled(self.can_run(cmd), button).clicked() {
            self.run(cmd);
            ui.close();
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        use Command::*;
        egui::MenuBar::new().ui(ui, |ui| {
            let mut bar = Vec::new();
            bar.push(ui.menu_button("File", |ui| {
                for c in [New, Open, Save, SaveAs] {
                    self.command_item(ui, c);
                }
                ui.separator();
                self.command_item(ui, Import);
                self.command_item(ui, ExportSvg);
                self.command_item(ui, ExportDxf);
                self.command_item(ui, SaveGcode);
                ui.separator();
                ui.menu_button("Open Recent", |ui| {
                    if self.prefs.recent.is_empty() {
                        ui.weak("No recent files");
                    }
                    let mut chosen = None;
                    for p in &self.prefs.recent {
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if ui.button(name).on_hover_text(p.display().to_string()).clicked() {
                            chosen = Some(p.clone());
                            ui.close();
                        }
                    }
                    if let Some(p) = chosen {
                        self.open_recent(p);
                    }
                    if !self.prefs.recent.is_empty() {
                        ui.separator();
                        if ui.button("Clear list").clicked() {
                            self.prefs.recent.clear();
                            ui.close();
                        }
                    }
                });
                ui.separator();
                if ui.button("Exit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }).response);
            bar.push(ui.menu_button("Edit", |ui| {
                for c in [Undo, Redo] {
                    self.command_item(ui, c);
                }
                ui.separator();
                for c in [Cut, Copy, Paste, PasteInPlace, Duplicate, Delete] {
                    self.command_item(ui, c);
                }
                ui.separator();
                for c in [SelectAll, InvertSelection, SelectCurrentLayer, SelectOpenShapes] {
                    self.command_item(ui, c);
                }
                ui.separator();
                for c in [Group, Ungroup, Lock, Unlock, ConvertToPath, SplitText, DeleteDuplicates] {
                    self.command_item(ui, c);
                }
            }).response);
            bar.push(ui.menu_button("Arrange", |ui| {
                ui.menu_button("Align", |ui| {
                    for a in [edit::Align::Left, edit::Align::HCenter, edit::Align::Right, edit::Align::Top, edit::Align::VCenter, edit::Align::Bottom, edit::Align::Centers] {
                        self.command_item(ui, Command::Align(a));
                    }
                });
                ui.menu_button("Distribute", |ui| {
                    self.command_item(ui, Distribute(true));
                    self.command_item(ui, Distribute(false));
                });
                ui.menu_button("Dock", |ui| {
                    for s in [Side::Left, Side::Right, Side::Up, Side::Down] {
                        self.command_item(ui, Dock(s));
                    }
                });
                self.command_item(ui, MakeSameWidth);
                self.command_item(ui, MakeSameHeight);
                ui.separator();
                ui.menu_button("Order", |ui| {
                    for o in [edit::Order::ToFront, edit::Order::Forward, edit::Order::Backward, edit::Order::ToBack] {
                        self.command_item(ui, Command::Order(o));
                    }
                });
                for c in [Mirror(true), Mirror(false), Rotate(90.0), Rotate(-90.0), MoveToBedCenter] {
                    self.command_item(ui, c);
                }
                ui.separator();
                for c in [GridArray, CircularArray, Nest, MarkSlots, PlaceOnSlots, FitIntoBox, SplitTiles, Parameters] {
                    self.command_item(ui, c);
                }
            }).response);
            bar.push(ui.menu_button("Tools", |ui| {
                self.command_item(ui, Offset);
                ui.menu_button("Boolean", |ui| {
                    for op in [BooleanOp::Union, BooleanOp::Subtract, BooleanOp::Intersect] {
                        self.command_item(ui, Boolean(op));
                    }
                });
                ui.separator();
                for c in [BreakApart, AutoJoin, ClosePaths, ReversePath, RoundCorners, TextOnPath, CopyAlongPath, Warp, Simplify, Blend, ClipToShape] {
                    self.command_item(ui, c);
                }
                ui.separator();
                for c in [Stencil, ContourCut, Inlay, RubberStamp] {
                    self.command_item(ui, c);
                }
                ui.separator();
                for c in [Rhinestones, Lattice, Knockout, Hatch, PiercePoints, Depth] {
                    self.command_item(ui, c);
                }
                ui.separator();
                for c in [TraceImage, PixScan, PhotoLines, ShadowBox, ClipImage, UnclipImage] {
                    self.command_item(ui, c);
                }
                ui.separator();
                for c in [Barcode, Monogram, MaterialTest, BoxGenerator] {
                    self.command_item(ui, c);
                }
            }).response);
            bar.push(ui.menu_button("Laser", |ui| {
                for c in [Preview, Frame, Start, RunInSections, ProductionQueue, GoToOrigin, RotarySetup, MachineSettings] {
                    self.command_item(ui, c);
                }
            }).response);
            bar.push(ui.menu_button("View", |ui| {
                self.command_item(ui, ZoomToBed);
                self.command_item(ui, ZoomToSelection);
                ui.separator();
                ui.checkbox(&mut self.prefs.show_rulers, "Rulers");
                ui.checkbox(&mut self.prefs.snap_to_objects, "Snap to Objects");
                ui.checkbox(&mut self.prefs.snap_to_grid, "Snap to Grid");
                ui.checkbox(&mut self.prefs.burn_preview, "Burn Preview").on_hover_text("Show images dithered at their layer's line interval, as the laser will burn them");
                ui.checkbox(&mut self.prefs.smart_guides, "Smart Guides").on_hover_text("While moving, line shapes up with other shapes, the bed and guides");
                ui.checkbox(&mut self.canvas.show_constraints, "Show Constraints").on_hover_text("Draw locked sizes and distances (Arrange > Parameters & Constraints) as blue dimensions");
                ui.menu_button("Symmetry", |ui| {
                    use crate::canvas::SymmetryMode;
                    let sym = &mut self.canvas.symmetry;
                    ui.weak("Shapes you draw are repeated as you draw them.");
                    for m in SymmetryMode::ALL {
                        if ui.radio_value(&mut sym.mode, m, m.label()).clicked() && sym.center == Point::ORIGIN {
                            let (w, h) = self.doc.bed_size;
                            sym.center = Point::new(w / 2.0, h / 2.0);
                        }
                    }
                    if sym.mode == SymmetryMode::Radial {
                        ui.horizontal(|ui| {
                            ui.label("Copies");
                            ui.add(egui::DragValue::new(&mut sym.count).range(2..=64));
                        });
                        ui.checkbox(&mut sym.mirror, "Mirror alternate slices");
                    }
                    ui.separator();
                    if ui.button("Centre on the bed").clicked() {
                        let (w, h) = self.doc.bed_size;
                        sym.center = Point::new(w / 2.0, h / 2.0);
                    }
                    let sel_centre = self.doc.bounds_of(self.selection.iter().copied()).map(|b| b.center());
                    if ui.add_enabled(sel_centre.is_some(), egui::Button::new("Centre on the selection")).clicked()
                        && let Some(c) = sel_centre
                    {
                        sym.center = c;
                    }
                });
                if ui.add_enabled(!self.doc.guides.is_empty(), egui::Button::new("Clear Guides")).on_hover_text("Remove all ruler guides (drag from a ruler to add one)").clicked() {
                    self.checkpoint_named("Clear guides");
                    self.doc.guides.clear();
                    ui.close();
                }
                ui.horizontal(|ui| {
                    ui.label("Grid spacing");
                    let u = self.prefs.units;
                    u.drag(ui, &mut self.prefs.grid_mm, 0.5..=100.0);
                });
                ui.horizontal(|ui| {
                    ui.label("Units");
                    for u in [Units::Millimetres, Units::Inches] {
                        ui.radio_value(&mut self.prefs.units, u, u.label());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Work area");
                    let (mut w, mut h) = self.doc.bed_size;
                    let u = self.prefs.units;
                    let changed = u.drag(ui, &mut w, 10.0..=5000.0) | u.drag(ui, &mut h, 10.0..=5000.0);
                    if changed {
                        let now = ui.input(|i| i.time);
                        self.checkpoint_merged("Work area", now);
                        self.doc.bed_size = (w, h);
                    }
                });
            }).response);
            switch_menu_on_hover(ui, &bar);
        });
    }

    /// The canvas right-click menu.
    fn context_menu(&mut self, ui: &mut egui::Ui) {
        use Command::*;
        for c in [Cut, Copy, Paste, Duplicate, Delete] {
            self.command_item(ui, c);
        }
        ui.separator();
        for c in [Group, Ungroup, Lock, Unlock] {
            self.command_item(ui, c);
        }
        ui.separator();
        ui.menu_button("Arrange", |ui| {
            for c in [Order(edit::Order::ToFront), Order(edit::Order::ToBack), Mirror(true), Mirror(false), Rotate(90.0), Rotate(-90.0)] {
                self.command_item(ui, c);
            }
            ui.separator();
            for a in [edit::Align::Left, edit::Align::HCenter, edit::Align::Right, edit::Align::Top, edit::Align::VCenter, edit::Align::Bottom, edit::Align::Centers] {
                self.command_item(ui, Command::Align(a));
            }
        });
        ui.menu_button("Shape", |ui| {
            for c in [Boolean(BooleanOp::Union), Boolean(BooleanOp::Subtract), Boolean(BooleanOp::Intersect), Offset, ConvertToPath, BreakApart, AutoJoin, ClosePaths, ReversePath] {
                self.command_item(ui, c);
            }
        });
        if self.can_run(PlaceOnSlots) {
            self.command_item(ui, PlaceOnSlots);
        }
        if self.can_run(TraceImage) {
            self.command_item(ui, TraceImage);
        }
        ui.separator();
        self.command_item(ui, ZoomToSelection);
    }

    /// The Settings window (view and editing preferences).
    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        crate::dialogs::centered(ctx, "Settings").open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("settings-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Units");
                ui.horizontal(|ui| {
                    for u in [Units::Millimetres, Units::Inches] {
                        ui.radio_value(&mut self.prefs.units, u, u.label());
                    }
                });
                ui.end_row();
                ui.label("Y axis");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.prefs.y_up, true, "Up (origin bottom-left)");
                    ui.radio_value(&mut self.prefs.y_up, false, "Down (origin top-left)");
                });
                ui.end_row();
                ui.label("Grid spacing");
                let u = self.prefs.units;
                u.drag(ui, &mut self.prefs.grid_mm, 0.5..=100.0);
                ui.end_row();
                ui.label("Snapping");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.prefs.snap_to_grid, "Snap to grid");
                    ui.checkbox(&mut self.prefs.snap_to_objects, "Snap to objects");
                });
                ui.end_row();
                ui.label("Rulers");
                ui.checkbox(&mut self.prefs.show_rulers, "Show rulers");
                ui.end_row();
                ui.label("Theme");
                egui::widgets::global_theme_preference_buttons(ui);
                ui.end_row();
                ui.label("Design area");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.prefs.board, BoardTheme::MatchTheme, "Match theme");
                    ui.radio_value(&mut self.prefs.board, BoardTheme::Light, "☀ Light");
                    ui.radio_value(&mut self.prefs.board, BoardTheme::Dark, "🌙 Dark");
                });
                ui.end_row();
            });
            ui.separator();
            ui.strong("Remote monitoring");
            let r = &mut self.remote_settings;
            let before = (r.enabled, r.port, r.pin.clone(), r.pin_to_view);
            ui.checkbox(&mut r.enabled, "Show jobs on a web page for phones and other computers on this network");
            ui.add_enabled_ui(r.enabled, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Port");
                    ui.add(egui::DragValue::new(&mut r.port).range(1024..=65535));
                    ui.label("PIN");
                    ui.monospace(egui::RichText::new(&r.pin).strong());
                    if ui.small_button("New PIN").clicked() {
                        r.pin = crate::remote::new_pin();
                    }
                });
                ui.checkbox(&mut r.pin_to_view, "Require the PIN to view").on_hover_text("When off, anyone on your network can watch progress and the camera. Pause, Resume and Stop always need the PIN.");
            });
            if (r.enabled, r.port, r.pin.clone(), r.pin_to_view) != before {
                self.remote_error = None;
            }
            if let Some(e) = &self.remote_error {
                ui.colored_label(egui::Color32::from_rgb(220, 70, 70), e);
            } else if let Some(server) = &self.remote {
                let host = crate::remote::local_ip().map(|ip| ip.to_string()).unwrap_or_else(|| "this-pc".into());
                ui.horizontal(|ui| {
                    ui.label("On your phone, open");
                    ui.monospace(egui::RichText::new(format!("http://{host}:{}", server.port)).strong());
                });
            }
            if self.remote_settings.enabled {
                if !self.remote_settings.pin_to_view {
                    ui.weak("Anyone on your network can watch, without the PIN.");
                }
                ui.weak("Anyone on your network with the PIN can pause or stop the laser. Windows may ask you to allow BurnCraft through the firewall: allow it on private networks only.");
            }
        });
        self.show_settings = open;
    }

    /// Runs the remote-monitoring server to match the settings, applies its commands and publishes status.
    fn remote_tick(&mut self, ctx: &egui::Context) {
        let s = &self.remote_settings;
        let stale = self.remote.as_ref().is_some_and(|r| !s.enabled || r.port != s.port || r.pin != s.pin || r.pin_to_view != s.pin_to_view);
        if stale {
            self.remote = None;
        }
        if s.enabled && self.remote.is_none() && self.remote_error.is_none() {
            match crate::remote::RemoteServer::start(s.port, s.pin.clone(), s.pin_to_view, ctx.clone()) {
                Ok(server) => self.remote = Some(server),
                Err(e) => self.remote_error = Some(format!("Couldn't start remote monitoring on port {}: {e}", s.port)),
            }
        }
        let Some(server) = &self.remote else { return };
        let commands = server.commands();
        for cmd in commands {
            use crate::remote::RemoteCmd;
            let device = &mut self.laser.device;
            match cmd {
                RemoteCmd::Pause => device.set_paused(true),
                RemoteCmd::Resume => device.set_paused(false),
                RemoteCmd::Stop => device.stop(),
            }
            self.status = format!("{cmd:?} from the remote page");
        }
        let Some(server) = &self.remote else { return };
        let now = ctx.input(|i| i.time);
        if now - self.remote_published >= 0.5 {
            self.remote_published = now;
            let d = &self.laser.device;
            let frame = self.camera.latest_frame();
            let status = crate::remote::RemoteStatus {
                machine: self.laser.profile_name().to_string(),
                connected: d.connected,
                description: d.description.clone(),
                state: d.state.as_ref().map(|s| s.state.clone()).unwrap_or_default(),
                position: d.state.as_ref().map(|s| (s.x, s.y)),
                message: d.message.clone().unwrap_or_default(),
                job_running: d.job_running(),
                paused: d.paused,
                progress: d.progress.filter(|(done, total, _)| done < total).map(|(done, total, what)| (done as f64 / total.max(1) as f64, what.to_string())),
                elapsed_s: d.job_started.map(|start| d.job_finished.unwrap_or_else(std::time::Instant::now).duration_since(start).as_secs_f64()),
                estimate_s: self.laser.job_estimate.get(),
                end_not_reported: self.laser.settings.controller == crate::laser::ControllerKind::Ruida,
                camera: frame.is_some(),
            };
            server.publish(&status);
            if now - self.remote_camera >= 1.0 {
                self.remote_camera = now;
                server.publish_camera(frame.as_ref().and_then(crate::remote::encode_jpeg));
            }
        }
        // Keep publishing while the window is idle.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut keep = true;
        match &mut dialog {
            Dialog::Barcode(d) => match d.ui(ctx) {
                Some(Outcome::Ok) => {
                    self.apply_barcode(d);
                    keep = false;
                }
                Some(Outcome::Cancel) => keep = false,
                None => {}
            },
            Dialog::Trace(d) => {
                let shape = self.doc.shape(d.target).cloned();
                match shape.map(|s| d.ui(ctx, &s)) {
                    Some(Some(Outcome::Ok)) => {
                        if let Some(path) = d.result.clone() {
                            self.checkpoint_named("Trace image");
                            // Traced outlines go on the current layer, unless that is an image layer.
                            let layer = if self.doc.layers[self.current_layer].mode == LayerMode::Image {
                                self.doc.spare_layer().unwrap_or(0)
                            } else {
                                self.current_layer
                            };
                            if d.delete_image {
                                self.doc.remove_where(|id| id == d.target);
                            }
                            let id = self.doc.add_shape(ShapeKind::Path(path), Affine::IDENTITY, layer);
                            self.selection = BTreeSet::from([id]);
                            self.status = format!("Traced the image onto layer {}", bc_doc::layer_label(layer));
                        }
                        keep = false;
                    }
                    Some(Some(Outcome::Cancel)) | None => keep = false,
                    Some(None) => {}
                }
            }
            Dialog::Nest(d) => {
                if let Some(o) = d.ui(ctx) {
                    if matches!(o, Outcome::Ok) {
                        let (w, h) = self.doc.bed_size;
                        let m = d.margin;
                        let area = bc_geom::Rect::new(m, m, (w - m).max(m), (h - m).max(m));
                        let (spacing, rotate) = (d.spacing, d.allow_rotate);
                        let total = edit::units(&self.doc, &self.selection).len();
                        self.checkpoint_named("Nest");
                        let placed = if d.true_shape {
                            edit::nest_true_shape(&mut self.doc, &self.selection, area, spacing, d.rotation_step, d.fine)
                        } else {
                            edit::nest(&mut self.doc, &self.selection, area, spacing, rotate)
                        };
                        self.status = if placed == total { format!("Nested {placed} item(s)") } else { format!("Nested {placed} of {total} items; the rest don't fit") };
                    }
                    keep = false;
                }
            }
            Dialog::Text(d) => match d.ui(ctx, &self.fonts) {
                Some(Outcome::Ok) => {
                    self.apply_text(d);
                    keep = false;
                }
                Some(Outcome::Cancel) => keep = false,
                None => {}
            },
            Dialog::GridArray(d) => {
                if let Some(o) = d.ui(ctx) {
                    if matches!(o, Outcome::Ok) {
                        let (c, r, gap) = (d.columns, d.rows, Vec2::new(d.gap_x, d.gap_y));
                        self.edit_named("Grid array", |doc, sel| {
                            let copies = edit::grid_array(doc, sel, c, r, gap);
                            sel.extend(copies);
                        });
                    }
                    keep = false;
                }
            }
            Dialog::CircularArray(d) => {
                if let Some(o) = d.ui(ctx) {
                    if matches!(o, Outcome::Ok) {
                        let (n, centre, rot) = (d.count, Point::new(d.center_x, d.center_y), d.rotate);
                        self.edit_named("Circular array", |doc, sel| {
                            let copies = edit::circular_array(doc, sel, n, centre, rot);
                            sel.extend(copies);
                        });
                    }
                    keep = false;
                }
            }
            Dialog::Offset(d) => {
                if let Some(o) = d.ui(ctx) {
                    if matches!(o, Outcome::Ok) {
                        let (dist, del, layer) = (d.distance, d.delete_originals, self.current_layer);
                        self.checkpoint_named("Offset");
                        match edit::offset(&mut self.doc, &self.selection, dist, layer, del) {
                            Some(id) => self.selection = BTreeSet::from([id]),
                            None => self.status = "Offset produced nothing (select closed shapes, or use a smaller inward distance)".into(),
                        }
                    }
                    keep = false;
                }
            }
            Dialog::Tool(d) => {
                if let Some(o) = d.ui(ctx, self.fonts.families()) {
                    if matches!(o, Outcome::Ok) {
                        self.apply_tool(d);
                    }
                    keep = false;
                }
            }
            Dialog::PlaceSlots(d) => {
                if let Some(o) = d.ui(ctx) {
                    if matches!(o, Outcome::Ok)
                        && let Some((slots, design)) = edit::slots_and_design(&self.doc, &self.selection)
                    {
                        self.checkpoint_named("Place on each slot");
                        let ids = edit::place_on_slots(&mut self.doc, &slots, &design, d.fit, d.margin, d.keep_original);
                        self.status = format!("Placed the design on {} slot(s). The slots are on a tool layer, so they won't burn.", slots.len());
                        if !ids.is_empty() {
                            self.selection = ids.into_iter().collect();
                        }
                    }
                    keep = false;
                }
            }
            Dialog::CopyAlong(d) => {
                if let Some(o) = d.ui(ctx) {
                    if matches!(o, Outcome::Ok)
                        && let Some((guide, items)) = edit::guide_and_items(&self.doc, &self.selection)
                    {
                        self.checkpoint_named("Copy along path");
                        let copies = edit::copy_along_path(&mut self.doc, &items, guide, d.count, d.rotate);
                        self.status = format!("Placed {} cop{} along the path", d.count, if d.count == 1 { "y" } else { "ies" });
                        self.selection = copies.into_iter().collect();
                    }
                    keep = false;
                }
            }
            Dialog::ShapeEdit(d) => {
                let outcome = d.ui(ctx);
                if d.applied != Some(d.edit) {
                    // Show the current settings in the document.
                    let results = d.results();
                    let count = |p: &bc_geom::BezPath| p.elements().len();
                    let before: usize = d.originals.iter().map(|s| count(&s.world_path())).sum();
                    let after: usize = results.iter().map(|(_, p)| count(p)).sum();
                    d.info = format!("{} shape(s): {before} → {after} path points", results.len());
                    for (id, path) in results {
                        if let Some(s) = self.doc.shape_mut(id) {
                            s.kind = ShapeKind::Path(path);
                            s.transform = Affine::IDENTITY;
                        }
                    }
                    d.applied = Some(d.edit);
                }
                match outcome {
                    Some(Outcome::Ok) => keep = false,
                    Some(Outcome::Cancel) => {
                        for orig in &d.originals {
                            if let Some(s) = self.doc.shape_mut(orig.id) {
                                *s = orig.clone();
                            }
                        }
                        keep = false;
                    }
                    None => {}
                }
            }
            Dialog::Box(d) => {
                let (bed_w, bed_h) = self.doc.bed_size;
                if d.sign.font.is_empty() {
                    d.sign.font = self.fonts.default_family();
                }
                if let Some(o) = d.ui(ctx, bed_w - 20.0, self.fonts.families()) {
                    if matches!(o, Outcome::Ok)
                        && let Ok(parts) = d.parts(bed_w - 20.0)
                    {
                        let at = if d.tab == crate::dialogs::MakerTab::Puzzle { d.place_at } else { None };
                        self.add_generated(parts, (bed_w, bed_h), at);
                        if d.tab == crate::dialogs::MakerTab::Sign {
                            self.add_sign_engraving(&d.sign, Point::new(10.0, 10.0));
                        }
                        if d.tab == crate::dialogs::MakerTab::Packaging
                            && let Ok(dieline) = d.dieline()
                        {
                            self.add_score_lines(dieline.folds, Point::new(10.0, 10.0));
                        }
                    }
                    keep = false;
                }
            }
            Dialog::MaterialTest(t) => {
                let bed = self.doc.bed_size;
                t.speed = self.laser.speed(self.prefs.units);
                match t.ui(ctx, self.laser.device.connected) {
                    Some(TestAction::Preview) => self.preview = Some(Preview::new(t.job(&self.fonts), "material test".into(), self.laser.settings.rapid_speed)),
                    Some(TestAction::Frame) => {
                        if let Err(e) = self.laser.frame(&t.job(&self.fonts), bed) {
                            self.error(e);
                        }
                    }
                    Some(TestAction::Start) => {
                        if let Err(e) = self.laser.start(&t.job(&self.fonts), bed, "Run the material test?") {
                            self.error(e);
                        }
                    }
                    Some(TestAction::Close) => keep = false,
                    None => {}
                }
            }
            Dialog::Sections(d) => {
                use crate::dialogs::SectionAction;
                let bed = self.doc.bed_size;
                match d.ui(ctx, bed, self.laser.device.connected) {
                    Some(SectionAction::Preview(k)) => self.preview = Some(Preview::new(d.section(k), format!("section {}", k + 1), self.laser.settings.rapid_speed)),
                    Some(SectionAction::Frame(k)) => {
                        if let Err(e) = self.laser.absolute(|l| l.frame(&d.section(k), bed)) {
                            self.error(e);
                        }
                    }
                    Some(SectionAction::Start(k)) => match self.laser.absolute(|l| l.start(&d.section(k), bed, &format!("Start section {}?", k + 1))) {
                        Ok(true) => {
                            d.done.insert(k);
                            self.status = format!("Sending section {}…", k + 1);
                        }
                        Ok(false) => {}
                        Err(e) => self.error(e),
                    },
                    Some(SectionAction::Close) => keep = false,
                    None => {}
                }
            }
        }
        if keep && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
    }

    /// Adds generated parts (box panels, hinge) as one group on a line layer, 10 mm in from the bed corner.
    fn add_generated(&mut self, parts: Vec<(&'static str, bc_geom::BezPath)>, bed: (f64, f64), at: Option<Point>) {
        // Cut on the current layer if it cuts lines; otherwise on the first unused layer.
        let layer = if self.doc.layers[self.current_layer].mode == LayerMode::Line {
            self.current_layer
        } else {
            self.doc.spare_layer().unwrap_or(self.current_layer)
        };
        let names: Vec<&str> = parts.iter().map(|(n, _)| *n).collect();
        self.checkpoint_named(&format!("Create {}", if names.len() == 1 { names[0] } else { "box" }));
        let ids: BTreeSet<u64> = parts.into_iter().map(|(_, p)| self.doc.add_shape(ShapeKind::Path(Affine::translate(at.unwrap_or(Point::new(10.0, 10.0)).to_vec2()) * p), Affine::IDENTITY, layer)).collect();
        self.doc.group(&ids);
        if let Some(b) = self.doc.bounds_of(ids.iter().copied())
            && (b.x1 > bed.0 || b.y1 > bed.1)
        {
            self.status = format!("Created {} part(s) on layer {}; they're larger than the work area", ids.len(), bc_doc::layer_label(layer));
        } else {
            self.status = format!("Created {} part(s) on layer {}", ids.len(), bc_doc::layer_label(layer));
        }
        self.selection = ids;
    }

    /// Keeps the Shape Builder's areas in step with the selection while its tool is on.
    fn refresh_builder(&mut self) {
        if self.canvas.tool != crate::canvas::Tool::ShapeBuilder {
            if self.builder.take().is_some() {
                self.canvas.builder_regions.clear();
            }
            return;
        }
        if self.builder.as_ref().is_some_and(|(r, s, _)| *r == self.revision && *s == self.selection) {
            return;
        }
        let paths: Vec<bc_geom::BezPath> = self.builder_ids().iter().filter_map(|id| self.doc.shape(*id)).map(|s| s.world_path()).collect();
        let built = bc_geom::curveops::Builder::new(&paths);
        self.canvas.builder_regions = built.as_ref().map(|b| b.outlines().to_vec()).unwrap_or_default();
        if built.is_none() {
            self.status = "Shape Builder: select two or more overlapping closed shapes".into();
        }
        self.builder = Some((self.revision, self.selection.clone(), built));
    }

    /// The selected shapes the Shape Builder works on (not pictures), in document order.
    fn builder_ids(&self) -> Vec<u64> {
        self.doc.shapes.iter().filter(|s| self.selection.contains(&s.id) && !matches!(s.kind, ShapeKind::Image { .. })).map(|s| s.id).collect()
    }

    /// Shape Builder: merges the areas under `points` into one shape (or with `delete`, removes
    /// them). The selected shapes are replaced by the merged shape and what's left of each.
    fn shape_build(&mut self, points: &[Point], delete: bool) {
        let ids = self.builder_ids();
        let Some((_, _, Some(builder))) = &self.builder else { return };
        let mut picked: Vec<usize> = Vec::new();
        for p in points {
            if let Some(i) = builder.region_at(*p)
                && !picked.contains(&i)
            {
                picked.push(i);
            }
        }
        if picked.is_empty() {
            self.status = "Shape Builder: click inside an outlined area".into();
            return;
        }
        let built = builder.apply(&picked, delete);
        let layer_of = |i: usize| ids.get(i).and_then(|id| self.doc.shape(*id)).map(|s| s.layer).unwrap_or(self.current_layer);
        let mut added: Vec<(bc_geom::BezPath, usize)> = built.rest.into_iter().map(|(i, p)| (p, layer_of(i))).collect();
        if let Some((p, owner)) = built.merged {
            added.push((p, layer_of(owner)));
        }
        self.checkpoint_named(if delete { "Shape Builder delete" } else { "Shape Builder merge" });
        self.doc.remove_where(|id| ids.contains(&id));
        self.selection = added.into_iter().map(|(p, layer)| self.doc.add_shape(ShapeKind::Path(p), Affine::IDENTITY, layer)).collect();
        self.status = if delete { format!("Deleted {} area(s)", picked.len()) } else { format!("Merged {} area(s) into one shape", picked.len()) };
    }

    /// The first layer nothing is on yet, set up for `mode` (or the current layer if all are used).
    pub(crate) fn spare_layer(&mut self, mode: LayerMode, name: &str) -> usize {
        match self.doc.spare_layer() {
            Some(i) => {
                let l = &mut self.doc.layers[i];
                l.mode = mode;
                if !name.is_empty() {
                    l.name = name.into();
                }
                i
            }
            None => self.current_layer,
        }
    }

    /// A generated sign's border band and fitted text, on an engrave layer, grouped with the sign
    /// just created (the selection). `at` is the sign's top-left corner.
    /// Fold lines for a dieline made by `add_generated` at `at`: on a Score layer (low power, so
    /// they mark the card without cutting through), grouped with the outline.
    fn add_score_lines(&mut self, folds: bc_geom::BezPath, at: Point) {
        let fresh = !self.doc.layers.iter().any(|l| l.name == "Score");
        let layer = self.doc.layers.iter().position(|l| l.name == "Score").unwrap_or_else(|| self.spare_layer(LayerMode::Line, "Score"));
        if fresh {
            // A light pass: fast and low, to be tuned on the card in use.
            let l = &mut self.doc.layers[layer];
            l.min_power = l.min_power.min(5.0);
            l.max_power = 8.0;
            l.speed = l.speed.max(100.0);
        }
        let mut ids = self.selection.clone();
        ids.insert(self.doc.add_shape(ShapeKind::Path(Affine::translate(at.to_vec2()) * folds), Affine::IDENTITY, layer));
        self.doc.group(&ids);
        self.selection = ids;
        self.status = format!("{}; fold lines on layer {} (Score) to mark at low power", self.status, bc_doc::layer_label(layer));
    }

    fn add_sign_engraving(&mut self, sign: &crate::dialogs::SignSettings, at: Point) {
        let centre = at + bc_geom::Vec2::new(sign.width / 2.0, sign.height / 2.0);
        let layer = self.spare_layer(LayerMode::Fill, "Engrave");
        let mut ids = self.selection.clone();
        if let Some(band) = sign.border() {
            ids.insert(self.doc.add_shape(ShapeKind::Path(band), Affine::translate(centre.to_vec2()), layer));
        }
        let text = sign.text.trim();
        if !text.is_empty() {
            let style = bc_doc::TextStyle::default();
            let (room_w, room_h) = sign.text_box();
            // Measure at the largest size, then shrink to fit.
            if let Some(outline) = self.fonts.outline(text, &sign.font, sign.text_height, false, false, &style)
                && let Some(b) = bc_geom::bounds(&outline)
            {
                let k = (room_w / b.width().max(1e-9)).min(room_h / b.height().max(1e-9)).min(1.0);
                let height = sign.text_height * k;
                if let Some(outline) = self.fonts.outline(text, &sign.font, height, false, false, &style) {
                    let kind = ShapeKind::Text { text: text.to_string(), font: sign.font.clone(), height, bold: false, italic: false, style, outline };
                    ids.insert(self.doc.add_shape(kind, Affine::translate(centre.to_vec2()), layer));
                }
            }
        }
        if ids.len() > self.selection.len() {
            self.doc.group(&ids);
            self.selection = ids;
            self.status = format!("Created the sign: cut on its own layer, border and text engraved on layer {}", bc_doc::layer_label(layer));
        }
    }

    fn preview_window(&mut self, ctx: &egui::Context) {
        let Some(preview) = &mut self.preview else { return };
        let bed = self.doc.bed_size;
        match preview.ui(ctx, bed, self.laser.device.connected) {
            Some(PreviewAction::Close) => self.preview = None,
            Some(PreviewAction::Frame) => {
                let r = self.laser.frame(&preview.job, bed);
                if let Err(e) = r {
                    self.error(e);
                }
            }
            Some(PreviewAction::Start) => {
                let r = self.laser.start(&preview.job, bed, &format!("Start the {}?", preview.title));
                if let Err(e) = r {
                    self.error(e);
                }
            }
            None => {}
        }
    }

    fn apply_canvas_actions(&mut self, actions: Vec<CanvasAction>) {
        for action in actions {
            match action {
                CanvasAction::Checkpoint(label) => self.checkpoint_named(label),
                CanvasAction::AddShape { kind, transform } => {
                    let copies = self.canvas.symmetry.copies();
                    let mut ids = BTreeSet::from([self.doc.add_shape(kind.clone(), transform, self.current_layer)]);
                    for c in copies {
                        ids.insert(self.doc.add_shape(kind.clone(), c * transform, self.current_layer));
                    }
                    if ids.len() > 1 {
                        self.doc.group(&ids);
                    }
                    self.selection = ids;
                }
                CanvasAction::Erase(points, radius) => self.erase_images(&points, radius),
                CanvasAction::EraseColour(at) => self.erase_colour(at),
                CanvasAction::Transform(t) => edit::transform(&mut self.doc, &self.selection, t),
                CanvasAction::SetSelection(sel) => self.set_selection(sel),
                CanvasAction::TextAt(p) => self.open_text_dialog(None, p),
                CanvasAction::EditText(id) => self.open_text_dialog(Some(id), Point::ORIGIN),
                CanvasAction::SetPath(id, path) => {
                    if let Some(s) = self.doc.shape_mut(id) {
                        s.kind = ShapeKind::Path(path);
                        s.transform = Affine::IDENTITY;
                    }
                }
                CanvasAction::Status(text) => self.status = text,
                CanvasAction::MoveLaser(p) => {
                    if self.laser.device.connected {
                        self.laser.move_to_design(self.doc.bed_size, p);
                    } else {
                        self.status = "Connect to the laser to position the head".into();
                    }
                }
                CanvasAction::SetGuides(guides) => self.doc.guides = guides,
                CanvasAction::Dimension(a, b) => self.add_dimension(a, b),
                CanvasAction::Trim(p, tolerance) => {
                    let mut changed = self.doc.clone();
                    match edit::trim_at(&mut changed, p, tolerance) {
                        Some(id) => {
                            self.checkpoint_named("Trim");
                            let gone = changed.shape(id).is_none();
                            self.doc = changed;
                            self.selection.retain(|s| *s != id || !gone);
                            self.status = if gone { "Trimmed away the last piece".into() } else { "Trimmed".into() };
                        }
                        None => self.status = "Nothing to trim there: click a piece of outline between lines that cross it".into(),
                    }
                }
                CanvasAction::ShapeBuild { points, delete } => self.shape_build(&points, delete),
                CanvasAction::Knife(a, b) => {
                    let mut changed = self.doc.clone();
                    let pieces = edit::knife(&mut changed, a, b);
                    if pieces.is_empty() {
                        self.status = "The cut didn't go right across any shape".into();
                    } else {
                        self.checkpoint_named("Knife");
                        self.doc = changed;
                        self.status = format!("Cut into {} pieces", pieces.len());
                        self.selection = pieces.into_iter().collect();
                    }
                }
                CanvasAction::RoundCorner(id, vertex) => {
                    let radius = self.corner_radius;
                    let mut changed = self.doc.clone();
                    let style = self.corner_style;
                    if edit::set_corner(&mut changed, id, vertex, radius, style) {
                        self.checkpoint_named(match (radius > 0.0, style) {
                            (false, _) => "Sharpen corner",
                            (true, bc_geom::CornerStyle::Round) => "Round corner",
                            (true, bc_geom::CornerStyle::Chamfer) => "Chamfer corner",
                        });
                        self.doc = changed;
                        self.status = if radius > 0.0 { format!("Corner radius set to {}", self.prefs.units.format(radius)) } else { "Corner made sharp".into() };
                    } else {
                        self.status = "That corner already has this radius".into();
                    }
                }
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let u = self.prefs.units;
            if let Some(p) = self.cursor {
                let k = u.mm_per_unit();
                ui.monospace(format!("x: {:.2}, y: {:.2} {}", p.x / k, self.display_y(p.y) / k, u.suffix().trim()));
            }
            ui.separator();
            ui.label(&self.status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("Zoom {:.0}%", self.canvas.view.zoom / 1.5 * 100.0));
                if let Some(b) = self.doc.bounds_of(self.selection.iter().copied()) {
                    ui.label(format!("{} × {}", u.format(b.width()), u.format(b.height())));
                }
            });
        });
    }
}

impl eframe::App for BurnCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::startup::frame_shown();
        let ctx = ui.ctx().clone();
        if let Some(bed) = self.laser.poll()
            && bed != self.doc.bed_size {
                self.doc.bed_size = bed;
                self.canvas.request_fit();
                self.status = format!("Work area for {}: {:.0} × {:.0} mm", self.laser.profile_name(), bed.0, bed.1);
            }
        // Files dropped onto the window are imported.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for path in dropped {
            if let Err(e) = self.import_file(&path) {
                self.error(format!("Could not import {}: {e}", path.display()));
            }
        }
        self.shortcuts(&ctx);
        self.remote_tick(&ctx);
        self.crash_safety(&ctx);
        // Only when it changes: every window command makes egui draw another frame, so sending it
        // each frame kept the window redrawing non-stop (high processor use while idle).
        let title = self.title();
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        self.canvas.snap_to_grid = self.prefs.snap_to_grid;
        self.canvas.snap_to_objects = self.prefs.snap_to_objects;
        self.canvas.grid_mm = self.prefs.grid_mm;
        self.canvas.dark_board = match self.prefs.board {
            BoardTheme::MatchTheme => ctx.global_style().visuals.dark_mode,
            BoardTheme::Light => false,
            BoardTheme::Dark => true,
        };
        self.canvas.show_rulers = self.prefs.show_rulers;
        self.canvas.smart_guides = self.prefs.smart_guides;
        self.canvas.burn_preview = self.prefs.burn_preview;
        self.canvas.units = self.prefs.units;
        self.canvas.y_up = self.prefs.y_up;
        self.canvas.allow_move = self.prefs.allow_move;
        self.canvas.allow_size = self.prefs.allow_size;
        self.canvas.allow_rotate = self.prefs.allow_rotate;
        self.canvas.allow_shear = self.prefs.allow_shear;
        self.canvas.machine_origin = Some(self.laser.machine(self.doc.bed_size).to_design((0.0, 0.0)));
        // While Trace Image is open, its result shows on the design as it'll be made.
        self.refresh_builder();
        self.canvas.highlight = match &self.dialog {
            Some(Dialog::Trace(d)) => d.result.clone(),
            _ => None,
        };

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar").show(ui, |ui| self.top_toolbar(ui));
        egui::Panel::top("editbar").show(ui, |ui| {
            egui::ScrollArea::horizontal().show(ui, |ui| self.edit_bar(ui));
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        // Drag the right edge to widen it; it remembers its width.
        // As many icon columns as the strip is wide; never narrower than the window height needs.
        // The panel's remembered width is keyed by that column count, so it snaps narrower or wider
        // when the window height changes how many columns are needed; manual widening sticks until then.
        let min_width = Self::left_toolbar_min_width(ui.available_height());
        let panel_id = egui::Id::new(("tools", min_width as u32));
        egui::Panel::left(panel_id).resizable(true).default_size(min_width).size_range(min_width..=240.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.left_toolbar(ui));
        });
        let bed = self.doc.bed_size;
        self.camera.update(&ctx);
        let (laser_action, camera_action) = self.right_panels(ui, bed);
        if let Some(a) = laser_action {
            self.laser_action(a);
        }
        match camera_action {
            Some(CameraAction::MarkTarget(job)) => {
                if let Err(e) = self.laser.absolute(|l| l.start(&job, bed, "Mark the camera alignment target? Cover the bed with scrap material first.")) {
                    self.error(e);
                }
            }
            Some(CameraAction::Detected(items)) => self.add_slots(&items),
            None => {}
        }
        let head = self.laser.head_position(bed);
        let overlay = self.camera.overlay();
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let (actions, cursor, response) = self.canvas.ui(ui, &self.doc, &self.selection, head, overlay.as_ref());
            self.cursor = cursor.or(self.cursor);
            self.apply_canvas_actions(actions);
            response.context_menu(|ui| self.context_menu(ui));
        });
        self.dialogs(&ctx);
        self.params_window(&ctx);
        self.enforce_constraints();
        self.preview_window(&ctx);
        self.camera.alignment_window(&ctx, bed);
        self.cut_editor_window(&ctx);
        self.remove_layer_window(&ctx);
        self.settings_window(&ctx);
        self.laser.grbl_config_window(&ctx);
        if self.show_rotary {
            let mut open = true;
            self.laser.rotary_window(&ctx, &mut open);
            self.show_rotary = open;
        }
        if self.show_devices {
            let mut open = true;
            let mut bed = self.doc.bed_size;
            let u = self.prefs.units;
            if self.laser.devices_window(&ctx, &mut open, &mut bed, u) {
                let now = ctx.input(|i| i.time);
                self.checkpoint_merged("Work area", now);
                self.doc.bed_size = bed;
            }
            self.show_devices = open;
        }
        if let Some(path) = self.startup_file.take() {
            // A project opens; anything else (pictures, vectors) is imported.
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXTENSION)) {
                self.open_path(path);
            } else if let Err(e) = self.import_file(&path) {
                self.error(format!("Could not import {}: {e}", path.display()));
            }
        }
        self.queue_tick(&ctx);
        if let Some(p) = self.pixscan.ui(&ctx) {
            self.add_pixscan(p);
        }
        if let Some(upload) = self.uploads.first_mut() {
            use crate::upload::UploadOutcome;
            match upload.ui(&ctx) {
                UploadOutcome::Open => {}
                UploadOutcome::Cancel => {
                    self.uploads.remove(0);
                }
                UploadOutcome::Add => {
                    let upload = self.uploads.remove(0);
                    self.place_upload(upload);
                }
            }
        }
        self.frame_watch.frame(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, DEVICES_KEY, &self.laser.store());
        eframe::set_value(storage, REMOTE_KEY, &self.remote_settings);
        eframe::set_value(storage, LIBRARY_KEY, &self.library);
        eframe::set_value(storage, CAMERA_KEY, &self.camera.settings);
        eframe::set_value(storage, VIEW_KEY, &self.prefs);
    }
}

/// Windows-style menu bar: while one menu is open, hovering another top-level menu opens it instead.
fn switch_menu_on_hover(ui: &egui::Ui, bar: &[egui::Response]) {
    let ctx = ui.ctx();
    let ids: Vec<egui::Id> = bar.iter().map(egui::Popup::default_response_id).collect();
    if !ids.iter().any(|id| egui::Popup::is_id_open(ctx, *id)) {
        return;
    }
    let Some(pointer) = ctx.pointer_hover_pos() else { return };
    if let Some((id, _)) = ids.iter().zip(bar).find(|(id, r)| r.rect.contains(pointer) && !egui::Popup::is_id_open(ctx, **id)) {
        egui::Popup::open_id(ctx, *id);
        ctx.request_repaint();
    }
}
#[cfg(test)]
mod menu_tests {
    use super::*;

    /// Runs one frame of a two-menu bar; returns the File and Edit button responses.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<egui::Response> {
        let input = egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() };
        let mut bar = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                bar.clear();
                bar.push(ui.menu_button("File", |ui| ui.label("new")).response);
                bar.push(ui.menu_button("Edit", |ui| ui.label("undo")).response);
                switch_menu_on_hover(ui, &bar);
            });
        });
        // No renderer here: discard the font texture updates.
        output.textures_delta.clear();
        bar
    }

    #[test]
    fn hovering_another_menu_switches_to_it() {
        let ctx = egui::Context::default();
        let bar = frame(&ctx, vec![]);
        let (file, edit) = (bar[0].rect.center(), bar[1].rect.center());
        let press = |pressed| egui::Event::PointerButton { pos: file, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&ctx, vec![egui::Event::PointerMoved(file), press(true)]);
        let bar = frame(&ctx, vec![press(false)]);
        let open = |bar: &[egui::Response], i: usize| egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&bar[i]));
        assert!(open(&bar, 0), "File opens on click");
        frame(&ctx, vec![egui::Event::PointerMoved(edit)]);
        let bar = frame(&ctx, vec![]);
        assert!(open(&bar, 1) && !open(&bar, 0), "hovering Edit switches to it");
    }

    #[test]
    fn hovering_without_an_open_menu_does_nothing() {
        let ctx = egui::Context::default();
        let bar = frame(&ctx, vec![]);
        frame(&ctx, vec![egui::Event::PointerMoved(bar[1].rect.center())]);
        let bar = frame(&ctx, vec![]);
        assert!(!egui::Popup::is_any_open(&ctx) && !bar.is_empty());
    }
}