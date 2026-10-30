//! BurnCraft document model: shapes, layers (cut settings), undo history and
//! the native project file format (`.bcproj`, JSON).

pub mod constraints;
pub mod edit;
pub mod effects;
pub mod vars;

use bc_geom::{Affine, BezPath, Point, Rect};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Number of cut layers (sent to the laser) in the colour palette.
pub const CUT_LAYER_COUNT: usize = 255;

/// Cut layers in LightBurn files and in BurnCraft projects before format 2, where Tool1 and Tool2 came
/// straight after them (indices 30 and 31).
pub const LEGACY_CUT_LAYERS: usize = 30;

/// A layer index from a LightBurn file or an older project: cut layers keep their number, and the
/// old Tool1 / Tool2 (30, 31) move to where tool layers are now. None for anything past those.
pub fn from_legacy_index(index: usize) -> Option<usize> {
    match index {
        i if i < LEGACY_CUT_LAYERS => Some(i),
        i if i < LEGACY_CUT_LAYERS + 2 => Some(CUT_LAYER_COUNT + i - LEGACY_CUT_LAYERS),
        _ => None,
    }
}

/// Total layers: the cut layers plus two tool layers (Tool1, Tool2) for guides that are never cut.
pub const LAYER_COUNT: usize = CUT_LAYER_COUNT + 2;

/// Whether `index` is a tool (guide) layer.
pub fn is_tool_layer(index: usize) -> bool {
    index >= CUT_LAYER_COUNT
}

/// Short layer label: "01".."255", "Tool1", "Tool2".
/// Cut layers are shown numbered from 01 (layer index 0 is "01"); the index itself, used in files
/// and by the machine output, still starts at 0.
pub fn layer_label(index: usize) -> String {
    if is_tool_layer(index) { format!("Tool{}", index - CUT_LAYER_COUNT + 1) } else { format!("{:02}", index + 1) }
}

/// A layer's default name: its number ("01" … "255"), or "Tool1" / "Tool2".
pub fn default_layer_name(index: usize) -> String {
    layer_label(index)
}

/// Default names layers had before: "C00" for the first layer (also LightBurn's default), and for
/// a short while "C01". Such names are replaced by [`default_layer_name`] so the name and the
/// number shown agree.
pub fn is_old_default_name(index: usize, name: &str) -> bool {
    if is_tool_layer(index) {
        // Tool layers used to be called "T1" and "T2" (also their names in LightBurn projects).
        return name == format!("T{}", index - CUT_LAYER_COUNT + 1);
    }
    name == format!("C{index:02}") || name == format!("C{:02}", index + 1)
}

/// Project file extension.
pub const PROJECT_EXTENSION: &str = "bcproj";

/// Current project file format version. Version 2 has 255 cut layers (Tool1 and Tool2 after them) and
/// saves only the layers that differ from their defaults.
pub const FORMAT_VERSION: u32 = 2;

/// The standard colours (RGB) of the first 30 cut layers, then of Tool1 and Tool2. Later layers get
/// generated colours (see [`Layer::color`]).
pub const LAYER_COLORS: [[u8; 3]; LEGACY_CUT_LAYERS + 2] = [
    [20, 20, 20], [30, 90, 220], [220, 40, 40], [30, 160, 60], [235, 140, 20],
    [140, 60, 200], [0, 170, 190], [210, 50, 160], [120, 80, 40], [100, 100, 100],
    [80, 140, 255], [255, 110, 110], [90, 210, 110], [255, 190, 70], [180, 120, 240],
    [70, 215, 225], [245, 120, 205], [170, 130, 80], [160, 160, 160], [20, 50, 130],
    [130, 20, 20], [20, 100, 40], [150, 90, 0], [80, 30, 130], [0, 110, 120],
    [140, 20, 100], [200, 200, 60], [60, 200, 160], [255, 80, 30], [90, 120, 160],
    // Tool layers.
    [245, 160, 60], [70, 190, 240],
];

/// The hue (degrees) of a colour, or None for a grey.
fn hue([r, g, b]: [u8; 3]) -> Option<f64> {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    if d < 0.1 {
        return None;
    }
    let h = if max == r { ((g - b) / d).rem_euclid(6.0) } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
    Some(h * 60.0)
}

fn hsv_rgb(h: f64, s: f64, v: f64) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r, g, b].map(|v| ((v + m) * 255.0).round() as u8)
}

/// How a layer is turned into laser motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerMode {
    /// Follow the outlines (vector cut or score).
    Line,
    /// Raster-fill the inside of closed shapes.
    Fill,
    /// Fill first, then trace the outline.
    FillThenLine,
    /// Raster-engrave bitmaps.
    Image,
}

impl LayerMode {
    pub const ALL: [LayerMode; 4] = [LayerMode::Line, LayerMode::Fill, LayerMode::FillThenLine, LayerMode::Image];

    pub fn label(self) -> &'static str {
        match self {
            LayerMode::Line => "Line",
            LayerMode::Fill => "Fill",
            LayerMode::FillThenLine => "Fill + Line",
            LayerMode::Image => "Image",
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_tab_size() -> f64 {
    0.5
}

fn default_frequency() -> f64 {
    30.0
}

fn default_perf_cut() -> f64 {
    3.0
}

fn default_perf_skip() -> f64 {
    1.0
}

/// How images are converted to on/off laser dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DitherMode {
    Threshold,
    Ordered,
    #[default]
    FloydSteinberg,
    Atkinson,
    Jarvis,
    Stucki,
    /// Varies laser power with darkness instead of dithering (controllers that support it).
    Grayscale,
}

impl DitherMode {
    pub const ALL: [DitherMode; 7] = [
        DitherMode::Threshold,
        DitherMode::Ordered,
        DitherMode::FloydSteinberg,
        DitherMode::Atkinson,
        DitherMode::Jarvis,
        DitherMode::Stucki,
        DitherMode::Grayscale,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DitherMode::Threshold => "Threshold",
            DitherMode::Ordered => "Ordered",
            DitherMode::FloydSteinberg => "Floyd–Steinberg",
            DitherMode::Atkinson => "Atkinson",
            DitherMode::Jarvis => "Jarvis",
            DitherMode::Stucki => "Stucki",
            DitherMode::Grayscale => "Grayscale",
        }
    }
}

/// Image adjustments stored with each image shape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImageAdjust {
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub invert: bool,
    /// Stretch to the full black-to-white range.
    #[serde(default)]
    pub auto_level: bool,
    /// Unsharp-mask strength (0 = off).
    #[serde(default)]
    pub sharpen: f32,
    /// Whiten a plain background joined to the edges, within this tolerance (0 = off).
    #[serde(default)]
    pub remove_background: f32,
    #[serde(default)]
    pub effect: ImageEffect,
}

/// Artistic effects for engraving photos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ImageEffect {
    #[default]
    None,
    Sketch,
    LineArt,
    Edges,
}

impl ImageEffect {
    pub const ALL: [ImageEffect; 4] = [ImageEffect::None, ImageEffect::Sketch, ImageEffect::LineArt, ImageEffect::Edges];

    pub fn label(self) -> &'static str {
        match self {
            ImageEffect::None => "None",
            ImageEffect::Sketch => "Pencil sketch",
            ImageEffect::LineArt => "Line art",
            ImageEffect::Edges => "Edges",
        }
    }
}

impl Default for ImageAdjust {
    fn default() -> Self {
        Self { brightness: 0.0, contrast: 0.0, gamma: 1.0, invert: false, auto_level: false, sharpen: 0.0, remove_background: 0.0, effect: ImageEffect::None }
    }
}

/// A guide line on the design area.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GuideLine {
    /// A vertical line at x = `pos`; otherwise horizontal at y = `pos` (mm).
    pub vertical: bool,
    pub pos: f64,
}
/// Per-layer cut settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub mode: LayerMode,
    /// Head speed in mm/s.
    pub speed: f64,
    /// Minimum power, percent of the laser's maximum.
    pub min_power: f64,
    /// Maximum power, percent of the laser's maximum.
    pub max_power: f64,
    pub passes: u32,
    /// Spacing between fill scan lines (and image pixel size), in mm.
    pub interval: f64,
    /// Scan in both directions when filling or engraving images.
    #[serde(default = "default_true")]
    pub bidirectional: bool,
    #[serde(default)]
    pub dither: DitherMode,
    /// Fill scan direction in degrees (0 = horizontal).
    #[serde(default)]
    pub scan_angle: f64,
    /// Add a second fill pass at 90° to the first.
    #[serde(default)]
    pub cross_hatch: bool,
    /// Fill with rings following the outline (contours) instead of scan lines.
    #[serde(default)]
    pub offset_fill: bool,
    /// Laser-off run-up before and after each fill/image line, in mm.
    #[serde(default)]
    pub overscan: f64,
    /// Kerf compensation for closed cuts, in mm: positive grows parts (holes shrink).
    #[serde(default)]
    pub kerf: f64,
    /// Number of tabs (bridges) left uncut on each closed shape; 0 = none.
    #[serde(default)]
    pub tab_count: u32,
    /// Tab width, in mm.
    #[serde(default = "default_tab_size")]
    pub tab_size: f64,
    /// Don't put tabs on holes (inner shapes).
    #[serde(default = "default_true")]
    pub tabs_skip_inner: bool,
    /// Lead-in line length for closed cuts, in mm (starts in the scrap side); 0 = none.
    #[serde(default)]
    pub lead_in: f64,
    #[serde(default = "default_true")]
    pub air_assist: bool,
    /// Pulse frequency in kHz (galvo lasers).
    #[serde(default = "default_frequency")]
    pub frequency: f64,
    /// Perforated / dashed cutting: cut `perf_cut` mm, skip `perf_skip` mm, repeat (Line modes).
    #[serde(default)]
    pub perforate: bool,
    #[serde(default = "default_perf_cut")]
    pub perf_cut: f64,
    #[serde(default = "default_perf_skip")]
    pub perf_skip: f64,
    /// Whether the layer is sent to the laser.
    pub output: bool,
    /// Whether the layer is drawn in the editor.
    pub visible: bool,
    /// A colour the user chose, in place of the layer's standard one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_color: Option<[u8; 3]>,
    /// Added to the palette by the user (layers with shapes on them are always shown).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub added: bool,
}

impl Layer {
    pub fn new(index: usize) -> Self {
        Self {
            name: default_layer_name(index),
            mode: LayerMode::Line,
            speed: 100.0,
            min_power: 10.0,
            max_power: 20.0,
            passes: 1,
            interval: 0.1,
            bidirectional: true,
            dither: DitherMode::default(),
            scan_angle: 0.0,
            cross_hatch: false,
            offset_fill: false,
            overscan: 0.0,
            kerf: 0.0,
            tab_count: 0,
            tab_size: default_tab_size(),
            tabs_skip_inner: true,
            lead_in: 0.0,
            air_assist: true,
            frequency: default_frequency(),
            perforate: false,
            perf_cut: default_perf_cut(),
            perf_skip: default_perf_skip(),
            output: !is_tool_layer(index),
            visible: true,
            custom_color: None,
            added: false,
        }
    }

    /// The standard colour of layer `index`. Past the first 30, hues step by the golden angle so
    /// neighbouring layers never look alike, in three shades.
    pub fn color(index: usize) -> [u8; 3] {
        let index = index % LAYER_COUNT;
        if is_tool_layer(index) {
            return LAYER_COLORS[LEGACY_CUT_LAYERS + index - CUT_LAYER_COUNT];
        }
        if index < LEGACY_CUT_LAYERS {
            return LAYER_COLORS[index];
        }
        let n = index - LEGACY_CUT_LAYERS;
        let (s, v) = [(0.75, 0.9), (0.55, 0.7), (0.85, 0.55)][n % 3];
        hsv_rgb((n as f64 * 137.508 + 15.0) % 360.0, s, v)
    }

    /// This layer's colour: the user's choice, or the standard one for `index`.
    pub fn rgb(&self, index: usize) -> [u8; 3] {
        self.custom_color.unwrap_or_else(|| Self::color(index))
    }
}

/// The geometry of a shape, in its local coordinate space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShapeKind {
    /// Rectangle centred on the local origin.
    Rect { width: f64, height: f64, corner_radius: f64 },
    /// Ellipse centred on the local origin.
    Ellipse { radius_x: f64, radius_y: f64 },
    /// Regular polygon centred on the local origin.
    Polygon { sides: u32, radius: f64 },
    /// Star centred on the local origin.
    Star { points: u32, outer: f64, inner: f64 },
    /// Free-form path (polylines, curves).
    Path(BezPath),
    /// Text, stored with its rendered outline so it displays and cuts even if the font is missing.
    Text {
        text: String,
        font: String,
        /// Capital-letter height in mm.
        height: f64,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        italic: bool,
        #[serde(default)]
        style: TextStyle,
        outline: BezPath,
    },
    /// Bitmap image centred on the local origin. `data` is the original file, base64-encoded
    /// (shared, so copies and undo snapshots don't duplicate it).
    Image {
        data: std::sync::Arc<str>,
        width: f64,
        height: f64,
        adjust: ImageAdjust,
        /// Only the part inside this outline (image-local coordinates) is shown and engraved.
        #[serde(default)]
        clip: Option<BezPath>,
    },
    /// A QR code or 1D barcode, stored with its rendered outline (filled rectangles).
    Barcode {
        symbology: Symbology,
        content: String,
        /// QR: side length; 1D: bar height (mm).
        size: f64,
        outline: BezPath,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

/// Which edge (or the middle) of a text block stays put when the text changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextVAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// Layout options for text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    /// Line alignment, and which side stays put when the text changes.
    pub align: TextAlign,
    /// Which edge stays put vertically when the text changes (lines added, size changed…).
    pub valign: TextVAlign,
    /// Extra space between letters, in mm.
    pub letter_spacing: f64,
    /// Line height as a multiple of the font's normal line height.
    pub line_spacing: f64,
    /// Render the text in capitals.
    pub upper_case: bool,
    /// Merge overlapping letters into one outline (for script fonts).
    pub weld: bool,
    /// Lay the (first line of) text around a circle or along a path instead of straight.
    pub curve: Option<TextCurve>,
    /// Pair kerning: extra space (mm, negative to tighten) after particular letters, as
    /// (index of the letter in the text, counting every character including line breaks; mm).
    pub kerning: Vec<(usize, f64)>,
}

impl TextStyle {
    /// The point of a straight text block's bounds that stays put when the text is re-laid out:
    /// its left / centre / right and top / middle / bottom, by the alignment.
    pub fn anchor(&self, bounds: Rect) -> Point {
        let x = match self.align {
            TextAlign::Left => bounds.x0,
            TextAlign::Center => bounds.center().x,
            TextAlign::Right => bounds.x1,
        };
        let y = match self.valign {
            TextVAlign::Top => bounds.y0,
            TextVAlign::Middle => bounds.center().y,
            TextVAlign::Bottom => bounds.y1,
        };
        Point::new(x, y)
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self { align: TextAlign::Center, valign: TextVAlign::Middle, letter_spacing: 0.0, line_spacing: 1.0, upper_case: false, weld: false, curve: None, kerning: Vec::new() }
    }
}

/// How curved text follows its guide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TextCurve {
    /// Around a circle of `radius` mm (to the letters' baseline). Top text reads clockwise over the
    /// top; bottom text reads left to right along the bottom, letters upright.
    Circle { radius: f64, bottom: bool },
    /// Along `path` (in the text's local coordinates), starting `offset` mm along it (centred when
    /// aligned centre). `flip` puts the letters on the other side, reading the other way.
    Path { path: bc_geom::BezPath, offset: f64, flip: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Symbology {
    #[default]
    Qr,
    /// A QR code with an empty square in the middle for a logo (using the strongest error correction
    /// so it still scans).
    QrLogo,
    Code128,
}

impl Symbology {
    pub const ALL: [Symbology; 3] = [Symbology::Qr, Symbology::QrLogo, Symbology::Code128];

    pub fn label(self) -> &'static str {
        match self {
            Symbology::Qr => "QR code",
            Symbology::QrLogo => "QR code with logo space",
            Symbology::Code128 => "Code 128",
        }
    }

    pub fn is_qr(self) -> bool {
        matches!(self, Symbology::Qr | Symbology::QrLogo)
    }
}

impl ShapeKind {
    /// An image shape from raw file bytes.
    pub fn image(bytes: &[u8], width: f64, height: f64) -> Self {
        use base64::Engine as _;
        ShapeKind::Image { data: base64::engine::general_purpose::STANDARD.encode(bytes).into(), width, height, adjust: ImageAdjust::default(), clip: None }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            ShapeKind::Rect { .. } => "Rectangle",
            ShapeKind::Ellipse { .. } => "Ellipse",
            ShapeKind::Polygon { .. } => "Polygon",
            ShapeKind::Star { .. } => "Star",
            ShapeKind::Path(_) => "Path",
            ShapeKind::Text { .. } => "Text",
            ShapeKind::Image { .. } => "Image",
            ShapeKind::Barcode { symbology, .. } => symbology.label(),
        }
    }
}

/// A drawable, outputtable object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: u64,
    pub kind: ShapeKind,
    /// Local-to-document transform.
    pub transform: Affine,
    /// Index into [`Document::layers`].
    pub layer: usize,
    pub locked: bool,
    /// Shapes with the same group id are selected and moved together.
    #[serde(default)]
    pub group: Option<u64>,
    /// Live effects, applied in order to the placed outline (see [`effects`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<effects::Effect>,
}

impl Shape {
    /// Path in local coordinates. For images, this is the image's frame.
    pub fn local_path(&self) -> BezPath {
        match &self.kind {
            ShapeKind::Rect { width, height, corner_radius } => bc_geom::rect_path(*width, *height, *corner_radius),
            ShapeKind::Ellipse { radius_x, radius_y } => bc_geom::ellipse_path(*radius_x, *radius_y),
            ShapeKind::Polygon { sides, radius } => bc_geom::polygon_path(*sides, *radius),
            ShapeKind::Star { points, outer, inner } => bc_geom::star_path(*points, *outer, *inner),
            ShapeKind::Path(path) => path.clone(),
            ShapeKind::Text { outline, .. } | ShapeKind::Barcode { outline, .. } => outline.clone(),
            ShapeKind::Image { clip: Some(clip), .. } => clip.clone(),
            ShapeKind::Image { width, height, .. } => bc_geom::rect_path(*width, *height, 0.0),
        }
    }

    /// Decoded original image file bytes, for image shapes.
    pub fn image_bytes(&self) -> Option<Vec<u8>> {
        use base64::Engine as _;
        match &self.kind {
            ShapeKind::Image { data, .. } => base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).ok(),
            _ => None,
        }
    }

    /// Whether the shape is a closed outline (fills and booleans apply).
    pub fn is_closed(&self) -> bool {
        match &self.kind {
            ShapeKind::Rect { .. }
            | ShapeKind::Ellipse { .. }
            | ShapeKind::Polygon { .. }
            | ShapeKind::Star { .. }
            | ShapeKind::Image { .. }
            | ShapeKind::Barcode { .. } => true,
            // Text in a single-line font is open strokes.
            ShapeKind::Path(p) | ShapeKind::Text { outline: p, .. } => bc_geom::flatten(p, 0.05).iter().all(|l| l.closed),
        }
    }

    /// Path in document coordinates (mm).
    /// The outline as placed in the document, with any live effects applied.
    /// [`world_path`](Self::world_path) for drawing: slow live effects are worked out in the
    /// background, and until they're ready the shape comes back without them (see
    /// [`effects::busy`]).
    pub fn world_path_preview(&self) -> BezPath {
        let placed = self.transform * self.local_path();
        if self.effects.is_empty() || matches!(self.kind, ShapeKind::Image { .. }) {
            return placed;
        }
        effects::apply_all_in_background(&self.effects, &placed, self.transform).unwrap_or(placed)
    }

    pub fn world_path(&self) -> BezPath {
        let placed = self.transform * self.local_path();
        if self.effects.is_empty() || matches!(self.kind, ShapeKind::Image { .. }) {
            return placed;
        }
        effects::apply_all_placed(&self.effects, placed, self.transform)
    }

    pub fn bounds(&self) -> Option<Rect> {
        bc_geom::bounds(&self.world_path())
    }

    /// Whether the shape's outline is within `tolerance` of `p`, or `p` is inside a closed shape.
    pub fn hit(&self, p: Point, tolerance: f64) -> bool {
        let path = self.world_path();
        if bc_geom::distance_to_outline(&path, p) <= tolerance {
            return true;
        }
        self.is_closed() && bc_geom::contains(&path, p)
    }
}

/// Layers in project files: only those that differ from their defaults, each with its index. Files
/// before format 2 list every layer in order, without indices; those are read by position.
mod layer_list {
    use super::Layer;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize)]
    struct Saved<'a> {
        index: usize,
        #[serde(flatten)]
        layer: &'a Layer,
    }

    #[derive(Deserialize)]
    struct Loaded {
        index: Option<usize>,
        #[serde(flatten)]
        layer: Layer,
    }

    pub fn serialize<S: Serializer>(layers: &[Layer], s: S) -> Result<S::Ok, S::Error> {
        let changed: Vec<Saved> = layers.iter().enumerate().filter(|(i, l)| **l != Layer::new(*i)).map(|(index, layer)| Saved { index, layer }).collect();
        changed.serialize(s)
    }

    /// Fills in defaults around the saved layers. (A file before format 2 still has Tool1 and Tool2 at
    /// 30 and 31 here; `Document::from_json` moves them.)
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Layer>, D::Error> {
        let loaded = Vec::<Loaded>::deserialize(d)?;
        let mut layers: Vec<Layer> = Vec::new();
        for (position, l) in loaded.into_iter().enumerate() {
            let index = l.index.unwrap_or(position);
            if index >= super::LAYER_COUNT {
                continue;
            }
            while layers.len() <= index {
                layers.push(Layer::new(layers.len()));
            }
            layers[index] = l.layer;
        }
        while layers.len() < super::LAYER_COUNT {
            layers.push(Layer::new(layers.len()));
        }
        Ok(layers)
    }
}

/// A BurnCraft project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format_version: u32,
    /// Shapes in draw order (last is on top).
    pub shapes: Vec<Shape>,
    #[serde(with = "layer_list")]
    pub layers: Vec<Layer>,
    /// Work area size in mm (width, height).
    pub bed_size: (f64, f64),
    /// Serial number and CSV data for variable text.
    #[serde(default)]
    pub variables: vars::VariableText,
    /// Guide lines dragged out of the rulers (never cut).
    #[serde(default)]
    pub guides: Vec<GuideLine>,
    /// Named values and locked sizes / distances (parametric design).
    #[serde(default)]
    pub params: constraints::Parametric,
    next_id: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            shapes: Vec::new(),
            layers: (0..LAYER_COUNT).map(Layer::new).collect(),
            bed_size: (600.0, 400.0),
            variables: vars::VariableText::default(),
            guides: Vec::new(),
            params: constraints::Parametric::default(),
            next_id: 1,
        }
    }
}

#[derive(Debug)]
pub enum DocError {
    Io(std::io::Error),
    Format(serde_json::Error),
    UnsupportedVersion(u32),
}

impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DocError::Io(e) => write!(f, "{e}"),
            DocError::Format(e) => write!(f, "invalid project file: {e}"),
            DocError::UnsupportedVersion(v) => write!(f, "project format version {v} is newer than this version of BurnCraft supports"),
        }
    }
}

impl std::error::Error for DocError {}

impl Document {
    /// Adds a shape, assigning it a fresh id, and returns that id.
    pub fn add_shape(&mut self, kind: ShapeKind, transform: Affine, layer: usize) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.shapes.push(Shape { id, kind, transform, layer: layer.min(LAYER_COUNT - 1), locked: false, group: None, effects: Vec::new() });
        id
    }

    /// Puts the given shapes into one new group. Returns the group id.
    pub fn group(&mut self, ids: &std::collections::BTreeSet<u64>) -> Option<u64> {
        if ids.len() < 2 {
            return None;
        }
        let gid = self.next_id;
        self.next_id += 1;
        for s in self.shapes.iter_mut().filter(|s| ids.contains(&s.id)) {
            s.group = Some(gid);
        }
        Some(gid)
    }

    /// Removes the given shapes from their groups.
    pub fn ungroup(&mut self, ids: &std::collections::BTreeSet<u64>) {
        let groups: std::collections::BTreeSet<u64> = self.shapes.iter().filter(|s| ids.contains(&s.id)).filter_map(|s| s.group).collect();
        for s in self.shapes.iter_mut().filter(|s| s.group.is_some_and(|g| groups.contains(&g))) {
            s.group = None;
        }
    }

    /// The selection plus every other member of any group it touches.
    pub fn expand_groups(&self, ids: &std::collections::BTreeSet<u64>) -> std::collections::BTreeSet<u64> {
        let groups: std::collections::BTreeSet<u64> = self.shapes.iter().filter(|s| ids.contains(&s.id)).filter_map(|s| s.group).collect();
        self.shapes.iter().filter(|s| ids.contains(&s.id) || s.group.is_some_and(|g| groups.contains(&g))).map(|s| s.id).collect()
    }

    pub fn shape(&self, id: u64) -> Option<&Shape> {
        self.shapes.iter().find(|s| s.id == id)
    }

    pub fn shape_mut(&mut self, id: u64) -> Option<&mut Shape> {
        self.shapes.iter_mut().find(|s| s.id == id)
    }

    /// Removes the shapes whose ids satisfy `pred`. Returns how many were removed.
    pub fn remove_where(&mut self, mut pred: impl FnMut(u64) -> bool) -> usize {
        let before = self.shapes.len();
        self.shapes.retain(|s| !pred(s.id));
        before - self.shapes.len()
    }

    /// Topmost visible shape under `p`.
    /// The shape a click at `p` picks. A shape's line (within `tolerance`) comes first, topmost
    /// first, so shapes under others can still be reached. Failing that, the inside of a shape
    /// that's drawn filled: a picture, or a closed shape on a Fill, Fill + Line or Image layer.
    /// The empty inside of a shape on a Line layer doesn't count.
    pub fn hit_test(&self, p: Point, tolerance: f64) -> Option<u64> {
        let visible = || self.shapes.iter().rev().filter(|s| self.layers[s.layer].visible);
        if let Some(s) = visible().find(|s| bc_geom::distance_to_outline(&s.world_path(), p) <= tolerance) {
            return Some(s.id);
        }
        visible()
            .find(|s| {
                let filled = matches!(s.kind, ShapeKind::Image { .. }) || self.layers[s.layer].mode != LayerMode::Line;
                filled && s.is_closed() && bc_geom::contains(&s.world_path(), p)
            })
            .map(|s| s.id)
    }

    /// Layer indices that have at least one shape, in ascending order.
    pub fn used_layers(&self) -> Vec<usize> {
        let mut used = [false; LAYER_COUNT];
        for s in &self.shapes {
            used[s.layer] = true;
        }
        (0..LAYER_COUNT).filter(|&i| used[i]).collect()
    }

    /// The layers shown in the palette: any the user added and any with shapes on them (colour
    /// layers only, in order), or just the first layer when there are none.
    pub fn palette_layers(&self) -> Vec<usize> {
        let used = self.used_layers();
        let shown: Vec<usize> = (0..CUT_LAYER_COUNT).filter(|&i| self.layers[i].added || used.contains(&i)).collect();
        if shown.is_empty() { vec![0] } else { shown }
    }

    /// Takes layer `index` out of the palette, back to its standard settings. Its shapes move to
    /// layer `move_to`, or with None are deleted. Returns how many shapes it had.
    pub fn remove_layer(&mut self, index: usize, move_to: Option<usize>) -> usize {
        let count = self.shapes.iter().filter(|s| s.layer == index).count();
        match move_to {
            Some(to) => self.shapes.iter_mut().filter(|s| s.layer == index).for_each(|s| s.layer = to),
            None => self.shapes.retain(|s| s.layer != index),
        }
        self.layers[index] = Layer::new(index);
        count
    }

    /// The first colour layer that's free for a feature to set up and use: nothing on it, and not
    /// one the user added (and so may have set up already).
    pub fn spare_layer(&self) -> Option<usize> {
        let used = self.used_layers();
        (0..CUT_LAYER_COUNT).find(|&i| !used.contains(&i) && !self.layers[i].added)
    }

    /// The colour layer to add next, or None when all are in the palette.
    pub fn next_free_layer(&self) -> Option<usize> {
        let shown = self.palette_layers();
        (0..CUT_LAYER_COUNT).find(|i| !shown.contains(i))
    }

    /// A colour for a newly added layer that stands apart from the colours already in the
    /// palette: the hue furthest from theirs, at a strength that reads on light and dark boards.
    /// `seed` varies the choice among equally good hues.
    pub fn fresh_layer_color(&self, seed: u64) -> [u8; 3] {
        let hues: Vec<f64> = self.palette_layers().iter().filter_map(|&i| hue(self.layers[i].rgb(i))).collect();
        let offset = (seed % 360) as f64;
        let best = (0..72)
            .map(|k| (offset + k as f64 * 5.0) % 360.0)
            .max_by(|a, b| {
                let gap = |h: f64| hues.iter().map(|o| (h - o).abs().min(360.0 - (h - o).abs())).fold(360.0, f64::min);
                gap(*a).total_cmp(&gap(*b))
            })
            .unwrap_or(0.0);
        hsv_rgb(best, 0.7, 0.85)
    }

    /// Union of the bounds of the given shapes.
    pub fn bounds_of(&self, ids: impl IntoIterator<Item = u64>) -> Option<Rect> {
        ids.into_iter()
            .filter_map(|id| self.shape(id)?.bounds())
            .reduce(|a, b| a.union(b))
    }

    pub fn to_json(&self) -> Result<String, DocError> {
        serde_json::to_string_pretty(self).map_err(DocError::Format)
    }

    /// Tool layers (Tool1, Tool2) are never sent to the laser; keeps their Output switch off to match.
    pub fn tool_layers_off(&mut self) {
        for (i, l) in self.layers.iter_mut().enumerate() {
            if is_tool_layer(i) {
                l.output = false;
            }
        }
    }

    /// Renames layers still called by an old default name ("C00" for the first) to the current
    /// one ("01"), so a layer's name matches the number shown for it.
    pub fn renumber_default_names(&mut self) {
        for (i, l) in self.layers.iter_mut().enumerate() {
            if is_old_default_name(i, &l.name) {
                l.name = default_layer_name(i);
            }
        }
    }

    pub fn from_json(json: &str) -> Result<Self, DocError> {
        let mut doc: Document = serde_json::from_str(json).map_err(DocError::Format)?;
        if doc.format_version > FORMAT_VERSION {
            return Err(DocError::UnsupportedVersion(doc.format_version));
        }
        if doc.format_version < 2 {
            // 30 cut layers then Tool1, Tool2: move the tool layers (and their shapes) up.
            let old = std::mem::take(&mut doc.layers);
            doc.layers = (0..LAYER_COUNT).map(Layer::new).collect();
            for (i, l) in old.into_iter().enumerate() {
                if let Some(j) = from_legacy_index(i) {
                    doc.layers[j] = l;
                }
            }
            for s in &mut doc.shapes {
                s.layer = from_legacy_index(s.layer).unwrap_or(LAYER_COUNT - 1);
            }
            doc.format_version = FORMAT_VERSION;
        }
        doc.layers.truncate(LAYER_COUNT);
        while doc.layers.len() < LAYER_COUNT {
            doc.layers.push(Layer::new(doc.layers.len()));
        }
        for s in &mut doc.shapes {
            s.layer = s.layer.min(LAYER_COUNT - 1);
        }
        doc.tool_layers_off();
        doc.renumber_default_names();
        let max_id = doc.shapes.iter().map(|s| s.id).max().unwrap_or(0);
        doc.next_id = doc.next_id.max(max_id + 1);
        Ok(doc)
    }

    /// Saves safely: writes a temporary file next to `path`, then swaps it in, so a crash or full
    /// disk mid-save never leaves a half-written project.
    pub fn save(&self, path: &Path) -> Result<(), DocError> {
        let json = self.to_json()?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".saving");
        let tmp = std::path::PathBuf::from(tmp);
        std::fs::write(&tmp, json).map_err(DocError::Io)?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            DocError::Io(e)
        })
    }

    pub fn load(path: &Path) -> Result<Self, DocError> {
        Self::from_json(&std::fs::read_to_string(path).map_err(DocError::Io)?)
    }
}

/// Snapshot-based undo/redo. Call [`History::checkpoint`] before each edit.
#[derive(Debug, Default)]
pub struct History {
    /// Snapshots before each edit, with the edit's name.
    undo: Vec<(Document, String)>,
    redo: Vec<(Document, String)>,
    /// Key and time of the last mergeable checkpoint.
    last_merge: Option<(String, f64)>,
}

impl History {
    const LIMIT: usize = 200;
    /// Mergeable edits closer together than this (seconds) share one undo step.
    const MERGE_WINDOW: f64 = 1.0;

    /// Records `doc` as it is *before* an edit.
    pub fn checkpoint(&mut self, doc: &Document) {
        self.checkpoint_named(doc, "Edit");
    }

    /// Records `doc` before an edit named `label` (shown in the undo history).
    pub fn checkpoint_named(&mut self, doc: &Document, label: &str) {
        self.last_merge = None;
        self.push(doc, label);
    }

    /// Like [`History::checkpoint`], but consecutive calls with the same `key` within a second
    /// (e.g. while dragging a slider) are merged into one undo step. `now` is a time in seconds.
    pub fn checkpoint_merged(&mut self, doc: &Document, key: &str, now: f64) {
        let merge = self.last_merge.as_ref().is_some_and(|(k, t)| k == key && now - t < Self::MERGE_WINDOW);
        if !merge {
            self.push(doc, key);
        }
        self.last_merge = Some((key.to_string(), now));
    }

    fn push(&mut self, doc: &Document, label: &str) {
        self.undo.push((doc.clone(), label.to_string()));
        if self.undo.len() > Self::LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, doc: &mut Document) -> bool {
        self.last_merge = None;
        match self.undo.pop() {
            Some((prev, label)) => {
                self.redo.push((std::mem::replace(doc, prev), label));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, doc: &mut Document) -> bool {
        match self.redo.pop() {
            Some((next, label)) => {
                self.undo.push((std::mem::replace(doc, next), label));
                true
            }
            None => false,
        }
    }

    /// Names of the undoable edits, oldest first.
    pub fn undo_labels(&self) -> Vec<&str> {
        self.undo.iter().map(|(_, l)| l.as_str()).collect()
    }

    /// Names of the redoable edits, next first.
    pub fn redo_labels(&self) -> Vec<&str> {
        self.redo.iter().rev().map(|(_, l)| l.as_str()).collect()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn layers_are_numbered_from_one() {
        assert_eq!(layer_label(0), "01");
        assert_eq!(layer_label(CUT_LAYER_COUNT - 1), "255");
        assert_eq!(layer_label(CUT_LAYER_COUNT), "Tool1");
        assert_eq!(Layer::new(0).name, "01");
        assert_eq!(Layer::new(CUT_LAYER_COUNT + 1).name, "Tool2");
        // Projects saved with the old names ("C00" for the first layer) are renamed on loading;
        // names the user chose are kept, even ones that look like defaults for another layer.
        let mut doc = Document::default();
        doc.layers[0].name = "C00".into();
        doc.layers[1].name = "C05".into();
        doc.layers[2].name = "Engrave".into();
        doc.layers[3].name = "C04".into();
        let back = Document::from_json(&doc.to_json().unwrap()).unwrap();
        let names: Vec<&str> = back.layers[..4].iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["01", "C05", "Engrave", "04"]);
    }

    fn sample() -> Document {
        let mut doc = Document::default();
        doc.add_shape(ShapeKind::Rect { width: 20.0, height: 10.0, corner_radius: 0.0 }, Affine::translate((50.0, 50.0)), 1);
        doc.add_shape(ShapeKind::Ellipse { radius_x: 5.0, radius_y: 5.0 }, Affine::translate((100.0, 50.0)), 3);
        doc
    }

    #[test]
    fn clicks_pick_lines_first_and_only_filled_insides() {
        let mut doc = Document::default();
        let rect = |doc: &mut Document, c: (f64, f64), size: f64, layer| doc.add_shape(ShapeKind::Rect { width: size, height: size, corner_radius: 0.0 }, Affine::translate(c), layer);
        // A small square inside a big one that's drawn on top.
        let small = rect(&mut doc, (50.0, 50.0), 10.0, 0);
        let big = rect(&mut doc, (50.0, 50.0), 80.0, 0);
        // Layer 0 cuts lines: the big square's empty middle doesn't catch clicks.
        assert_eq!(doc.hit_test(Point::new(30.0, 30.0), 0.5), None);
        assert_eq!(doc.hit_test(Point::new(55.0, 50.0), 0.5), Some(small), "the small square's line");
        assert_eq!(doc.hit_test(Point::new(90.0, 50.0), 0.5), Some(big), "the big square's line");
        // On a Fill layer the inside counts, but lines still win.
        doc.layers[0].mode = LayerMode::Fill;
        assert_eq!(doc.hit_test(Point::new(30.0, 30.0), 0.5), Some(big));
        assert_eq!(doc.hit_test(Point::new(55.0, 50.0), 0.5), Some(small));
        assert_eq!(doc.hit_test(Point::new(52.0, 50.0), 0.5), Some(big), "inside both: the top one");
    }

    #[test]
    fn tool_layers_load_with_output_off() {
        let mut doc = Document::default();
        doc.layers[CUT_LAYER_COUNT].output = true;
        doc.layers[CUT_LAYER_COUNT + 1].output = true;
        let back = Document::from_json(&serde_json::to_string(&doc).unwrap()).unwrap();
        assert!(!back.layers[CUT_LAYER_COUNT].output && !back.layers[CUT_LAYER_COUNT + 1].output);
        assert!(back.layers[0].output, "colour layers are untouched");
    }

    #[test]
    fn json_round_trip() {
        let doc = sample();
        let back = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(doc, back);
    }

    #[test]
    fn hit_test_finds_topmost() {
        let mut doc = sample();
        // On Line layers the empty middles don't catch clicks…
        assert_eq!(doc.hit_test(Point::new(50.0, 50.0), 0.5), None);
        // …on filled layers they do, topmost first.
        for l in &mut doc.layers {
            l.mode = LayerMode::Fill;
        }
        assert_eq!(doc.hit_test(Point::new(50.0, 50.0), 0.5), Some(1));
        assert_eq!(doc.hit_test(Point::new(100.0, 50.0), 0.5), Some(2));
        assert_eq!(doc.hit_test(Point::new(300.0, 300.0), 0.5), None);
    }

    #[test]
    fn used_layers_sorted() {
        assert_eq!(sample().used_layers(), vec![1, 3]);
    }

    #[test]
    fn the_palette_shows_the_first_layer_those_in_use_and_those_added() {
        let mut doc = Document::default();
        assert_eq!(doc.palette_layers(), vec![0]);
        assert_eq!(doc.next_free_layer(), Some(1));
        let mut doc2 = sample();
        assert_eq!(doc2.palette_layers(), vec![1, 3]);
        assert_eq!(doc2.next_free_layer(), Some(0));
        doc2.layers[2].added = true;
        assert_eq!(doc2.palette_layers(), vec![1, 2, 3]);
        // Features looking for a spare layer leave added ones alone, but may use an empty first layer.
        assert_eq!(doc2.spare_layer(), Some(0));
        doc.layers[0].added = true;
        assert_eq!(doc.spare_layer(), Some(1));
    }

    #[test]
    fn removing_a_layer_moves_or_deletes_its_shapes() {
        let mut doc = sample();
        doc.layers[3].added = true;
        doc.layers[3].custom_color = Some([1, 2, 3]);
        let on_3 = doc.shapes.iter().filter(|s| s.layer == 3).count();
        assert_eq!(doc.remove_layer(3, Some(1)), on_3);
        assert_eq!(doc.used_layers(), vec![1]);
        assert_eq!(doc.layers[3], Layer::new(3), "back to standard");
        assert_eq!(doc.palette_layers(), vec![1]);
        let total = doc.shapes.len();
        assert_eq!(doc.remove_layer(1, None), total);
        assert!(doc.shapes.is_empty());
        assert_eq!(doc.palette_layers(), vec![0], "the first layer is left when nothing else is");
    }

    #[test]
    fn a_new_layer_colour_stands_apart_and_a_chosen_colour_is_used_and_saved() {
        let mut doc = Document::default();
        doc.layers[1].custom_color = Some([255, 0, 0]);
        doc.layers[1].added = true;
        // Hues in the palette: layer 1's red (layer 0 is near-black, so has none). The new colour
        // is far from red.
        let c = doc.fresh_layer_color(7);
        let h = hue(c).unwrap();
        assert!((h - 180.0).abs() <= 30.0, "hue {h} should be opposite red");
        assert_eq!(doc.layers[1].rgb(1), [255, 0, 0]);
        assert_eq!(doc.layers[2].rgb(2), Layer::color(2));
        let back: Document = serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        assert_eq!(back.layers[1].custom_color, Some([255, 0, 0]));
        assert!(back.layers[1].added && !back.layers[2].added);
    }

    #[test]
    fn older_projects_move_their_tool_layers_up() {
        // A format 1 file: 30 cut layers then Tool1, Tool2, every layer listed in order.
        let mut doc = sample();
        let guide = doc.add_shape(ShapeKind::Ellipse { radius_x: 1.0, radius_y: 1.0 }, Affine::IDENTITY, CUT_LAYER_COUNT + 1);
        let mut v = serde_json::to_value(&doc).unwrap();
        let mut old: Vec<Layer> = (0..LEGACY_CUT_LAYERS + 2).map(|i| Layer::new(from_legacy_index(i).unwrap())).collect();
        old[3].speed = 42.0;
        old[30].name = "T1".into();
        old[31].name = "Guides".into();
        v["format_version"] = 1.into();
        v["layers"] = serde_json::to_value(&old).unwrap();
        for s in v["shapes"].as_array_mut().unwrap() {
            if s["id"] == guide {
                s["layer"] = 31.into();
            }
        }
        let back = Document::from_json(&v.to_string()).unwrap();
        assert_eq!(back.format_version, FORMAT_VERSION);
        assert_eq!(back.layers.len(), LAYER_COUNT);
        assert_eq!(back.layers[3].speed, 42.0);
        assert_eq!(back.layers[CUT_LAYER_COUNT].name, "Tool1", "the old default name is updated");
        assert_eq!(back.layers[CUT_LAYER_COUNT + 1].name, "Guides", "a name of the user's own is kept");
        assert_eq!(back.shape(guide).unwrap().layer, CUT_LAYER_COUNT + 1, "the T2 shape stays on T2");
        assert_eq!(back.shapes.iter().map(|s| s.layer).collect::<Vec<_>>()[..2], [1, 3]);
    }

    #[test]
    fn projects_save_only_changed_layers_and_load_back_the_same() {
        let mut doc = sample();
        doc.layers[200].speed = 7.0;
        doc.layers[200].added = true;
        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let saved: Vec<u64> = v["layers"].as_array().unwrap().iter().map(|l| l["index"].as_u64().unwrap()).collect();
        assert!(saved.contains(&200) && saved.len() < 10, "{saved:?}");
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn layers_past_thirty_get_distinct_colours() {
        let colours: Vec<[u8; 3]> = (0..CUT_LAYER_COUNT).map(Layer::color).collect();
        for (i, c) in colours.iter().enumerate().skip(LEGACY_CUT_LAYERS) {
            assert_ne!(*c, colours[i - 1], "layer {i} looks like the one before");
        }
        assert_eq!(Layer::color(CUT_LAYER_COUNT), LAYER_COLORS[LEGACY_CUT_LAYERS], "T1 keeps its colour");
        assert_eq!(layer_label(CUT_LAYER_COUNT + 1), "Tool2");
    }

    #[test]
    fn undo_redo() {
        let mut doc = sample();
        let mut history = History::default();
        history.checkpoint(&doc);
        doc.remove_where(|id| id == 1);
        assert_eq!(doc.shapes.len(), 1);
        assert!(history.undo(&mut doc));
        assert_eq!(doc.shapes.len(), 2);
        assert!(history.redo(&mut doc));
        assert_eq!(doc.shapes.len(), 1);
    }

    #[test]
    fn merged_checkpoints() {
        let mut doc = sample();
        let mut h = History::default();
        h.checkpoint_merged(&doc, "power", 0.0);
        doc.layers[0].max_power = 50.0;
        h.checkpoint_merged(&doc, "power", 0.5);
        doc.layers[0].max_power = 60.0;
        h.checkpoint_merged(&doc, "power", 2.0); // outside the window: new step
        doc.layers[0].max_power = 70.0;
        assert!(h.undo(&mut doc));
        assert_eq!(doc.layers[0].max_power, 60.0);
        assert!(h.undo(&mut doc));
        assert_eq!(doc.layers[0].max_power, 20.0);
        assert!(!h.undo(&mut doc));
    }

    #[test]
    fn grouping() {
        let mut doc = sample();
        let g = doc.group(&BTreeSet::from([1, 2])).unwrap();
        assert_eq!(doc.expand_groups(&BTreeSet::from([1])), BTreeSet::from([1, 2]));
        doc.ungroup(&BTreeSet::from([2]));
        assert!(doc.shapes.iter().all(|s| s.group.is_none()));
        assert!(g > 2);
    }

    #[test]
    fn image_round_trip() {
        let mut doc = Document::default();
        let id = doc.add_shape(ShapeKind::image(&[1, 2, 3, 250], 10.0, 5.0), Affine::IDENTITY, 0);
        let back = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(back.shape(id).unwrap().image_bytes().unwrap(), vec![1, 2, 3, 250]);
    }

    #[test]
    fn ids_continue_after_load() {
        let mut doc = Document::from_json(&sample().to_json().unwrap()).unwrap();
        let id = doc.add_shape(ShapeKind::Ellipse { radius_x: 1.0, radius_y: 1.0 }, Affine::IDENTITY, 0);
        assert_eq!(id, 3);
    }
}
