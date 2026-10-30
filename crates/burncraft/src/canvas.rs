//! The design canvas: view transform, drawing, rulers, snapping and pointer tools.

use crate::units::Units;
use bc_doc::{Document, ImageAdjust, Shape, ShapeKind};
use bc_geom::nodes::{self, Handle, HandleKind};
use bc_geom::{Affine, BezPath, PathEl, Point, Rect, Vec2};
use eframe::egui::{self, Color32, Key, Pos2, Sense, Stroke, StrokeKind};
use std::collections::{BTreeSet, HashMap};

/// Screen <-> document (mm) mapping. Document Y points down, like the screen.
#[derive(Debug, Clone, Copy)]
pub struct View {
    /// Screen position of the document origin, relative to the canvas rect.
    pub offset: egui::Vec2,
    /// Pixels per millimetre.
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        Self { offset: egui::vec2(20.0, 20.0), zoom: 1.5 }
    }
}

impl View {
    pub fn to_screen(self, origin: Pos2, p: Point) -> Pos2 {
        origin + self.offset + egui::vec2(p.x as f32, p.y as f32) * self.zoom
    }

    pub fn to_doc(self, origin: Pos2, p: Pos2) -> Point {
        let v = (p - origin - self.offset) / self.zoom;
        Point::new(v.x as f64, v.y as f64)
    }

    /// Fits `area` (mm) into a viewport of `size` pixels with a margin.
    fn fit_rect(&mut self, area: Rect, size: egui::Vec2) {
        let margin = 40.0;
        let zx = (size.x - 2.0 * margin) / area.width().max(1.0) as f32;
        let zy = (size.y - 2.0 * margin) / area.height().max(1.0) as f32;
        self.zoom = zx.min(zy).clamp(0.05, 200.0);
        let centre = egui::vec2(area.center().x as f32, area.center().y as f32) * self.zoom;
        self.offset = size / 2.0 - centre;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Nodes,
    Line,
    Rect,
    Ellipse,
    Polygon,
    Star,
    Text,
    PositionLaser,
    Measure,
    /// Click a corner to round it with the toolbar radius (chosen from the round-corner button).
    Corner,
    /// Click a piece of outline to delete it back to where other lines cross it.
    Trim,
    /// Drag a line to split the shapes it cuts through.
    Knife,
    /// Drag between two points to add a dimension (on a tool layer, never cut).
    Dimension,
    /// Drag over images to rub parts of them out.
    Eraser,
    /// Click or drag across the areas where selected shapes overlap to merge them (Alt: delete).
    ShapeBuilder,
}

impl Tool {
    pub const ALL: [Tool; 10] = [Tool::Select, Tool::Nodes, Tool::Line, Tool::Rect, Tool::Ellipse, Tool::Polygon, Tool::Star, Tool::Text, Tool::PositionLaser, Tool::Measure];

    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Tool::Select => Icon::Select,
            Tool::Nodes => Icon::Nodes,
            Tool::Line => Icon::Line,
            Tool::Rect => Icon::Rect,
            Tool::Ellipse => Icon::Ellipse,
            Tool::Polygon => Icon::Polygon,
            Tool::Star => Icon::Star,
            Tool::Text => Icon::Text,
            Tool::PositionLaser => Icon::PositionLaser,
            Tool::Measure => Icon::Measure,
            Tool::Corner => Icon::Corner,
            Tool::Trim => Icon::Trim,
            Tool::Knife => Icon::Knife,
            Tool::Eraser => Icon::Eraser,
            Tool::Dimension => Icon::Dimension,
            Tool::ShapeBuilder => Icon::ShapeBuilder,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Nodes => "Edit nodes",
            Tool::Line => "Line",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Polygon => "Polygon",
            Tool::Star => "Star",
            Tool::Text => "Text",
            Tool::PositionLaser => "Position laser",
            Tool::Measure => "Measure",
            Tool::Corner => "Round corner",
            Tool::Trim => "Trim",
            Tool::Knife => "Knife",
            Tool::Eraser => "Image eraser",
            Tool::Dimension => "Dimension",
            Tool::ShapeBuilder => "Shape Builder",
        }
    }

    /// Single-key shortcut (Select is Esc).
    pub fn key(self) -> Option<Key> {
        match self {
            Tool::Select => None,
            Tool::Nodes => Some(Key::N),
            Tool::Line => Some(Key::L),
            Tool::Rect => Some(Key::R),
            Tool::Ellipse => Some(Key::E),
            Tool::Polygon => Some(Key::P),
            Tool::Star => Some(Key::S),
            Tool::Text => Some(Key::T),
            Tool::PositionLaser | Tool::Corner | Tool::Trim | Tool::Knife | Tool::Dimension | Tool::Eraser => None,
            Tool::ShapeBuilder => Some(Key::B),
            Tool::Measure => Some(Key::M),
        }
    }

    /// How to use the tool, as short points for the Tips tab.
    pub fn tips(self) -> &'static [&'static str] {
        match self {
            Tool::Select => &[
                "Click a shape's line to select it (or the inside of a filled shape). Shift+click adds or removes.",
                "Drag a box on empty space: left to right picks shapes fully inside it; right to left picks every shape it touches.",
                "Drag a shape's line, or the centre handle, to move it. Arrow keys nudge 1 mm (Shift: 10 mm, Ctrl: 0.1 mm).",
                "Corner handles resize, keeping the proportions (hold Ctrl to stretch).",
                "Side handles stretch one way only.",
                "The small handles on the top and right edges slant (shear) the selection.",
                "The circle above the selection rotates it (hold Shift for 15° steps).",
                "Double-click text or a barcode to edit it. Right-click for more commands.",
            ],
            Tool::Nodes => &[
                "Drag a point, or a curve handle, to reshape the selected shape.",
                "Double-click an edge to add a point.",
                "Click a point and press Delete to remove it.",
            ],
            Tool::Line => &[
                "Click to add points.",
                "Double-click or press Enter to finish an open line.",
                "Click the first point again to close the shape.",
            ],
            Tool::Rect | Tool::Ellipse => &["Drag to draw.", "Hold Shift for a square or a circle."],
            Tool::Polygon | Tool::Star => &["Drag out from the centre to draw.", "Set the number of sides or points under the tools."],
            Tool::Text => &["Click where the text should go.", "Double-click existing text to edit it."],
            Tool::PositionLaser => &["Click to move the laser head there, with the laser off."],
            Tool::Measure => &["Drag between two points to see the distance and angle. Nothing is added to the design."],
            Tool::Trim => &["Click a piece of outline to delete it back to the nearest lines crossing it.", "Press Esc when you're done."],
            Tool::Knife => &["Drag a line right across shapes to split each of them in two.", "Press Esc when you're done."],
            Tool::Eraser => &["Brush: drag over a picture to rub parts of it out; they won't burn. Set the brush size under the tools.", "Colour: click an area of a picture (a white background, say) to clear it, and everything touching it of that colour. Tick All to clear that colour all over the picture.", "Press Esc when you're done."],
            Tool::Dimension => &["Drag between two points to add a labelled dimension.", "Dimensions are shown but never cut."],
            Tool::ShapeBuilder => &[
                "Select overlapping shapes first: every area their outlines enclose is outlined.",
                "Click an area, or drag across several, to merge them into one shape.",
                "Hold Alt and click or drag to delete areas instead.",
                "Press Esc when you're done.",
            ],
            Tool::Corner => &[
                "Each corner shows a marker; filled markers are already rounded.",
                "Set the radius (or chamfer size) under the tools, then click a corner.",
                "A radius of 0 makes a rounded corner sharp again. Press Esc when you're done.",
            ],
        }
    }
}

/// An in-progress pointer gesture.
#[derive(Debug, Clone)]
enum Gesture {
    /// Dragging selected shapes; `last` is the previous (snapped) pointer position (mm).
    Move {
        /// Snapped pointer at the start, the moving shapes' bounds then, and the offset applied so far.
        start: Point,
        bounds: Rect,
        applied: Vec2,
        /// Smart-guide targets: x positions (vertical lines) and y positions (horizontal lines).
        targets: (Vec<f64>, Vec<f64>),
    },
    /// Knife tool: dragging a cut line from `start`.
    Knife { start: Point },
    /// Dimension tool: dragging from `start`.
    Dimension { start: Point },
    /// Eraser tool: the brush path so far (mm).
    Erase { points: Vec<Point> },
    /// Shape Builder: the pointer's path so far (mm).
    Build { points: Vec<Point> },
    /// Dragging a ruler guide: a new one (`index` None) or an existing one.
    Guide { index: Option<usize>, vertical: bool },
    /// Rubber-band selection from `start` (mm).
    Marquee { start: Point },
    /// Drawing a shape from `start` (mm).
    Create { start: Point },
    /// Resizing the selection from a corner handle; `anchor` is the opposite corner.
    Scale { anchor: Point, start: Rect, applied: Affine, axis: ScaleAxis },
    /// Rotating the selection about `center`.
    Rotate { center: Point, start_angle: f64, applied: f64 },
    /// Shearing from an edge handle. `horizontal`: the top/bottom edge slides sideways about `anchor` (the opposite edge).
    Shear { horizontal: bool, anchor: f64, span: f64, start: Point, applied: Affine },
    /// Dragging a node or curve handle of one shape.
    Node { shape: u64, handle: Handle, last: Point },
    /// Measuring from `start`.
    Measure { start: Point },
}

/// Colours of the design area.
struct BoardColors {
    /// Around the bed.
    outside: Color32,
    bed: Color32,
    grid: Color32,
    border: Color32,
}

/// Which way a resize may change the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScaleAxis {
    /// Corner handle: both directions (proportional unless Ctrl).
    Both,
    /// Left/right handle: width only.
    X,
    /// Top/bottom handle: height only.
    Y,
}

/// A handle on the selection box (Select tool).
#[derive(Debug, Clone, Copy, PartialEq)]
enum SelectHandle {
    Rotate { center: Point },
    /// Edge handle: `horizontal` slides the top/bottom edge sideways about `anchor` (the opposite edge).
    /// Middle of an edge (`horizontal`: top or bottom). Stretches one way; with Alt, shears about `anchor` (the opposite edge).
    Edge { horizontal: bool, anchor: f64, span: f64, bounds: Rect },
    /// Corner handle; `anchor` is the opposite corner, `nw_se` whether it's top-left/bottom-right on screen.
    Corner { anchor: Point, bounds: Rect, nw_se: bool },
    /// Shear handle on the top edge (`horizontal`: slides it sideways) or the right edge (slides it
    /// up and down), about the opposite edge `anchor`.
    Shear { horizontal: bool, anchor: f64, span: f64 },
    /// The centre handle: drag to move the selection.
    Move,
}

/// Cached texture for an image shape, keyed by its data and adjustments.
struct ImageTexture {
    data_ptr: usize,
    adjust: ImageAdjust,
    clip: Option<BezPath>,
    /// Burn preview settings it was made with (dither mode, interval bits), or None for the photo.
    preview: Option<(bc_doc::DitherMode, u64)>,
    texture: egui::TextureHandle,
}

/// Cached inside of a shape on a fill layer: a white mask over its bounds (tinted when drawn),
/// keyed by the shape's outline.
struct FillTexture {
    key: u64,
    bounds: Rect,
    texture: egui::TextureHandle,
}

/// Canvas state that persists between frames.
pub struct Canvas {
    pub view: View,
    pub tool: Tool,
    pub snap_to_grid: bool,
    pub snap_to_objects: bool,
    pub grid_mm: f64,
    pub show_rulers: bool,
    /// Draw the design area dark (set from the preferences each frame).
    pub dark_board: bool,
    /// Snap moving shapes into line with others (set from the preferences each frame).
    pub smart_guides: bool,
    /// Show images as the laser will burn them.
    pub burn_preview: bool,
    pub units: Units,
    /// Sides for new polygons / points for new stars.
    pub polygon_sides: u32,
    /// Inner radius of new stars, as a fraction of the outer radius.
    pub star_ratio: f64,
    /// Show Y increasing upwards from the bottom of the bed (labels only; data is unchanged).
    pub y_up: bool,
    /// Which drag operations the selection handles allow.
    pub allow_move: bool,
    pub allow_size: bool,
    pub allow_rotate: bool,
    pub allow_shear: bool,
    /// Where the machine's (0, 0) is on the bed, if a device is configured.
    pub machine_origin: Option<Point>,
    /// Lines to show over the design, highlighted (a live preview: what Trace Image will make).
    pub highlight: Option<bc_geom::BezPath>,
    /// Shape Builder: the areas the selected shapes enclose (set by the app while the tool is on).
    pub builder_regions: Vec<bc_geom::BezPath>,
    gesture: Option<Gesture>,
    /// Points placed so far by the Line tool (mm).
    line_points: Vec<Point>,
    needs_fit: bool,
    pending_zoom: Option<Rect>,
    pending_zoom_factor: Option<f32>,
    textures: HashMap<u64, ImageTexture>,
    fills: HashMap<u64, FillTexture>,
    /// Node selected with the node tool: (shape, element index).
    selected_node: Option<(u64, usize)>,
    /// Points other shapes offer for snapping (rebuilt every frame).
    snap_points: Vec<Point>,
    /// Smart-guide lines to draw this frame: (vertical, position mm).
    alignments: Vec<(bool, f64)>,
    /// The document's ruler guides (copied each frame for snapping).
    guides: Vec<bc_doc::GuideLine>,
    /// Last completed measurement.
    measured: Option<(Point, Point)>,
    /// Eraser brush diameter (mm).
    pub eraser_mm: f64,
    /// Image eraser: clear an area of one colour with a click, instead of brushing.
    pub eraser_by_colour: bool,
    /// How different (0..1) a pixel's colour may be and still be cleared with it.
    pub eraser_tolerance: f32,
    /// Clear that colour everywhere in the picture, not just the area touching the click.
    pub eraser_everywhere: bool,
    /// Live drawing symmetry.
    pub symmetry: Symmetry,
    /// Draw locked sizes and distances.
    pub show_constraints: bool,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            view: View::default(),
            tool: Tool::Select,
            snap_to_grid: false,
            snap_to_objects: true,
            grid_mm: 10.0,
            show_rulers: true,
            dark_board: false,
            smart_guides: true,
            burn_preview: false,
            alignments: Vec::new(),
            guides: Vec::new(),
            units: Units::Millimetres,
            polygon_sides: 6,
            star_ratio: 0.5,
            y_up: true,
            allow_move: true,
            allow_size: true,
            allow_rotate: true,
            allow_shear: true,
            machine_origin: None,
            highlight: None,
            builder_regions: Vec::new(),
            gesture: None,
            line_points: Vec::new(),
            needs_fit: true,
            pending_zoom: None,
            pending_zoom_factor: None,
            textures: HashMap::new(),
            fills: HashMap::new(),
            selected_node: None,
            snap_points: Vec::new(),
            measured: None,
            eraser_mm: 3.0,
            eraser_by_colour: false,
            eraser_tolerance: 0.15,
            eraser_everywhere: false,
            symmetry: Symmetry::default(),
            show_constraints: true,
        }
    }
}

/// What the canvas asks the app to do. Edits go through the app so it can record undo.
pub enum CanvasAction {
    /// Record a named undo checkpoint before the edit that follows this frame.
    Checkpoint(&'static str),
    AddShape { kind: ShapeKind, transform: Affine },
    /// Apply a document-space transform to the selection.
    Transform(Affine),
    SetSelection(BTreeSet<u64>),
    /// Text tool clicked at this point.
    TextAt(Point),
    /// Double-clicked a text shape or barcode.
    EditText(u64),
    /// Replace a shape's geometry with this path (document coordinates); it becomes a plain path.
    SetPath(u64, BezPath),
    Status(String),
    /// Move the laser head to this design point.
    MoveLaser(Point),
    /// Corner tool: round the corner at this point of this shape.
    RoundCorner(u64, Point),
    /// Replace the document's ruler guides.
    SetGuides(Vec<bc_doc::GuideLine>),
    /// Trim tool clicked here (with this pick tolerance, mm).
    Trim(Point, f64),
    /// Knife dragged from the first point to the second.
    Knife(Point, Point),
    /// Dimension dragged from the first point to the second.
    Dimension(Point, Point),
    /// Eraser brushed along these points with this radius (mm).
    Erase(Vec<Point>, f64),
    /// Image eraser by colour: clear the area of colour under this point.
    EraseColour(Point),
    /// Shape Builder: merge (or with `delete`, remove) the areas under these points.
    ShapeBuild { points: Vec<Point>, delete: bool },
}

/// Live symmetry while drawing: new shapes are repeated as mirror images or around a centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Symmetry {
    pub mode: SymmetryMode,
    /// Copies around the centre (radial mode).
    pub count: u32,
    /// Radial mode: also mirror every other copy (like a kaleidoscope).
    pub mirror: bool,
    pub center: Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymmetryMode {
    Off,
    /// Mirrored left–right about a vertical line.
    LeftRight,
    /// Mirrored top–bottom about a horizontal line.
    TopBottom,
    /// Both: four copies.
    FourWay,
    /// Turned copies around the centre.
    Radial,
}

impl SymmetryMode {
    pub const ALL: [SymmetryMode; 5] = [SymmetryMode::Off, SymmetryMode::LeftRight, SymmetryMode::TopBottom, SymmetryMode::FourWay, SymmetryMode::Radial];

    pub fn label(self) -> &'static str {
        match self {
            SymmetryMode::Off => "Off",
            SymmetryMode::LeftRight => "Mirror left–right",
            SymmetryMode::TopBottom => "Mirror top–bottom",
            SymmetryMode::FourWay => "Four-way mirror",
            SymmetryMode::Radial => "Radial (mandala)",
        }
    }
}

impl Default for Symmetry {
    fn default() -> Self {
        Self { mode: SymmetryMode::Off, count: 8, mirror: true, center: Point::ORIGIN }
    }
}

impl Symmetry {
    /// The transforms that make the extra copies of a newly drawn shape (not the original).
    pub fn copies(&self) -> Vec<Affine> {
        let c = self.center.to_vec2();
        let about = |t: Affine| Affine::translate(c) * t * Affine::translate(-c);
        let flip_x = Affine::scale_non_uniform(-1.0, 1.0);
        let flip_y = Affine::scale_non_uniform(1.0, -1.0);
        match self.mode {
            SymmetryMode::Off => Vec::new(),
            SymmetryMode::LeftRight => vec![about(flip_x)],
            SymmetryMode::TopBottom => vec![about(flip_y)],
            SymmetryMode::FourWay => vec![about(flip_x), about(flip_y), about(Affine::scale(-1.0))],
            SymmetryMode::Radial => {
                let n = self.count.clamp(2, 64);
                let step = std::f64::consts::TAU / n as f64;
                let mut out: Vec<Affine> = (1..n).map(|k| about(Affine::rotate(step * k as f64))).collect();
                if self.mirror {
                    // Mirror about the line through the centre at the first slice's middle.
                    let mirror = Affine::rotate(step / 2.0) * flip_y * Affine::rotate(-step / 2.0);
                    out.extend((0..n).map(|k| about(Affine::rotate(step * k as f64) * mirror)));
                }
                out
            }
        }
    }
}

const HANDLE_PX: f32 = 5.0;
/// How strongly shapes on layers with Output turned off are drawn (1 = fully).
const DISABLED_OPACITY: f32 = 0.3;
const ROTATE_HANDLE_GAP: f32 = 24.0;
const SNAP_PX: f32 = 8.0;
const RULER_PX: f32 = 18.0;

impl Canvas {
    pub fn request_fit(&mut self) {
        self.needs_fit = true;
    }

    pub fn zoom_to(&mut self, area: Rect) {
        self.pending_zoom = Some(area);
    }

    /// Zooms in (factor > 1) or out about the centre of the view.
    pub fn zoom_by(&mut self, factor: f32) {
        self.pending_zoom_factor = Some(self.pending_zoom_factor.unwrap_or(1.0) * factor);
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.gesture = None;
        self.line_points.clear();
        self.selected_node = None;
        self.measured = None;
    }

    /// Whether the node tool has a node selected (Delete then removes the node, not the shape).
    pub fn node_selected(&self) -> bool {
        self.tool == Tool::Nodes && self.selected_node.is_some()
    }

    /// Cancels any in-progress drawing. Returns whether there was something to cancel.
    pub fn cancel(&mut self) -> bool {
        let had = self.gesture.is_some() || !self.line_points.is_empty() || self.measured.is_some();
        self.gesture = None;
        self.line_points.clear();
        self.measured = None;
        had
    }

    // --- Snapping.

    fn rebuild_snap_points(&mut self, doc: &Document, selection: &BTreeSet<u64>) {
        self.snap_points.clear();
        if !self.snap_to_objects {
            return;
        }
        const LIMIT: usize = 20_000;
        for s in doc.shapes.iter().filter(|s| !selection.contains(&s.id) && doc.layers[s.layer].visible) {
            if let Some(b) = s.bounds() {
                self.snap_points.extend([b.center(), b.origin(), Point::new(b.x1, b.y0), Point::new(b.x1, b.y1), Point::new(b.x0, b.y1)]);
            }
            if !matches!(s.kind, ShapeKind::Image { .. } | ShapeKind::Text { .. } | ShapeKind::Barcode { .. }) {
                self.snap_points.extend(nodes::handles(&s.world_path()).into_iter().filter(|(h, _)| h.kind == HandleKind::Node).map(|(_, p)| p));
            }
            if self.snap_points.len() > LIMIT {
                break;
            }
        }
    }

    /// The object point within snapping distance of `p`, if any.
    fn object_snap(&self, p: Point) -> Option<Point> {
        let tol = (SNAP_PX / self.view.zoom) as f64;
        self.snap_points.iter().copied().filter(|q| q.distance(p) <= tol).min_by(|a, b| a.distance(p).total_cmp(&b.distance(p)))
    }

    /// Smart-guide targets for moving `moving`: other shapes' edges and centres, the bed's, and ruler guides.
    fn guide_targets(doc: &Document, moving: &BTreeSet<u64>) -> (Vec<f64>, Vec<f64>) {
        let (w, h) = doc.bed_size;
        let (mut xs, mut ys) = (vec![0.0, w / 2.0, w], vec![0.0, h / 2.0, h]);
        for b in doc.shapes.iter().filter(|s| !moving.contains(&s.id) && doc.layers[s.layer].visible).filter_map(|s| s.bounds()).take(2000) {
            xs.extend([b.x0, b.center().x, b.x1]);
            ys.extend([b.y0, b.center().y, b.y1]);
        }
        for g in &doc.guides {
            if g.vertical { xs.push(g.pos) } else { ys.push(g.pos) }
        }
        (xs, ys)
    }

    /// Snaps to nearby objects first, then to ruler guides and the grid.
    fn snap(&self, p: Point) -> Point {
        if let Some(q) = self.object_snap(p) {
            return q;
        }
        let tol = (SNAP_PX / self.view.zoom) as f64;
        let near = |v: f64, vertical: bool| self.guides.iter().filter(|g| g.vertical == vertical && (g.pos - v).abs() <= tol).map(|g| g.pos).next();
        let (gx, gy) = (near(p.x, true), near(p.y, false));
        if gx.is_some() || gy.is_some() {
            return Point::new(gx.unwrap_or(p.x), gy.unwrap_or(p.y));
        }
        if self.snap_to_grid && self.grid_mm > 0.0 {
            let g = self.grid_mm;
            Point::new((p.x / g).round() * g, (p.y / g).round() * g)
        } else {
            p
        }
    }

    fn snap_active(&self) -> bool {
        matches!(self.tool, Tool::Line | Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Star | Tool::Text | Tool::Measure | Tool::Dimension | Tool::Knife)
            || matches!(self.gesture, Some(Gesture::Move { .. } | Gesture::Node { .. }))
    }

    // --- Shape creation.

    /// The shape a create-drag from `a` to `b` makes. Shift makes rectangles/ellipses square/round.
    fn make_shape(&self, a: Point, b: Point, shift: bool) -> Option<(ShapeKind, Affine)> {
        let (mut w, mut h) = ((b.x - a.x).abs(), (b.y - a.y).abs());
        if shift {
            let s = w.max(h);
            (w, h) = (s, s);
        }
        if w < 0.01 || h < 0.01 {
            return None;
        }
        let center = Point::new(a.x + (b.x - a.x).signum() * w / 2.0, a.y + (b.y - a.y).signum() * h / 2.0);
        let r = w.min(h) / 2.0;
        let kind = match self.tool {
            Tool::Rect => ShapeKind::Rect { width: w, height: h, corner_radius: 0.0 },
            Tool::Ellipse => ShapeKind::Ellipse { radius_x: w / 2.0, radius_y: h / 2.0 },
            Tool::Polygon => ShapeKind::Polygon { sides: self.polygon_sides.max(3), radius: r },
            Tool::Star => ShapeKind::Star { points: self.polygon_sides.max(3), outer: r, inner: r * self.star_ratio.clamp(0.05, 0.99) },
            _ => return None,
        };
        Some((kind, Affine::translate(center.to_vec2())))
    }

    /// Draws the canvas and handles input. `head` is the laser head position (design mm), if known.
    /// Returns the actions, the cursor position (mm), and the canvas response (for the context menu).
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        doc: &Document,
        selection: &BTreeSet<u64>,
        head: Option<Point>,
        overlay: Option<&crate::camera::Overlay>,
    ) -> (Vec<CanvasAction>, Option<Point>, egui::Response) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = response.rect;
        let origin = rect.min;
        if self.needs_fit {
            self.view.fit_rect(Rect::new(0.0, 0.0, doc.bed_size.0, doc.bed_size.1), rect.size());
            self.needs_fit = false;
        }
        if let Some(area) = self.pending_zoom.take() {
            self.view.fit_rect(area, rect.size());
        }
        if let Some(f) = self.pending_zoom_factor.take() {
            let centre = rect.center();
            let anchor = self.view.to_doc(origin, centre);
            self.view.zoom = (self.view.zoom * f).clamp(0.05, 200.0);
            self.view.offset += centre - self.view.to_screen(origin, anchor);
        }
        self.rebuild_snap_points(doc, selection);

        let mut actions = Vec::new();
        let hover_doc = response.hover_pos().map(|p| self.view.to_doc(origin, p));

        // --- View navigation: wheel zooms about the cursor, middle/right drag pans.
        if let Some(hover) = response.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let factor = (scroll * 0.0015).exp();
                let anchor = self.view.to_doc(origin, hover);
                self.view.zoom = (self.view.zoom * factor).clamp(0.05, 200.0);
                let new_screen = self.view.to_screen(origin, anchor);
                self.view.offset += hover - new_screen;
            }
        }
        if response.dragged_by(egui::PointerButton::Middle) || response.dragged_by(egui::PointerButton::Secondary) {
            self.view.offset += response.drag_delta();
        }
        // Right-click on an unselected shape selects it, so the context menu applies to it.
        if response.secondary_clicked() {
            let hit = hover_doc.and_then(|p| doc.hit_test(p, 4.0 / self.view.zoom as f64));
            if let Some(id) = hit.filter(|id| !selection.contains(id)) {
                actions.push(CanvasAction::SetSelection(BTreeSet::from([id])));
            }
        }

        // --- Ruler guides: drag one out of the top or left ruler, drag an existing guide to move it,
        // or drag it back onto a ruler to remove it.
        self.alignments.clear();
        self.guides = doc.guides.clone();
        self.guide_gesture(ui, &response, doc, selection, &mut actions);

        // --- Tools.
        let pointer = response.interact_pointer_pos().map(|p| self.view.to_doc(origin, p));
        let dragging_guide = matches!(self.gesture, Some(Gesture::Guide { .. }));
        if !dragging_guide { match self.tool {
            Tool::Select => self.select_tool(&response, doc, selection, pointer, &mut actions),
            Tool::Nodes => self.node_tool(ui, &response, doc, selection, pointer, &mut actions),
            Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Star => self.create_tool(&response, pointer, &mut actions),
            Tool::Line => self.line_tool(&response, ui, pointer, &mut actions),
            Tool::Text => {
                if response.clicked_by(egui::PointerButton::Primary)
                    && let Some(p) = pointer {
                        actions.push(CanvasAction::TextAt(self.snap(p)));
                    }
            }
            Tool::Measure => self.measure_tool(&response, pointer, &mut actions),
            Tool::PositionLaser => {
                if response.clicked_by(egui::PointerButton::Primary)
                    && let Some(p) = pointer {
                        actions.push(CanvasAction::MoveLaser(self.snap(p)));
                    }
            }
            Tool::Corner => {
                if response.clicked_by(egui::PointerButton::Primary)
                    && let Some(h) = pointer.and_then(|p| self.corner_at(doc, p))
                {
                    actions.push(CanvasAction::RoundCorner(h.shape, h.corner.vertex));
                }
            }
            Tool::Trim => {
                if response.clicked_by(egui::PointerButton::Primary)
                    && let Some(p) = pointer
                {
                    actions.push(CanvasAction::Trim(p, 6.0 / self.view.zoom as f64));
                }
            }
            Tool::Dimension => {
                let primary = egui::PointerButton::Primary;
                if response.drag_started_by(primary)
                    && let Some(press) = ui.input(|i| i.pointer.press_origin())
                {
                    self.gesture = Some(Gesture::Dimension { start: self.snap(self.view.to_doc(origin, press)) });
                }
                if response.drag_stopped_by(primary)
                    && let (Some(Gesture::Dimension { start }), Some(end)) = (self.gesture.take(), pointer)
                {
                    let end = self.snap(end);
                    if start.distance(end) > 1e-6 {
                        actions.push(CanvasAction::Dimension(start, end));
                    }
                }
            }
            Tool::Knife => {
                let primary = egui::PointerButton::Primary;
                if response.drag_started_by(primary)
                    && let Some(press) = ui.input(|i| i.pointer.press_origin())
                {
                    self.gesture = Some(Gesture::Knife { start: self.snap(self.view.to_doc(origin, press)) });
                }
                if response.drag_stopped_by(primary)
                    && let (Some(Gesture::Knife { start }), Some(end)) = (self.gesture.take(), pointer)
                {
                    actions.push(CanvasAction::Knife(start, self.snap(end)));
                }
            }
            Tool::Eraser if self.eraser_by_colour => {
                if response.clicked_by(egui::PointerButton::Primary)
                    && let Some(p) = pointer
                {
                    actions.push(CanvasAction::EraseColour(p));
                }
            }
            Tool::Eraser => {
                let primary = egui::PointerButton::Primary;
                if response.drag_started_by(primary)
                    && let Some(press) = ui.input(|i| i.pointer.press_origin())
                {
                    self.gesture = Some(Gesture::Erase { points: vec![self.view.to_doc(origin, press)] });
                }
                if let (Some(Gesture::Erase { points }), Some(p)) = (&mut self.gesture, pointer)
                    && points.last().is_none_or(|l| l.distance(p) > self.eraser_mm / 4.0)
                {
                    points.push(p);
                }
                if response.drag_stopped_by(primary)
                    && let Some(Gesture::Erase { points }) = self.gesture.take()
                {
                    actions.push(CanvasAction::Erase(points, self.eraser_mm / 2.0));
                }
                if response.clicked_by(primary)
                    && let Some(p) = pointer
                {
                    actions.push(CanvasAction::Erase(vec![p], self.eraser_mm / 2.0));
                }
            }
            Tool::ShapeBuilder => {
                let primary = egui::PointerButton::Primary;
                let delete = ui.input(|i| i.modifiers.alt);
                if response.drag_started_by(primary)
                    && let Some(press) = ui.input(|i| i.pointer.press_origin())
                {
                    self.gesture = Some(Gesture::Build { points: vec![self.view.to_doc(origin, press)] });
                }
                if let (Some(Gesture::Build { points }), Some(p)) = (&mut self.gesture, pointer)
                    && points.last().is_none_or(|l| l.distance(p) > 0.5 / self.view.zoom as f64)
                {
                    points.push(p);
                }
                if response.drag_stopped_by(primary)
                    && let Some(Gesture::Build { points }) = self.gesture.take()
                {
                    actions.push(CanvasAction::ShapeBuild { points, delete });
                }
                if response.clicked_by(primary)
                    && let Some(p) = pointer
                {
                    actions.push(CanvasAction::ShapeBuild { points: vec![p], delete });
                }
            }
        } }

        // --- Pointer feedback: show what clicking or dragging here would do.
        let hover_screen = response.hover_pos();
        if (response.hovered() || self.gesture.is_some())
            && let Some(icon) = self.cursor_icon(&response, doc, selection, hover_screen)
        {
            ui.ctx().set_cursor_icon(icon);
        }
        let rotating_here = matches!(self.gesture, Some(Gesture::Rotate { .. }))
            || (self.tool == Tool::Select
                && self.gesture.is_none()
                && hover_screen.is_some_and(|p| matches!(self.select_handle_at(origin, doc, selection, p), Some(SelectHandle::Rotate { .. }))));

        // --- Drawing.
        painter.rect_filled(rect, 0.0, self.board().outside);
        self.draw_bed(&painter, origin, doc);
        if let Some(o) = self.machine_origin {
            let c = self.view.to_screen(origin, o);
            painter.rect_filled(egui::Rect::from_center_size(c, egui::vec2(8.0, 8.0)), 0.0, Color32::from_rgb(220, 30, 30));
        }
        if let Some(ov) = overlay {
            crate::camera::draw_overlay(&painter, ov, doc.bed_size, |p| self.view.to_screen(origin, p));
        }
        self.textures.retain(|id, _| doc.shape(*id).is_some());
        let filled = |s: &Shape| {
            !matches!(s.kind, ShapeKind::Image { .. }) && matches!(doc.layers[s.layer].mode, bc_doc::LayerMode::Fill | bc_doc::LayerMode::FillThenLine)
        };
        self.fills.retain(|id, _| doc.shape(*id).is_some_and(filled));
        for shape in &doc.shapes {
            if !doc.layers[shape.layer].visible {
                continue;
            }
            // Layers that won't be sent to the laser are drawn faded.
            let opacity = if doc.layers[shape.layer].output { 1.0 } else { DISABLED_OPACITY };
            if matches!(shape.kind, ShapeKind::Image { .. }) {
                self.draw_image(ui.ctx(), &painter, origin, shape, &doc.layers[shape.layer], opacity);
            }
            // Slow live effects are worked out off this thread; the plain outline shows meanwhile.
            let path = shape.world_path_preview();
            // Fill: solid in the layer colour. Fill + Line: lighter, so the cut line round it stands out.
            if filled(shape) {
                let shade = if doc.layers[shape.layer].mode == bc_doc::LayerMode::Fill { 1.0 } else { 0.45 };
                self.draw_fill(ui.ctx(), &painter, origin, shape.id, &path, self.layer_color(doc, shape.layer).gamma_multiply(shade * opacity));
            }
            let width = if selection.contains(&shape.id) { 2.5 } else { 1.2 };
            let stroke = Stroke::new(width, self.layer_color(doc, shape.layer).gamma_multiply(opacity));
            for line in bc_geom::flatten(&path, 0.25 / self.view.zoom as f64) {
                let pts: Vec<Pos2> = line.points.iter().map(|p| self.view.to_screen(origin, *p)).collect();
                if line.closed {
                    painter.add(egui::Shape::closed_line(pts, stroke));
                } else {
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }
        }
        if bc_doc::effects::busy() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
        }
        if self.tool == Tool::ShapeBuilder {
            let hover = response.hover_pos().map(|p| self.view.to_doc(origin, p));
            self.draw_builder(&painter, origin, ui.input(|i| i.modifiers.alt), hover);
        }
        if let Some(lines) = &self.highlight {
            // Yellow on a dark edge, readable over any picture and either design-area colour.
            let (edge, ink) = (Stroke::new(4.0, Color32::from_black_alpha(160)), Stroke::new(2.0, Color32::from_rgb(255, 214, 0)));
            for line in bc_geom::flatten(lines, 0.25 / self.view.zoom as f64) {
                let pts: Vec<Pos2> = line.points.iter().map(|p| self.view.to_screen(origin, *p)).collect();
                for s in [edge, ink] {
                    painter.add(if line.closed { egui::Shape::closed_line(pts.clone(), s) } else { egui::Shape::line(pts.clone(), s) });
                }
            }
        }
        self.draw_guides(&painter, rect, doc, response.hover_pos().or_else(|| ui.ctx().pointer_latest_pos()));
        let accent = ui.visuals().selection.stroke.color;
        if let Some(b) = doc.bounds_of(selection.iter().copied()) {
            let r = self.screen_rect(origin, b);
            let locked = doc.shapes.iter().any(|s| selection.contains(&s.id) && s.locked);
            let frame = if locked { Stroke::new(1.0, Color32::from_rgb(200, 120, 0)) } else { Stroke::new(1.0, accent) };
            painter.rect_stroke(r.expand(3.0), 0.0, frame, StrokeKind::Outside);
            if self.tool == Tool::Select && !locked {
                if self.allow_size {
                    for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
                        painter.rect(egui::Rect::from_center_size(c, egui::vec2(2.0 * HANDLE_PX, 2.0 * HANDLE_PX)), 0.0, Color32::WHITE, Stroke::new(1.0, accent), StrokeKind::Middle);
                    }
                }
                if self.allow_size || self.allow_shear {
                    // Side handles: smaller squares between the corners.
                    for (c, _) in Self::side_handles(r) {
                        painter.rect(egui::Rect::from_center_size(c, egui::vec2(1.6 * HANDLE_PX, 1.6 * HANDLE_PX)), 0.0, Color32::WHITE, Stroke::new(1.0, accent), StrokeKind::Middle);
                    }
                }
                if self.allow_shear {
                    // Shear handles: small squares set in from the side handles.
                    for (c, _) in Self::shear_handles(r) {
                        painter.rect(egui::Rect::from_center_size(c, egui::vec2(1.2 * HANDLE_PX, 1.2 * HANDLE_PX)), 0.0, Color32::WHITE, Stroke::new(1.0, accent), StrokeKind::Middle);
                    }
                }
                if self.allow_move
                    && let Some(c) = Self::move_handle(r)
                {
                    painter.rect(egui::Rect::from_center_size(c, egui::vec2(1.6 * HANDLE_PX, 1.6 * HANDLE_PX)), 0.0, Color32::WHITE, Stroke::new(1.0, accent), StrokeKind::Middle);
                }
                if self.allow_rotate {
                    let rot = Self::rotate_handle(r);
                    painter.line_segment([egui::pos2(rot.x, r.top() - 3.0), rot], Stroke::new(1.0, accent));
                    painter.circle(rot, HANDLE_PX, Color32::WHITE, Stroke::new(1.0, accent));
                }
            }
        }
        if self.tool == Tool::Nodes
            && let Some(shape) = Self::node_target(doc, selection) {
                self.draw_nodes(&painter, origin, shape, accent);
            }
        self.draw_gesture_preview(&painter, origin, hover_doc, accent, ui.input(|i| i.modifiers.shift));
        // Trim tool: show the outline a click would trim. Knife: the cut line being dragged.
        if self.tool == Tool::Trim
            && let Some(id) = hover_doc.and_then(|p| bc_doc::edit::outline_near(doc, p, 6.0 / self.view.zoom as f64))
            && let Some(s) = doc.shape(id)
        {
            for line in bc_geom::flatten(&s.world_path(), 0.25 / self.view.zoom as f64) {
                let pts: Vec<Pos2> = line.points.iter().map(|p| self.view.to_screen(origin, *p)).collect();
                let stroke = Stroke::new(2.5, Color32::from_rgb(230, 60, 60));
                painter.add(if line.closed { egui::Shape::closed_line(pts, stroke) } else { egui::Shape::line(pts, stroke) });
            }
        }
        if let (Some(Gesture::Dimension { start }), Some(p)) = (&self.gesture, hover_doc) {
            let end = self.snap(p);
            let ends = [self.view.to_screen(origin, *start), self.view.to_screen(origin, end)];
            painter.line_segment(ends, Stroke::new(1.5, accent));
            let label = self.units.format(start.distance(end));
            let galley = painter.layout_no_wrap(label, egui::FontId::proportional(12.0), Color32::WHITE);
            let pos = ends[0].lerp(ends[1], 0.5) + egui::vec2(8.0, -20.0);
            painter.rect_filled(egui::Rect::from_min_size(pos, galley.size()).expand(3.0), 3.0, accent);
            painter.galley(pos, galley, Color32::WHITE);
        }
        if self.show_constraints {
            self.draw_constraints(&painter, origin, doc);
        }
        if self.tool == Tool::Eraser {
            let radius = (self.eraser_mm / 2.0) as f32 * self.view.zoom;
            let brush = Color32::from_rgba_unmultiplied(230, 60, 60, 90);
            if let Some(Gesture::Erase { points }) = &self.gesture {
                let pts: Vec<Pos2> = points.iter().map(|p| self.view.to_screen(origin, *p)).collect();
                painter.add(egui::Shape::line(pts.clone(), Stroke::new(radius * 2.0, brush)));
                for p in pts {
                    painter.circle_filled(p, radius, brush);
                }
            }
            if let Some(h) = hover_doc {
                painter.circle_stroke(self.view.to_screen(origin, h), radius, Stroke::new(1.0, Color32::from_rgb(230, 60, 60)));
            }
        }
        if let (Some(Gesture::Knife { start }), Some(p)) = (&self.gesture, hover_doc.or_else(|| ui.ctx().pointer_latest_pos().map(|q| self.view.to_doc(origin, q)))) {
            let ends = [self.view.to_screen(origin, *start), self.view.to_screen(origin, self.snap(p))];
            painter.add(egui::Shape::dashed_line(&ends, Stroke::new(2.0, Color32::from_rgb(230, 60, 60)), 8.0, 4.0));
        }
        // Corner tool: mark every corner that can be rounded (hollow) or already is (filled), and
        // ring the one under the pointer with its current radius.
        if self.tool == Tool::Corner {
            // With a selection, only its corners; otherwise every shape's (capped for huge drawings).
            let only = (!selection.is_empty()).then_some(selection);
            let marker_bg = self.board().bed;
            for h in bc_doc::edit::corner_handles(doc, only).iter().take(3000) {
                let at = self.view.to_screen(origin, h.corner.handle);
                let (fill, stroke) = if h.corner.radius.is_some() { (accent, Stroke::new(1.0, marker_bg)) } else { (marker_bg, Stroke::new(1.5, accent)) };
                painter.circle(at, 3.5, fill, stroke);
            }
            if let Some(h) = hover_doc.and_then(|p| self.corner_at(doc, p)) {
                let at = self.view.to_screen(origin, h.corner.handle);
                painter.circle(at, 8.0, accent.gamma_multiply(0.25), Stroke::new(2.0, accent));
                let text = match h.corner.radius {
                    Some(r) => format!("R {}", self.units.format(r)),
                    None => "sharp".to_string(),
                };
                let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
                let pos = at + egui::vec2(12.0, -24.0);
                painter.rect_filled(egui::Rect::from_min_size(pos, galley.size()).expand(3.0), 3.0, accent);
                painter.galley(pos, galley, Color32::WHITE);
            }
        }
        // There's no standard rotate cursor, so label the grab hand with a rotate arrow.
        if rotating_here && let Some(p) = response.hover_pos().or_else(|| ui.ctx().pointer_latest_pos()) {
            let at = p + egui::vec2(14.0, 14.0);
            painter.circle_filled(at, 10.0, ui.visuals().extreme_bg_color);
            painter.text(at, egui::Align2::CENTER_CENTER, "⟳", egui::FontId::proportional(16.0), accent);
        }
        if self.snap_active()
            && let Some(q) = hover_doc.and_then(|p| self.object_snap(p)) {
                painter.circle_stroke(self.view.to_screen(origin, q), 6.0, Stroke::new(1.5, Color32::from_rgb(220, 0, 200)));
            }
        if let Some(h) = head {
            let c = self.view.to_screen(origin, h);
            let red = Stroke::new(1.5, Color32::from_rgb(230, 30, 30));
            painter.circle_stroke(c, 6.0, red);
            painter.line_segment([c - egui::vec2(10.0, 0.0), c + egui::vec2(10.0, 0.0)], red);
            painter.line_segment([c - egui::vec2(0.0, 10.0), c + egui::vec2(0.0, 10.0)], red);
        }
        if self.show_rulers {
            self.draw_rulers(&painter, rect, doc.bed_size.1, hover_doc, ui.visuals());
        }

        (actions, hover_doc, response)
    }

    fn screen_rect(&self, origin: Pos2, b: Rect) -> egui::Rect {
        egui::Rect::from_two_pos(self.view.to_screen(origin, b.origin()), self.view.to_screen(origin, Point::new(b.x1, b.y1)))
    }

    /// Side (middle-of-edge) handles of the selection box on screen, with whether each is on the
    /// top/bottom. Hidden on small selections, where they would cover the middle and block moving.
    fn side_handles(r: egui::Rect) -> Vec<(Pos2, bool)> {
        const MIN_PX: f32 = 30.0;
        let mut out = Vec::new();
        if r.height() >= MIN_PX {
            out.extend([(r.center_top(), true), (r.center_bottom(), true)]);
        }
        if r.width() >= MIN_PX {
            out.extend([(r.left_center(), false), (r.right_center(), false)]);
        }
        out
    }

    /// Where the shear handles sit: on the top edge three quarters across, and on the right edge a
    /// quarter of the way down. Hidden, like the side handles, when the selection is small.
    fn shear_handles(r: egui::Rect) -> Vec<(Pos2, bool)> {
        const MIN_PX: f32 = 30.0;
        let mut out = Vec::new();
        if r.width() >= MIN_PX {
            out.push((egui::pos2(r.left() + 0.75 * r.width(), r.top()), true));
        }
        if r.height() >= MIN_PX {
            out.push((egui::pos2(r.right(), r.top() + 0.25 * r.height()), false));
        }
        out
    }

    /// The centre (move) handle, shown when the selection is big enough to have room for it.
    fn move_handle(r: egui::Rect) -> Option<Pos2> {
        (r.width() >= 30.0 && r.height() >= 30.0).then(|| r.center())
    }

    fn rotate_handle(r: egui::Rect) -> Pos2 {
        egui::pos2(r.center().x, r.top() - ROTATE_HANDLE_GAP)
    }

    /// The selection handle at `screen`, if any: rotate first, then shear (edge) handles, then corners.
    fn select_handle_at(&self, origin: Pos2, doc: &Document, selection: &BTreeSet<u64>, screen: Pos2) -> Option<SelectHandle> {
        if doc.shapes.iter().any(|s| selection.contains(&s.id) && s.locked) {
            return None;
        }
        let b = doc.bounds_of(selection.iter().copied())?;
        let r = self.screen_rect(origin, b);
        let near = |c: Pos2| c.distance(screen) <= HANDLE_PX + 3.0;
        if self.allow_rotate && near(Self::rotate_handle(r)) {
            return Some(SelectHandle::Rotate { center: b.center() });
        }
        if self.allow_shear {
            for (c, horizontal) in Self::shear_handles(r) {
                if c.distance(screen) <= HANDLE_PX + 1.0 {
                    let (anchor, span) = if horizontal { (b.y1, b.height()) } else { (b.x0, -b.width()) };
                    return Some(SelectHandle::Shear { horizontal, anchor, span });
                }
            }
        }
        if self.allow_move && Self::move_handle(r).is_some_and(|c| c.distance(screen) <= HANDLE_PX + 3.0) {
            return Some(SelectHandle::Move);
        }
        if self.allow_size || self.allow_shear {
            let edges = [
                (r.center_top(), true, b.y1, b.height()),
                (r.center_bottom(), true, b.y0, -b.height()),
                (r.left_center(), false, b.x1, b.width()),
                (r.right_center(), false, b.x0, -b.width()),
            ];
            let shown: Vec<Pos2> = Self::side_handles(r).into_iter().map(|(c, _)| c).collect();
            if let Some((_, horizontal, anchor, span)) = edges.into_iter().filter(|e| shown.contains(&e.0)).find(|e| near(e.0)) {
                return Some(SelectHandle::Edge { horizontal, anchor, span, bounds: b });
            }
        }
        if self.allow_size {
            // Screen corners with their opposite (anchor) corner in document space; the first and
            // last run top-left ↔ bottom-right on screen.
            let corners = [(r.left_top(), Point::new(b.x1, b.y1), true), (r.right_top(), Point::new(b.x0, b.y1), false), (r.left_bottom(), Point::new(b.x1, b.y0), false), (r.right_bottom(), Point::new(b.x0, b.y0), true)];
            if let Some((_, anchor, nw_se)) = corners.into_iter().find(|c| near(c.0)) {
                return Some(SelectHandle::Corner { anchor, bounds: b, nw_se });
            }
        }
        None
    }

    /// The ruler guide within a few pixels of `screen`, if any.
    fn guide_at(&self, origin: Pos2, screen: Pos2) -> Option<usize> {
        self.guides.iter().position(|g| {
            let at = self.view.to_screen(origin, Point::new(g.pos, g.pos));
            (if g.vertical { at.x - screen.x } else { at.y - screen.y }).abs() <= 4.0
        })
    }

    /// Starts, updates and finishes dragging a ruler guide.
    fn guide_gesture(&mut self, ui: &egui::Ui, response: &egui::Response, doc: &Document, selection: &BTreeSet<u64>, actions: &mut Vec<CanvasAction>) {
        let primary = egui::PointerButton::Primary;
        let (rect, origin) = (response.rect, response.rect.min);
        let in_ruler = |p: Pos2, vertical: bool| self.show_rulers && if vertical { p.x < rect.left() + RULER_PX } else { p.y < rect.top() + RULER_PX };
        if response.drag_started_by(primary)
            && self.gesture.is_none()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            let (top, left) = (in_ruler(press, false), in_ruler(press, true));
            if top || left {
                self.gesture = Some(Gesture::Guide { index: None, vertical: left && !top });
            } else if self.tool == Tool::Select {
                // An existing guide, unless a shape or selection handle is there to drag instead.
                let press_doc = self.view.to_doc(origin, press);
                let busy = doc.hit_test(press_doc, 4.0 / self.view.zoom as f64).is_some() || self.select_handle_at(origin, doc, selection, press).is_some();
                if !busy && let Some(i) = self.guide_at(origin, press) {
                    self.gesture = Some(Gesture::Guide { index: Some(i), vertical: doc.guides[i].vertical });
                }
            }
        }
        let Some(Gesture::Guide { index, vertical }) = self.gesture else { return };
        if !response.drag_stopped_by(primary) {
            return;
        }
        self.gesture = None;
        let p = ui.input(|i| i.pointer.latest_pos());
        let mut guides = doc.guides.clone();
        let dropped_off = p.is_none_or(|p| !rect.contains(p) || in_ruler(p, vertical));
        let pos = p.map(|p| self.snap(self.view.to_doc(origin, p))).map(|d| if vertical { d.x } else { d.y });
        let label = match (index, dropped_off, pos) {
            (Some(i), true, _) if i < guides.len() => {
                guides.remove(i);
                "Remove guide"
            }
            (Some(i), false, Some(v)) if i < guides.len() => {
                guides[i].pos = v;
                "Move guide"
            }
            (None, false, Some(v)) => {
                guides.push(bc_doc::GuideLine { vertical, pos: v });
                "Add guide"
            }
            _ => return,
        };
        actions.push(CanvasAction::Checkpoint(label));
        actions.push(CanvasAction::SetGuides(guides));
    }

    /// Ruler guides (dashed), the one being dragged, and smart-guide alignment lines.
    fn draw_guides(&self, painter: &egui::Painter, rect: egui::Rect, doc: &Document, hover: Option<Pos2>) {
        let origin = rect.min;
        let full = |vertical: bool, pos: f64| -> [Pos2; 2] {
            let at = self.view.to_screen(origin, Point::new(pos, pos));
            if vertical { [egui::pos2(at.x, rect.top()), egui::pos2(at.x, rect.bottom())] } else { [egui::pos2(rect.left(), at.y), egui::pos2(rect.right(), at.y)] }
        };
        let guide = Stroke::new(1.0, Color32::from_rgb(0, 160, 255));
        let moving = match self.gesture {
            Some(Gesture::Guide { index, .. }) => index,
            _ => None,
        };
        for (_, g) in doc.guides.iter().enumerate().filter(|(i, _)| Some(*i) != moving) {
            painter.add(egui::Shape::dashed_line(&full(g.vertical, g.pos), guide, 6.0, 4.0));
        }
        if let (Some(Gesture::Guide { vertical, .. }), Some(p)) = (&self.gesture, hover) {
            let vertical = *vertical;
            let d = self.snap(self.view.to_doc(origin, p));
            painter.line_segment(full(vertical, if vertical { d.x } else { d.y }), Stroke::new(1.5, Color32::from_rgb(0, 160, 255)));
        }
        let magenta = Stroke::new(1.0, Color32::from_rgb(230, 0, 190));
        for &(vertical, pos) in &self.alignments {
            painter.line_segment(full(vertical, pos), magenta);
        }
    }
    /// Corner tool: the corner whose marker is within a few pixels of `p` (mm).
    fn corner_at(&self, doc: &Document, p: Point) -> Option<bc_doc::edit::CornerHandle> {
        bc_doc::edit::corner_near(doc, p, 10.0 / self.view.zoom as f64)
    }

    /// Whether dragging a side handle shears (Alt held, or resizing is turned off) rather than stretches.
    fn edge_shears(&self, alt: bool) -> bool {
        self.allow_shear && (alt || !self.allow_size)
    }

    /// The pointer for the current tool, gesture and what's under the cursor.
    fn cursor_icon(&self, response: &egui::Response, doc: &Document, selection: &BTreeSet<u64>, hover: Option<Pos2>) -> Option<egui::CursorIcon> {
        use egui::CursorIcon as C;
        if response.dragged_by(egui::PointerButton::Middle) || response.dragged_by(egui::PointerButton::Secondary) {
            return Some(C::Grabbing);
        }
        // While dragging, keep the cursor of the gesture in progress.
        match &self.gesture {
            Some(Gesture::Move { .. }) => return Some(C::Move),
            Some(Gesture::Rotate { .. }) => return Some(C::Grabbing),
            Some(Gesture::Scale { axis: ScaleAxis::X, .. }) => return Some(C::ResizeHorizontal),
            Some(Gesture::Scale { axis: ScaleAxis::Y, .. }) => return Some(C::ResizeVertical),
            Some(Gesture::Scale { anchor, start, .. }) => {
                // The dragged corner is opposite the anchor; work out its diagonal on screen.
                let far = Point::new(if anchor.x == start.x0 { start.x1 } else { start.x0 }, if anchor.y == start.y0 { start.y1 } else { start.y0 });
                let origin = response.rect.min;
                let (a, f) = (self.view.to_screen(origin, *anchor), self.view.to_screen(origin, far));
                return Some(if (f.x - a.x) * (f.y - a.y) > 0.0 { C::ResizeNwSe } else { C::ResizeNeSw });
            }
            Some(Gesture::Shear { horizontal, .. }) => return Some(if *horizontal { C::ResizeHorizontal } else { C::ResizeVertical }),
            Some(Gesture::Node { .. }) => return Some(C::Move),
            Some(Gesture::Marquee { .. } | Gesture::Create { .. } | Gesture::Measure { .. } | Gesture::Knife { .. } | Gesture::Dimension { .. } | Gesture::Erase { .. } | Gesture::Build { .. }) => return Some(C::Crosshair),
            Some(Gesture::Guide { vertical, .. }) => return Some(if *vertical { C::ResizeHorizontal } else { C::ResizeVertical }),
            None => {}
        }
        let screen = hover?;
        let origin = response.rect.min;
        let doc_point = self.view.to_doc(origin, screen);
        match self.tool {
            Tool::Select => Some(match self.select_handle_at(origin, doc, selection, screen) {
                Some(SelectHandle::Rotate { .. }) => C::Grab,
                Some(SelectHandle::Edge { horizontal, .. }) => {
                    // Stretch across the edge; with Alt, the edge slides along itself (shear).
                    let slide = self.edge_shears(response.ctx.input(|i| i.modifiers.alt));
                    if horizontal != slide { C::ResizeVertical } else { C::ResizeHorizontal }
                }
                Some(SelectHandle::Corner { nw_se, .. }) => {
                    if nw_se {
                        C::ResizeNwSe
                    } else {
                        C::ResizeNeSw
                    }
                }
                Some(SelectHandle::Shear { horizontal, .. }) => {
                    if horizontal {
                        C::ResizeHorizontal
                    } else {
                        C::ResizeVertical
                    }
                }
                Some(SelectHandle::Move) => C::Move,
                None if self.allow_move && doc.hit_test(doc_point, 4.0 / self.view.zoom as f64).is_some_and(|id| doc.shape(id).is_some_and(|s| !s.locked)) => C::Move,
                None => match self.guide_at(origin, screen) {
                    Some(i) if self.guides[i].vertical => C::ResizeHorizontal,
                    Some(_) => C::ResizeVertical,
                    None => C::Default,
                },
            }),
            Tool::Nodes => {
                let on_handle = Self::node_target(doc, selection).is_some_and(|shape| {
                    nodes::handles(&shape.world_path()).iter().any(|(_, p)| self.view.to_screen(origin, *p).distance(screen) <= HANDLE_PX + 3.0)
                });
                Some(if on_handle { C::Move } else { C::Default })
            }
            Tool::Text => Some(C::Text),
            Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Star | Tool::Line | Tool::Measure | Tool::PositionLaser => Some(C::Crosshair),
            Tool::Corner => Some(if self.corner_at(doc, doc_point).is_some() { C::PointingHand } else { C::Crosshair }),
            Tool::Trim => Some(if bc_doc::edit::outline_near(doc, doc_point, 6.0 / self.view.zoom as f64).is_some() { C::PointingHand } else { C::Crosshair }),
            Tool::Knife | Tool::Dimension => Some(C::Crosshair),
            Tool::Eraser | Tool::ShapeBuilder => Some(C::Crosshair),
        }
    }

    fn select_tool(&mut self, response: &egui::Response, doc: &Document, selection: &BTreeSet<u64>, pointer: Option<Point>, actions: &mut Vec<CanvasAction>) {
        let primary = egui::PointerButton::Primary;
        let hit_tol = 4.0 / self.view.zoom as f64;
        let (shift, ctrl) = response.ctx.input(|i| (i.modifiers.shift, i.modifiers.command));
        let origin = response.rect.min;

        if response.double_clicked()
            && let Some(id) = pointer.and_then(|p| doc.hit_test(p, hit_tol))
                && matches!(doc.shape(id).map(|s| &s.kind), Some(ShapeKind::Text { .. } | ShapeKind::Barcode { .. })) {
                    actions.push(CanvasAction::EditText(id));
                    return;
                }
        if response.clicked_by(primary)
            && let Some(p) = pointer {
                let mut next = if shift { selection.clone() } else { BTreeSet::new() };
                if let Some(id) = doc.hit_test(p, hit_tol) {
                    if shift && selection.contains(&id) {
                        next.remove(&id);
                    } else {
                        next.insert(id);
                    }
                }
                actions.push(CanvasAction::SetSelection(next));
            }
        if response.drag_started_by(primary)
            && let Some(press_screen) = response.ctx.input(|i| i.pointer.press_origin()) {
                let press = self.view.to_doc(origin, press_screen);
                // Handles first (rotate, shear, corners), then shapes, then marquee.
                let handle = self.select_handle_at(origin, doc, selection, press_screen);
                if let Some(SelectHandle::Rotate { center }) = handle {
                    actions.push(CanvasAction::Checkpoint("Rotate"));
                    self.gesture = Some(Gesture::Rotate { center, start_angle: (press - center).atan2(), applied: 0.0 });
                } else if let Some(SelectHandle::Edge { horizontal, anchor, span, bounds }) = handle {
                    if self.edge_shears(response.ctx.input(|i| i.modifiers.alt)) {
                        actions.push(CanvasAction::Checkpoint("Shear"));
                        self.gesture = Some(Gesture::Shear { horizontal, anchor, span, start: press, applied: Affine::IDENTITY });
                    } else {
                        // Stretch from the opposite edge, in one direction only.
                        actions.push(CanvasAction::Checkpoint("Resize"));
                        let c = bounds.center();
                        let (anchor, axis) = if horizontal { (Point::new(c.x, anchor), ScaleAxis::Y) } else { (Point::new(anchor, c.y), ScaleAxis::X) };
                        self.gesture = Some(Gesture::Scale { anchor, start: bounds, applied: Affine::IDENTITY, axis });
                    }
                } else if let Some(SelectHandle::Corner { anchor, bounds, .. }) = handle {
                    actions.push(CanvasAction::Checkpoint("Resize"));
                    self.gesture = Some(Gesture::Scale { anchor, start: bounds, applied: Affine::IDENTITY, axis: ScaleAxis::Both });
                } else if let Some(SelectHandle::Shear { horizontal, anchor, span }) = handle {
                    actions.push(CanvasAction::Checkpoint("Shear"));
                    self.gesture = Some(Gesture::Shear { horizontal, anchor, span, start: press, applied: Affine::IDENTITY });
                } else if let Some(SelectHandle::Move) = handle {
                    actions.push(CanvasAction::Checkpoint("Move"));
                    let bounds = doc.bounds_of(selection.iter().copied()).unwrap_or(Rect::from_points(press, press));
                    let targets = Self::guide_targets(doc, selection);
                    self.gesture = Some(Gesture::Move { start: self.snap(press), bounds, applied: Vec2::ZERO, targets });
                } else {
                    match doc.hit_test(press, hit_tol) {
                        Some(id) => {
                            if !selection.contains(&id) {
                                actions.push(CanvasAction::SetSelection(BTreeSet::from([id])));
                            }
                            if self.allow_move {
                                actions.push(CanvasAction::Checkpoint("Move"));
                                let moving: BTreeSet<u64> = if selection.contains(&id) { selection.clone() } else { BTreeSet::from([id]) };
                                let bounds = doc.bounds_of(moving.iter().copied()).unwrap_or(Rect::from_points(press, press));
                                let targets = Self::guide_targets(doc, &moving);
                                self.gesture = Some(Gesture::Move { start: self.snap(press), bounds, applied: Vec2::ZERO, targets });
                            }
                        }
                        None => self.gesture = Some(Gesture::Marquee { start: press }),
                    }
                }
            }
        if response.dragged_by(primary) {
            let snapped = pointer.map(|p| self.snap(p));
            match (&mut self.gesture, snapped, pointer) {
                (Some(Gesture::Move { start, bounds, applied, targets }), Some(snapped), _) => {
                    let mut offset = snapped - *start;
                    // Smart guides: line up the selection's edges or centre with nearby targets.
                    if self.smart_guides {
                        let tol = (SNAP_PX / self.view.zoom) as f64;
                        let moved = *bounds + offset;
                        let align = |values: [f64; 3], targets: &[f64]| -> Option<(f64, f64)> {
                            values.iter().flat_map(|v| targets.iter().map(move |t| (t - v, *t))).filter(|(d, _)| d.abs() <= tol).min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
                        };
                        if let Some((d, line)) = align([moved.x0, moved.center().x, moved.x1], &targets.0) {
                            offset.x += d;
                            self.alignments.push((true, line));
                        }
                        if let Some((d, line)) = align([moved.y0, moved.center().y, moved.y1], &targets.1) {
                            offset.y += d;
                            self.alignments.push((false, line));
                        }
                    }
                    let delta = offset - *applied;
                    if delta != Vec2::ZERO {
                        actions.push(CanvasAction::Transform(Affine::translate(delta)));
                        *applied = offset;
                    }
                }
                (Some(Gesture::Scale { anchor, start, applied, axis }), _, Some(p)) => {
                    // Scale factors from the anchor: corners are proportional unless Ctrl is held;
                    // side handles change one direction only.
                    let far = Point::new(if anchor.x == start.x0 { start.x1 } else { start.x0 }, if anchor.y == start.y0 { start.y1 } else { start.y0 });
                    let (mut sx, mut sy) = match axis {
                        ScaleAxis::X => ((p.x - anchor.x) / (far.x - anchor.x), 1.0),
                        ScaleAxis::Y => (1.0, (p.y - anchor.y) / (far.y - anchor.y)),
                        ScaleAxis::Both => ((p.x - anchor.x) / (far.x - anchor.x), (p.y - anchor.y) / (far.y - anchor.y)),
                    };
                    if *axis == ScaleAxis::Both && !ctrl {
                        let s = if sx.abs() > sy.abs() { sx } else { sy };
                        (sx, sy) = (s, s);
                    }
                    if !sx.is_finite() || !sy.is_finite() {
                        return; // zero-width or zero-height selection
                    }
                    let (sx, sy) = (if sx.abs() < 0.01 { 0.01 } else { sx }, if sy.abs() < 0.01 { 0.01 } else { sy });
                    let total = Affine::translate(anchor.to_vec2()) * Affine::scale_non_uniform(sx, sy) * Affine::translate(-anchor.to_vec2());
                    actions.push(CanvasAction::Transform(total * applied.inverse()));
                    *applied = total;
                }
                (Some(Gesture::Shear { horizontal, anchor, span, start, applied }), _, Some(p)) => {
                    // The dragged edge moves by the pointer's offset; the opposite edge (`anchor`) stays put.
                    let total = if *horizontal {
                        let s = -(p.x - start.x) / *span;
                        Affine::new([1.0, 0.0, s, 1.0, -s * *anchor, 0.0])
                    } else {
                        let s = -(p.y - start.y) / *span;
                        Affine::new([1.0, s, 0.0, 1.0, 0.0, -s * *anchor])
                    };
                    actions.push(CanvasAction::Transform(total * applied.inverse()));
                    *applied = total;
                }
                (Some(Gesture::Rotate { center, start_angle, applied }), _, Some(p)) => {
                    let mut angle = (p - *center).atan2() - *start_angle;
                    if shift {
                        let step = 15f64.to_radians();
                        angle = (angle / step).round() * step;
                    }
                    let delta = angle - *applied;
                    if delta.abs() > 1e-9 {
                        actions.push(CanvasAction::Transform(Affine::translate(center.to_vec2()) * Affine::rotate(delta) * Affine::translate(-center.to_vec2())));
                        *applied = angle;
                        actions.push(CanvasAction::Status(format!("Rotation {:.1}°", angle.to_degrees())));
                    }
                }
                _ => {}
            }
        }
        if response.drag_stopped_by(primary) {
            if let (Some(Gesture::Marquee { start }), Some(end)) = (self.gesture.take(), pointer) {
                let mut next = if shift { selection.clone() } else { BTreeSet::new() };
                next.extend(marquee_pick(doc, start, end));
                actions.push(CanvasAction::SetSelection(next));
            }
            self.gesture = None;
        }
    }

    fn create_tool(&mut self, response: &egui::Response, pointer: Option<Point>, actions: &mut Vec<CanvasAction>) {
        let primary = egui::PointerButton::Primary;
        if response.drag_started_by(primary) {
            let press = response.ctx.input(|i| i.pointer.press_origin()).map(|o| self.view.to_doc(response.rect.min, o));
            if let Some(p) = press.or(pointer) {
                self.gesture = Some(Gesture::Create { start: self.snap(p) });
            }
        }
        if response.drag_stopped_by(primary)
            && let (Some(Gesture::Create { start }), Some(end)) = (self.gesture.take(), pointer) {
                let shift = response.ctx.input(|i| i.modifiers.shift);
                if let Some((kind, transform)) = self.make_shape(start, self.snap(end), shift) {
                    actions.push(CanvasAction::Checkpoint("Draw shape"));
                    actions.push(CanvasAction::AddShape { kind, transform });
                }
            }
    }

    fn line_tool(&mut self, response: &egui::Response, ui: &egui::Ui, pointer: Option<Point>, actions: &mut Vec<CanvasAction>) {
        if response.clicked_by(egui::PointerButton::Primary)
            && let Some(p) = pointer {
                let p = self.snap(p);
                // Clicking the first point again closes the shape.
                let close_tol = 6.0 / self.view.zoom as f64;
                if self.line_points.len() > 2 && p.distance(self.line_points[0]) <= close_tol {
                    self.finish_line(true, actions);
                } else {
                    self.line_points.push(p);
                }
            }
        let finish = response.double_clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
        if finish {
            // A double-click also registers a click; drop the duplicate point it added.
            if response.double_clicked() && self.line_points.len() > 1 {
                self.line_points.pop();
            }
            self.finish_line(false, actions);
        }
    }

    fn finish_line(&mut self, closed: bool, actions: &mut Vec<CanvasAction>) {
        let points = std::mem::take(&mut self.line_points);
        if points.len() >= 2 {
            actions.push(CanvasAction::Checkpoint("Draw line"));
            actions.push(CanvasAction::AddShape { kind: ShapeKind::Path(bc_geom::polyline_path(&points, closed)), transform: Affine::IDENTITY });
        }
    }

    fn measure_text(&self, a: Point, b: Point) -> String {
        let d = b - a;
        let dy = if self.y_up { -d.y } else { d.y };
        format!("{} (ΔX {}, ΔY {}) at {:.1}°", self.units.format(d.hypot()), self.units.format(d.x), self.units.format(dy), (-d.y).atan2(d.x).to_degrees())
    }

    fn measure_tool(&mut self, response: &egui::Response, pointer: Option<Point>, actions: &mut Vec<CanvasAction>) {
        let primary = egui::PointerButton::Primary;
        if response.drag_started_by(primary) {
            let press = response.ctx.input(|i| i.pointer.press_origin()).map(|o| self.view.to_doc(response.rect.min, o));
            if let Some(p) = press {
                self.gesture = Some(Gesture::Measure { start: self.snap(p) });
                self.measured = None;
            }
        }
        if response.dragged_by(primary)
            && let (Some(Gesture::Measure { start }), Some(p)) = (&self.gesture, pointer) {
                actions.push(CanvasAction::Status(self.measure_text(*start, self.snap(p))));
            }
        if response.drag_stopped_by(primary)
            && let (Some(Gesture::Measure { start }), Some(p)) = (self.gesture.take(), pointer) {
                self.measured = Some((start, self.snap(p)));
            }
    }

    // --- Node tool.

    /// The single selected shape the node tool works on (not images).
    fn node_target<'a>(doc: &'a Document, selection: &BTreeSet<u64>) -> Option<&'a Shape> {
        if selection.len() != 1 {
            return None;
        }
        doc.shape(*selection.iter().next()?).filter(|s| !matches!(s.kind, ShapeKind::Image { .. }) && !s.locked)
    }

    fn node_tool(&mut self, ui: &egui::Ui, response: &egui::Response, doc: &Document, selection: &BTreeSet<u64>, pointer: Option<Point>, actions: &mut Vec<CanvasAction>) {
        let primary = egui::PointerButton::Primary;
        let origin = response.rect.min;
        let target = Self::node_target(doc, selection);
        if self.selected_node.is_some_and(|(id, _)| Some(id) != target.map(|s| s.id)) {
            self.selected_node = None;
        }
        let handle_at = |screen: Pos2| -> Option<(u64, Handle)> {
            let shape = target?;
            let path = shape.world_path();
            nodes::handles(&path).into_iter().filter(|(_, p)| self.view.to_screen(origin, *p).distance(screen) <= HANDLE_PX + 3.0).map(|(h, _)| (shape.id, h)).min_by_key(|(_, h)| h.kind != HandleKind::Node)
        };
        let hit_tol = 4.0 / self.view.zoom as f64;

        if response.double_clicked() {
            if let (Some(shape), Some(p)) = (target, pointer)
                && let Some(path) = nodes::insert_node(&shape.world_path(), p, hit_tol * 2.0) {
                    actions.push(CanvasAction::Checkpoint("Add point"));
                    actions.push(CanvasAction::SetPath(shape.id, path));
                }
            return;
        }
        if response.clicked_by(primary) {
            let press = response.interact_pointer_pos();
            match press.and_then(handle_at) {
                Some((id, h)) if h.kind == HandleKind::Node => self.selected_node = Some((id, h.element)),
                Some(_) => {}
                None => {
                    self.selected_node = None;
                    let hit = pointer.and_then(|p| doc.hit_test(p, hit_tol));
                    actions.push(CanvasAction::SetSelection(hit.into_iter().collect()));
                }
            }
        }
        if response.drag_started_by(primary)
            && let Some(press) = response.ctx.input(|i| i.pointer.press_origin())
                && let Some((id, h)) = handle_at(press) {
                    actions.push(CanvasAction::Checkpoint("Move point"));
                    if h.kind == HandleKind::Node {
                        self.selected_node = Some((id, h.element));
                    }
                    self.gesture = Some(Gesture::Node { shape: id, handle: h, last: self.view.to_doc(origin, press) });
                }
        if response.dragged_by(primary) {
            let snapped = pointer.map(|p| self.snap(p));
            if let (Some(Gesture::Node { shape, handle, last }), Some(p)) = (&mut self.gesture, snapped) {
                let delta = p - *last;
                if delta != Vec2::ZERO {
                    if let Some(s) = doc.shape(*shape) {
                        actions.push(CanvasAction::SetPath(*shape, nodes::move_handle(&s.world_path(), *handle, delta)));
                    }
                    *last = p;
                }
            }
        }
        if response.drag_stopped_by(primary) {
            self.gesture = None;
        }
        let delete = ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) && !ui.ctx().egui_wants_keyboard_input();
        if delete
            && let Some((id, element)) = self.selected_node.take()
                && let Some(path) = doc.shape(id).and_then(|s| nodes::delete_node(&s.world_path(), element)) {
                    actions.push(CanvasAction::Checkpoint("Delete point"));
                    actions.push(CanvasAction::SetPath(id, path));
                }
    }

    /// Draws nodes (squares), curve handles (circles) and handle lines for the node tool.
    fn draw_nodes(&self, painter: &egui::Painter, origin: Pos2, shape: &Shape, accent: Color32) {
        let path = shape.world_path();
        let to = |p: Point| self.view.to_screen(origin, p);
        let faint = Stroke::new(1.0, accent.gamma_multiply(0.6));
        let mut last = Point::ORIGIN;
        for el in path.elements() {
            match *el {
                PathEl::QuadTo(c, p) => {
                    painter.line_segment([to(last), to(c)], faint);
                    painter.line_segment([to(c), to(p)], faint);
                }
                PathEl::CurveTo(c1, c2, p) => {
                    painter.line_segment([to(last), to(c1)], faint);
                    painter.line_segment([to(c2), to(p)], faint);
                }
                _ => {}
            }
            if let Some(p) = el.end_point() {
                last = p;
            }
        }
        for (h, p) in nodes::handles(&path) {
            let c = to(p);
            if h.kind == HandleKind::Node {
                let selected = self.selected_node == Some((shape.id, h.element));
                let fill = if selected { accent } else { Color32::WHITE };
                painter.rect(egui::Rect::from_center_size(c, egui::vec2(8.0, 8.0)), 0.0, fill, Stroke::new(1.0, accent), StrokeKind::Middle);
            } else {
                painter.circle(c, 3.5, Color32::WHITE, Stroke::new(1.0, accent));
            }
        }
    }

    // --- Drawing helpers.

    /// Draws an image shape as a textured quad (so rotation and mirroring display correctly).
    /// Draws an image: as the photo (adjusted), or with burn preview on, as the laser will burn it
    /// on its layer (dithered or in grayscale levels at the layer's line interval).
    fn draw_image(&mut self, ctx: &egui::Context, painter: &egui::Painter, origin: Pos2, shape: &Shape, layer: &bc_doc::Layer, opacity: f32) {
        let ShapeKind::Image { data, width, height, adjust, clip } = &shape.kind else { return };
        let data_ptr = data.as_ptr() as usize;
        let preview = self.burn_preview.then_some((layer.dither, layer.interval.to_bits()));
        let stale = self.textures.get(&shape.id).is_none_or(|t| t.data_ptr != data_ptr || t.adjust != *adjust || t.clip != *clip || t.preview != preview);
        if stale {
            let gray = if preview.is_some() {
                // At most ~4 million dots; finer than that isn't visible on screen anyway.
                match bc_job::burn_preview(shape, layer, 4_000_000) {
                    Ok(g) => g,
                    Err(_) => return,
                }
            } else {
                let Some(bytes) = shape.image_bytes() else { return };
                let Ok(mut gray) = bc_raster::Gray::decode(&bytes) else { return };
                let longest = gray.width.max(gray.height);
                if longest > 1024 {
                    let s = 1024.0 / longest as f64;
                    gray = gray.resize(((gray.width as f64 * s) as usize).max(1), ((gray.height as f64 * s) as usize).max(1));
                }
                gray.adjust(&bc_job::adjustments(adjust));
                gray
            };
            let pixels: Vec<u8> = gray.data.iter().map(|v| (v * 255.0).round() as u8).collect();
            // See-through parts of the picture (erased, or a removed background) show the bed
            // through them, as does everything outside a clip.
            let mut alpha = shape.image_bytes().and_then(|b| bc_raster::opacity(&b, gray.width, gray.height));
            if let Some(clip) = clip {
                let area = Rect::new(-width / 2.0, -height / 2.0, width / 2.0, height / 2.0);
                let inside = bc_geom::scanline_mask(clip, area, gray.width, gray.height);
                let a = alpha.get_or_insert_with(|| vec![255; pixels.len()]);
                for (v, keep) in a.iter_mut().zip(inside) {
                    if !keep {
                        *v = 0;
                    }
                }
            }
            let image = match alpha {
                Some(a) => {
                    let rgba: Vec<u8> = pixels.iter().zip(&a).flat_map(|(&v, &o)| [v, v, v, o]).collect();
                    egui::ColorImage::from_rgba_unmultiplied([gray.width, gray.height], &rgba)
                }
                None => egui::ColorImage::from_gray([gray.width, gray.height], &pixels),
            };
            // Burn previews keep their dots crisp when zoomed in.
            let filter = if preview.is_some() { egui::TextureOptions::NEAREST } else { egui::TextureOptions::LINEAR };
            let texture = ctx.load_texture(format!("image-{}", shape.id), image, filter);
            self.textures.insert(shape.id, ImageTexture { data_ptr, adjust: *adjust, clip: clip.clone(), preview, texture });
        }
        let tex = &self.textures[&shape.id].texture;
        let (hw, hh) = (width / 2.0, height / 2.0);
        let corners = [(-hw, -hh, 0.0, 0.0), (hw, -hh, 1.0, 0.0), (hw, hh, 1.0, 1.0), (-hw, hh, 0.0, 1.0)];
        let mut mesh = egui::Mesh::with_texture(tex.id());
        for (x, y, u, v) in corners {
            let pos = self.view.to_screen(origin, shape.transform * Point::new(x, y));
            mesh.vertices.push(egui::epaint::Vertex { pos, uv: egui::pos2(u, v), color: Color32::WHITE.gamma_multiply(opacity) });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }

    /// Shades the inside of a shape on a fill layer (even-odd, so holes such as the middle of an
    /// "o" stay clear, as the laser fills it).
    fn draw_fill(&mut self, ctx: &egui::Context, painter: &egui::Painter, origin: Pos2, id: u64, path: &BezPath, color: Color32) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for p in path.elements().iter().flat_map(|el| match *el {
            PathEl::MoveTo(a) | PathEl::LineTo(a) => vec![a],
            PathEl::QuadTo(a, b) => vec![a, b],
            PathEl::CurveTo(a, b, c) => vec![a, b, c],
            PathEl::ClosePath => vec![Point::new(f64::NAN, 0.0)],
        }) {
            (p.x.to_bits(), p.y.to_bits()).hash(&mut h);
        }
        let key = h.finish();
        if self.fills.get(&id).is_none_or(|f| f.key != key) {
            let Some(bounds) = bc_geom::bounds(path).filter(|b| b.width() > 0.0 && b.height() > 0.0) else {
                self.fills.remove(&id);
                return;
            };
            // 0.1 mm cells, at most 1024 across: the outline drawn on top keeps the edge sharp.
            let cell = (bounds.width().max(bounds.height()) / 1024.0).max(0.1);
            let (cols, rows) = ((bounds.width() / cell).ceil() as usize, (bounds.height() / cell).ceil() as usize);
            // Only closed outlines are filled; open lines are engraved as strokes.
            let mut closed = BezPath::new();
            for sub in path.elements().split_inclusive(|el| matches!(el, PathEl::ClosePath)).flat_map(|run| {
                // A run is one closed subpath, possibly after open ones; split those off at each MoveTo.
                let starts: Vec<usize> = run.iter().enumerate().filter(|(_, el)| matches!(el, PathEl::MoveTo(_))).map(|(i, _)| i).collect();
                starts.iter().enumerate().map(|(k, &s)| &run[s..starts.get(k + 1).copied().unwrap_or(run.len())]).collect::<Vec<_>>()
            }) {
                let ends_closed = match (sub.first(), sub.last()) {
                    (_, Some(PathEl::ClosePath)) => true,
                    (Some(PathEl::MoveTo(a)), Some(PathEl::LineTo(b) | PathEl::QuadTo(_, b) | PathEl::CurveTo(_, _, b))) => a.distance(*b) < 1e-6,
                    _ => false,
                };
                if ends_closed {
                    sub.iter().for_each(|el| closed.push(*el));
                }
            }
            let mask = bc_geom::scanline_mask(&closed, bounds, cols, rows);
            let rgba: Vec<u8> = mask.iter().flat_map(|&inside| if inside { [255; 4] } else { [0; 4] }).collect();
            let image = egui::ColorImage::from_rgba_unmultiplied([cols, rows], &rgba);
            let texture = ctx.load_texture(format!("fill-{id}"), image, egui::TextureOptions::LINEAR);
            self.fills.insert(id, FillTexture { key, bounds, texture });
        }
        let f = &self.fills[&id];
        let screen = egui::Rect::from_two_pos(self.view.to_screen(origin, Point::new(f.bounds.x0, f.bounds.y0)), self.view.to_screen(origin, Point::new(f.bounds.x1, f.bounds.y1)));
        painter.image(f.texture.id(), screen, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), color);
    }

    /// The Shape Builder's areas: each outlined, and those under the pointer (or its drag) drawn
    /// thick, green to merge or red to delete.
    fn draw_builder(&self, painter: &egui::Painter, origin: Pos2, delete: bool, pointer: Option<Point>) {
        let mut picked: Vec<usize> = Vec::new();
        let points: Vec<Point> = match &self.gesture {
            Some(Gesture::Build { points }) => points.clone(),
            _ => pointer.into_iter().collect(),
        };
        for p in points {
            if let Some(i) = self.builder_regions.iter().position(|r| bc_geom::contains(r, p))
                && !picked.contains(&i)
            {
                picked.push(i);
            }
        }
        let tint = if delete { Color32::from_rgb(220, 50, 50) } else { Color32::from_rgb(40, 170, 80) };
        let tol = 0.25 / self.view.zoom as f64;
        for (i, r) in self.builder_regions.iter().enumerate() {
            let on = picked.contains(&i);
            let stroke = if on { Stroke::new(2.5, tint) } else { Stroke::new(1.0, Color32::from_rgb(90, 140, 230)) };
            for line in bc_geom::flatten(r, tol) {
                let pts: Vec<Pos2> = line.points.iter().map(|p| self.view.to_screen(origin, *p)).collect();
                painter.add(if line.closed { egui::Shape::closed_line(pts, stroke) } else { egui::Shape::line(pts, stroke) });
            }
        }
    }

    fn board(&self) -> BoardColors {
        if self.dark_board {
            BoardColors { outside: Color32::from_gray(18), bed: Color32::from_gray(34), grid: Color32::from_gray(50), border: Color32::from_gray(150) }
        } else {
            BoardColors { outside: Color32::from_gray(214), bed: Color32::from_gray(250), grid: Color32::from_gray(228), border: Color32::from_gray(120) }
        }
    }

    /// A layer's drawing colour. On a dark board, very dark colours are lightened so they show:
    /// black becomes light grey, and dark colours (navy, dark red, …) keep their hue.
    fn layer_color(&self, doc: &Document, layer: usize) -> Color32 {
        let [r, g, b] = doc.layers[layer].rgb(layer);
        let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        if !self.dark_board || luma >= 70.0 {
            return Color32::from_rgb(r, g, b);
        }
        if r.max(g).max(b) - r.min(g).min(b) < 30 {
            return Color32::from_gray(225);
        }
        // Mix with white until it's about as light as the mid-tone layer colours.
        let t = (130.0 - luma) / (255.0 - luma);
        let lift = |c: u8| (c as f32 + (255.0 - c as f32) * t).round() as u8;
        Color32::from_rgb(lift(r), lift(g), lift(b))
    }

    fn draw_bed(&self, painter: &egui::Painter, origin: Pos2, doc: &Document) {
        let colors = self.board();
        let (w, h) = doc.bed_size;
        let bed = egui::Rect::from_two_pos(self.view.to_screen(origin, Point::ORIGIN), self.view.to_screen(origin, Point::new(w, h)));
        painter.rect_filled(bed, 0.0, colors.bed);
        // Grid: every `grid_mm`, thinned out when lines would be closer than 6 px.
        let mut step = self.grid_mm.max(0.1);
        while step * (self.view.zoom as f64) < 6.0 {
            step *= 2.0;
        }
        let grid = Stroke::new(1.0, colors.grid);
        let mut x = 0.0;
        while x <= w {
            painter.line_segment([self.view.to_screen(origin, Point::new(x, 0.0)), self.view.to_screen(origin, Point::new(x, h))], grid);
            x += step;
        }
        let mut y = 0.0;
        while y <= h {
            painter.line_segment([self.view.to_screen(origin, Point::new(0.0, y)), self.view.to_screen(origin, Point::new(w, y))], grid);
            y += step;
        }
        painter.rect_stroke(bed, 0.0, Stroke::new(1.5, colors.border), StrokeKind::Outside);
    }

    /// Rulers on all four edges, in the display units (Y counted up from the bed bottom when `y_up`),
    /// with a cursor marker.
    fn draw_rulers(&self, painter: &egui::Painter, rect: egui::Rect, bed_height: f64, hover: Option<Point>, visuals: &egui::Visuals) {
        let origin = rect.min;
        let bg = visuals.panel_fill;
        let ink = visuals.text_color();
        let font = egui::FontId::monospace(9.0);
        let unit = self.units.mm_per_unit();
        // Major tick spacing: a 1-2-5 step in display units giving at least ~60 px.
        let px_per_unit = self.view.zoom as f64 * unit;
        let mut major = 1e-3;
        'find: for exp in -3..6 {
            for m in [1.0, 2.0, 5.0] {
                let s = m * 10f64.powi(exp);
                if s * px_per_unit >= 60.0 {
                    major = s;
                    break 'find;
                }
            }
        }
        let minor = major / 5.0;
        let label = |v: f64| {
            let s = format!("{:.3}", v);
            let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
            if s == "-0" { "0".to_string() } else { s }
        };
        let tick = Stroke::new(1.0, ink.gamma_multiply(0.6));
        let strips = [
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + RULER_PX)),
            egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - RULER_PX), rect.max),
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + RULER_PX, rect.max.y)),
            egui::Rect::from_min_max(egui::pos2(rect.max.x - RULER_PX, rect.min.y), rect.max),
        ];
        for s in strips {
            painter.rect_filled(s, 0.0, bg);
        }
        // Displayed value <-> document coordinate.
        let disp_y = |y_mm: f64| if self.y_up { bed_height - y_mm } else { y_mm };
        // Horizontal rulers (top and bottom).
        let (x0, x1) = (self.view.to_doc(origin, rect.left_top()).x / unit, self.view.to_doc(origin, rect.right_top()).x / unit);
        for (i, strip) in strips[..2].iter().enumerate() {
            let mut v = (x0 / minor).floor() * minor;
            while v <= x1 {
                let sx = self.view.to_screen(origin, Point::new(v * unit, 0.0)).x;
                let is_major = ((v / major).round() * major - v).abs() < minor * 0.01;
                let len = if is_major { RULER_PX } else { RULER_PX * 0.35 };
                let (a, b) = if i == 0 { (strip.bottom() - len, strip.bottom()) } else { (strip.top(), strip.top() + len) };
                painter.line_segment([egui::pos2(sx, a), egui::pos2(sx, b)], tick);
                if is_major && sx > rect.left() + RULER_PX && sx < rect.right() - RULER_PX {
                    let y = if i == 0 { strip.top() + 1.0 } else { strip.bottom() - 10.0 };
                    painter.text(egui::pos2(sx + 2.0, y), egui::Align2::LEFT_TOP, label(v), font.clone(), ink);
                }
                v += minor;
            }
        }
        // Vertical rulers (left and right): ticks at displayed values.
        let ya = disp_y(self.view.to_doc(origin, rect.left_top()).y) / unit;
        let yb = disp_y(self.view.to_doc(origin, rect.left_bottom()).y) / unit;
        let (lo, hi) = (ya.min(yb), ya.max(yb));
        for (i, strip) in strips[2..].iter().enumerate() {
            let mut v = (lo / minor).floor() * minor;
            while v <= hi {
                let sy = self.view.to_screen(origin, Point::new(0.0, disp_y(v * unit))).y;
                let is_major = ((v / major).round() * major - v).abs() < minor * 0.01;
                let len = if is_major { RULER_PX } else { RULER_PX * 0.35 };
                let (a, b) = if i == 0 { (strip.right() - len, strip.right()) } else { (strip.left(), strip.left() + len) };
                painter.line_segment([egui::pos2(a, sy), egui::pos2(b, sy)], tick);
                if is_major && sy > rect.top() + RULER_PX && sy < rect.bottom() - RULER_PX {
                    painter.text(egui::pos2(strip.left() + 1.0, sy + 2.0), egui::Align2::LEFT_TOP, label(v), font.clone(), ink);
                }
                v += minor;
            }
        }
        if let Some(p) = hover {
            let s = self.view.to_screen(origin, p);
            let mark = Stroke::new(1.0, Color32::from_rgb(220, 0, 200));
            for strip in &strips[..2] {
                painter.line_segment([egui::pos2(s.x, strip.top()), egui::pos2(s.x, strip.bottom())], mark);
            }
            for strip in &strips[2..] {
                painter.line_segment([egui::pos2(strip.left(), s.y), egui::pos2(strip.right(), s.y)], mark);
            }
        }
        for corner in [rect.left_top(), rect.right_top() - egui::vec2(RULER_PX, 0.0), rect.left_bottom() - egui::vec2(0.0, RULER_PX), rect.right_bottom() - egui::vec2(RULER_PX, RULER_PX)] {
            painter.rect_filled(egui::Rect::from_min_size(corner, egui::vec2(RULER_PX, RULER_PX)), 0.0, bg);
        }
        painter.text(rect.min + egui::vec2(3.0, 4.0), egui::Align2::LEFT_TOP, self.units.suffix().trim(), font, ink);
    }

    fn draw_gesture_preview(&self, painter: &egui::Painter, origin: Pos2, hover: Option<Point>, color: Color32, shift: bool) {
        let stroke = Stroke::new(1.0, color);
        let to = |p: Point| self.view.to_screen(origin, p);
        match (&self.gesture, hover) {
            (Some(Gesture::Marquee { start }), Some(h)) => {
                let r = egui::Rect::from_two_pos(to(*start), to(h));
                if h.x < start.x {
                    // Dragging leftwards selects whatever the box touches: dashed, green.
                    let green = Color32::from_rgb(60, 190, 90);
                    painter.rect_filled(r, 0.0, green.gamma_multiply(0.08));
                    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
                    painter.extend(egui::Shape::dashed_line(&corners, Stroke::new(1.0, green), 6.0, 4.0));
                } else {
                    // Rightwards selects only what's fully inside: solid.
                    painter.rect(r, 0.0, color.gamma_multiply(0.08), stroke, StrokeKind::Middle);
                }
            }
            (Some(Gesture::Create { start }), Some(h)) => {
                if let Some((kind, t)) = self.make_shape(*start, self.snap(h), shift) {
                    let shape = Shape { id: 0, kind, transform: t, layer: 0, locked: false, group: None, effects: Vec::new() };
                    let path = shape.world_path();
                    let ghost = Stroke::new(1.0, color.gamma_multiply(0.45));
                    for (copy, stroke) in std::iter::once((Affine::IDENTITY, stroke)).chain(self.symmetry.copies().into_iter().map(|c| (c, ghost))) {
                        for line in bc_geom::flatten(&(copy * path.clone()), 0.25 / self.view.zoom as f64) {
                            let pts = line.points.iter().map(|p| to(*p)).collect();
                            painter.add(egui::Shape::closed_line(pts, stroke));
                        }
                    }
                }
            }
            (Some(Gesture::Measure { start }), Some(h)) => self.draw_measure(painter, origin, *start, self.snap(h)),
            _ => {}
        }
        if let (None, Some((a, b))) = (&self.gesture, self.measured) {
            self.draw_measure(painter, origin, a, b);
        }
        if !self.line_points.is_empty() {
            let mut doc_pts = self.line_points.clone();
            if let Some(h) = hover {
                doc_pts.push(self.snap(h));
            }
            painter.add(egui::Shape::line(doc_pts.iter().map(|p| to(*p)).collect(), stroke));
            let ghost = Stroke::new(1.0, color.gamma_multiply(0.45));
            for copy in self.symmetry.copies() {
                painter.add(egui::Shape::line(doc_pts.iter().map(|p| to(copy * *p)).collect(), ghost));
            }
        }
        // Symmetry axes, while a drawing tool is active.
        if self.symmetry.mode != SymmetryMode::Off && matches!(self.tool, Tool::Line | Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Star) {
            let c = self.symmetry.center;
            let reach = 2000.0;
            let axis = Stroke::new(1.0, Color32::from_rgb(0, 160, 200).gamma_multiply(0.7));
            let mut angles: Vec<f64> = match self.symmetry.mode {
                SymmetryMode::LeftRight => vec![std::f64::consts::FRAC_PI_2],
                SymmetryMode::TopBottom => vec![0.0],
                SymmetryMode::FourWay => vec![0.0, std::f64::consts::FRAC_PI_2],
                _ => (0..self.symmetry.count.clamp(2, 64)).map(|k| std::f64::consts::TAU * k as f64 / self.symmetry.count.clamp(2, 64) as f64).collect(),
            };
            if self.symmetry.mode == SymmetryMode::Radial {
                // Slice edges from the centre outward only.
                for a in angles.drain(..) {
                    let end = c + Vec2::new(a.cos(), a.sin()) * reach;
                    painter.add(egui::Shape::dashed_line(&[to(c), to(end)], axis, 6.0, 4.0));
                }
            }
            for a in angles {
                let d = Vec2::new(a.cos(), a.sin()) * reach;
                painter.add(egui::Shape::dashed_line(&[to(c - d), to(c + d)], axis, 6.0, 4.0));
            }
            painter.circle_stroke(to(c), 4.0, axis);
        }
    }

    /// Locked sizes and distances, as blue dimension lines with their values.
    fn draw_constraints(&self, painter: &egui::Painter, origin: Pos2, doc: &Document) {
        use bc_doc::constraints::{unit, Axis, ConstraintKind};
        let ink = Color32::from_rgb(40, 120, 230);
        let stroke = Stroke::new(1.2, ink);
        let to = |p: Point| self.view.to_screen(origin, p);
        let gap = 14.0 / self.view.zoom as f64;
        let bounds = |id: u64| doc.bounds_of(unit(doc, id));
        let label = |at: Pos2, text: String| {
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(11.0), ink);
            let r = egui::Rect::from_center_size(at, galley.size() + egui::vec2(6.0, 2.0));
            painter.rect_filled(r, 3.0, Color32::from_white_alpha(225));
            painter.galley(r.min + egui::vec2(3.0, 1.0), galley, ink);
        };
        // A dimension from a to b with short end ticks.
        let dim = |a: Point, b: Point, text: String, enabled: bool| {
            let s = if enabled { stroke } else { Stroke::new(1.0, ink.gamma_multiply(0.4)) };
            let (sa, sb) = (to(a), to(b));
            painter.line_segment([sa, sb], s);
            let d = (sb - sa).normalized();
            let n = egui::vec2(-d.y, d.x) * 4.0;
            painter.line_segment([sa - n, sa + n], s);
            painter.line_segment([sb - n, sb + n], s);
            label(sa + (sb - sa) / 2.0, text);
        };
        for c in &doc.params.constraints {
            let value = c.measure(doc).map_or_else(|| "?".into(), |v| format!("{v:.2}"));
            let text = if c.value.trim().parse::<f64>().is_ok() { value } else { format!("{} = {value}", c.value.trim()) };
            match c.kind {
                ConstraintKind::Size { shape, axis, .. } => {
                    let Some(r) = bounds(shape) else { continue };
                    match axis {
                        Axis::X => dim(Point::new(r.x0, r.y0 - gap), Point::new(r.x1, r.y0 - gap), text, c.enabled),
                        Axis::Y => dim(Point::new(r.x0 - gap, r.y0), Point::new(r.x0 - gap, r.y1), text, c.enabled),
                    }
                }
                ConstraintKind::Distance { axis, from, from_anchor, to: target, to_anchor } => {
                    let (Some(a), Some(b)) = (bounds(from), bounds(target)) else { continue };
                    let pick = |anchor: bc_doc::constraints::Anchor, lo: f64, hi: f64| match anchor {
                        bc_doc::constraints::Anchor::Start => lo,
                        bc_doc::constraints::Anchor::Middle => (lo + hi) / 2.0,
                        bc_doc::constraints::Anchor::End => hi,
                    };
                    match axis {
                        Axis::X => {
                            let (xa, xb) = (pick(from_anchor, a.x0, a.x1), pick(to_anchor, b.x0, b.x1));
                            let y = (a.center().y + b.center().y) / 2.0;
                            dim(Point::new(xa, y), Point::new(xb, y), text, c.enabled);
                        }
                        Axis::Y => {
                            let (ya, yb) = (pick(from_anchor, a.y0, a.y1), pick(to_anchor, b.y0, b.y1));
                            let x = (a.center().x + b.center().x) / 2.0;
                            dim(Point::new(x, ya), Point::new(x, yb), text, c.enabled);
                        }
                    }
                }
            }
        }
    }

    fn draw_measure(&self, painter: &egui::Painter, origin: Pos2, a: Point, b: Point) {
        let (sa, sb) = (self.view.to_screen(origin, a), self.view.to_screen(origin, b));
        let ink = Stroke::new(1.5, Color32::from_rgb(220, 0, 200));
        painter.line_segment([sa, sb], ink);
        painter.circle_filled(sa, 3.0, ink.color);
        painter.circle_filled(sb, 3.0, ink.color);
        let text = self.measure_text(a, b);
        let at = sa + (sb - sa) / 2.0 + egui::vec2(8.0, -8.0);
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::BLACK);
        let bg = egui::Rect::from_min_size(at - egui::vec2(3.0, galley.size().y + 1.0), galley.size() + egui::vec2(6.0, 2.0));
        painter.rect_filled(bg, 3.0, Color32::from_white_alpha(230));
        painter.galley(bg.min + egui::vec2(3.0, 1.0), galley, Color32::BLACK);
    }
}

/// The shapes a selection box from `start` to `end` picks. Dragged left to right it picks shapes
/// entirely inside the box; dragged right to left, any shape whose lines the box touches (and
/// pictures it overlaps). Hidden layers are skipped.
fn marquee_pick(doc: &Document, start: Point, end: Point) -> Vec<u64> {
    let area = Rect::from_points(start, end);
    let crossing = end.x < start.x;
    doc.shapes
        .iter()
        .filter(|s| doc.layers[s.layer].visible)
        .filter(|s| {
            let Some(b) = s.bounds() else { return false };
            if !crossing {
                return area.contains(b.origin()) && area.contains(Point::new(b.x1, b.y1));
            }
            let overlaps = b.x0 <= area.x1 && area.x0 <= b.x1 && b.y0 <= area.y1 && area.y0 <= b.y1;
            if !overlaps {
                return false;
            }
            matches!(s.kind, ShapeKind::Image { .. }) || bc_geom::outline_touches(&s.world_path(), area)
        })
        .map(|s| s.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drags the right-side handle of a 40 × 20 mm rectangle in a headless canvas; returns its new bounds.
    fn drag_right_handle(modifiers: egui::Modifiers) -> Rect {
        let ctx = egui::Context::default();
        let mut doc = Document::default();
        let id = doc.add_shape(ShapeKind::Rect { width: 40.0, height: 20.0, corner_radius: 0.0 }, Affine::translate((100.0, 100.0)), 0);
        let sel = BTreeSet::from([id]);
        let mut canvas = Canvas::default();
        let origin = std::cell::Cell::new(Pos2::ZERO);
        let frame = |canvas: &mut Canvas, doc: &Document, events: Vec<egui::Event>| -> Vec<CanvasAction> {
            let events = std::iter::once(egui::Event::ModifiersChanged(modifiers)).chain(events).collect();
            let input = egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 800.0))), ..Default::default() };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::no_frame().show(ui, |ui| {
                    let (a, _, response) = canvas.ui(ui, doc, &sel, None, None);
                    origin.set(response.rect.min);
                    actions = a;
                });
            });
            out.textures_delta.clear();
            actions
        };
        frame(&mut canvas, &doc, vec![]);
        frame(&mut canvas, &doc, vec![]);
        let start = canvas.screen_rect(origin.get(), doc.bounds_of([id]).unwrap()).right_center();
        let end = start + egui::vec2(60.0, 25.0); // drifting vertically must not change the height
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers };
        let mut actions = frame(&mut canvas, &doc, vec![egui::Event::PointerMoved(start), button(start, true)]);
        for i in 1..=6 {
            actions.extend(frame(&mut canvas, &doc, vec![egui::Event::PointerMoved(start + (end - start) * (i as f32 / 6.0))]));
        }
        actions.extend(frame(&mut canvas, &doc, vec![button(end, false)]));
        let total = actions.iter().fold(Affine::IDENTITY, |acc, a| if let CanvasAction::Transform(t) = a { *t * acc } else { acc });
        bc_geom::bounds(&(total * doc.shape(id).unwrap().world_path())).unwrap()
    }

    #[test]
    fn side_handle_stretches_one_way() {
        let b = drag_right_handle(egui::Modifiers::NONE);
        assert!(b.width() > 45.0, "wider: {b:?}");
        assert!((b.height() - 20.0).abs() < 1e-6, "same height: {b:?}");
        assert!((b.x0 - 80.0).abs() < 1e-6, "left edge stays put: {b:?}");
    }

    #[test]
    fn alt_on_a_side_handle_shears() {
        let b = drag_right_handle(egui::Modifiers::ALT);
        assert!((b.width() - 40.0).abs() < 1e-6, "same width: {b:?}");
        assert!(b.height() > 21.0, "slanted, so the bounding box grows taller: {b:?}");
    }
    #[test]
    fn corner_tool_click_rounds_that_corner() {
        let ctx = egui::Context::default();
        let mut doc = Document::default();
        let id = doc.add_shape(ShapeKind::Rect { width: 40.0, height: 20.0, corner_radius: 0.0 }, Affine::translate((100.0, 100.0)), 0);
        let sel = BTreeSet::new();
        let mut canvas = Canvas::default();
        canvas.set_tool(Tool::Corner);
        let origin = std::cell::Cell::new(Pos2::ZERO);
        let frame = |canvas: &mut Canvas, events: Vec<egui::Event>| -> Vec<CanvasAction> {
            let input = egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 800.0))), ..Default::default() };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::no_frame().show(ui, |ui| {
                    let (a, _, response) = canvas.ui(ui, &doc, &sel, None, None);
                    origin.set(response.rect.min);
                    actions = a;
                });
            });
            out.textures_delta.clear();
            actions
        };
        frame(&mut canvas, vec![]);
        frame(&mut canvas, vec![]);
        // Click a few pixels inside the bottom-right corner.
        let corner = Point::new(120.0, 110.0);
        let at = canvas.view.to_screen(origin.get(), corner) + egui::vec2(-3.0, -2.0);
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let mut actions = frame(&mut canvas, vec![egui::Event::PointerMoved(at), button(true)]);
        actions.extend(frame(&mut canvas, vec![button(false)]));
        let hits: Vec<(u64, Point)> = actions.iter().filter_map(|a| if let CanvasAction::RoundCorner(i, p) = a { Some((*i, *p)) } else { None }).collect();
        assert_eq!(hits.len(), 1, "{} actions", actions.len());
        assert_eq!(hits[0].0, id);
        assert!(hits[0].1.distance(corner) < 1e-6, "{:?}", hits[0].1);
    }
    /// Runs a headless canvas: two settling frames, then a primary-button drag from `from` to `to`
    /// (screen points computed from the canvas origin after settling). Returns all actions.
    fn run_drag(canvas: &mut Canvas, doc: &Document, sel: &BTreeSet<u64>, from: impl Fn(&Canvas, Pos2) -> Pos2, to: impl Fn(&Canvas, Pos2) -> Pos2) -> Vec<CanvasAction> {
        let ctx = egui::Context::default();
        let origin = std::cell::Cell::new(Pos2::ZERO);
        let frame = |canvas: &mut Canvas, events: Vec<egui::Event>| -> Vec<CanvasAction> {
            let input = egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 800.0))), ..Default::default() };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::no_frame().show(ui, |ui| {
                    let (a, _, response) = canvas.ui(ui, doc, sel, None, None);
                    origin.set(response.rect.min);
                    actions = a;
                });
            });
            out.textures_delta.clear();
            actions
        };
        frame(canvas, vec![]);
        frame(canvas, vec![]);
        let (a, b) = (from(canvas, origin.get()), to(canvas, origin.get()));
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let mut actions = frame(canvas, vec![egui::Event::PointerMoved(a), button(a, true)]);
        for i in 1..=6 {
            actions.extend(frame(canvas, vec![egui::Event::PointerMoved(a + (b - a) * (i as f32 / 6.0))]));
        }
        actions.extend(frame(canvas, vec![button(b, false)]));
        actions
    }

    #[test]
    fn smart_guides_line_shapes_up() {
        let mut doc = Document::default();
        let _anchor = doc.add_shape(ShapeKind::Rect { width: 40.0, height: 20.0, corner_radius: 0.0 }, Affine::translate((100.0, 60.0)), 0); // left edge x = 80
        let mover = doc.add_shape(ShapeKind::Rect { width: 30.0, height: 10.0, corner_radius: 0.0 }, Affine::translate((200.0, 150.0)), 0); // left edge x = 185
        let sel = BTreeSet::from([mover]);
        let mut canvas = Canvas::default();
        // Grab the mover by its top line (on a Line layer the empty middle doesn't pick it) and
        // drop it so its left edge lands 1 px right of x = 80.
        let actions = run_drag(
            &mut canvas,
            &doc,
            &sel,
            |c, o| c.view.to_screen(o, Point::new(200.0, 145.0)),
            |c, o| {
                let one_px = 1.0 / c.view.zoom as f64;
                c.view.to_screen(o, Point::new(200.0 - (185.0 - 80.0) + one_px, 145.0))
            },
        );
        let total = actions.iter().fold(Affine::IDENTITY, |acc, a| if let CanvasAction::Transform(t) = a { *t * acc } else { acc });
        let b = bc_geom::bounds(&(total * doc.shape(mover).unwrap().world_path())).unwrap();
        assert!((b.x0 - 80.0).abs() < 1e-9, "snapped exactly onto the other shape's left edge: {b:?}");
    }

    #[test]
    fn guides_come_out_of_the_ruler_and_go_back() {
        let mut doc = Document::default();
        let sel = BTreeSet::new();
        let mut canvas = Canvas::default();
        // From the top ruler down onto the bed: a horizontal guide.
        let actions = run_drag(&mut canvas, &doc, &sel, |_, o| o + egui::vec2(300.0, 5.0), |c, o| c.view.to_screen(o, Point::new(150.0, 123.0)));
        let guides = actions.iter().find_map(|a| if let CanvasAction::SetGuides(g) = a { Some(g.clone()) } else { None }).expect("a guide was added");
        assert_eq!(guides.len(), 1);
        assert!(!guides[0].vertical && (guides[0].pos - 123.0).abs() < 0.5, "{guides:?}");
        // Drag it back onto the ruler: removed.
        doc.guides = guides;
        let actions = run_drag(&mut canvas, &doc, &sel, |c, o| c.view.to_screen(o, Point::new(150.0, doc.guides[0].pos)), |_, o| o + egui::vec2(300.0, 5.0));
        let after = actions.iter().find_map(|a| if let CanvasAction::SetGuides(g) = a { Some(g.clone()) } else { None }).expect("guides changed");
        assert!(after.is_empty());
    }
    #[test]
    fn knife_drag_reports_the_cut_line() {
        let mut doc = Document::default();
        doc.add_shape(ShapeKind::Rect { width: 40.0, height: 40.0, corner_radius: 0.0 }, Affine::translate((100.0, 100.0)), 0);
        let mut canvas = Canvas::default();
        canvas.set_tool(Tool::Knife);
        let actions = run_drag(&mut canvas, &doc, &BTreeSet::new(), |c, o| c.view.to_screen(o, Point::new(100.0, 70.0)), |c, o| c.view.to_screen(o, Point::new(100.0, 130.0)));
        let cut = actions.iter().find_map(|a| if let CanvasAction::Knife(a, b) = a { Some((*a, *b)) } else { None }).expect("knife action");
        assert!((cut.0.x - 100.0).abs() < 0.5 && (cut.0.y - 70.0).abs() < 0.5 && (cut.1.y - 130.0).abs() < 0.5, "{cut:?}");
        let mut doc2 = doc.clone();
        assert_eq!(bc_doc::edit::knife(&mut doc2, cut.0, cut.1).len(), 2);
    }
    #[test]
    fn symmetry_copies() {
        let c = Point::new(100.0, 100.0);
        let p = Point::new(110.0, 95.0);
        let sym = |mode, count, mirror| Symmetry { mode, count, mirror, center: c };
        assert!(sym(SymmetryMode::Off, 8, true).copies().is_empty());
        let lr = sym(SymmetryMode::LeftRight, 8, false).copies();
        assert_eq!(lr.len(), 1);
        assert!((lr[0] * p - Point::new(90.0, 95.0)).hypot() < 1e-9);
        assert_eq!(sym(SymmetryMode::FourWay, 8, false).copies().len(), 3);
        // Radial: n - 1 turned copies, plus n mirrored ones; all the same distance from the centre.
        let radial = sym(SymmetryMode::Radial, 6, true).copies();
        assert_eq!(radial.len(), 5 + 6);
        assert!(radial.iter().all(|t| ((*t * p - c).hypot() - (p - c).hypot()).abs() < 1e-9));
        let quarter = sym(SymmetryMode::Radial, 4, false).copies();
        assert!((quarter[0] * p - Point::new(105.0, 110.0)).hypot() < 1e-9, "{:?}", quarter[0] * p);
    }

    #[test]
    fn selection_box_direction_decides_inside_or_touching() {
        let mut doc = Document::default();
        let rect = |doc: &mut Document, c: (f64, f64), w: f64, h: f64| doc.add_shape(ShapeKind::Rect { width: w, height: h, corner_radius: 0.0 }, Affine::translate(c), 0);
        let inside = rect(&mut doc, (50.0, 50.0), 10.0, 10.0); // 45..55
        let partly = rect(&mut doc, (70.0, 50.0), 20.0, 10.0); // 60..80, sticks out of the box
        rect(&mut doc, (55.0, 50.0), 200.0, 200.0); // encloses the box, its lines far away: never picked
        rect(&mut doc, (200.0, 50.0), 10.0, 10.0); // nowhere near: never picked
        let (a, b) = (Point::new(40.0, 40.0), Point::new(65.0, 60.0));
        // Left to right: only what's fully inside.
        assert_eq!(marquee_pick(&doc, a, b), vec![inside]);
        // Right to left: whatever the box's area touches (by lines), not shapes merely around it.
        let mut touched = marquee_pick(&doc, Point::new(b.x, a.y), Point::new(a.x, b.y));
        touched.sort();
        assert_eq!(touched, vec![inside, partly]);
    }

    #[test]
    fn shear_and_move_handles() {
        let mut doc = Document::default();
        let id = doc.add_shape(ShapeKind::Rect { width: 200.0, height: 100.0, corner_radius: 0.0 }, Affine::translate((300.0, 200.0)), 0);
        let sel = BTreeSet::from([id]);
        // The canvas fits the bed on the first frame, so find the handles from the live view.
        let screen_rect = |c: &Canvas, o: Pos2| c.screen_rect(o, Rect::new(200.0, 150.0, 400.0, 250.0));
        // The centre handle moves the selection.
        let mut canvas = Canvas::default();
        let actions = run_drag(&mut canvas, &doc, &sel, |c, o| screen_rect(c, o).center(), |c, o| screen_rect(c, o).center() + egui::vec2(40.0, 0.0));
        let moved: Vec<Affine> = actions.iter().filter_map(|a| if let CanvasAction::Transform(t) = a { Some(*t) } else { None }).collect();
        let total = moved.iter().fold(Affine::IDENTITY, |acc, t| *t * acc);
        assert!(total.as_coeffs()[4] > 1.0 && total.as_coeffs()[0] == 1.0, "a move to the right: {total:?}");
        // The top shear handle slides the top edge sideways and leaves the bottom edge alone.
        let mut canvas = Canvas::default();
        let top = |c: &Canvas, o: Pos2| Canvas::shear_handles(screen_rect(c, o))[0].0;
        let actions = run_drag(&mut canvas, &doc, &sel, top, |c, o| top(c, o) + egui::vec2(40.0, 0.0));
        assert!(actions.iter().any(|a| matches!(a, CanvasAction::Checkpoint("Shear"))));
        let total = actions.iter().filter_map(|a| if let CanvasAction::Transform(t) = a { Some(*t) } else { None }).fold(Affine::IDENTITY, |acc, t| t * acc);
        let (top_left, bottom_left) = (total * Point::new(200.0, 150.0), total * Point::new(200.0, 250.0));
        assert!(top_left.x > 201.0 && (top_left.y - 150.0).abs() < 1e-9, "top slides right: {top_left:?}");
        assert!((bottom_left - Point::new(200.0, 250.0)).hypot() < 1e-9, "bottom stays: {bottom_left:?}");
    }

    #[test]
    fn handles_under_the_pointer() {
        let mut doc = Document::default();
        let id = doc.add_shape(ShapeKind::Rect { width: 40.0, height: 20.0, corner_radius: 0.0 }, Affine::translate((50.0, 50.0)), 0);
        let sel = BTreeSet::from([id]);
        let canvas = Canvas::default();
        let o = Pos2::ZERO;
        let at = |x: f64, y: f64| canvas.view.to_screen(o, Point::new(x, y));
        let r = canvas.screen_rect(o, doc.bounds_of([id]).unwrap());
        let corner = |p: Pos2| match canvas.select_handle_at(o, &doc, &sel, p) {
            Some(SelectHandle::Corner { nw_se, .. }) => Some(nw_se),
            _ => None,
        };
        assert_eq!(corner(r.left_top()), Some(true));
        assert_eq!(corner(r.right_bottom()), Some(true));
        assert_eq!(corner(r.right_top()), Some(false));
        assert_eq!(corner(r.left_bottom()), Some(false));
        assert!(matches!(canvas.select_handle_at(o, &doc, &sel, r.center_top()), Some(SelectHandle::Edge { horizontal: true, .. })));
        assert!(matches!(canvas.select_handle_at(o, &doc, &sel, r.left_center()), Some(SelectHandle::Edge { horizontal: false, .. })));
        assert!(matches!(canvas.select_handle_at(o, &doc, &sel, Canvas::rotate_handle(r)), Some(SelectHandle::Rotate { .. })));
        assert_eq!(canvas.select_handle_at(o, &doc, &sel, at(50.0, 50.0)), Some(SelectHandle::Move), "the middle is the move handle");
        let shear = Canvas::shear_handles(r);
        assert!(matches!(canvas.select_handle_at(o, &doc, &sel, shear[0].0), Some(SelectHandle::Shear { horizontal: true, .. })));
        assert!(matches!(canvas.select_handle_at(o, &doc, &sel, shear[1].0), Some(SelectHandle::Shear { horizontal: false, .. })));
        // Locked shapes have no handles.
        doc.shapes[0].locked = true;
        assert_eq!(canvas.select_handle_at(o, &doc, &sel, r.left_top()), None);
    }
}