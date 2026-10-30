//! Notes in startup.log that help explain slowness on a user's machine: which graphics adapter
//! draws the window, and what keeps the window redrawing when nothing is happening.

use crate::startup::note;
use eframe::egui;
use std::collections::HashMap;

/// Logs the graphics adapter and renderer in use. A "Cpu" device type means software drawing,
/// which is slow and uses a lot of processor time.
pub fn log_renderer(cc: &eframe::CreationContext<'_>) {
    if let Some(rs) = &cc.wgpu_render_state {
        let i = rs.adapter.get_info();
        note(&format!("graphics: {} via {:?} ({:?}), driver {} {}", i.name, i.backend, i.device_type, i.driver, i.driver_info));
    }
    if let Some(gl) = &cc.gl {
        use eframe::glow::HasContext;
        // SAFETY: plain string queries on the context eframe created and keeps current.
        let (vendor, renderer, version) = unsafe { (gl.get_parameter_string(eframe::glow::VENDOR), gl.get_parameter_string(eframe::glow::RENDERER), gl.get_parameter_string(eframe::glow::VERSION)) };
        note(&format!("graphics: OpenGL {version}, {renderer} ({vendor})"));
    }
}

/// Watches for the window redrawing continuously while idle, and logs why.
#[derive(Default)]
pub struct FrameWatch {
    since: Option<f64>,
    idle_frames: u32,
    causes: HashMap<String, u32>,
    reports: u32,
}

/// Seconds per check, and idle frames per second above which a check is logged.
const WINDOW: f64 = 10.0;
const BUSY_FPS: f64 = 5.0;
/// Most reports per run, so the log stays small.
const MAX_REPORTS: u32 = 3;

impl FrameWatch {
    /// Call once per frame, at the end of the frame.
    pub fn frame(&mut self, ctx: &egui::Context) {
        if self.reports >= MAX_REPORTS {
            return;
        }
        let now = ctx.input(|i| i.time);
        let since = *self.since.get_or_insert(now);
        // Frames with input (moving the mouse, typing) are expected; count the others.
        let input = ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving() || i.pointer.any_down());
        if !input {
            self.idle_frames += 1;
            for cause in ctx.repaint_causes() {
                *self.causes.entry(cause.to_string()).or_default() += 1;
            }
        }
        if now - since < WINDOW {
            return;
        }
        let fps = f64::from(self.idle_frames) / (now - since);
        if fps > BUSY_FPS {
            self.reports += 1;
            let mut causes: Vec<(String, u32)> = self.causes.drain().collect();
            causes.sort_by_key(|c| std::cmp::Reverse(c.1));
            let list: Vec<String> = causes.iter().take(8).map(|(c, n)| format!("{c} ×{n}")).collect();
            let screen = ctx.content_rect().size();
            note(&format!(
                "redrawing while idle: {fps:.0} frames/s for {:.0} s; window {:.0} × {:.0} points at {:.2} pixels/point; asked for by: {}",
                now - since,
                screen.x,
                screen.y,
                ctx.pixels_per_point(),
                if list.is_empty() { "(nothing recorded)".to_string() } else { list.join("; ") }
            ));
        }
        self.since = Some(now);
        self.idle_frames = 0;
        self.causes.clear();
    }
}
