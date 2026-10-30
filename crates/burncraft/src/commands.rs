//! Every user action, in one place: label, keyboard shortcut, when it's available, and what it does.
//! Menus, the right-click menu and keyboard shortcuts all run commands through [`BurnCraftApp::run`].

use crate::app::{BurnCraftApp, Dialog};
use crate::dialogs::{BarcodeDialog, CircularArrayDialog, GridArrayDialog, NestDialog, OffsetDialog, TraceDialog};
use crate::laser::LaserAction;
use bc_doc::edit::{self, Align, Order, Side};
use bc_doc::ShapeKind;
use bc_geom::ops::BooleanOp;
use bc_geom::{Affine, Point};
use eframe::egui::{Key, KeyboardShortcut, Modifiers};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    // File
    New,
    Open,
    Save,
    SaveAs,
    Import,
    ExportSvg,
    ExportDxf,
    SaveGcode,
    // Edit
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    PasteInPlace,
    Duplicate,
    Delete,
    SelectAll,
    InvertSelection,
    SelectOpenShapes,
    SelectCurrentLayer,
    Group,
    Ungroup,
    Lock,
    Unlock,
    ConvertToPath,
    SplitText,
    TextOnPath,
    ClipImage,
    ClipToShape,
    Blend,
    Warp,
    Simplify,
    CopyAlongPath,
    UnclipImage,
    DeleteDuplicates,
    // Arrange
    Align(Align),
    Distribute(bool),
    Dock(Side),
    MakeSameWidth,
    MakeSameHeight,
    Mirror(bool),
    Rotate(f64),
    MoveToBedCenter,
    Order(Order),
    GridArray,
    CircularArray,
    Nest,
    PlaceOnSlots,
    MarkSlots,
    FitIntoBox,
    SplitTiles,
    Parameters,
    // Tools
    Stencil,
    PhotoLines,
    Inlay,
    Rhinestones,
    Lattice,
    Knockout,
    Hatch,
    PiercePoints,
    Depth,
    RubberStamp,
    ContourCut,
    ShadowBox,
    Monogram,
    Offset,
    Boolean(BooleanOp),
    BreakApart,
    ClosePaths,
    ReversePath,
    AutoJoin,
    TraceImage,
    RoundCorners,
    Barcode,
    MaterialTest,
    BoxGenerator,
    RotarySetup,
    MachineSettings,
    ProductionQueue,
    PixScan,
    // Laser
    Preview,
    Frame,
    Start,
    RunInSections,
    GoToOrigin,
    Settings,
    Devices,
    // View
    ZoomToBed,
    ZoomToSelection,
}

const fn key(m: Modifiers, k: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(m, k))
}
const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);
const NONE: Modifiers = Modifiers::NONE;

impl Command {
    pub fn label(self) -> String {
        use Command::*;
        match self {
            New => "New".into(),
            Open => "Open…".into(),
            Save => "Save".into(),
            SaveAs => "Save As…".into(),
            Import => "Import…".into(),
            ExportSvg => "Export SVG…".into(),
            ExportDxf => "Export DXF…".into(),
            SaveGcode => "Save G-code…".into(),
            Undo => "Undo".into(),
            Redo => "Redo".into(),
            Cut => "Cut".into(),
            Copy => "Copy".into(),
            Paste => "Paste".into(),
            PasteInPlace => "Paste in Place".into(),
            Duplicate => "Duplicate".into(),
            Delete => "Delete".into(),
            SelectAll => "Select All".into(),
            InvertSelection => "Invert Selection".into(),
            SelectOpenShapes => "Select Open Shapes".into(),
            SelectCurrentLayer => "Select All in Current Layer".into(),
            Group => "Group".into(),
            Ungroup => "Ungroup".into(),
            Lock => "Lock".into(),
            Unlock => "Unlock".into(),
            ConvertToPath => "Convert to Path".into(),
            SplitText => "Split Text into Letters".into(),
            TextOnPath => "Put Text on Path".into(),
            ClipImage => "Crop / Clip Image to Shape".into(),
            ClipToShape => "Clip to Top Shape".into(),
            Blend => "Blend Two Shapes".into(),
            Warp => "Warp…".into(),
            Simplify => "Simplify / Smooth…".into(),
            CopyAlongPath => "Copy Along Path…".into(),
            UnclipImage => "Remove Image Clip".into(),
            DeleteDuplicates => "Delete Duplicates".into(),
            Align(a) => match a {
                bc_doc::edit::Align::Left => "Align Left",
                bc_doc::edit::Align::HCenter => "Align Horizontal Centres",
                bc_doc::edit::Align::Right => "Align Right",
                bc_doc::edit::Align::Top => "Align Top",
                bc_doc::edit::Align::VCenter => "Align Vertical Centres",
                bc_doc::edit::Align::Centers => "Align Centres",
                bc_doc::edit::Align::Bottom => "Align Bottom",
            }
            .into(),
            Distribute(h) => if h { "Distribute Horizontally" } else { "Distribute Vertically" }.into(),
            Dock(s) => match s {
                Side::Left => "Dock Left",
                Side::Right => "Dock Right",
                Side::Up => "Dock Up",
                Side::Down => "Dock Down",
            }
            .into(),
            MakeSameWidth => "Make Same Width".into(),
            MakeSameHeight => "Make Same Height".into(),
            Mirror(h) => if h { "Mirror Horizontally" } else { "Mirror Vertically" }.into(),
            Rotate(d) => if d > 0.0 { format!("Rotate {d}° Clockwise") } else { format!("Rotate {}° Counter-clockwise", -d) },
            MoveToBedCenter => "Move to Bed Centre".into(),
            Order(o) => match o {
                edit::Order::ToFront => "Bring to Front",
                edit::Order::ToBack => "Send to Back",
                edit::Order::Forward => "Bring Forward",
                edit::Order::Backward => "Send Backward",
            }
            .into(),
            GridArray => "Grid Array…".into(),
            CircularArray => "Circular Array…".into(),
            Nest => "Nest Selected…".into(),
            PlaceOnSlots => "Place on Each Slot…".into(),
            MarkSlots => "Mark as Jig Slots".into(),
            FitIntoBox => "Fit Into Box".into(),
            SplitTiles => "Split into Tiles…".into(),
            Parameters => "Parameters & Constraints…".into(),
            Stencil => "Stencil Bridges…".into(),
            PhotoLines => "Photo to Lines…".into(),
            Inlay => "Inlay…".into(),
            Rhinestones => "Rhinestone Template…".into(),
            Lattice => "Lattice…".into(),
            Knockout => "Knockout (Recess)…".into(),
            Hatch => "Hatch Fill…".into(),
            PiercePoints => "Pierce Points…".into(),
            Depth => "Shadow / 3D…".into(),
            RubberStamp => "Rubber Stamp…".into(),
            ContourCut => "Contour Cut Line…".into(),
            ShadowBox => "Shadow Box…".into(),
            Monogram => "Monogram…".into(),
            Offset => "Offset Shapes…".into(),
            Boolean(op) => match op {
                BooleanOp::Union => "Weld / Union",
                BooleanOp::Subtract => "Subtract",
                BooleanOp::Intersect => "Intersect",
            }
            .into(),
            BreakApart => "Break Apart".into(),
            ClosePaths => "Close Paths".into(),
            ReversePath => "Reverse Direction".into(),
            AutoJoin => "Auto-Join".into(),
            TraceImage => "Trace Image…".into(),
            RoundCorners => "Round All Corners".into(),
            GoToOrigin => "Go to Origin".into(),
            Settings => "Settings…".into(),
            Devices => "Devices…".into(),
            Barcode => "Barcode / QR Code…".into(),
            MaterialTest => "Material Test…".into(),
            BoxGenerator => "Makers: Box, Puzzle, Gears, Sign…".into(),
            RotarySetup => "Rotary Setup…".into(),
            MachineSettings => "Machine Settings (GRBL)…".into(),
            ProductionQueue => "Production Queue…".into(),
            PixScan => "PixScan (Photo to True Size)…".into(),
            Preview => "Preview".into(),
            Frame => "Frame".into(),
            Start => "Start".into(),
            RunInSections => "Run in Sections…".into(),
            ZoomToBed => "Zoom to Bed".into(),
            ZoomToSelection => "Zoom to Selection".into(),
        }
    }

    pub fn shortcut(self) -> Option<KeyboardShortcut> {
        use Command::*;
        match self {
            New => key(CTRL, Key::N),
            Open => key(CTRL, Key::O),
            Save => key(CTRL, Key::S),
            SaveAs => key(CTRL_SHIFT, Key::S),
            Import => key(CTRL, Key::I),
            Undo => key(CTRL, Key::Z),
            Redo => key(CTRL, Key::Y),
            Cut => key(CTRL, Key::X),
            Copy => key(CTRL, Key::C),
            Paste => key(CTRL, Key::V),
            PasteInPlace => key(CTRL_SHIFT, Key::V),
            Duplicate => key(CTRL, Key::D),
            SelectAll => key(CTRL, Key::A),
            InvertSelection => key(CTRL_SHIFT, Key::I),
            Group => key(CTRL, Key::G),
            Ungroup => key(CTRL_SHIFT, Key::G),
            Order(edit::Order::Forward) => key(NONE, Key::PageUp),
            Order(edit::Order::Backward) => key(NONE, Key::PageDown),
            Order(edit::Order::ToFront) => key(Modifiers::SHIFT, Key::PageUp),
            Order(edit::Order::ToBack) => key(Modifiers::SHIFT, Key::PageDown),
            Boolean(BooleanOp::Union) => key(CTRL, Key::W),
            Offset => key(Modifiers::ALT, Key::O),
            Preview => key(Modifiers::ALT, Key::P),
            ZoomToBed => key(CTRL, Key::Num0),
            ZoomToSelection => key(CTRL, Key::Num2),
            RotarySetup => key(CTRL_SHIFT, Key::R),
            _ => None,
        }
    }

    /// Commands that change or depend on the selection are only active outside text fields
    /// (so Ctrl+C in a text box copies text, not shapes).
    pub fn needs_canvas_focus(self) -> bool {
        !matches!(self, Command::New | Command::Open | Command::Save | Command::SaveAs | Command::Import | Command::Preview | Command::ZoomToBed)
    }

    /// All commands that have a keyboard shortcut, Ctrl+Shift variants first so they win.
    pub fn with_shortcuts() -> Vec<Command> {
        use Command::*;
        let mut all = vec![
            SaveAs, PasteInPlace, InvertSelection, Ungroup, RotarySetup, New, Open, Save, Import, Undo, Redo, Cut, Copy, Paste, Duplicate, SelectAll, Group,
            Order(edit::Order::ToFront), Order(edit::Order::ToBack), Order(edit::Order::Forward), Order(edit::Order::Backward),
            Boolean(BooleanOp::Union), Offset, Preview, ZoomToBed, ZoomToSelection,
        ];
        all.retain(|c| c.shortcut().is_some());
        all
    }
}

impl BurnCraftApp {
    /// Whether `cmd` can run now (menus grey out the rest).
    pub fn can_run(&self, cmd: Command) -> bool {
        use Command::*;
        let n = self.selection.len();
        let units = edit::units(&self.doc, &self.selection).len();
        let connected = self.laser.device.connected;
        match cmd {
            Undo => self.history.can_undo(),
            Redo => self.history.can_redo(),
            SplitText => self.selection.iter().filter_map(|id| self.doc.shape(*id)).any(|s| matches!(s.kind, ShapeKind::Text { .. }) && !s.locked),
            Cut | Copy | Duplicate | Delete | Lock | Unlock | ConvertToPath | Mirror(_) | Rotate(_) | MoveToBedCenter | Order(_) | GridArray | CircularArray | Nest | Offset
            | BreakApart | ClosePaths | ReversePath | ZoomToSelection | Ungroup | RoundCorners => n > 0,
            Paste | PasteInPlace => !self.clipboard.is_empty(),
            ExportSvg | ExportDxf | SelectAll | InvertSelection | DeleteDuplicates | SelectOpenShapes | SelectCurrentLayer => !self.doc.shapes.is_empty(),
            Group | Boolean(_) | AutoJoin => n > 1,
            Align(_) | MakeSameWidth | MakeSameHeight | Dock(_) => units > 1,
            Distribute(_) => units > 2,
            TraceImage => self.single_image().is_some(),
            Frame | Start | GoToOrigin => connected,
            Settings | Devices => true,
            TextOnPath => self.text_and_guide().is_some(),
            CopyAlongPath => edit::guide_and_items(&self.doc, &self.selection).is_some(),
            PlaceOnSlots => edit::slots_and_design(&self.doc, &self.selection).is_some(),
            MarkSlots => self.selection.iter().filter_map(|id| self.doc.shape(*id)).any(|s| s.is_closed() && !bc_doc::is_tool_layer(s.layer) && !matches!(s.kind, ShapeKind::Image { .. })),
            FitIntoBox => edit::frame_and_items(&self.doc, &self.selection).is_some(),
            Stencil | Inlay | RubberStamp | Lattice | Hatch | Depth => self.selection.iter().filter_map(|id| self.doc.shape(*id)).any(|s| s.is_closed() && !matches!(s.kind, ShapeKind::Image { .. })),
            Rhinestones | PiercePoints => self.selection.iter().filter_map(|id| self.doc.shape(*id)).any(|s| !matches!(s.kind, ShapeKind::Image { .. })),
            Knockout => self.selection.iter().filter_map(|id| self.doc.shape(*id)).filter(|s| s.is_closed() && !matches!(s.kind, ShapeKind::Image { .. })).count() >= 2,
            ContourCut | SplitTiles => !self.selection.is_empty(),
            Parameters => true,
            PhotoLines | ShadowBox => self.single_image().is_some(),
            Monogram => true,
            Warp | Simplify => self.selection.iter().filter_map(|id| self.doc.shape(*id)).any(|s| !s.locked && !matches!(s.kind, ShapeKind::Image { .. })),
            ClipImage => {
                let kinds: Vec<&ShapeKind> = self.selection.iter().filter_map(|id| self.doc.shape(*id)).map(|s| &s.kind).collect();
                kinds.len() == 2 && kinds.iter().any(|k| matches!(k, ShapeKind::Image { .. })) && kinds.iter().any(|k| !matches!(k, ShapeKind::Image { .. } | ShapeKind::Text { .. } | ShapeKind::Barcode { .. }))
            }
            ClipToShape => self.clip_parts().is_some(),
            Blend => self.blend_pair().is_some(),
            UnclipImage => self.selection.iter().filter_map(|id| self.doc.shape(*id)).any(|s| matches!(&s.kind, ShapeKind::Image { clip: Some(_), .. })),
            SaveGcode => self.laser.settings.controller == crate::laser::ControllerKind::Grbl && !self.doc.shapes.is_empty(),
            MachineSettings => self.laser.settings.controller == crate::laser::ControllerKind::Grbl && self.laser.settings.dialect.is_grbl(),
            New | Open | Save | SaveAs | Import | Barcode | MaterialTest | BoxGenerator | RotarySetup | Preview | RunInSections | ZoomToBed | ProductionQueue | PixScan => true,
        }
    }

    /// Clip to Top Shape: the front-most selected shape (closed) as the mask, and the other
    /// selected shapes that aren't pictures.
    fn clip_parts(&self) -> Option<(u64, Vec<u64>)> {
        let sel: Vec<&bc_doc::Shape> = self.doc.shapes.iter().filter(|s| self.selection.contains(&s.id)).collect();
        let (mask, rest) = sel.split_last()?;
        if !mask.is_closed() || matches!(mask.kind, ShapeKind::Image { .. }) {
            return None;
        }
        let parts: Vec<u64> = rest.iter().filter(|s| !matches!(s.kind, ShapeKind::Image { .. })).map(|s| s.id).collect();
        (!parts.is_empty()).then_some((mask.id, parts))
    }

    /// Blend: exactly two selected shapes that aren't pictures, back one first.
    fn blend_pair(&self) -> Option<(u64, u64)> {
        let sel: Vec<&bc_doc::Shape> = self.doc.shapes.iter().filter(|s| self.selection.contains(&s.id) && !matches!(s.kind, ShapeKind::Image { .. })).collect();
        match sel[..] {
            [a, b] if self.selection.len() == 2 => Some((a.id, b.id)),
            _ => None,
        }
    }

    fn single_image(&self) -> Option<u64> {
        let id = *self.selection.iter().next()?;
        (self.selection.len() == 1 && matches!(self.doc.shape(id)?.kind, ShapeKind::Image { .. })).then_some(id)
    }

    /// Runs a command, recording undo where it edits the design.
    pub fn run(&mut self, cmd: Command) {
        use Command::*;
        if !self.can_run(cmd) {
            return;
        }
        let label = cmd.label().trim_end_matches('…').to_string();
        match cmd {
            New => self.new_project(),
            Open => self.open(),
            Save => self.save(false),
            SaveAs => self.save(true),
            Import => self.import(),
            ExportSvg => self.export_svg(),
            ExportDxf => self.export_dxf(),
            SaveGcode => self.save_gcode(),
            Undo => self.undo(),
            Redo => self.redo(),
            Copy => self.copy_selection(),
            Cut => {
                self.copy_selection();
                self.edit_named(&label, |doc, sel| {
                    doc.remove_where(|id| sel.contains(&id));
                    sel.clear();
                });
            }
            Paste | PasteInPlace => {
                let shapes = self.clipboard.clone();
                let offset = if cmd == PasteInPlace {
                    Affine::IDENTITY
                } else {
                    // Centre the pasted shapes on the cursor if it's over the canvas, else offset them a little.
                    let b = shapes.iter().filter_map(|s| s.bounds()).reduce(|a, b| a.union(b));
                    match (self.cursor, b) {
                        (Some(c), Some(b)) => Affine::translate(c - b.center()),
                        _ => Affine::translate((5.0, 5.0)),
                    }
                };
                self.edit_named(&label, |doc, sel| *sel = edit::insert_copies(doc, &shapes, offset));
            }
            Duplicate => self.edit_named(&label, |doc, sel| *sel = edit::duplicate(doc, sel, Affine::translate((5.0, 5.0)))),
            Delete => self.edit_named(&label, |doc, sel| {
                doc.remove_where(|id| sel.contains(&id));
                sel.clear();
            }),
            SelectAll => self.selection = self.doc.shapes.iter().map(|s| s.id).collect(),
            InvertSelection => self.selection = self.doc.shapes.iter().map(|s| s.id).filter(|id| !self.selection.contains(id)).collect(),
            SelectOpenShapes => {
                self.selection = edit::open_shapes(&self.doc);
                self.status = format!("{} open shape(s)", self.selection.len());
            }
            SelectCurrentLayer => {
                let layer = self.current_layer;
                self.selection = self.doc.shapes.iter().filter(|s| s.layer == layer).map(|s| s.id).collect();
            }
            Group => self.edit_named(&label, |doc, sel| {
                doc.group(sel);
            }),
            Ungroup => self.edit_named(&label, |doc, sel| doc.ungroup(sel)),
            Lock | Unlock => {
                let locked = cmd == Lock;
                self.edit_named(&label, |doc, sel| {
                    for s in doc.shapes.iter_mut().filter(|s| sel.contains(&s.id)) {
                        s.locked = locked;
                    }
                });
            }
            ConvertToPath => self.edit_named(&label, |doc, sel| edit::convert_to_path(doc, sel)),
            SplitText => self.split_text(),
            TextOnPath => self.text_on_path(),
            CopyAlongPath => self.dialog = Some(Dialog::CopyAlong(crate::dialogs::CopyAlongDialog::default())),
            Warp => self.open_shape_edit(crate::dialogs::ShapeEdit::Warp { kind: bc_geom::warp::Warp::Arch, amount: 0.2 }),
            Simplify => self.open_shape_edit(crate::dialogs::ShapeEdit::Simplify { tolerance: 0.1, smooth: false }),
            ClipImage => {
                self.checkpoint_named(&label);
                match edit::clip_image(&mut self.doc, &self.selection) {
                    Some(id) => {
                        self.selection = BTreeSet::from([id]);
                        self.status = "Image clipped (Remove Image Clip brings the rest back)".into();
                    }
                    None => self.status = "Select one image and one closed shape".into(),
                }
            }
            ClipToShape => {
                let Some((mask_id, parts)) = self.clip_parts() else { return };
                let Some(mask) = self.doc.shape(mask_id).map(|s| s.world_path()) else { return };
                self.checkpoint_named(&label);
                for id in &parts {
                    if let Some(s) = self.doc.shape_mut(*id) {
                        s.effects.push(bc_doc::effects::Effect::Clip { mask: s.transform.inverse() * mask.clone() });
                    }
                }
                self.doc.remove_where(|id| id == mask_id);
                self.selection = parts.iter().copied().collect();
                self.status = format!("Clipped {} shape(s) to the top shape (remove the Clip effect in the Shape tab to undo it)", parts.len());
            }
            Blend => {
                let Some((start, end)) = self.blend_pair() else { return };
                let Some(end_path) = self.doc.shape(end).map(|s| s.world_path()) else { return };
                self.checkpoint_named(&label);
                if let Some(s) = self.doc.shape_mut(start) {
                    s.effects.push(bc_doc::effects::Effect::Blend { steps: 10, end: s.transform.inverse() * end_path });
                }
                self.doc.remove_where(|id| id == end);
                self.selection = BTreeSet::from([start]);
                self.laser_tab = crate::panels::LaserTab::Shape;
                self.status = "Blended: set the number of steps in the Shape tab".into();
            }
            UnclipImage => self.edit_named(&label, |doc, sel| {
                edit::unclip_images(doc, sel);
            }),
            DeleteDuplicates => {
                self.checkpoint_named(&label);
                let n = edit::delete_duplicates(&mut self.doc);
                self.prune_selection();
                self.status = format!("Removed {n} duplicate shape(s)");
            }
            Align(a) => self.edit_named(&label, |doc, sel| edit::align(doc, sel, a)),
            Distribute(h) => self.edit_named(&label, |doc, sel| edit::distribute(doc, sel, h)),
            Dock(side) => self.edit_named(&label, |doc, sel| edit::dock(doc, sel, side, 0.0)),
            MakeSameWidth => self.edit_named(&label, |doc, sel| edit::make_same_size(doc, sel, true)),
            MakeSameHeight => self.edit_named(&label, |doc, sel| edit::make_same_size(doc, sel, false)),
            Mirror(h) => self.edit_named(&label, |doc, sel| edit::mirror(doc, sel, h)),
            Rotate(d) => self.edit_named(&label, |doc, sel| edit::rotate(doc, sel, d)),
            MoveToBedCenter => {
                let (w, h) = self.doc.bed_size;
                self.edit_named(&label, |doc, sel| {
                    if let Some(b) = doc.bounds_of(sel.iter().copied()) {
                        edit::translate(doc, sel, Point::new(w / 2.0, h / 2.0) - b.center());
                    }
                });
            }
            Order(o) => self.edit_named(&label, |doc, sel| edit::reorder(doc, sel, o)),
            GridArray => self.dialog = Some(Dialog::GridArray(GridArrayDialog { columns: 3, rows: 2, gap_x: 2.0, gap_y: 2.0 })),
            CircularArray => {
                let c = self.doc.bounds_of(self.selection.iter().copied()).map(|b| b.center()).unwrap_or_default();
                self.dialog = Some(Dialog::CircularArray(CircularArrayDialog { count: 6, center_x: c.x, center_y: c.y + 30.0, rotate: true }));
            }
            Nest => self.dialog = Some(Dialog::Nest(NestDialog { spacing: 2.0, margin: 5.0, allow_rotate: true, true_shape: true, rotation_step: 90.0, fine: false })),
            Offset => self.dialog = Some(Dialog::Offset(OffsetDialog { distance: 2.0, delete_originals: false })),
            Boolean(op) => {
                let closed = self.doc.shapes.iter().filter(|s| self.selection.contains(&s.id) && s.is_closed()).count();
                if closed < 2 {
                    self.status = "Select at least two closed shapes".into();
                    return;
                }
                self.edit_named(&label, |doc, sel| *sel = edit::boolean(doc, sel, op).into_iter().collect());
            }
            BreakApart => self.edit_named(&label, |doc, sel| *sel = edit::break_apart(doc, sel)),
            ClosePaths => {
                self.checkpoint_named(&label);
                let n = edit::close_paths(&mut self.doc, &self.selection, 1.0);
                self.status = format!("Closed {n} path(s) (ends within 1 mm)");
            }
            ReversePath => self.edit_named(&label, |doc, sel| edit::reverse(doc, sel)),
            AutoJoin => {
                let before = self.selection.len();
                self.edit_named(&label, |doc, sel| *sel = edit::auto_join(doc, sel, 0.05));
                self.status = format!("Joined {before} shape(s) into {}", self.selection.len());
            }
            TraceImage => {
                if let Some(id) = self.single_image() {
                    self.dialog = Some(Dialog::Trace(TraceDialog::new(id)));
                }
            }
            Barcode => {
                let (w, h) = self.doc.bed_size;
                self.dialog = Some(Dialog::Barcode(BarcodeDialog { target: None, at: Point::new(w / 2.0, h / 2.0), symbology: bc_doc::Symbology::Qr, content: String::new(), size: 25.0 }));
            }
            MaterialTest => self.dialog = Some(Dialog::MaterialTest(crate::dialogs::MaterialTest::default())),
            BoxGenerator => {
                // With something selected, the puzzle covers it (e.g. a photo to cut into pieces).
                let mut d = crate::dialogs::BoxDialog::default();
                if let Some(b) = self.doc.bounds_of(self.selection.iter().copied()).filter(|b| b.width() > 5.0 && b.height() > 5.0) {
                    d.puzzle.width = b.width();
                    d.puzzle.height = b.height();
                    d.place_at = Some(b.origin());
                }
                self.dialog = Some(Dialog::Box(Box::new(d)));
            }
            RotarySetup => self.show_rotary = true,
            MachineSettings => self.laser.grbl_config.open = true,
            ProductionQueue => self.queue.open = true,
            PixScan => self.pixscan.open = true,
            RunInSections => self.open_sections(),
            MarkSlots => {
                // Slots go on the tool layer: shown on the design, never sent to the laser.
                self.edit_named(&label, |doc, sel| {
                    for s in doc.shapes.iter_mut().filter(|s| sel.contains(&s.id) && s.is_closed() && !matches!(s.kind, ShapeKind::Image { .. })) {
                        s.layer = bc_doc::CUT_LAYER_COUNT;
                    }
                });
                self.status = "Marked as jig slots (never burned). Select them with your design, then Arrange > Place on Each Slot. Click a palette colour to turn a slot back into a normal shape.".into();
            }
            Parameters => self.show_params = true,
            SplitTiles => {
                let Some(b) = self.doc.bounds_of(self.selection.iter().copied()) else { return };
                let bed = self.doc.bed_size;
                // Enough tiles that each fits the bed with room for the joints.
                let fit = |len: f64, room: f64| ((len / (room - 40.0).max(10.0)).ceil() as usize).max(1);
                let (cols, rows) = (fit(b.width(), bed.0), fit(b.height(), bed.1));
                let spec = bc_geom::tiles::TileSpec { cols: if cols * rows == 1 { 2 } else { cols }, rows, joint: bc_geom::tiles::Joint::Dovetail, size: 15.0, spacing: 60.0 };
                self.dialog = Some(Dialog::Tool(crate::tools::ToolDialog::Tiles { spec, gap: 15.0, area: (b.width(), b.height()), bed }));
            }
            FitIntoBox => {
                if let Some((frame, items)) = edit::frame_and_items(&self.doc, &self.selection) {
                    let margin = self.doc.shape(frame).and_then(|s| s.bounds()).map_or(0.0, |b| b.width().min(b.height()) * 0.08);
                    self.edit_named(&label, |doc, _| {
                        edit::fit_into(doc, &items, frame, margin);
                    });
                }
            }
            Stencil | PhotoLines | Inlay | RubberStamp | ContourCut | ShadowBox | Monogram | Rhinestones | Lattice | Knockout | Hatch | PiercePoints | Depth => {
                use crate::tools::ToolDialog;
                let image = self.single_image().unwrap_or(0);
                let d = match cmd {
                    Stencil => ToolDialog::Stencil { width: 1.5, horizontal: false },
                    PhotoLines => ToolDialog::PhotoLines { image, options: bc_raster::lines::LineOptions::default() },
                    Inlay => ToolDialog::Inlay { kerf: 0.1, clearance: 0.0, mirror: false },
                    RubberStamp => ToolDialog::Stamp { margin: 4.0, shoulder: 0.5 },
                    ContourCut => ToolDialog::Contour { distance: 2.0 },
                    ShadowBox => ToolDialog::ShadowBox { image, layers: 4, border: 10.0, smooth: 0.3 },
                    Rhinestones => ToolDialog::Rhinestones { diameter: 3.2, gap: 0.8, layout: bc_geom::pathtools::StoneLayout::Outline },
                    Lattice => ToolDialog::Lattice { cell: 8.0, bar: 2.0 },
                    Knockout => ToolDialog::Knockout { gap: 0.15 },
                    Hatch => ToolDialog::Hatch { spacing: 0.5, angle: 45.0, cross: false },
                    PiercePoints => ToolDialog::Pierce { diameter: 0.5 },
                    Depth => ToolDialog::Depth { kind: 0, dx: 2.0, dy: 2.0, skew: 0.8, squash: 0.3 },
                    _ => ToolDialog::Monogram { first: String::new(), last: String::new(), middle: String::new(), font: self.fonts.default_family(), height: 40.0, frame: crate::tools::MonogramFrame::Ring },
                };
                self.dialog = Some(Dialog::Tool(d));
            }
            PlaceOnSlots => {
                let slots = edit::slots_and_design(&self.doc, &self.selection).map_or(0, |(s, _)| s.len());
                self.dialog = Some(Dialog::PlaceSlots(crate::dialogs::PlaceSlotsDialog { slots, fit: false, margin: 2.0, keep_original: false }));
            }
            Preview => self.laser_action(LaserAction::Preview),
            Frame => self.laser_action(LaserAction::Frame),
            Start => self.laser_action(LaserAction::Start),
            RoundCorners => {
                let r = self.corner_radius;
                self.edit_named(&label, |doc, sel| edit::round_corners(doc, sel, r));
            }
            GoToOrigin => self.laser.go_to_origin(),
            Settings => self.show_settings = true,
            Devices => self.show_devices = true,
            ZoomToBed => self.canvas.request_fit(),
            ZoomToSelection => {
                if let Some(b) = self.doc.bounds_of(self.selection.iter().copied()) {
                    self.canvas.zoom_to(b);
                }
            }
        }
    }

    fn copy_selection(&mut self) {
        self.clipboard = self.doc.shapes.iter().filter(|s| self.selection.contains(&s.id)).cloned().collect();
        self.status = format!("Copied {} shape(s)", self.clipboard.len());
    }

    pub(crate) fn edit_named(&mut self, label: &str, f: impl FnOnce(&mut bc_doc::Document, &mut BTreeSet<u64>)) {
        self.checkpoint_named(label);
        f(&mut self.doc, &mut self.selection);
    }
}
