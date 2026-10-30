//! Start-up safeguards: a step-by-step start-up log, a check for a copy that's already running,
//! and a check that the saved window position is still on a screen.
//!
//! A release build has no console, so if start-up stops early (or the window opens somewhere it
//! can't be seen) there would otherwise be nothing to go on.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

fn data_dir() -> PathBuf {
    eframe::storage_dir("BurnCraft").unwrap_or_else(std::env::temp_dir)
}

fn log_path() -> PathBuf {
    data_dir().join("startup.log")
}

static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Line written just before the GPU renderer starts, and the one written once a frame is shown.
const OPENING_GPU: &str = "opening the window (Wgpu renderer)";
const FIRST_FRAME_NOTE: &str = "first frame drawn: the window is open";

/// Marker file: the GPU renderer crashed on this PC, so use OpenGL from now on.
fn opengl_marker() -> PathBuf {
    data_dir().join("use-opengl")
}

/// Whether to skip the GPU renderer: it crashed here before (remembered), or the previous start
/// died while it was starting up (the log ends at that step). Call before [`begin`].
pub fn prefer_opengl() -> bool {
    if opengl_marker().exists() {
        return true;
    }
    let Ok(previous) = std::fs::read_to_string(log_path()) else { return false };
    let died_starting_gpu = previous.lines().last().is_some_and(|l| l.ends_with(OPENING_GPU)) && !previous.contains(FIRST_FRAME_NOTE);
    if died_starting_gpu {
        let _ = std::fs::write(opengl_marker(), "The GPU renderer crashed while starting on this PC; BurnCraft uses OpenGL. Delete this file to try the GPU renderer again.\n");
    }
    died_starting_gpu
}

/// Starts a fresh start-up log for this run.
pub fn begin() {
    STARTED.get_or_init(Instant::now);
    let _ = std::fs::create_dir_all(data_dir());
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let _ = std::fs::write(log_path(), format!("BurnCraft {} starting (unix time {stamp})\n", env!("CARGO_PKG_VERSION")));
}

/// Adds a line to the start-up log, with the time since start.
pub fn note(what: &str) {
    let t = STARTED.get().map_or(0.0, |s| s.elapsed().as_secs_f64());
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(log_path()) {
        let _ = writeln!(f, "{t:7.2}s  {what}");
    }
}

static FIRST_FRAME: AtomicBool = AtomicBool::new(false);

/// Called every frame; logs the first one (the window is up and drawing).
pub fn frame_shown() {
    if !FIRST_FRAME.swap(true, Ordering::Relaxed) {
        note(FIRST_FRAME_NOTE);
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
        fn SetUnhandledExceptionFilter(filter: Option<unsafe extern "system" fn(*const ExceptionPointers) -> i32>) -> *mut c_void;
        fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
    }

    #[repr(C)]
    pub struct ExceptionRecord {
        code: u32,
        flags: u32,
        next: *const ExceptionRecord,
        address: *const c_void,
        parameter_count: u32,
        information: [usize; 15],
    }

    #[repr(C)]
    pub struct ExceptionPointers {
        record: *const ExceptionRecord,
        context: *const c_void,
    }

    /// The DLL or exe that contains `address`.
    fn module_of(address: *const c_void) -> String {
        const FROM_ADDRESS: u32 = 0x4;
        const UNCHANGED_REFCOUNT: u32 = 0x2;
        let mut module = std::ptr::null_mut();
        // SAFETY: looks up the module containing an address; the handle isn't kept.
        if unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, address, &mut module) } == 0 {
            return "unknown module".into();
        }
        let mut name = [0u16; 512];
        // SAFETY: writes at most `name.len()` characters.
        let n = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
        String::from_utf16_lossy(&name[..n.min(name.len())])
    }

    fn describe(code: u32) -> &'static str {
        match code {
            0xC000_0005 => "access violation",
            0xC000_001D => "illegal instruction (this CPU may be too old)",
            0xC000_00FD => "stack overflow",
            0xC000_0135 => "a required DLL was not found",
            0xC000_0139 => "a required function is missing from a DLL",
            0xC000_0409 => "fast fail (security check)",
            0xC06D_007E => "a delay-loaded DLL was not found",
            0xC06D_007F => "a function is missing from a delay-loaded DLL",
            _ => "native exception",
        }
    }

    unsafe extern "system" fn on_crash(info: *const ExceptionPointers) -> i32 {
        // SAFETY: Windows passes valid exception pointers to the filter.
        let (code, address) = unsafe { info.as_ref().and_then(|i| i.record.as_ref()).map_or((0, std::ptr::null()), |r| (r.code, r.address)) };
        let text = format!("Windows error 0x{code:08X} ({}) at {address:p} in {}", describe(code), module_of(address));
        super::native_crash(&text);
        0 // EXCEPTION_CONTINUE_SEARCH: let Windows end the process.
    }

    /// Reports crashes that happen outside Rust (graphics drivers, system DLLs), which the panic
    /// hook never sees.
    pub fn install_crash_handler() {
        // SAFETY: registers a process-wide filter function with a matching signature.
        unsafe { SetUnhandledExceptionFilter(Some(on_crash)) };
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
        fn MessageBoxW(owner: *mut c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Windows' own message box: always shown, on top, and waits for an answer. Returns whether
    /// Yes (or OK) was chosen.
    pub fn message_box(title: &str, text: &str, warning: bool, yes_no: bool) -> bool {
        const MB_YESNO: u32 = 0x4;
        const MB_ICONERROR: u32 = 0x10;
        const MB_ICONWARNING: u32 = 0x30;
        const MB_SETFOREGROUND: u32 = 0x1_0000;
        const MB_TOPMOST: u32 = 0x4_0000;
        const IDOK: i32 = 1;
        const IDYES: i32 = 6;
        let kind = (if yes_no { MB_YESNO } else { 0 }) | (if warning { MB_ICONWARNING } else { MB_ICONERROR }) | MB_SETFOREGROUND | MB_TOPMOST;
        let (text, title) = (wide(text), wide(title));
        // SAFETY: valid NUL-terminated strings; no owner window.
        let answer = unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), kind) };
        answer == IDYES || answer == IDOK
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;

    /// Claims the "BurnCraft is running" marker. False if another copy already holds it. The
    /// marker is released by Windows when this process ends, however it ends.
    pub fn claim_single_instance() -> bool {
        let name: Vec<u16> = "Local\\BurnCraft.Running".encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: a valid NUL-terminated name; the handle is deliberately kept for the process's life.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        // SAFETY: reads this thread's last error, set by the call above.
        !handle.is_null() && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
    }

    /// The desktop spanning all monitors: (left, top, width, height) in pixels.
    pub fn virtual_screen() -> (f64, f64, f64, f64) {
        // SAFETY: plain metric queries (SM_XVIRTUALSCREEN … SM_CYVIRTUALSCREEN).
        unsafe { (GetSystemMetrics(76) as f64, GetSystemMetrics(77) as f64, GetSystemMetrics(78) as f64, GetSystemMetrics(79) as f64) }
    }
}

/// Shows a message that works even before (or without) a window. Returns whether Yes was chosen.
pub fn message_box(title: &str, text: &str, warning: bool, yes_no: bool) -> bool {
    #[cfg(windows)]
    {
        win::message_box(title, text, warning, yes_no)
    }
    #[cfg(not(windows))]
    {
        let level = if warning { rfd::MessageLevel::Warning } else { rfd::MessageLevel::Error };
        let buttons = if yes_no { rfd::MessageButtons::YesNo } else { rfd::MessageButtons::Ok };
        let r = rfd::MessageDialog::new().set_level(level).set_title(title).set_description(text).set_buttons(buttons).show();
        matches!(r, rfd::MessageDialogResult::Yes | rfd::MessageDialogResult::Ok)
    }
}

/// Logs a crash outside Rust and tells the user, from inside the crash handler.
fn native_crash(text: &str) {
    note(&format!("CRASHED: {text}"));
    let path = data_dir().join("crash.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "--- BurnCraft {} native crash\n{text}\n", env!("CARGO_PKG_VERSION"));
    }
    let hint = if text.contains("Wgpu") || !FIRST_FRAME.load(Ordering::Relaxed) {
        "\n\nIf this happened while BurnCraft was opening, it will use OpenGL graphics next time. Please start it again."
    } else {
        ""
    };
    message_box("BurnCraft crashed", &format!("BurnCraft crashed.\n\n{text}{hint}\n\nDetails were saved to:\n{}", path.display()), false, false);
}

/// Reports crashes in native code (drivers, system DLLs) as well as Rust panics.
pub fn install_crash_handler() {
    #[cfg(windows)]
    win::install_crash_handler();
}

/// False if another copy of BurnCraft is already running.
pub fn claim_single_instance() -> bool {
    #[cfg(windows)]
    {
        win::claim_single_instance()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Reads the window's saved top-left corner (pixels) from the settings file, if there is one.
fn saved_window_position() -> Option<(f64, f64)> {
    let text = std::fs::read_to_string(data_dir().join("app.ron")).ok()?;
    let at = text.find("outer_position_pixels:Some((x:")? + "outer_position_pixels:Some((x:".len();
    let rest = &text[at..];
    let x: f64 = rest[..rest.find(',')?].trim().parse().ok()?;
    let rest = &rest[rest.find("y:")? + 2..];
    let y: f64 = rest[..rest.find(')')?].trim().parse().ok()?;
    Some((x, y))
}

/// Whether the saved window position is somewhere on the current screens. A window closed while
/// minimised is saved far off screen (around -32000), and a second monitor may have gone; restoring
/// either would open BurnCraft where it can't be seen. True when nothing is saved.
pub fn saved_window_is_visible() -> bool {
    let Some((x, y)) = saved_window_position() else { return true };
    #[cfg(windows)]
    {
        let (left, top, width, height) = win::virtual_screen();
        // At least the top-left 100 × 50 pixels of the title bar must be on the desktop.
        x >= left - 50.0 && y >= top - 10.0 && x + 100.0 <= left + width && y + 50.0 <= top + height
    }
    #[cfg(not(windows))]
    {
        x > -10_000.0 && y > -10_000.0
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn single_instance_marker() {
        // The first claim in this process succeeds; a second one sees it's taken.
        assert!(super::claim_single_instance());
        #[cfg(windows)]
        assert!(!super::claim_single_instance());
    }
}
