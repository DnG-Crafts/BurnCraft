//! Camera capture and alignment for overlaying the bed on the design.

pub mod detect;
pub mod homography;

pub use homography::Homography;

use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// An RGB frame.
#[derive(Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
    /// Increases with every new frame.
    pub sequence: u64,
}

/// Whether the system's camera support is installed. On Windows that's Media Foundation, which
/// "N" editions lack until the Media Feature Pack is added. BurnCraft loads it only on demand, so
/// it must be checked before any camera call.
pub fn available() -> bool {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryW(name: *const u16) -> *mut core::ffi::c_void;
        }
        ["mf.dll", "mfplat.dll", "mfreadwrite.dll"].iter().all(|dll| {
            let wide: Vec<u16> = dll.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: a valid NUL-terminated UTF-16 string; the module stays loaded for later use.
            !unsafe { LoadLibraryW(wide.as_ptr()) }.is_null()
        })
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Message for when [`available`] is false.
pub const UNAVAILABLE: &str = "Camera support isn't installed on this PC. On Windows \"N\" editions, add the Media Feature Pack (Settings > Apps > Optional features).";

/// Cameras present, as (index, name).
pub fn list_cameras() -> Vec<(u32, String)> {
    if !available() {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    {
        mac::ask_permission();
        return mac::video_devices().into_iter().enumerate().map(|(i, (_, name))| (i as u32, name)).collect();
    }
    #[allow(unreachable_code)]
    nokhwa::query(ApiBackend::Auto)
        .map(|cams| {
            cams.into_iter()
                .filter_map(|c| match c.index() {
                    CameraIndex::Index(i) => Some((*i, c.human_name())),
                    CameraIndex::String(_) => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The name of camera threads. Panics on them come from the camera library and are shown as camera
/// errors, so the app's crash report can leave them out.
pub const CAMERA_THREAD: &str = "camera";

/// A camera streaming on a background thread. The latest frame is in [`CameraStream::latest`].
pub struct CameraStream {
    latest: Arc<Mutex<Option<Frame>>>,
    error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
}

impl CameraStream {
    pub fn start(index: u32) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let error = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (l, e, s) = (latest.clone(), error.clone(), stop.clone());
        if !available() {
            *error.lock().unwrap() = Some(UNAVAILABLE.into());
            return Self { latest, error, stop };
        }
        let spawned = thread::Builder::new().name(CAMERA_THREAD.into()).spawn(move || {
            // A panic inside the camera library (see `open_camera`) ends only the camera.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| capture_loop(index, &l, &s)))
                .unwrap_or_else(|_| Err("The camera stopped unexpectedly. Try starting it again, or another camera.".into()));
            if let Err(msg) = result {
                *e.lock().unwrap() = Some(msg);
            }
        });
        if let Err(err) = spawned {
            *error.lock().unwrap() = Some(err.to_string());
        }
        Self { latest, error, stop }
    }

    /// The most recent frame, if any.
    pub fn latest(&self) -> Option<Frame> {
        self.latest.lock().unwrap().clone()
    }

    /// The most recent frame's sequence number (cheap; no copy).
    pub fn latest_sequence(&self) -> Option<u64> {
        self.latest.lock().unwrap().as_ref().map(|f| f.sequence)
    }

    /// Why the camera stopped, if it failed.
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}

impl Drop for CameraStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Which camera to open for a listed index. On macOS cameras are opened by their system ID, from
/// the same list [`list_cameras`] shows.
fn camera_index(index: u32) -> Result<CameraIndex, String> {
    #[cfg(target_os = "macos")]
    {
        mac::wait_for_permission()?;
        return mac::video_devices()
            .into_iter()
            .nth(index as usize)
            .map(|(id, _)| CameraIndex::String(id))
            .ok_or_else(|| "The camera isn't connected any more. Press ⟳ to look again.".to_string());
    }
    #[allow(unreachable_code)]
    Ok(CameraIndex::Index(index))
}

/// Opens a camera at the best size it offers, falling back to other formats if one is refused.
///
/// On macOS, nokhwa can pair a picture size with a frame rate from another of the camera's formats,
/// which macOS rejects by raising an exception (a panic here). That's caught and the next request
/// tried; the camera thread is named "camera" so the app doesn't report these as crashes.
fn open_camera(index: u32) -> Result<nokhwa::Camera, String> {
    let which = camera_index(index)?;
    let mut last = String::new();
    let requests = [
        RequestedFormatType::AbsoluteHighestResolution,
        RequestedFormatType::HighestFrameRate(30),
        RequestedFormatType::HighestFrameRate(15),
        RequestedFormatType::AbsoluteHighestFrameRate,
        RequestedFormatType::None,
    ];
    for kind in requests {
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| nokhwa::Camera::new(which.clone(), RequestedFormat::new::<RgbFormat>(kind))));
        match attempt {
            Ok(Ok(camera)) => return Ok(camera),
            Ok(Err(e)) => last = e.to_string(),
            Err(panic) => {
                last = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "the camera refused the format".into());
            }
        }
    }
    Err(format!("The camera couldn't be started: {last}"))
}

/// Makes a decoded frame's pixel data agree with its size. Some cameras (seen on macOS) report one
/// size but deliver a row more or less; the rest of BurnCraft relies on width × height × 3 bytes.
/// Whole extra or missing rows set the height; anything else is trimmed, or dropped if too short.
fn fit_frame(width: usize, height: usize, mut rgb: Vec<u8>) -> Option<(usize, usize, Vec<u8>)> {
    let row = width * 3;
    if width == 0 || rgb.len() < row {
        return None;
    }
    let rows = rgb.len() / row;
    rgb.truncate(rows * row);
    // Trust the reported height when the data covers it; otherwise use the rows actually there.
    let height = if rows >= height { height } else { rows };
    rgb.truncate(height * row);
    Some((width, height, rgb))
}

fn capture_loop(index: u32, latest: &Mutex<Option<Frame>>, stop: &AtomicBool) -> Result<(), String> {
    let mut camera = open_camera(index)?;
    camera.open_stream().map_err(|e| e.to_string())?;
    let mut sequence = 0;
    while !stop.load(Ordering::Relaxed) {
        let buffer = camera.frame().map_err(|e| e.to_string())?;
        let image = buffer.decode_image::<RgbFormat>().map_err(|e| e.to_string())?;
        let Some((width, height, rgb)) = fit_frame(image.width() as usize, image.height() as usize, image.into_raw()) else {
            continue;
        };
        sequence += 1;
        *latest.lock().unwrap() = Some(Frame { width, height, rgb, sequence });
        // ~10 frames per second is plenty for an overlay.
        thread::sleep(Duration::from_millis(100));
    }
    let _ = camera.stop_stream();
    Ok(())
}

/// macOS: camera permission, and the list of video devices.
///
/// nokhwa's own macOS listing only finds USB cameras on recent macOS versions (it asks for a
/// device type older versions don't know), and a camera can't be opened at all until the user has
/// allowed it. So BurnCraft asks for permission, lists every video device with AVFoundation's
/// `devicesWithMediaType:`, and opens cameras by their unique ID.
#[cfg(target_os = "macos")]
mod mac {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::CStr;
    use std::os::raw::c_char;
    use std::time::{Duration, Instant};

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        static AVMediaTypeVideo: *mut Object;
    }

    /// Asks the user for camera access, if they haven't been asked yet. Returns at once; macOS
    /// shows its own prompt.
    pub fn ask_permission() {
        if !nokhwa::nokhwa_check() {
            nokhwa::nokhwa_initialize(|_| {});
        }
    }

    /// Waits (up to a minute) for the user to answer the permission prompt.
    pub fn wait_for_permission() -> Result<(), String> {
        ask_permission();
        let start = Instant::now();
        while !nokhwa::nokhwa_check() {
            if start.elapsed() > Duration::from_secs(60) {
                return Err("BurnCraft isn't allowed to use the camera. Allow it in System Settings > Privacy & Security > Camera (when BurnCraft is started from Terminal, allow Terminal), then start the camera again.".into());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    }

    /// Every video device, as (unique ID, name), built-in and USB alike.
    pub fn video_devices() -> Vec<(String, String)> {
        // SAFETY: plain AVFoundation class methods and NSArray / NSString accessors; every object
        // is checked for nil before use and nothing is retained past this function.
        unsafe {
            let devices: *mut Object = msg_send![class!(AVCaptureDevice), devicesWithMediaType: AVMediaTypeVideo];
            if devices.is_null() {
                return Vec::new();
            }
            let count: usize = msg_send![devices, count];
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                let device: *mut Object = msg_send![devices, objectAtIndex: i];
                if device.is_null() {
                    continue;
                }
                let id: *mut Object = msg_send![device, uniqueID];
                let name: *mut Object = msg_send![device, localizedName];
                let id = string(id);
                if !id.is_empty() {
                    out.push((id, string(name)));
                }
            }
            out
        }
    }

    /// An NSString as a Rust string ("" for nil).
    unsafe fn string(s: *mut Object) -> String {
        if s.is_null() {
            return String::new();
        }
        // SAFETY: `s` is a non-nil NSString; UTF8String returns a NUL-terminated buffer it owns.
        unsafe {
            let p: *const c_char = msg_send![s, UTF8String];
            if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fit_frame;

    #[test]
    fn frames_always_match_their_size() {
        // One row too many (as a Mac camera sent: 960 × 544 reported, 545 rows of data).
        let (w, h, rgb) = fit_frame(960, 544, vec![0; 960 * 545 * 3]).unwrap();
        assert_eq!((w, h, rgb.len()), (960, 544, 960 * 544 * 3));
        // A row short: the height follows the data.
        let (_, h, rgb) = fit_frame(4, 3, vec![0; 4 * 2 * 3 + 5]).unwrap();
        assert_eq!((h, rgb.len()), (2, 4 * 2 * 3));
        // Exact data is untouched; empty data is dropped.
        assert_eq!(fit_frame(2, 2, vec![7; 12]).unwrap().2, vec![7; 12]);
        assert!(fit_frame(10, 10, vec![0; 5]).is_none());
        assert!(fit_frame(0, 10, vec![]).is_none());
    }
}
