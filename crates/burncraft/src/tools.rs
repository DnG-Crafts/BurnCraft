//! Small design tools with a settings window: stencil bridges, photo line art, inlays, rubber
//! stamps, contour cut lines, shadow boxes and monograms.

use crate::app::BurnCraftApp;
use crate::dialogs::{ok_cancel, window, Outcome};
use bc_doc::{edit, LayerMode, ShapeKind};
use bc_geom::{Affine, BezPath, Point};
use bc_raster::lines::{LineOptions, LineStyle};
use eframe::egui;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonogramFrame {
    None,
    Circle,
    Ring,
    Diamond,
}

impl MonogramFrame {
    const ALL: [MonogramFrame; 4] = [MonogramFrame::None, MonogramFrame::Circle, MonogramFrame::Ring, MonogramFrame::Diamond];

    fn label(self) -> &'static str {
        match self {
            MonogramFrame::None => "None",
            MonogramFrame::Circle => "Circle",
            MonogramFrame::Ring => "Ring",
            MonogramFrame::Diamond => "Diamond",
        }
    }
}

pub enum ToolDialog {
    Stencil { width: f64, horizontal: bool },
    PhotoLines { image: u64, options: LineOptions },
    Inlay { kerf: f64, clearance: f64, mirror: bool },
    Stamp { margin: f64, shoulder: f64 },
    Contour { distance: f64 },
    ShadowBox { image: u64, layers: usize, border: f64, smooth: f64 },
    Monogram { first: String, last: String, middle: String, font: String, height: f64, frame: MonogramFrame },
    Tiles { spec: bc_geom::tiles::TileSpec, gap: f64, area: (f64, f64), bed: (f64, f64) },
    Rhinestones { diameter: f64, gap: f64, layout: bc_geom::pathtools::StoneLayout },
    Lattice { cell: f64, bar: f64 },
    Knockout { gap: f64 },
    Hatch { spacing: f64, angle: f64, cross: bool },
    Pierce { diameter: f64 },
    /// `kind`: 0 drop shadow, 1 cast shadow, 2 extrude.
    Depth { kind: usize, dx: f64, dy: f64, skew: f64, squash: f64 },
}

fn mm(ui: &mut egui::Ui, v: &mut f64, range: std::ops::RangeInclusive<f64>) -> egui::Response {
    ui.add(egui::DragValue::new(v).range(range).speed(0.05).suffix(" mm"))
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(label);
        add(ui);
    });
}

impl ToolDialog {
    pub fn ui(&mut self, ctx: &egui::Context, families: &[String]) -> Option<Outcome> {
        let mut out = None;
        match self {
            ToolDialog::Rhinestones { diameter, gap, layout } => {
                window(ctx, "Rhinestone template").show(ctx, |ui| {
                    ui.weak("Holes for rhinestones, laid out on the selection. Cut them from template\nmaterial, brush the stones in, and press transfer tape over them.");
                    row(ui, "Hole size", |ui| {
                        mm(ui, diameter, 1.0..=20.0).on_hover_text("The stone's size plus a little room: SS10 stones (2.8 mm) suit holes of about 3.2 mm");
                    });
                    row(ui, "Gap between", |ui| {
                        mm(ui, gap, 0.0..=20.0);
                    });
                    row(ui, "Layout", |ui| {
                        egui::ComboBox::from_id_salt("stone-layout").selected_text(layout.label()).show_ui(ui, |ui| {
                            for l in bc_geom::pathtools::StoneLayout::ALL {
                                ui.selectable_value(layout, l, l.label());
                            }
                        });
                    });
                    out = ok_cancel(ui, "Create", *diameter > 0.0);
                });
            }
            ToolDialog::Lattice { cell, bar } => {
                window(ctx, "Lattice").show(ctx, |ui| {
                    ui.weak("Cuts a grid of square holes in the selected shape, keeping solid bars\nbetween them and round the edge: vents, speaker grilles, panels.");
                    row(ui, "Grid spacing", |ui| {
                        mm(ui, cell, 1.0..=200.0);
                    });
                    row(ui, "Bar width", |ui| {
                        mm(ui, bar, 0.3..=50.0).on_hover_text("Material left between holes and round the edge");
                    });
                    out = ok_cancel(ui, "Make lattice", *cell > *bar);
                });
            }
            ToolDialog::Knockout { gap } => {
                window(ctx, "Knockout").show(ctx, |ui| {
                    ui.weak("Cuts a recess in the largest selected shape where the other selected\nshapes sit, so they drop into it: inlays, or a cut-out for a part.");
                    row(ui, "Gap", |ui| {
                        mm(ui, gap, 0.0..=5.0).on_hover_text("Extra room round each piece. Allow for the laser's kerf; 0 for a tight fit.");
                    });
                    out = ok_cancel(ui, "Cut recess", true);
                });
            }
            ToolDialog::Hatch { spacing, angle, cross } => {
                window(ctx, "Hatch fill").show(ctx, |ui| {
                    ui.weak("Fills the selection with parallel lines, on a new layer. Burned as lines,\na hatch is a fast way to fill an area, or gives a cross-hatched look.");
                    row(ui, "Line spacing", |ui| {
                        mm(ui, spacing, 0.05..=20.0);
                    });
                    row(ui, "Angle", |ui| {
                        ui.add(egui::DragValue::new(angle).range(-180.0..=180.0).speed(1.0).suffix("°"));
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(cross, "Cross-hatch (second set at right angles)");
                    });
                    out = ok_cancel(ui, "Fill", *spacing > 0.0);
                });
            }
            ToolDialog::Depth { kind, dx, dy, skew, squash } => {
                window(ctx, "Shadow / 3D").show(ctx, |ui| {
                    ui.weak("Adds a shadow or a 3D look to the selection, on a new layer: engrave it\nunder the design, or cut it from a second material for layered work.");
                    row(ui, "Effect", |ui| {
                        ui.radio_value(kind, 0, "Drop shadow");
                        ui.radio_value(kind, 1, "Cast shadow");
                        ui.radio_value(kind, 2, "3D extrude");
                    });
                    if *kind == 1 {
                        row(ui, "Lean", |ui| {
                            ui.add(egui::DragValue::new(skew).range(-5.0..=5.0).speed(0.02)).on_hover_text("How far the shadow leans sideways, per mm of height");
                        });
                        row(ui, "Flatten to", |ui| {
                            ui.add(egui::DragValue::new(squash).range(0.02..=2.0).speed(0.01)).on_hover_text("The shadow's height as a fraction of the design's: 0.2–0.4 looks like a floor shadow");
                        });
                    } else {
                        row(ui, if *kind == 0 { "Offset" } else { "Depth" }, |ui| {
                            ui.label("X");
                            mm(ui, dx, -200.0..=200.0);
                            ui.label("Y");
                            mm(ui, dy, -200.0..=200.0);
                        });
                    }
                    out = ok_cancel(ui, "Add", true);
                });
            }
            ToolDialog::Pierce { diameter } => {
                window(ctx, "Pierce points").show(ctx, |ui| {
                    ui.weak("Puts a small dot where each selected outline starts, on a new Pierce\nlayer. Burn that layer first (more power, or several passes) so each\ncut starts through thick material.");
                    row(ui, "Dot size", |ui| {
                        mm(ui, diameter, 0.05..=5.0);
                    });
                    out = ok_cancel(ui, "Add dots", true);
                });
            }
            ToolDialog::Stencil { width, horizontal } => {
                window(ctx, "Stencil bridges").show(ctx, |ui| {
                    ui.weak("Cuts bridges through letters and shapes with holes, so the middle of an\nO, A or B stays attached when the shape is cut out of a sheet.");
                    row(ui, "Bridge width", |ui| {
                        mm(ui, width, 0.2..=20.0);
                    });
                    row(ui, "Direction", |ui| {
                        ui.radio_value(horizontal, false, "Vertical");
                        ui.radio_value(horizontal, true, "Horizontal");
                    });
                    out = ok_cancel(ui, "Add bridges", *width > 0.0);
                });
            }
            ToolDialog::PhotoLines { options, .. } => {
                window(ctx, "Photo to lines").show(ctx, |ui| {
                    ui.weak("Redraws the picture as lines: they burn quickly as vectors, also on galvos.");
                    row(ui, "Style", |ui| {
                        for (style, label, spacing) in [(LineStyle::Spiral, "Spiral", 1.2), (LineStyle::Waves, "Waves", 1.2), (LineStyle::Stipple, "Stipple", 0.6)] {
                            if ui.radio(options.style == style, label).clicked() && options.style != style {
                                options.style = style;
                                options.spacing = spacing;
                            }
                        }
                    });
                    let what = match options.style {
                        LineStyle::Spiral => "Turn spacing",
                        LineStyle::Waves => "Row spacing",
                        LineStyle::Stipple => "Dot spacing",
                    };
                    row(ui, what, |ui| {
                        mm(ui, &mut options.spacing, 0.2..=10.0);
                    });
                    if options.style == LineStyle::Stipple {
                        row(ui, "Dot size", |ui| {
                            mm(ui, &mut options.dot, 0.05..=5.0);
                        });
                        row(ui, "Pattern", |ui| {
                            if ui.button("Shuffle").clicked() {
                                options.seed = options.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                            }
                        });
                    } else {
                        row(ui, "Strength", |ui| {
                            let mut pct = options.strength * 100.0;
                            if ui.add(egui::Slider::new(&mut pct, 10.0..=100.0).suffix(" %")).changed() {
                                options.strength = pct / 100.0;
                            }
                        });
                    }
                    ui.weak("The image's brightness, contrast and effect settings apply. The lines go on\na new Line layer; the photo itself stays (hide or delete it).");
                    out = ok_cancel(ui, "Create", true);
                });
            }
            ToolDialog::Inlay { kerf, clearance, mirror } => {
                window(ctx, "Inlay").show(ctx, |ui| {
                    ui.weak("Makes a pocket (where the shapes are) and a matching insert beside it.\nBoth allow for the kerf so the insert fits tightly.");
                    row(ui, "Kerf", |ui| {
                        ui.add(egui::DragValue::new(kerf).range(0.0..=1.0).speed(0.005).suffix(" mm"));
                    });
                    row(ui, "Clearance", |ui| {
                        ui.add(egui::DragValue::new(clearance).range(-0.5..=1.0).speed(0.005).suffix(" mm")).on_hover_text("Extra room for glue or an easier fit; negative for a press fit");
                    });
                    ui.checkbox(mirror, "Mirror the insert (to glue it in face down)");
                    out = ok_cancel(ui, "Create", true);
                });
            }
            ToolDialog::Stamp { margin, shoulder } => {
                window(ctx, "Rubber stamp").show(ctx, |ui| {
                    ui.weak("Mirrors the selection and engraves away everything around it, leaving the\ndesign raised, with a cut line around the stamp.");
                    row(ui, "Border", |ui| {
                        mm(ui, margin, 0.0..=50.0);
                    });
                    row(ui, "Shoulder", |ui| {
                        mm(ui, shoulder, 0.0..=5.0).on_hover_text("A band around the design engraved at half power, giving the raised parts a sturdier base. 0 for none.");
                    });
                    out = ok_cancel(ui, "Create", true);
                });
            }
            ToolDialog::Contour { distance } => {
                window(ctx, "Contour cut line").show(ctx, |ui| {
                    ui.weak("Draws a cut line around the selection, for stickers, magnets and badges.\nImages are outlined around everything that isn't white.");
                    row(ui, "Distance", |ui| {
                        mm(ui, distance, 0.0..=50.0);
                    });
                    out = ok_cancel(ui, "Create", true);
                });
            }
            ToolDialog::ShadowBox { layers, border, smooth, .. } => {
                window(ctx, "Shadow box").show(ctx, |ui| {
                    ui.weak("Splits the picture into stacked plates by brightness: the front plate has the\nbiggest opening, each one behind it a smaller one, and the back is solid.");
                    row(ui, "Plates", |ui| {
                        ui.add(egui::DragValue::new(layers).range(2..=8));
                    });
                    row(ui, "Frame border", |ui| {
                        mm(ui, border, 2.0..=100.0);
                    });
                    row(ui, "Smoothing", |ui| {
                        mm(ui, smooth, 0.05..=3.0);
                    });
                    out = ok_cancel(ui, "Create", true);
                });
            }
            ToolDialog::Tiles { spec, gap, area, bed } => {
                use bc_geom::tiles::Joint;
                window(ctx, "Split into tiles").show(ctx, |ui| {
                    ui.weak("Cuts a design that's bigger than the bed into tiles that lock together.\nEach tile is grouped; move one onto the bed and use Cut Selected Graphics.");
                    row(ui, "Tiles", |ui| {
                        ui.add(egui::DragValue::new(&mut spec.cols).range(1..=20));
                        ui.label("across ×");
                        ui.add(egui::DragValue::new(&mut spec.rows).range(1..=20));
                        ui.label("down");
                    });
                    let (tw, th) = (area.0 / spec.cols.max(1) as f64, area.1 / spec.rows.max(1) as f64);
                    // Joints stick out into the neighbouring tile by up to their depth.
                    let reach = if spec.joint == Joint::Straight { 0.0 } else { spec.size * 2.0 };
                    let fits = tw + reach <= bed.0 && th + reach <= bed.1;
                    let text = format!("Each tile about {tw:.0} × {th:.0} mm");
                    if fits {
                        ui.weak(format!("{text}: fits the {:.0} × {:.0} mm bed.", bed.0, bed.1));
                    } else {
                        ui.colored_label(ui.visuals().warn_fg_color, format!("{text}: bigger than the {:.0} × {:.0} mm bed; use more tiles.", bed.0, bed.1));
                    }
                    row(ui, "Joints", |ui| {
                        for j in Joint::ALL {
                            ui.radio_value(&mut spec.joint, j, j.label());
                        }
                    });
                    if spec.joint != Joint::Straight {
                        row(ui, "Joint size", |ui| {
                            mm(ui, &mut spec.size, 3.0..=100.0);
                        });
                        row(ui, "One about every", |ui| {
                            mm(ui, &mut spec.spacing, 10.0..=1000.0);
                        });
                    }
                    row(ui, "Spread tiles apart by", |ui| {
                        mm(ui, gap, 0.0..=100.0);
                    });
                    out = ok_cancel(ui, "Split", spec.cols * spec.rows > 1);
                });
            }
            ToolDialog::Monogram { first, last, middle, font, height, frame } => {
                window(ctx, "Monogram").show(ctx, |ui| {
                    ui.weak("A large surname initial between the first and middle initials.");
                    row(ui, "Initials", |ui| {
                        ui.add(egui::TextEdit::singleline(first).desired_width(30.0).char_limit(2).hint_text("A"));
                        ui.add(egui::TextEdit::singleline(last).desired_width(30.0).char_limit(2).hint_text("S"));
                        ui.add(egui::TextEdit::singleline(middle).desired_width(30.0).char_limit(2).hint_text("B"));
                        ui.weak("first · last · middle");
                    });
                    row(ui, "Font", |ui| {
                        egui::ComboBox::from_id_salt("mono-font").selected_text(font.as_str()).height(300.0).show_ui(ui, |ui| {
                            for f in families {
                                ui.selectable_value(font, f.clone(), f);
                            }
                        });
                    });
                    row(ui, "Height", |ui| {
                        mm(ui, height, 5.0..=500.0);
                    });
                    row(ui, "Frame", |ui| {
                        for f in MonogramFrame::ALL {
                            ui.radio_value(frame, f, f.label());
                        }
                    });
                    out = ok_cancel(ui, "Create", !last.trim().is_empty());
                });
            }
        }
        out
    }
}

impl BurnCraftApp {
    /// Runs a confirmed tool dialog.
    /// The selected shapes' outlines (document coordinates), images left out; and the shapes.
    fn selected_paths(&self, sel: &BTreeSet<u64>) -> Vec<(u64, BezPath)> {
        self.doc.shapes.iter().filter(|s| sel.contains(&s.id) && !matches!(s.kind, ShapeKind::Image { .. })).map(|s| (s.id, s.world_path())).collect()
    }

    /// Adds `path` as a new shape on a fresh layer for `mode` called `name`, selected.
    fn add_on_new_layer(&mut self, undo: &str, path: BezPath, mode: LayerMode, name: &str) -> usize {
        self.checkpoint_named(undo);
        let layer = self.spare_layer(mode, name);
        let id = self.doc.add_shape(ShapeKind::Path(path), Affine::IDENTITY, layer);
        self.selection = [id].into();
        layer
    }

    pub(crate) fn apply_tool(&mut self, d: &ToolDialog) {
        let sel = self.selection.clone();
        match d {
            ToolDialog::Rhinestones { diameter, gap, layout } => {
                let paths: Vec<BezPath> = self.selected_paths(&sel).into_iter().map(|(_, p)| p).collect();
                match bc_geom::pathtools::rhinestones(&paths, *diameter, *gap, *layout) {
                    Some(holes) => {
                        let n = holes.elements().iter().filter(|e| matches!(e, bc_geom::PathEl::MoveTo(_))).count();
                        let layer = self.add_on_new_layer("Rhinestone template", holes, LayerMode::Line, "Rhinestones");
                        self.status = format!("{n} stone holes on layer {} (Rhinestones). The original shapes are still there: hide or delete them before cutting.", bc_doc::layer_label(layer));
                    }
                    None => self.error("The selection is too small for stones of that size.".into()),
                }
            }
            ToolDialog::Lattice { cell, bar } => {
                let shapes = self.selected_paths(&sel);
                let paths: Vec<BezPath> = shapes.iter().map(|(_, p)| p.clone()).collect();
                match bc_geom::pathtools::lattice(&paths, *cell, *bar) {
                    Some(l) => {
                        let layer = self.doc.shape(shapes[0].0).map_or(self.current_layer, |s| s.layer);
                        self.checkpoint_named("Lattice");
                        let ids: BTreeSet<u64> = shapes.iter().map(|(id, _)| *id).collect();
                        self.doc.remove_where(|id| ids.contains(&id));
                        let id = self.doc.add_shape(ShapeKind::Path(l), Affine::IDENTITY, layer);
                        self.selection = [id].into();
                        self.status = "Lattice made".into();
                    }
                    None => self.error("No holes fit: make the grid spacing smaller or the shape bigger.".into()),
                }
            }
            ToolDialog::Knockout { gap } => {
                let mut shapes = self.selected_paths(&sel);
                // The largest shape is the base; the others are what sits in it.
                shapes.sort_by(|a, b| bc_geom::ops::area(&b.1).total_cmp(&bc_geom::ops::area(&a.1)));
                let Some((base_id, base)) = shapes.first().cloned() else { return };
                let tops: Vec<BezPath> = shapes[1..].iter().map(|(_, p)| p.clone()).collect();
                match bc_geom::pathtools::knockout(&[base], &tops, *gap) {
                    Some(k) => {
                        let layer = self.doc.shape(base_id).map_or(self.current_layer, |s| s.layer);
                        self.checkpoint_named("Knockout");
                        self.doc.remove_where(|id| id == base_id);
                        let id = self.doc.add_shape(ShapeKind::Path(k), Affine::IDENTITY, layer);
                        self.selection = [id].into();
                        self.status = "Recess cut into the largest shape; the pieces are where they were".into();
                    }
                    None => self.error("Select a larger closed shape and the shapes to recess into it.".into()),
                }
            }
            ToolDialog::Hatch { spacing, angle, cross } => {
                let paths: Vec<BezPath> = self.selected_paths(&sel).into_iter().map(|(_, p)| p).collect();
                match bc_geom::pathtools::hatch(&paths, *spacing, *angle, *cross) {
                    Some(h) => {
                        let layer = self.add_on_new_layer("Hatch fill", h, LayerMode::Line, "Hatch");
                        self.status = format!("Hatch lines on layer {} (Hatch)", bc_doc::layer_label(layer));
                    }
                    None => self.error("Select closed shapes to fill.".into()),
                }
            }
            ToolDialog::Depth { kind, dx, dy, skew, squash } => {
                use bc_geom::pathtools::Depth;
                let paths: Vec<BezPath> = self.selected_paths(&sel).into_iter().map(|(_, p)| p).collect();
                let (effect, name) = match kind {
                    0 => (Depth::DropShadow { dx: *dx, dy: *dy }, "Shadow"),
                    1 => (Depth::CastShadow { skew: *skew, squash: *squash }, "Shadow"),
                    _ => (Depth::Extrude { dx: *dx, dy: *dy }, "3D lines"),
                };
                match bc_geom::pathtools::depth(&paths, effect) {
                    Some(p) => {
                        let mode = if *kind == 2 { LayerMode::Line } else { LayerMode::Fill };
                        let layer = self.add_on_new_layer(name, p, mode, name);
                        self.status = format!("Added on layer {} ({name})", bc_doc::layer_label(layer));
                    }
                    None => self.error("Select closed shapes or text.".into()),
                }
            }
            ToolDialog::Pierce { diameter } => {
                let paths: Vec<BezPath> = self.selected_paths(&sel).into_iter().map(|(_, p)| p).collect();
                match bc_geom::pathtools::pierce_points(&paths, *diameter) {
                    Some(dots) => {
                        let layer = self.add_on_new_layer("Pierce points", dots, LayerMode::Line, "Pierce");
                        self.status = format!("Pierce dots on layer {} (Pierce): burn it before the cut layers", bc_doc::layer_label(layer));
                    }
                    None => self.error("Nothing selected has an outline.".into()),
                }
            }
            ToolDialog::Stencil { width, horizontal } => {
                self.checkpoint_named("Stencil bridges");
                let n = edit::stencil(&mut self.doc, &sel, *width, *horizontal);
                self.status = if n == 0 { "Nothing selected has a hole that needs a bridge".into() } else { format!("Added bridges to {n} shape(s)") };
            }
            ToolDialog::PhotoLines { image, options } => {
                let Some(shape) = self.doc.shape(*image).cloned() else { return };
                match bc_job::photo_lines(&shape, options) {
                    Ok(path) => {
                        self.checkpoint_named("Photo to lines");
                        let layer = self.spare_layer(LayerMode::Line, "Photo lines");
                        let id = self.doc.add_shape(ShapeKind::Path(path), Affine::IDENTITY, layer);
                        self.selection = [id].into();
                        self.status = format!("Line art created on layer {}", bc_doc::layer_label(layer));
                    }
                    Err(e) => self.error(e),
                }
            }
            ToolDialog::Inlay { kerf, clearance, mirror } => {
                self.checkpoint_named("Inlay");
                match edit::inlay(&mut self.doc, &sel, *kerf, *clearance, *mirror) {
                    Some((p, i)) => {
                        self.selection = [p, i].into();
                        self.status = "Pocket (left) and insert (right) created: cut the pocket in the base and the insert from the inlay material".into();
                    }
                    None => self.error("Select one or more closed shapes or text to make an inlay from.".into()),
                }
            }
            ToolDialog::Stamp { margin, shoulder } => {
                let Some(stamp) = edit::rubber_stamp(&self.doc, &sel, *margin, *shoulder) else {
                    self.error("Select closed shapes or text to make a stamp from.".into());
                    return;
                };
                self.checkpoint_named("Rubber stamp");
                let bg_layer = self.spare_layer(LayerMode::Fill, "Stamp background");
                let mut ids = BTreeSet::from([self.doc.add_shape(ShapeKind::Path(stamp.background), Affine::IDENTITY, bg_layer)]);
                if let Some(band) = stamp.shoulder {
                    let layer = self.spare_layer(LayerMode::Fill, "Stamp shoulder");
                    let (min, max) = (self.doc.layers[bg_layer].min_power, self.doc.layers[bg_layer].max_power);
                    let l = &mut self.doc.layers[layer];
                    (l.min_power, l.max_power) = (min / 2.0, max / 2.0);
                    ids.insert(self.doc.add_shape(ShapeKind::Path(band), Affine::IDENTITY, layer));
                }
                let cut = self.spare_layer(LayerMode::Line, "Stamp cut");
                ids.insert(self.doc.add_shape(ShapeKind::Path(stamp.outline), Affine::IDENTITY, cut));
                let b = self.doc.bounds_of(sel.iter().copied()).map_or(0.0, |b| b.width() + 2.0 * margin + 10.0);
                edit::transform(&mut self.doc, &ids, Affine::translate((b, 0.0)));
                self.doc.group(&ids);
                self.selection = ids;
                self.status = "Stamp created beside the design: engrave the background deep (several passes), then cut".into();
            }
            ToolDialog::Contour { distance } => {
                let mut paths: Vec<BezPath> = Vec::new();
                for s in self.doc.shapes.iter().filter(|s| sel.contains(&s.id)) {
                    match &s.kind {
                        ShapeKind::Image { .. } => {
                            if let Ok(p) = bc_job::trace_image(s, 0.97, 0.2, 1.0) {
                                paths.push(p);
                            }
                        }
                        _ if s.is_closed() => paths.push(s.world_path()),
                        // Open lines and single-line text: outline the strokes themselves.
                        _ => paths.extend(stroke_pieces(&s.world_path())),
                    }
                }
                match bc_geom::ops::contour(&paths, *distance) {
                    Some(outline) => {
                        self.checkpoint_named("Contour cut line");
                        let layer = self.spare_layer(LayerMode::Line, "Contour cut");
                        let id = self.doc.add_shape(ShapeKind::Path(outline), Affine::IDENTITY, layer);
                        self.selection = [id].into();
                        self.status = format!("Cut line added on layer {}", bc_doc::layer_label(layer));
                    }
                    None => self.error("Nothing selected has an outline to follow.".into()),
                }
            }
            ToolDialog::ShadowBox { image, layers, border, smooth } => {
                let Some(shape) = self.doc.shape(*image).cloned() else { return };
                let Some(b) = shape.bounds() else { return };
                let frame = bc_geom::rect_path(b.width() + 2.0 * border, b.height() + 2.0 * border, 0.0);
                let mut plates: Vec<BezPath> = Vec::new();
                for k in 0..layers.saturating_sub(1) {
                    // Front plate: everything darker than 80 % white is open; further back, less.
                    let threshold = 1.0 - (k + 1) as f32 / *layers as f32;
                    match bc_job::trace_image(&shape, threshold.max(0.02), *smooth, 4.0) {
                        Ok(cut) => {
                            let mut plate = Affine::translate(b.center().to_vec2()) * frame.clone();
                            plate.extend(cut);
                            plates.push(plate);
                        }
                        Err(e) => {
                            self.error(e);
                            return;
                        }
                    }
                }
                plates.push(Affine::translate(b.center().to_vec2()) * frame.clone());
                self.checkpoint_named("Shadow box");
                let layer = self.spare_layer(LayerMode::Line, "Shadow box");
                let step = b.width() + 2.0 * border + 10.0;
                let ids: BTreeSet<u64> = plates
                    .into_iter()
                    .enumerate()
                    .map(|(k, p)| self.doc.add_shape(ShapeKind::Path(Affine::translate((step * (k + 1) as f64, 0.0)) * p), Affine::IDENTITY, layer))
                    .collect();
                self.doc.group(&ids);
                self.selection = ids;
                self.status = format!("{layers} plates created on layer {}, front to back, left to right", bc_doc::layer_label(layer));
            }
            ToolDialog::Monogram { first, last, middle, font, height, frame } => self.add_monogram([first, last, middle], font, *height, *frame),
            ToolDialog::Tiles { spec, gap, .. } => {
                self.checkpoint_named("Split into tiles");
                let (tiles, images) = edit::split_into_tiles(&mut self.doc, &sel, spec, *gap);
                self.selection = tiles.iter().flatten().copied().collect();
                self.status = format!(
                    "Split into {} tiles{}",
                    tiles.len(),
                    if images > 0 { format!("; {images} image(s) can't be split and were left as they are") } else { String::new() }
                );
            }
        }
    }

    fn add_monogram(&mut self, [first, last, middle]: [&String; 3], font: &str, height: f64, frame: MonogramFrame) {
        let style = bc_doc::TextStyle::default();
        let (w, h) = self.doc.bed_size;
        let centre = Point::new(w / 2.0, h / 2.0);
        let letter = |app: &Self, text: &str, size: f64| -> Option<(ShapeKind, f64)> {
            let t = text.trim();
            if t.is_empty() {
                return None;
            }
            let outline = app.fonts.outline(t, font, size, false, false, &style)?;
            let width = bc_geom::bounds(&outline)?.width();
            Some((ShapeKind::Text { text: t.into(), font: font.into(), height: size, bold: false, italic: false, style: style.clone(), outline }, width))
        };
        let Some((big, big_w)) = letter(self, last, height) else {
            self.error(format!("Could not render \"{last}\" in {font}"));
            return;
        };
        self.checkpoint_named("Monogram");
        let layer = self.current_layer;
        let mut ids = BTreeSet::from([self.doc.add_shape(big, Affine::translate(centre.to_vec2()), layer)]);
        let small = height * 0.6;
        let gap = height * 0.12;
        let mut reach = big_w / 2.0;
        for (text, side) in [(first, -1.0), (middle, 1.0)] {
            if let Some((kind, w)) = letter(self, text, small) {
                let x = side * (big_w / 2.0 + gap + w / 2.0);
                reach = reach.max(x.abs() + w / 2.0);
                ids.insert(self.doc.add_shape(kind, Affine::translate((centre.x + x, centre.y)), layer));
            }
        }
        let r = reach.max(height / 2.0) + height * 0.25;
        let frame_path = match frame {
            MonogramFrame::None => None,
            MonogramFrame::Circle => Some(bc_geom::ellipse_path(r, r)),
            MonogramFrame::Ring => {
                let mut ring = bc_geom::ellipse_path(r + height * 0.08, r + height * 0.08);
                ring.extend(bc_geom::ellipse_path(r, r));
                Some(ring)
            }
            MonogramFrame::Diamond => Some(bc_geom::polygon_path(4, r * 1.35)),
        };
        if let Some(p) = frame_path {
            ids.insert(self.doc.add_shape(ShapeKind::Path(p), Affine::translate(centre.to_vec2()), layer));
        }
        self.doc.group(&ids);
        self.selection = ids;
        self.status = "Monogram added at the centre of the bed".into();
    }
}

/// Thin closed pieces covering open strokes (0.3 mm each side), one per segment, so open lines
/// can be wrapped by a contour (which merges them).
fn stroke_pieces(path: &BezPath) -> Vec<BezPath> {
    let mut out = Vec::new();
    for l in bc_geom::flatten(path, 0.1) {
        for w in l.points.windows(2) {
            let d = w[1] - w[0];
            let len = d.hypot();
            if len < 1e-9 {
                continue;
            }
            let n = bc_geom::Vec2::new(-d.y, d.x) * (0.3 / len);
            let a = d * (0.3 / len);
            out.push(bc_geom::polyline_path(&[w[0] - a + n, w[1] + a + n, w[1] + a - n, w[0] - a - n], true));
        }
    }
    out
}
