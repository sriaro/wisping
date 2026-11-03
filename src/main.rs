#![windows_subsystem = "windows"]

mod app;
mod engine;
mod fonts;
mod geo;
mod i18n;
mod i18n_zh;
mod motion;
mod notice;
mod pages;
mod silent;
mod views;
mod winui;

slint::include_modules!();

use slint::ComponentHandle;

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let text = info.to_string();
        engine::data::log(&format!("panic {}", text));
        if !app::report_panic(text.clone()) {
            notice::fatal(&text);
        }
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
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let relaunch = match args.as_slice() {
        [] => false,
        [arg] if arg == "--relaunch" => true,
        [arg] if arg == "--clean" => std::process::exit(silent::clean()),
        _ => std::process::exit(2),
    };
    install_panic_hook();
    if !winui::claim_instance(if relaunch { 6000 } else { 0 }) && winui::activate_existing(5000) {
        return;
    }
    if let Err(e) = run_app() {
        engine::data::log(&format!("fatal {}", e));
        notice::fatal(&e.to_string());
    }
}
