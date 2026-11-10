#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod editor;
mod recording;
mod scaling;
mod sources;
mod theme;
mod tiles;

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// Written by the real program: "starting", then "ready" once the window exists, or "failed".
static START_FLAG: OnceLock<PathBuf> = OnceLock::new();

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--startup-watch") {
        watch_for_window(args.get(1).cloned().unwrap_or_default());
        return;
    }

    // A windowed program has nowhere to print, so keep a panic trail on disk.
    std::panic::set_hook(Box::new(|info| {
        let _ = append_log(&format!("panic: {info}"));
    }));
    let log = append_log("process started");
    let flag = std::env::temp_dir().join(format!("sensor_plotter-{}.start", std::process::id()));
    let _ = std::fs::write(&flag, "starting");
    let _ = START_FLAG.set(flag.clone());
    spawn_watchdog(&flag);

    // OpenGL first. DirectX startup hangs on some Windows 10 graphics drivers
    // and never returns, so an error dialog after it would never appear.
    let mut errors = Vec::new();
    match launch(eframe::Renderer::Glow, false) {
        Ok(()) => return,
        Err(err) => {
            let _ = append_log(&format!("OpenGL failed: {err}"));
            errors.push(format!("OpenGL: {err}"));
        }
    }
    match launch(eframe::Renderer::Wgpu, false) {
        Ok(()) => return,
        Err(err) => {
            let _ = append_log(&format!("DirectX failed: {err}"));
            errors.push(format!("DirectX: {err}"));
        }
    }
    match launch(eframe::Renderer::Wgpu, true) {
        Ok(()) => return,
        Err(err) => {
            let _ = append_log(&format!("software renderer failed: {err}"));
            errors.push(format!("Windows software renderer: {err}"));
        }
    }

    let _ = std::fs::write(&flag, "failed");
    message_box(&format!(
        "Sensor Plotter could not open a window on this computer.\n\n{}\n\nDetails were saved to:\n{}",
        errors.join("\n\n"),
        log.display()
    ));
}

/// Separate process so a crash or hang in the graphics driver still produces a dialog.
fn spawn_watchdog(flag: &std::path::Path) {
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = std::process::Command::new(exe).arg("--startup-watch").arg(flag).spawn();
}

fn watch_for_window(flag: String) {
    std::thread::sleep(std::time::Duration::from_secs(12));
    let state = std::fs::read_to_string(&flag).unwrap_or_default();
    if state.trim() == "ready" || state.trim() == "failed" {
        return;
    }
    let log = append_log("no window after 12 seconds");
    message_box(&format!(
        "Sensor Plotter did not open a window.\n\n\
         The graphics driver on this computer is not responding.\n\n\
         Details were saved to:\n{}",
        log.display()
    ));
}

/// Starts the window and returns when it closes.
/// Graphics failures come back as errors instead of ending the process.
fn launch(renderer: eframe::Renderer, software: bool) -> Result<(), String> {
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        eframe::run_native(
            "Sensor Plotter",
            native_options(renderer, software),
            Box::new(|cc| {
                let app = app::SensorPlotterApp::new(cc);
                if let Some(flag) = START_FLAG.get() {
                    let _ = std::fs::write(flag, "ready");
                }
                Ok(Box::new(app))
            }),
        )
    }));
    match run {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.to_string()),
        Err(payload) => Err(panic_message(payload)),
    }
}

fn native_options(renderer: eframe::Renderer, software: bool) -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(concat!("Sensor Plotter ", env!("CARGO_PKG_VERSION")))
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([720.0, 480.0])
            .with_position(egui::pos2(80.0, 60.0)),
        renderer,
        ..Default::default()
    };
    // Ask the graphics card only for what it actually supports. The default
    // request needs an 8192-pixel texture, which older cards reject, and the
    // program then exits before any window appears.
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.power_preference = eframe::wgpu::PowerPreference::LowPower;
        setup.device_descriptor = Arc::new(|adapter| eframe::wgpu::DeviceDescriptor {
            label: Some("egui wgpu device"),
            required_limits: adapter.limits(),
            ..Default::default()
        });
        if software {
            setup.native_adapter_selector = Some(Arc::new(|adapters, _surface| {
                adapters
                    .iter()
                    .find(|adapter| adapter.get_info().device_type == eframe::wgpu::DeviceType::Cpu)
                    .or_else(|| adapters.first())
                    .cloned()
                    .ok_or_else(|| "No graphics adapter, including the Windows software renderer.".to_string())
            }));
        }
    }
    options
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast_ref::<&str>() {
        Some(msg) => (*msg).to_string(),
        None => match payload.downcast_ref::<String>() {
            Some(msg) => msg.clone(),
            None => "the graphics driver crashed while opening the window".to_string(),
        },
    }
}

/// Appends to a log beside the exe, or in the temp folder if that is not writable.
fn append_log(text: &str) -> PathBuf {
    let line = format!("[{}] {text}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        paths.push(dir.join("sensor_plotter.log"));
    }
    paths.push(std::env::temp_dir().join("sensor_plotter.log"));
    for path in &paths {
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(line.as_bytes());
            return path.clone();
        }
    }
    paths.pop().unwrap_or_else(|| PathBuf::from("sensor_plotter.log"))
}

fn message_box(text: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }
    let text = wide(text);
    let title = wide("Sensor Plotter");
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), 0x0000_0010);
    }
}

unsafe extern "system" {
    fn MessageBoxW(hwnd: *mut std::ffi::c_void, text: *const u16, caption: *const u16, utype: u32) -> i32;
}
