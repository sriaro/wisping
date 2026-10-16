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
mod views;
mod winui;

slint::include_modules!();

use engine::{cleaner, registry, startup};
use slint::ComponentHandle;

fn report(file: &str) {
    let mut out = String::new();
    for mut t in cleaner::targets() {
        cleaner::scan(&mut t);
        out.push_str(&format!("CLEAN {} {} {}{}\n", t.id, t.count, engine::data::format_size(t.bytes), if t.needs_admin { " admin" } else { "" }));
    }
    for i in registry::run() {
        out.push_str(&format!("REG {} {} -> {}\n", i.kind, i.display(), i.detail));
    }
    for e in startup::list() {
        out.push_str(&format!("START {:?} {} | {} | {} | enabled={} missing={} system={} {} {}\n", e.kind, e.name, e.command, e.publisher, e.enabled, e.missing, e.system, e.detail, e.key_path));
    }
    let _ = std::fs::write(file, out);
}

fn disk_report(root: &str, file: &str) {
    use engine::diskmap::{self, dups, owners};
    use std::time::Instant;
    let mut out = String::new();
    let t = Instant::now();
    let progress = diskmap::Progress::default();
    let tree = diskmap::scan(std::path::Path::new(root), &progress);
    out.push_str(&format!("scan {:.2}s nodes {} size {} files {}
", t.elapsed().as_secs_f64(), tree.nodes.len(), engine::data::format_size(tree.nodes[0].size), tree.nodes[0].files));
    let t = Instant::now();
    let o = owners::build(&tree);
    out.push_str(&format!("owners {:.2}s count {}
", t.elapsed().as_secs_f64(), o.list.len()));
    for x in o.list.iter().take(8) {
        out.push_str(&format!("  owner {} {} dirs {}
", x.name, engine::data::format_size(x.size), x.dirs.len()));
    }
    let t = Instant::now();
    let g = dups::find(&tree, &std::sync::atomic::AtomicBool::new(false));
    out.push_str(&format!("dups {:.2}s groups {} wasted {}
", t.elapsed().as_secs_f64(), g.len(), engine::data::format_size(g.iter().map(|x| x.wasted()).sum())));
    for x in g.iter().take(5) {
        out.push_str(&format!("  dup {} x{} {}
", engine::data::format_size(x.size), x.members.len(), tree.path_of(x.members[0]).display()));
    }
    for c in tree.sorted_children(0).into_iter().take(10) {
        let n = &tree.nodes[c as usize];
        out.push_str(&format!("  top {} {} {:?}
", tree.name(n), engine::data::format_size(n.size), n.kind));
    }
    let _ = std::fs::write(file, out);
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let text = info.to_string();
        engine::data::log(&format!("panic {}", text));
        winui::fatal("Wisp", &text);
    }));
}

fn select_backend() {
    let offscreen = |attrs: slint::winit_030::winit::window::WindowAttributes| attrs.with_position(slint::winit_030::winit::dpi::PhysicalPosition::new(-32000, -32000));
    let force_software = std::env::var("WISP_RENDERER").is_ok_and(|v| v == "software");
    let gpu = if force_software { Err(slint::PlatformError::Other(String::new())) } else { slint::BackendSelector::new().backend_name("winit".into()).renderer_name("femtovg".into()).with_winit_window_attributes_hook(offscreen).select() };
    if gpu.is_err() {
        let _ = slint::BackendSelector::new().backend_name("winit".into()).renderer_name("software".into()).with_winit_window_attributes_hook(offscreen).select();
    }
}

fn run_app() -> Result<(), slint::PlatformError> {
    engine::settings::load();
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--selftest" {
        std::process::exit(if selftest::run(&args[2]) { 0 } else { 1 });
    }
    if args.len() >= 3 && args[1] == "--scan" {
        report(&args[2]);
        return;
    }
    if args.len() >= 4 && args[1] == "--disk" {
        disk_report(&args[2], &args[3]);
        return;
    }
    if args.len() >= 3 && args[1] == "--perf" {
        preview::perf(&args[2]);
        return;
    }
    if args.len() >= 3 && args[1] == "--glass-probe" {
        preview::glass_probe(&args[2]);
        return;
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
    if !winui::claim_instance(if relaunch { 6000 } else { 0 }) {
        winui::activate_existing();
        return;
    }
    if let Err(e) = run_app() {
        engine::data::log(&format!("fatal {}", e));
        winui::fatal("Wisp", &e.to_string());
    }
}
