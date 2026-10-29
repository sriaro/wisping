#![windows_subsystem = "windows"]

mod app;
mod engine;
mod fonts;
mod geo;
mod i18n;
mod i18n_zh;
mod motion;
mod pages;
mod preview;
mod selftest;
mod silent;
mod views;
mod winui;

slint::include_modules!();

use slint::ComponentHandle;

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let text = info.to_string();
        engine::data::log(&format!("panic {}", text));
        winui::fatal("Wisping", &text);
    }));
}

fn select_backend() {
    let offscreen = |attrs: slint::winit_030::winit::window::WindowAttributes| attrs.with_position(slint::winit_030::winit::dpi::PhysicalPosition::new(-32000, -32000));
    let force_software = std::env::var("WISPING_RENDERER").is_ok_and(|v| v == "software");
    let gpu = if force_software { Err(slint::PlatformError::Other(String::new())) } else { slint::BackendSelector::new().backend_name("winit".into()).renderer_name("femtovg".into()).with_winit_window_attributes_hook(offscreen).select() };
    if gpu.is_err() {
        let _ = slint::BackendSelector::new().backend_name("winit".into()).renderer_name("software".into()).with_winit_window_attributes_hook(offscreen).select();
    }
}

fn run_app() -> Result<(), slint::PlatformError> {
    engine::settings::load();
    engine::data::harden_default();
    select_backend();
    fonts::install();
    let ui = MainWindow::new()?;
    app::setup(&ui);
    app::restore_state(&ui);
    ui.show()?;
    app::bind_window(&ui);
    app::style_window_later(&ui);
    let result = ui.run();
    app::save_state(&ui);
    result
}

fn harden_dll_search() {
    use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LOAD_LIBRARY_SEARCH_USER_DIRS, SetDefaultDllDirectories};
    if engine::sys::is_admin() {
        let _ = unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS) };
    }
}

fn main() {
    harden_dll_search();
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1] == "--render" && engine::sys::is_admin() {
        std::process::exit(2);
    }
    if args.len() >= 2 && args[1] == "--clean" {
        std::process::exit(silent::clean());
    }
    if args.len() >= 3 && args[1] == "--selftest" {
        std::process::exit(if selftest::run(&args[2]) { 0 } else { 1 });
    }
    if args.len() >= 3 && args[1] == "--render" {
        let dir = args[2].clone();
        std::panic::set_hook(Box::new({
            let dir = dir.clone();
            move |info| {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(std::path::Path::new(&dir).join("error.txt"), info.to_string());
            }
        }));
        preview::run(&dir);
        return;
    }
    install_panic_hook();
    let relaunch = args.iter().any(|a| a == "--relaunch");
    if !winui::claim_instance(if relaunch { 6000 } else { 0 }) && winui::activate_existing(5000) {
        return;
    }
    if let Err(e) = run_app() {
        engine::data::log(&format!("fatal {}", e));
        winui::fatal("Wisping", &e.to_string());
    }
}
