// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod art;
mod camera;
mod canvas;
mod commands;
mod units;
mod upload;
mod device;
mod device_wizard;
mod diagnostics;
mod dialogs;
mod editbar;
mod grbl_config;
mod icons;
mod toolbar;
mod tools;
mod laser;
mod panels;
mod params;
mod pixscan;
mod queue;
mod recovery;
mod remote;
mod startup;

/// Where crash reports go: next to the saved settings (%APPDATA%\BurnCraft\data on Windows).
fn crash_log_path() -> std::path::PathBuf {
    eframe::storage_dir("BurnCraft").unwrap_or_else(std::env::temp_dir).join("crash.log")
}

/// Appends `text` to the crash log and shows it in a message box. A release build has no console,
/// so without this a failure at startup would close the program without a word.
fn report(title: &str, text: &str) {
    let path = crash_log_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let entry = format!("--- BurnCraft {} (unix time {stamp})\n{title}\n{text}\n\n", env!("CARGO_PKG_VERSION"));
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| std::io::Write::write_all(&mut f, entry.as_bytes()));
    eprintln!("{title}: {text}");
    startup::note(&format!("{title} (details in {})", path.display()));
    startup::message_box("BurnCraft", &format!("{title}\n\n{text}\n\nThis was saved to:\n{}", path.display()), false, false);
}

/// Picks a graphics adapter for the DirectX 12 / Vulkan renderer, refusing software ones
/// (Windows' "Microsoft Basic Render Driver" draws every frame on the processor, which makes
/// BurnCraft use most of a core). With no hardware adapter the renderer fails to start, and
/// BurnCraft tries OpenGL, which usually reaches the graphics chip through its own driver.
fn hardware_adapter(adapters: &[eframe::wgpu::Adapter], surface: Option<&eframe::wgpu::Surface<'_>>) -> Result<eframe::wgpu::Adapter, String> {
    use eframe::wgpu::DeviceType;
    let rank = |t: DeviceType| match t {
        DeviceType::DiscreteGpu => 0,
        DeviceType::IntegratedGpu => 1,
        DeviceType::VirtualGpu | DeviceType::Other => 2,
        DeviceType::Cpu => 3,
    };
    adapters
        .iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .filter(|a| a.get_info().device_type != DeviceType::Cpu)
        .min_by_key(|a| rank(a.get_info().device_type))
        .cloned()
        .ok_or_else(|| {
            let found: Vec<String> = adapters.iter().map(|a| format!("{} ({:?})", a.get_info().name, a.get_info().device_type)).collect();
            format!("no hardware graphics adapter for DirectX 12 / Vulkan (found: {})", if found.is_empty() { "none".into() } else { found.join(", ") })
        })
}

/// `software_ok` lets the DirectX 12 / Vulkan renderer use a software adapter (the last resort).
fn options(renderer: eframe::Renderer, persist_window: bool, software_ok: bool) -> eframe::NativeOptions {
    // Drawn by build.rs.
    let icon = eframe::egui::IconData { rgba: include_bytes!(concat!(env!("OUT_DIR"), "/icon-256.rgba")).to_vec(), width: 256, height: 256 };
    let selector: Option<eframe::egui_wgpu::NativeAdapterSelectorMethod> = if software_ok { None } else { Some(std::sync::Arc::new(hardware_adapter)) };
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("BurnCraft")
            .with_icon(icon)
            .with_inner_size([1500.0, 950.0])
            .with_min_inner_size([1000.0, 650.0])
            .with_drag_and_drop(true),
        renderer,
        persist_window,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(eframe::egui_wgpu::WgpuSetupCreateNew {
                native_adapter_selector: selector,
                ..eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn run(renderer: eframe::Renderer, persist_window: bool, software_ok: bool) -> eframe::Result {
    startup::note(&format!("opening the window ({renderer:?} renderer{})", if software_ok { ", software drawing allowed" } else { "" }));
    eframe::run_native(
        "BurnCraft",
        options(renderer, persist_window, software_ok),
        Box::new(|cc| {
            startup::note("window created; loading settings and fonts");
            let app = app::BurnCraftApp::new(cc);
            startup::note("ready");
            Ok(Box::new(app))
        }),
    )
}

fn main() {
    // Decide before the log is overwritten: did the GPU renderer kill the last start?
    let prefer_opengl = startup::prefer_opengl();
    startup::begin();
    startup::install_crash_handler();
    if prefer_opengl {
        startup::note("using OpenGL: the GPU renderer crashed while starting on this PC before");
    }
    // Support/test switch: crash in native code on purpose, to check crashes get reported.
    if std::env::var("BURNCRAFT_TEST_CRASH").as_deref() == Ok("native") {
        startup::note("crashing on purpose (BURNCRAFT_TEST_CRASH)");
        // SAFETY: deliberately invalid; this is the point of the test switch.
        unsafe { std::ptr::read_volatile(std::ptr::dangling::<u8>().wrapping_sub(1)) };
    }
    // On Windows the GPU renderer uses DirectX 12 only. Vulkan pulls in add-ons ("layers") from
    // overlays and capture tools, a common cause of crashes at start-up.
    if cfg!(windows) && std::env::var_os("WGPU_BACKEND").is_none() {
        // SAFETY: still single-threaded; nothing else reads the environment yet.
        unsafe { std::env::set_var("WGPU_BACKEND", "dx12") };
    }
    std::panic::set_hook(Box::new(|info| {
        // The camera thread catches its own panics and shows them as a camera error.
        if std::thread::current().name() == Some(bc_camera::CAMERA_THREAD) {
            startup::note(&format!("camera: {info}"));
            return;
        }
        let place = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let message = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into());
        report("BurnCraft crashed.", &format!("{message}{place}\n\n{}", std::backtrace::Backtrace::force_capture()));
    }));
    if !startup::claim_single_instance() {
        startup::note("another copy is already running");
        let start_anyway = startup::message_box(
            "BurnCraft",
            "BurnCraft is already running.\n\nIt may be minimised or off the edge of the screen: look for it on the taskbar, \
             or end \"burncraft.exe\" in Task Manager (Ctrl+Shift+Esc).\n\nStart another copy anyway?",
            true,
            true,
        );
        startup::note(if start_anyway { "user chose to start another copy" } else { "user chose not to start another copy" });
        if !start_anyway {
            return;
        }
    }
    let persist_window = startup::saved_window_is_visible();
    if !persist_window {
        startup::note("the saved window position is off screen; opening in the default position");
    }
    // The GPU renderer (DirectX 12 / Vulkan) is the default; OpenGL works on older graphics chips,
    // virtual machines and remote desktop. Set BURNCRAFT_RENDERER=gl to go straight to OpenGL.
    let force_gl = std::env::var("BURNCRAFT_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("gl") || v.eq_ignore_ascii_case("opengl"));
    // Order: DirectX 12 / Vulkan on a hardware adapter, then OpenGL, then (so BurnCraft still
    // opens on a PC with no working graphics driver) DirectX 12 / Vulkan drawing in software.
    let software = |earlier: String| {
        startup::note(&format!("{earlier}; using software drawing (slow)"));
        run(eframe::Renderer::Wgpu, persist_window, true).map_err(|last| format!("{earlier}\nSoftware: {last}"))
    };
    let result = if force_gl || prefer_opengl {
        run(eframe::Renderer::Glow, persist_window, false).map_err(|e| format!("OpenGL: {e}")).or_else(software)
    } else {
        run(eframe::Renderer::Wgpu, persist_window, false)
            .or_else(|first| {
                startup::note(&format!("GPU renderer failed: {first}; trying OpenGL"));
                run(eframe::Renderer::Glow, persist_window, false).map_err(|second| format!("DirectX/Vulkan: {first}\nOpenGL: {second}"))
            })
            .or_else(software)
    };
    startup::note(match &result {
        Ok(()) => "closed normally",
        Err(_) => "the graphics could not start",
    });
    if let Err(e) = result {
        report(
            "BurnCraft couldn't start its graphics.",
            &format!("{e}\n\nUpdating your graphics driver usually fixes this. BurnCraft needs DirectX 12, Vulkan or OpenGL 2 or newer."),
        );
    }
}
