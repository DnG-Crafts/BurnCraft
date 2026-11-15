//! Remote monitoring: a small web page on the local network showing the machine, job progress
//! and camera, with Pause / Resume / Stop. Off by default. The page itself is public, but every
//! data request and control action needs the PIN, and wrong PINs are slowed down.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server};

const PAGE: &str = include_str!("remote.html");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteSettings {
    pub enabled: bool,
    pub port: u16,
    pub pin: String,
    /// Ask for the PIN before showing the page's status and camera. Pause/Resume/Stop always need it.
    pub pin_to_view: bool,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self { enabled: false, port: 8787, pin: new_pin(), pin_to_view: true }
    }
}

/// A random 6-digit PIN.
pub fn new_pin() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    format!("{:06}", h.finish() % 1_000_000)
}

/// What the page shows; serialised to JSON for `/api/status`.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct RemoteStatus {
    pub machine: String,
    pub connected: bool,
    pub description: String,
    pub state: String,
    pub position: Option<(f64, f64)>,
    pub message: String,
    pub job_running: bool,
    pub paused: bool,
    /// Sending/running progress as a fraction, with what is happening.
    pub progress: Option<(f64, String)>,
    pub elapsed_s: Option<f64>,
    pub estimate_s: Option<f64>,
    /// The controller doesn't report when a job ends (Ruida), so "remaining" is from the estimate.
    pub end_not_reported: bool,
    pub camera: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteCmd {
    Pause,
    Resume,
    Stop,
}

#[derive(Default)]
struct Shared {
    status: String,
    camera: Option<Vec<u8>>,
}

pub struct RemoteServer {
    shared: Arc<Mutex<Shared>>,
    commands: Receiver<RemoteCmd>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub port: u16,
    pub pin: String,
    pub pin_to_view: bool,
}

impl RemoteServer {
    /// Listens on all network interfaces at `port`.
    pub fn start(port: u16, pin: String, pin_to_view: bool, ctx: egui::Context) -> io::Result<Self> {
        let server = Server::http(("0.0.0.0", port)).map_err(|e| io::Error::other(e.to_string()))?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, commands) = mpsc::channel();
        let thread = {
            let (shared, stop, pin) = (shared.clone(), stop.clone(), pin.clone());
            std::thread::Builder::new().name("remote".into()).spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(200)) {
                        handle(request, &pin, pin_to_view, &shared, &tx, &ctx);
                    }
                }
            })?
        };
        Ok(Self { shared, commands, stop, thread: Some(thread), port, pin, pin_to_view })
    }

    pub fn publish(&self, status: &RemoteStatus) {
        let json = serde_json::to_string(status).unwrap_or_default();
        self.shared.lock().unwrap().status = json;
    }

    pub fn publish_camera(&self, jpeg: Option<Vec<u8>>) {
        self.shared.lock().unwrap().camera = jpeg;
    }

    /// Commands received from the page since the last call.
    pub fn commands(&self) -> Vec<RemoteCmd> {
        self.commands.try_iter().collect()
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

/// Compares without stopping at the first difference.
fn pin_matches(given: &str, pin: &str) -> bool {
    given.len() == pin.len() && given.bytes().zip(pin.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

fn handle(request: Request, pin: &str, pin_to_view: bool, shared: &Mutex<Shared>, tx: &Sender<RemoteCmd>, ctx: &egui::Context) {
    let url = request.url().split('?').next().unwrap_or("").to_string();
    let method = request.method().clone();
    let no_store = header("Cache-Control", "no-store");
    if method == Method::Get && (url == "/" || url == "/index.html") {
        let _ = request.respond(Response::from_string(PAGE).with_header(header("Content-Type", "text/html; charset=utf-8")).with_header(no_store));
        return;
    }
    if !url.starts_with("/api/") {
        let _ = request.respond(Response::from_string("Not found").with_status_code(404));
        return;
    }
    let given = request.headers().iter().find(|h| h.field.equiv("X-Pin")).map(|h| h.value.as_str().to_string()).unwrap_or_default();
    // Watching can be open to anyone on the network; controlling the laser always needs the PIN.
    let needs_pin = pin_to_view || method != Method::Get;
    if needs_pin && !pin_matches(&given, pin) {
        std::thread::sleep(Duration::from_millis(700)); // slow down guessing
        let _ = request.respond(Response::from_string("Wrong PIN").with_status_code(401));
        return;
    }
    let command = match (&method, url.as_str()) {
        (Method::Get, "/api/status") => {
            let json = shared.lock().unwrap().status.clone();
            let _ = request.respond(Response::from_string(json).with_header(header("Content-Type", "application/json")).with_header(no_store));
            return;
        }
        (Method::Get, "/api/camera.jpg") => {
            let jpeg = shared.lock().unwrap().camera.clone();
            let _ = match jpeg {
                Some(bytes) => request.respond(Response::from_data(bytes).with_header(header("Content-Type", "image/jpeg")).with_header(no_store)),
                None => request.respond(Response::from_string("No camera").with_status_code(404)),
            };
            return;
        }
        (Method::Post, "/api/pause") => RemoteCmd::Pause,
        (Method::Post, "/api/resume") => RemoteCmd::Resume,
        (Method::Post, "/api/stop") => RemoteCmd::Stop,
        _ => {
            let _ = request.respond(Response::from_string("Not found").with_status_code(404));
            return;
        }
    };
    let _ = tx.send(command);
    ctx.request_repaint();
    let _ = request.respond(Response::empty(204));
}

/// The PC's address on the local network, for showing the page's URL.
pub fn local_ip() -> Option<std::net::IpAddr> {
    // Connecting a UDP socket sends nothing; it just picks the outgoing interface.
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:80").ok()?;
    Some(s.local_addr().ok()?.ip())
}

/// A camera frame as JPEG, at most 960 px wide.
pub fn encode_jpeg(frame: &bc_camera::Frame) -> Option<Vec<u8>> {
    let image = image::RgbImage::from_raw(frame.width as u32, frame.height as u32, frame.rgb.clone())?;
    let image = if image.width() > 960 {
        let h = (image.height() as f64 * 960.0 / image.width() as f64).round() as u32;
        image::imageops::resize(&image, 960, h.max(1), image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70).encode_image(&image).ok()?;
    Some(out)
}
