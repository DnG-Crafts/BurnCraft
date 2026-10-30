//! Toolbar icons, drawn with simple vector strokes (BurnCraft's own designs).

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui};
use std::f32::consts::{PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    New,
    Open,
    Save,
    Import,
    Export,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    ZoomFit,
    ZoomIn,
    ZoomOut,
    ZoomSelection,
    Camera,
    Device,
    Settings,
    Tools,
    Group,
    Ungroup,
    MirrorH,
    MirrorV,
    AlignLeft,
    AlignHCenter,
    AlignRight,
    AlignTop,
    AlignVCenter,
    AlignCenters,
    AlignBottom,
    DistributeH,
    DistributeV,
    Preview,
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
    Offset,
    Weld,
    Subtract,
    Intersect,
    GridArray,
    CircularArray,
    Trace,
    Barcode,
    Corner,
    Trim,
    Knife,
    Dimension,
    Eraser,
    ShapeBuilder,
    Chamfer,
    Lock,
    Unlocked,
    // Side ribbon.
    Layers,
    Move,
    Console,
    Tips,
    Laser,
    Library,
    Art,
    Shape,
    Variables,
    History,
}

/// A square icon button. `selected` draws it pressed.
pub fn button(ui: &mut Ui, icon: Icon, size: f32, selected: bool, enabled: bool, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), if enabled { Sense::click() } else { Sense::hover() });
    let visuals = ui.visuals();
    let bg = if selected {
        visuals.selection.bg_fill.gamma_multiply(0.6)
    } else if enabled && response.hovered() {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 3.0, bg);
    let ink = if enabled { visuals.strong_text_color() } else { visuals.weak_text_color().gamma_multiply(0.5) };
    paint(ui.painter(), rect.shrink(size * 0.16), icon, ink, visuals.selection.stroke.color);
    response.on_hover_text(tooltip)
}

fn pts(r: Rect, list: &[(f32, f32)]) -> Vec<Pos2> {
    list.iter().map(|&(x, y)| r.lerp_inside(egui::vec2(x, y))).collect()
}

/// Draws `icon` inside `r` (coordinates below are 0..1 within the box).
pub fn paint(p: &egui::Painter, r: Rect, icon: Icon, ink: Color32, accent: Color32) {
    let w = (r.width() / 14.0).max(1.2);
    let s = Stroke::new(w, ink);
    let a = Stroke::new(w, accent);
    let at = |x: f32, y: f32| r.lerp_inside(egui::vec2(x, y));
    let line = |list: &[(f32, f32)], st: Stroke| {
        p.add(egui::Shape::line(pts(r, list), st));
    };
    let closed = |list: &[(f32, f32)], st: Stroke| {
        p.add(egui::Shape::closed_line(pts(r, list), st));
    };
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32, st: Stroke| {
        p.rect_stroke(Rect::from_min_max(at(x0, y0), at(x1, y1)), 1.0, st, StrokeKind::Middle);
    };
    let fill_rect = |x0: f32, y0: f32, x1: f32, y1: f32, c: Color32| {
        p.rect_filled(Rect::from_min_max(at(x0, y0), at(x1, y1)), 1.0, c);
    };
    let arc = |cx: f32, cy: f32, rad: f32, a0: f32, a1: f32, st: Stroke| {
        let c = at(cx, cy);
        let rr = rad * r.width();
        let n = 16;
        let v: Vec<Pos2> = (0..=n).map(|i| {
            let t = a0 + (a1 - a0) * i as f32 / n as f32;
            c + egui::vec2(t.cos(), t.sin()) * rr
        }).collect();
        p.add(egui::Shape::line(v, st));
    };
    let arrow_head = |tip: (f32, f32), dir: f32, st: Stroke| {
        let t = at(tip.0, tip.1);
        let l = r.width() * 0.22;
        for d in [dir + 2.6, dir - 2.6] {
            p.line_segment([t, t + egui::vec2(d.cos(), d.sin()) * l], st);
        }
    };
    let magnifier = |st: Stroke| {
        p.circle_stroke(at(0.42, 0.42), r.width() * 0.3, st);
        p.line_segment([at(0.64, 0.64), at(0.95, 0.95)], Stroke::new(st.width * 1.6, st.color));
    };
    match icon {
        Icon::New => {
            closed(&[(0.2, 0.05), (0.62, 0.05), (0.85, 0.28), (0.85, 0.95), (0.2, 0.95)], s);
            line(&[(0.62, 0.05), (0.62, 0.28), (0.85, 0.28)], s);
            line(&[(0.52, 0.5), (0.52, 0.8)], a);
            line(&[(0.37, 0.65), (0.67, 0.65)], a);
        }
        Icon::Open => {
            closed(&[(0.05, 0.2), (0.38, 0.2), (0.48, 0.32), (0.85, 0.32), (0.85, 0.85), (0.05, 0.85)], s);
            closed(&[(0.18, 0.48), (1.0, 0.48), (0.85, 0.85), (0.05, 0.85)], a);
        }
        Icon::Save => {
            closed(&[(0.08, 0.08), (0.78, 0.08), (0.92, 0.22), (0.92, 0.92), (0.08, 0.92)], s);
            rect(0.26, 0.08, 0.7, 0.36, s);
            fill_rect(0.25, 0.55, 0.75, 0.92, accent.gamma_multiply(0.5));
        }
        Icon::Import => {
            line(&[(0.1, 0.6), (0.1, 0.92), (0.9, 0.92), (0.9, 0.6)], s);
            line(&[(0.5, 0.05), (0.5, 0.7)], a);
            arrow_head((0.5, 0.72), PI / 2.0, a);
        }
        Icon::Export => {
            line(&[(0.1, 0.6), (0.1, 0.92), (0.9, 0.92), (0.9, 0.6)], s);
            line(&[(0.5, 0.72), (0.5, 0.08)], a);
            arrow_head((0.5, 0.06), -PI / 2.0, a);
        }
        Icon::Undo => {
            arc(0.52, 0.55, 0.32, -PI * 0.9, PI * 0.6, s);
            arrow_head((0.21, 0.5), -PI * 0.45, s);
        }
        Icon::Redo => {
            arc(0.48, 0.55, 0.32, -PI * 0.1, -PI * 1.6, s);
            arrow_head((0.79, 0.5), -PI * 0.55, s);
        }
        Icon::Cut => {
            p.circle_stroke(at(0.25, 0.78), r.width() * 0.15, s);
            p.circle_stroke(at(0.75, 0.78), r.width() * 0.15, s);
            line(&[(0.33, 0.66), (0.8, 0.05)], s);
            line(&[(0.67, 0.66), (0.2, 0.05)], s);
        }
        Icon::Copy => {
            rect(0.05, 0.05, 0.62, 0.65, s);
            fill_rect(0.38, 0.35, 0.95, 0.95, Color32::TRANSPARENT);
            rect(0.38, 0.35, 0.95, 0.95, a);
        }
        Icon::Paste => {
            rect(0.12, 0.15, 0.88, 0.95, s);
            rect(0.32, 0.05, 0.68, 0.25, s);
            line(&[(0.3, 0.5), (0.7, 0.5)], a);
            line(&[(0.3, 0.7), (0.7, 0.7)], a);
        }
        Icon::Delete => {
            line(&[(0.1, 0.2), (0.9, 0.2)], s);
            line(&[(0.38, 0.2), (0.38, 0.06), (0.62, 0.06), (0.62, 0.2)], s);
            line(&[(0.2, 0.2), (0.27, 0.95), (0.73, 0.95), (0.8, 0.2)], s);
            line(&[(0.43, 0.35), (0.43, 0.82)], s);
            line(&[(0.57, 0.35), (0.57, 0.82)], s);
        }
        Icon::ZoomFit => {
            for (x, y, dx, dy) in [(0.05, 0.05, 1.0, 1.0), (0.95, 0.05, -1.0, 1.0), (0.05, 0.95, 1.0, -1.0), (0.95, 0.95, -1.0, -1.0)] {
                line(&[(x + dx * 0.3, y), (x, y), (x, y + dy * 0.3)], s);
            }
            rect(0.32, 0.32, 0.68, 0.68, a);
        }
        Icon::ZoomIn | Icon::ZoomOut | Icon::ZoomSelection => {
            magnifier(s);
            match icon {
                Icon::ZoomIn => {
                    line(&[(0.27, 0.42), (0.57, 0.42)], a);
                    line(&[(0.42, 0.27), (0.42, 0.57)], a);
                }
                Icon::ZoomOut => line(&[(0.27, 0.42), (0.57, 0.42)], a),
                _ => rect(0.3, 0.3, 0.54, 0.54, a),
            }
        }
        Icon::Camera => {
            rect(0.05, 0.28, 0.95, 0.88, s);
            line(&[(0.3, 0.28), (0.38, 0.12), (0.62, 0.12), (0.7, 0.28)], s);
            p.circle_stroke(at(0.5, 0.58), r.width() * 0.18, a);
        }
        Icon::Device => {
            rect(0.05, 0.08, 0.95, 0.68, s);
            line(&[(0.5, 0.68), (0.5, 0.85)], s);
            line(&[(0.25, 0.92), (0.75, 0.92)], s);
            p.circle_filled(at(0.5, 0.38), r.width() * 0.07, accent);
        }
        Icon::Settings => {
            let c = at(0.5, 0.5);
            for i in 0..8 {
                let t = i as f32 * TAU / 8.0;
                p.line_segment([c + egui::vec2(t.cos(), t.sin()) * r.width() * 0.3, c + egui::vec2(t.cos(), t.sin()) * r.width() * 0.47], Stroke::new(w * 1.8, ink));
            }
            p.circle_stroke(c, r.width() * 0.3, s);
            p.circle_stroke(c, r.width() * 0.12, a);
        }
        Icon::Tools => {
            line(&[(0.15, 0.85), (0.6, 0.4)], Stroke::new(w * 2.0, ink));
            arc(0.7, 0.3, 0.18, PI * 0.25, PI * 1.75, s);
            fill_rect(0.1, 0.8, 0.22, 0.92, accent);
        }
        Icon::Group | Icon::Ungroup => {
            rect(0.08, 0.08, 0.55, 0.55, s);
            rect(0.45, 0.45, 0.92, 0.92, s);
            if icon == Icon::Group {
                rect(0.02, 0.02, 0.98, 0.98, Stroke::new(w * 0.8, accent));
            } else {
                line(&[(0.62, 0.15), (0.88, 0.15)], a);
                line(&[(0.15, 0.62), (0.15, 0.88)], a);
            }
        }
        Icon::MirrorH => {
            closed(&[(0.42, 0.15), (0.42, 0.85), (0.05, 0.85)], s);
            closed(&[(0.58, 0.15), (0.58, 0.85), (0.95, 0.85)], a);
            line(&[(0.5, 0.0), (0.5, 1.0)], Stroke::new(w * 0.7, ink));
        }
        Icon::MirrorV => {
            closed(&[(0.15, 0.42), (0.85, 0.42), (0.85, 0.05)], s);
            closed(&[(0.15, 0.58), (0.85, 0.58), (0.85, 0.95)], a);
            line(&[(0.0, 0.5), (1.0, 0.5)], Stroke::new(w * 0.7, ink));
        }
        Icon::AlignLeft | Icon::AlignHCenter | Icon::AlignRight => {
            let (x0a, x0b) = match icon {
                Icon::AlignLeft => (0.12, 0.12),
                Icon::AlignHCenter => (0.2, 0.32),
                _ => (0.28, 0.52),
            };
            fill_rect(x0a, 0.15, x0a + 0.6, 0.38, ink.gamma_multiply(0.7));
            fill_rect(x0b, 0.6, x0b + 0.36, 0.83, ink.gamma_multiply(0.7));
            let gx = match icon {
                Icon::AlignLeft => 0.08,
                Icon::AlignHCenter => 0.5,
                _ => 0.92,
            };
            line(&[(gx, 0.0), (gx, 1.0)], a);
        }
        Icon::AlignTop | Icon::AlignVCenter | Icon::AlignBottom => {
            let (y0a, y0b) = match icon {
                Icon::AlignTop => (0.12, 0.12),
                Icon::AlignVCenter => (0.2, 0.32),
                _ => (0.28, 0.52),
            };
            fill_rect(0.15, y0a, 0.38, y0a + 0.6, ink.gamma_multiply(0.7));
            fill_rect(0.6, y0b, 0.83, y0b + 0.36, ink.gamma_multiply(0.7));
            let gy = match icon {
                Icon::AlignTop => 0.08,
                Icon::AlignVCenter => 0.5,
                _ => 0.92,
            };
            line(&[(0.0, gy), (1.0, gy)], a);
        }
        Icon::AlignCenters => {
            // A big and a small box sharing one centre, on crossed centre lines.
            fill_rect(0.12, 0.2, 0.88, 0.8, ink.gamma_multiply(0.35));
            fill_rect(0.32, 0.36, 0.68, 0.64, ink.gamma_multiply(0.8));
            line(&[(0.5, 0.0), (0.5, 1.0)], a);
            line(&[(0.0, 0.5), (1.0, 0.5)], a);
        }
        Icon::DistributeH => {
            for x in [0.05, 0.4, 0.75] {
                fill_rect(x, 0.25, x + 0.2, 0.75, ink.gamma_multiply(0.7));
            }
            line(&[(0.0, 0.92), (1.0, 0.92)], a);
        }
        Icon::DistributeV => {
            for y in [0.05, 0.4, 0.75] {
                fill_rect(0.25, y, 0.75, y + 0.2, ink.gamma_multiply(0.7));
            }
            line(&[(0.92, 0.0), (0.92, 1.0)], a);
        }
        Icon::Preview => {
            closed(&[(0.05, 0.5), (0.3, 0.22), (0.7, 0.22), (0.95, 0.5), (0.7, 0.78), (0.3, 0.78)], s);
            p.circle_filled(at(0.5, 0.5), r.width() * 0.14, accent);
        }
        Icon::Select => {
            closed(&[(0.2, 0.05), (0.2, 0.8), (0.4, 0.62), (0.55, 0.95), (0.68, 0.88), (0.53, 0.56), (0.78, 0.56)], s);
        }
        Icon::Nodes => {
            line(&[(0.1, 0.85), (0.35, 0.3), (0.9, 0.15)], s);
            for (x, y) in [(0.1, 0.85), (0.35, 0.3), (0.9, 0.15)] {
                p.rect_filled(Rect::from_center_size(at(x, y), egui::vec2(w * 3.2, w * 3.2)), 0.0, accent);
            }
        }
        Icon::Line => {
            line(&[(0.1, 0.9), (0.3, 0.7), (0.85, 0.15), (0.95, 0.05)], s);
            line(&[(0.1, 0.9), (0.05, 0.95)], a);
            closed(&[(0.25, 0.75), (0.75, 0.25), (0.85, 0.35), (0.35, 0.85)], s);
        }
        Icon::Rect => rect(0.08, 0.2, 0.92, 0.8, s),
        Icon::Ellipse => {
            p.add(egui::Shape::ellipse_stroke(at(0.5, 0.5), egui::vec2(r.width() * 0.45, r.height() * 0.33), s));
        }
        Icon::Polygon | Icon::Star => {
            let n = if icon == Icon::Polygon { 6 } else { 10 };
            let c = (0.5, 0.52);
            let v: Vec<(f32, f32)> = (0..n)
                .map(|i| {
                    let t = i as f32 * TAU / n as f32 - PI / 2.0;
                    let rad = if icon == Icon::Star && i % 2 == 1 { 0.2 } else { 0.46 };
                    (c.0 + rad * t.cos(), c.1 + rad * t.sin())
                })
                .collect();
            closed(&v, s);
        }
        Icon::Text => {
            line(&[(0.1, 0.95), (0.5, 0.05), (0.9, 0.95)], Stroke::new(w * 1.4, ink));
            line(&[(0.27, 0.6), (0.73, 0.6)], Stroke::new(w * 1.4, ink));
        }
        Icon::PositionLaser => {
            arc(0.5, 0.38, 0.28, PI * 0.85, PI * 2.15, s);
            line(&[(0.25, 0.53), (0.5, 0.95), (0.75, 0.53)], s);
            p.circle_filled(at(0.5, 0.38), r.width() * 0.1, accent);
        }
        Icon::Measure => {
            closed(&[(0.05, 0.7), (0.7, 0.05), (0.95, 0.3), (0.3, 0.95)], s);
            for i in 1..5 {
                let t = i as f32 / 5.0;
                let (x, y) = (0.05 + 0.65 * t, 0.7 - 0.65 * t);
                line(&[(x, y), (x + 0.1, y + 0.1)], a);
            }
        }
        Icon::Offset => {
            rect(0.3, 0.3, 0.7, 0.7, s);
            p.rect_stroke(Rect::from_min_max(at(0.08, 0.08), at(0.92, 0.92)), r.width() * 0.2, a, StrokeKind::Middle);
        }
        Icon::Weld | Icon::Subtract | Icon::Intersect => {
            let (c1, c2, rad) = (at(0.37, 0.5), at(0.63, 0.5), r.width() * 0.3);
            match icon {
                Icon::Weld => {
                    p.circle_filled(c1, rad, accent.gamma_multiply(0.45));
                    p.circle_filled(c2, rad, accent.gamma_multiply(0.45));
                }
                Icon::Subtract => {
                    p.circle_filled(c1, rad, accent.gamma_multiply(0.45));
                }
                _ => {}
            }
            p.circle_stroke(c1, rad, s);
            p.circle_stroke(c2, rad, s);
            if icon == Icon::Intersect {
                p.circle_filled(at(0.5, 0.5), rad * 0.45, accent);
            }
        }
        Icon::GridArray => {
            for y in [0.08, 0.4, 0.72] {
                for x in [0.08, 0.4, 0.72] {
                    rect(x, y, x + 0.2, y + 0.2, s);
                }
            }
        }
        Icon::CircularArray => {
            for i in 0..8 {
                let t = i as f32 * TAU / 8.0;
                p.circle_stroke(at(0.5 + 0.36 * t.cos(), 0.5 + 0.36 * t.sin()), r.width() * 0.08, if i == 0 { a } else { s });
            }
        }
        Icon::Trace => {
            fill_rect(0.05, 0.05, 0.55, 0.55, ink.gamma_multiply(0.4));
            closed(&[(0.45, 0.45), (0.95, 0.45), (0.95, 0.95), (0.45, 0.95)], a);
            line(&[(0.05, 0.95), (0.95, 0.05)], Stroke::new(w * 0.7, ink));
        }
        Icon::Barcode => {
            for (x, wd) in [(0.05, 0.08), (0.18, 0.04), (0.27, 0.1), (0.42, 0.04), (0.5, 0.06), (0.62, 0.1), (0.77, 0.04), (0.85, 0.08)] {
                fill_rect(x, 0.12, x + wd, 0.88, ink);
            }
        }
        Icon::Corner => {
            line(&[(0.1, 0.95), (0.1, 0.5)], s);
            arc(0.55, 0.5, 0.45, PI, PI * 1.5, a);
            line(&[(0.55, 0.05), (0.95, 0.05)], s);
        }
        Icon::Chamfer => {
            // A corner with a straight bevel (accent).
            line(&[(0.1, 0.95), (0.1, 0.45)], s);
            line(&[(0.1, 0.45), (0.5, 0.05)], a);
            line(&[(0.5, 0.05), (0.95, 0.05)], s);
        }
        Icon::Dimension => {
            // A measured span: end ticks, the dimension line with arrows, and a label bar.
            line(&[(0.1, 0.35), (0.1, 0.85)], s);
            line(&[(0.9, 0.35), (0.9, 0.85)], s);
            line(&[(0.1, 0.6), (0.9, 0.6)], a);
            line(&[(0.22, 0.5), (0.1, 0.6), (0.22, 0.7)], a);
            line(&[(0.78, 0.5), (0.9, 0.6), (0.78, 0.7)], a);
            line(&[(0.35, 0.25), (0.65, 0.25)], s);
        }
        Icon::Trim => {
            // Two crossing lines; the piece between them (accent) is what gets removed.
            line(&[(0.3, 0.1), (0.3, 0.9)], s);
            line(&[(0.7, 0.1), (0.7, 0.9)], s);
            line(&[(0.05, 0.5), (0.3, 0.5)], s);
            line(&[(0.7, 0.5), (0.95, 0.5)], s);
            line(&[(0.4, 0.4), (0.6, 0.6)], a);
            line(&[(0.4, 0.6), (0.6, 0.4)], a);
        }
        Icon::Knife => {
            // A blade (accent) on a handle, cutting a shape.
            rect(0.1, 0.55, 0.6, 0.92, s);
            line(&[(0.15, 0.95), (0.45, 0.65)], s);
            line(&[(0.45, 0.65), (0.92, 0.12), (0.62, 0.62)], a);
        }
        Icon::ShapeBuilder => {
            // Two overlapping circles, the overlap (accent) being merged.
            p.circle_stroke(at(0.36, 0.5), r.width() * 0.3, s);
            p.circle_stroke(at(0.64, 0.5), r.width() * 0.3, s);
            line(&[(0.5, 0.28), (0.5, 0.72)], a);
            line(&[(0.42, 0.5), (0.58, 0.5)], a);
        }
        Icon::Eraser => {
            // A tilted eraser block; its rubber end (accent) wiping a line.
            line(&[(0.32, 0.43), (0.6, 0.15), (0.92, 0.47), (0.64, 0.75), (0.32, 0.43)], s);
            line(&[(0.32, 0.43), (0.12, 0.63), (0.44, 0.95), (0.64, 0.75)], a);
            line(&[(0.44, 0.95), (0.95, 0.95)], s);
        }
        Icon::Lock | Icon::Unlocked => {
            rect(0.18, 0.45, 0.82, 0.92, s);
            if icon == Icon::Lock {
                arc(0.5, 0.42, 0.22, PI, TAU, s);
            } else {
                arc(0.72, 0.38, 0.2, PI, TAU, s);
            }
            p.circle_filled(at(0.5, 0.68), r.width() * 0.06, accent);
        }
        Icon::Layers => {
            // Three stacked sheets, the top one (accent) current.
            closed(&[(0.5, 0.52), (0.95, 0.72), (0.5, 0.92), (0.05, 0.72)], s);
            closed(&[(0.5, 0.32), (0.95, 0.52), (0.5, 0.72), (0.05, 0.52)], s);
            closed(&[(0.5, 0.1), (0.95, 0.3), (0.5, 0.5), (0.05, 0.3)], a);
        }
        Icon::Move => {
            // Four-way arrows.
            line(&[(0.5, 0.06), (0.5, 0.94)], s);
            line(&[(0.06, 0.5), (0.94, 0.5)], s);
            arrow_head((0.5, 0.05), -PI / 2.0, a);
            arrow_head((0.5, 0.95), PI / 2.0, a);
            arrow_head((0.05, 0.5), PI, a);
            arrow_head((0.95, 0.5), 0.0, a);
        }
        Icon::Console => {
            // A terminal window with a prompt.
            rect(0.05, 0.12, 0.95, 0.88, s);
            line(&[(0.2, 0.35), (0.38, 0.5), (0.2, 0.65)], a);
            line(&[(0.47, 0.68), (0.75, 0.68)], a);
        }
        Icon::Tips => {
            // A light bulb.
            arc(0.5, 0.4, 0.3, PI * 0.8, PI * 2.2, s);
            line(&[(0.26, 0.58), (0.36, 0.75), (0.64, 0.75), (0.74, 0.58)], s);
            line(&[(0.38, 0.86), (0.62, 0.86)], s);
            line(&[(0.43, 0.96), (0.57, 0.96)], s);
            line(&[(0.5, 0.48), (0.5, 0.66)], a);
        }
        Icon::Laser => {
            // A laser head with its beam (accent) on the work.
            rect(0.3, 0.05, 0.7, 0.42, s);
            line(&[(0.4, 0.42), (0.45, 0.55), (0.55, 0.55), (0.6, 0.42)], s);
            line(&[(0.5, 0.55), (0.5, 0.85)], a);
            line(&[(0.1, 0.92), (0.9, 0.92)], s);
            line(&[(0.35, 0.82), (0.25, 0.75)], a);
            line(&[(0.65, 0.82), (0.75, 0.75)], a);
        }
        Icon::Library => {
            // Books on a shelf.
            rect(0.08, 0.18, 0.28, 0.88, s);
            rect(0.32, 0.08, 0.52, 0.88, s);
            closed(&[(0.6, 0.22), (0.78, 0.17), (0.94, 0.84), (0.76, 0.89)], a);
            line(&[(0.36, 0.25), (0.48, 0.25)], s);
        }
        Icon::Art => {
            // A painter's palette.
            arc(0.5, 0.5, 0.44, PI * 0.35, PI * 2.15, s);
            line(&[(0.66, 0.92), (0.6, 0.72), (0.78, 0.66), (0.94, 0.62)], s);
            for (x, y) in [(0.3, 0.38), (0.5, 0.26), (0.7, 0.38), (0.3, 0.62)] {
                p.circle_filled(at(x, y), r.width() * 0.07, accent);
            }
        }
        Icon::Shape => {
            // Sliders: a shape's settings.
            for (y, k) in [(0.22, 0.3), (0.5, 0.68), (0.78, 0.45)] {
                line(&[(0.08, y), (0.92, y)], s);
                p.circle_filled(at(k, y), r.width() * 0.09, accent);
            }
        }
        Icon::Variables => {
            // {x}
            line(&[(0.28, 0.08), (0.18, 0.12), (0.18, 0.42), (0.06, 0.5), (0.18, 0.58), (0.18, 0.88), (0.28, 0.92)], s);
            line(&[(0.72, 0.08), (0.82, 0.12), (0.82, 0.42), (0.94, 0.5), (0.82, 0.58), (0.82, 0.88), (0.72, 0.92)], s);
            line(&[(0.36, 0.34), (0.64, 0.66)], a);
            line(&[(0.64, 0.34), (0.36, 0.66)], a);
        }
        Icon::History => {
            // A clock with a back arrow.
            arc(0.5, 0.5, 0.42, -PI * 0.75, PI * 1.05, s);
            arrow_head((0.2, 0.2), -PI * 0.3, s);
            line(&[(0.5, 0.25), (0.5, 0.52), (0.7, 0.62)], a);
        }
    }
}
