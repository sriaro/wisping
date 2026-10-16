use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
use slint::{ComponentHandle, PhysicalSize, Timer, TimerMode};

use crate::app::{self, Core};
use crate::engine::cleaner;
use crate::engine::regbackup::Hive;
use crate::engine::registry::RegIssue;
use crate::engine::startup::{StartupEntry, StartupKind};
use crate::i18n;
use crate::{AppState, MainWindow, Theme};

fn save(ui: &MainWindow, path: &Path) {
    let Ok(snap) = ui.window().take_snapshot() else { return };
    let (w, h) = (snap.width(), snap.height());
    let file = BufWriter::new(File::create(path).unwrap());
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(snap.as_bytes()).unwrap();
}

fn sample(scanned: bool) -> Core {
    let mut core = app::make_core(false);
    core.startups.clear();
    let sizes: [(&str, u64, u64, bool); 15] = [
        ("usertemp", 21_200_000_000, 48622, true),
        ("wintemp", 660_700_000, 22, true),
        ("thumb", 8_400_000, 30, true),
        ("recent", 174_900, 43, false),
        ("crash", 189_500_000, 5, true),
        ("shader", 843_000_000, 521, false),
        ("wu", 138_000_000, 6, false),
        ("do", 1_290_000_000, 6, true),
        ("logs", 67_300_000, 1, false),
        ("chrome", 0, 0, true),
        ("edge", 99_400_000, 2572, true),
        ("brave", 0, 0, true),
        ("firefox", 32_700_000, 8, true),
        ("prefetch", 10_000_000, 362, false),
        ("recycle", 0, 0, false),
    ];
    for t in core.targets.iter_mut() {
        if let Some((_, bytes, count, selected)) = sizes.iter().find(|s| s.0 == t.id) {
            t.bytes = *bytes;
            t.count = *count;
            t.selected = *selected;
            t.scanned = scanned;
        }
    }
    let mui = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\Shell\\MuiCache";
    core.issues = vec![
        issue("muicache", mui, Some("C:\\Users\\Demo\\AppData\\Local\\Temp\\setup_9f3a.exe.FriendlyAppName"), "C:\\Users\\Demo\\AppData\\Local\\Temp\\setup_9f3a.exe", true),
        issue("muicache", mui, Some("D:\\Games\\OldLauncher\\launcher.exe.ApplicationCompany"), "D:\\Games\\OldLauncher\\launcher.exe", true),
        issue("apppath", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\oldtool.exe", None, "C:\\Program Files\\OldTool\\oldtool.exe", true),
        issue("shareddll", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\SharedDLLs", Some("C:\\Program Files\\Common Files\\Legacy\\legacy32.dll"), "C:\\Program Files\\Common Files\\Legacy\\legacy32.dll", true),
        issue("uninstall", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\DemoApp", None, "Demo App 2.1 -> C:\\Program Files\\DemoApp\\uninstall.exe", false),
        RegIssue::new(Hive::Machine, "com", "SOFTWARE\\Classes\\CLSID\\{48D8EE61-D4C0-4727-8760-8F15B27ED919}", None, "C:\\Program Files (x86)\\Microsoft\\EdgeUpdate\\psmachine_64.dll"),
    ];
    core.reg_scanned = scanned;
    core.startups = vec![
        entry(StartupKind::Run, "SecurityHealth", "%windir%\\system32\\SecurityHealthSystray.exe", true, false, Hive::Machine),
        entry(StartupKind::Run, "Discord", "\"C:\\Users\\Demo\\AppData\\Local\\Discord\\Update.exe\" --processStart Discord.exe", true, false, Hive::User),
        entry(StartupKind::Run, "OldUpdater", "\"C:\\Program Files\\OldUpdater\\updater.exe\" /silent", true, true, Hive::User),
        entry(StartupKind::Folder, "Snipping Helper", "C:\\Users\\Demo\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\Snipping Helper.lnk", false, false, Hive::User),
        entry(StartupKind::Packaged, "Claude / ClaudeStartup", "Claude_pzs8sxrjxfjjc", false, false, Hive::User),
    ];
    core.all_scanned = scanned;
    core
}

fn issue(kind: &str, key: &str, value: Option<&str>, detail: &str, selected: bool) -> RegIssue {
    let mut i = RegIssue::new(Hive::User, kind, key, value, detail);
    i.selected = selected;
    i
}

fn entry(kind: StartupKind, name: &str, command: &str, enabled: bool, missing: bool, hive: Hive) -> StartupEntry {
    StartupEntry {
        kind,
        name: name.to_string(),
        command: command.to_string(),
        enabled,
        missing,
        policy: false,
        hive,
        key_path: String::new(),
        approved_sub: String::new(),
        value_name: String::new(),
        file_path: None,
        ..Default::default()
    }
}

fn set_busy_ring(ui: &MainWindow) {
    let st = ui.global::<AppState>();
    st.set_busy(true);
    st.set_ring_mode(1);
    st.set_hero_title(i18n::get("hero.busy.title").into());
    st.set_hero_text(i18n::get("hero.busy.text").into());
    st.set_ring_big("".into());
    st.set_ring_small(i18n::get("ring.busy").into());
    st.set_status(i18n::get("status.scanning").into());
}

type Step = Box<dyn Fn(&MainWindow)>;

fn scenes() -> Vec<(String, Step)> {
    let mut out: Vec<(String, Step)> = Vec::new();
    for (theme, dark) in [("light", false), ("dark", true)] {
        let base = move |ui: &MainWindow, page: i32| {
            ui.global::<Theme>().set_dark(dark);
            let st = ui.global::<AppState>();
            st.set_page(page);
            ui.global::<Theme>().set_shown_page(page);
            st.set_busy(false);
            st.set_dialog_open(false);
            st.set_toast_open(false);
            st.set_details_open(false);
            st.set_status(i18n::get("status.idle").into());
        };
        out.push((format!("{}-home-idle", theme), Box::new(move |ui| {
            base(ui, 0);
            crate::views::push_all(ui, &sample(false));
        })));
        out.push((format!("{}-home-busy", theme), Box::new(move |ui| {
            base(ui, 0);
            crate::views::push_all(ui, &sample(false));
            set_busy_ring(ui);
        })));
        for (i, name) in [(0, "home"), (1, "clean"), (2, "reg"), (3, "start")] {
            out.push((format!("{}-{}", theme, name), Box::new(move |ui| {
                base(ui, i);
                crate::views::push_all(ui, &sample(true));
            })));
        }
        out.push((format!("{}-uninstall", theme), Box::new(move |ui| {
            base(ui, 4);
            crate::pages::uninstall::preview(ui);
        })));
        out.push((format!("{}-disk", theme), Box::new(move |ui| {
            base(ui, 5);
            let root = std::env::var("PC_PREVIEW_ROOT").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from("."));
            crate::pages::disk::preview(ui, root, true);
        })));
        out.push((format!("{}-tools", theme), Box::new(move |ui| {
            base(ui, 6);
            crate::pages::tools::preview(ui);
        })));
        for (tab, name) in [(0, "backups"), (1, "backups-history"), (2, "backups-data")] {
            out.push((format!("{}-{}", theme, name), Box::new(move |ui| {
                base(ui, 7);
                ui.global::<crate::BackupsState>().set_tab(tab);
                crate::pages::backups::preview(ui);
            })));
        }
        for id in ["aimodels", "aiapps"] {
            out.push((format!("{}-details-{}", theme, id), Box::new(move |ui| {
                base(ui, 1);
                crate::views::push_all(ui, &sample(true));
                let mut t = cleaner::targets().into_iter().find(|t| t.id == id).unwrap();
                cleaner::scan(&mut t);
                let st = ui.global::<AppState>();
                let (icon, tint) = crate::views::cat_style(id);
                st.set_details_title(i18n::get(&format!("t.{}", id)).into());
                st.set_details_icon(icon.into());
                st.set_details_tint(tint);
                crate::views::fill_details(ui, &cleaner::details(&t));
                st.set_details_open(true);
            })));
        }
        out.push((format!("{}-details-unicode", theme), Box::new(move |ui| {
            base(ui, 1);
            crate::views::push_all(ui, &sample(true));
            let names = [
                "Test 😀 👍 吉 𝕏 ok",
                "Привет мир — Ёлка.txt",
                "مرحبا بالعالم.docx",
                "你好，世界 日本語のファイル.zip",
                "안녕하세요 한국어.log",
                "नमस्ते दुनिया.pdf",
                "สวัสดีชาวโลก.mp4",
                "Café Résumé Ångström Straße.png",
            ];
            let rows: Vec<cleaner::Detail> = names
                .iter()
                .enumerate()
                .map(|(i, n)| cleaner::Detail {
                    name: n.to_string(),
                    place: "C:\\Users\\用户\\AppData\\Local\\Temp\\ünïcödé".to_string(),
                    bytes: (900u64 - i as u64 * 100) << 20,
                    count: 3 + i as u64,
                    path: "C:\\Windows".to_string(),
                })
                .collect();
            let st = ui.global::<AppState>();
            st.set_details_title("Unicode names".into());
            st.set_details_icon("folder".into());
            st.set_details_tint(1);
            crate::views::fill_details(ui, &rows);
            st.set_details_open(true);
        })));
        out.push((format!("{}-dialog", theme), Box::new(move |ui| {
            base(ui, 1);
            crate::views::push_all(ui, &sample(true));
            let st = ui.global::<AppState>();
            st.set_dialog_title(i18n::get("confirm.clean.title").into());
            st.set_dialog_text(i18n::fmt("confirm.clean.text", &["22.4 GB"]).into());
            st.set_dialog_ok(i18n::get("confirm.clean.ok").into());
            st.set_dialog_open(true);
        })));
        out.push((format!("{}-toast", theme), Box::new(move |ui| {
            base(ui, 1);
            crate::views::push_all(ui, &sample(true));
            let st = ui.global::<AppState>();
            st.set_toast_text(i18n::fmt("toast.freed", &["22.4 GB", "0 B"]).into());
            st.set_toast_open(true);
        })));
    }
    let _ = cleaner::targets;
    out
}

pub fn run(dir: &str) {
    fs::create_dir_all(dir).unwrap();
    let out: PathBuf = PathBuf::from(dir);
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(std::env::var("WISP_RENDERER").unwrap_or_else(|_| "femtovg".into()).as_str().into())
        .with_winit_window_attributes_hook(|attrs| attrs.with_position(PhysicalPosition::new(-32000, -32000)).with_skip_taskbar(true))
        .select()
        .unwrap();
    crate::fonts::install();
    let ui = MainWindow::new().unwrap();
    app::install_text(&ui);
    crate::pages::install(&app::Ctx::new(ui.as_weak(), std::sync::Arc::new(std::sync::Mutex::new(Core::default()))));
    let dim = |name: &str, default: u32| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    ui.window().set_size(PhysicalSize::new(dim("PC_PREVIEW_W", 1620), dim("PC_PREVIEW_H", 1110)));
    crate::motion::sync_scale(&ui);
    ui.show().unwrap();

    let only = std::env::var("PC_PREVIEW_ONLY").unwrap_or_default();
    let steps = Rc::new(scenes().into_iter().filter(|s| s.0.contains(only.as_str())).collect::<Vec<_>>());
    let index = Rc::new(std::cell::Cell::new(0usize));
    let phase = Rc::new(std::cell::Cell::new(0u8));
    let timer = Rc::new(Timer::default());
    let weak = ui.as_weak();
    let keep = timer.clone();
    let tick = std::env::var("PC_PREVIEW_TICK").ok().and_then(|v| v.parse().ok()).unwrap_or(450u64);
    timer.start(TimerMode::Repeated, Duration::from_millis(tick), move || {
        let Some(ui) = weak.upgrade() else { return };
        let i = index.get();
        if i >= steps.len() {
            keep.stop();
            let _ = slint::quit_event_loop();
            return;
        }
        if phase.get() == 0 {
            (steps[i].1)(&ui);
            phase.set(1);
        } else {
            save(&ui, &out.join(format!("{}.png", steps[i].0)));
            index.set(i + 1);
            phase.set(0);
        }
    });
    slint::run_event_loop().unwrap();
}

pub fn glass_probe(file: &str) {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        .with_winit_window_attributes_hook(|attrs| attrs.with_position(PhysicalPosition::new(-32000, -32000)).with_skip_taskbar(true))
        .select()
        .unwrap();
    let ui = MainWindow::new().unwrap();
    app::install_text(&ui);
    ui.global::<crate::AppState>().set_glass_enabled(true);
    ui.show().unwrap();
    app::bind_window(&ui);
    let path = file.to_string();
    let weak = ui.as_weak();
    Timer::single_shot(Duration::from_millis(500), move || {
        let ok = crate::winui::apply_backdrop(false);
        let before = crate::winui::window_style().unwrap_or(0);
        crate::winui::strip_caption_buttons();
        let after = crate::winui::window_style().unwrap_or(0);
        let transparent = weak.upgrade().map(|ui| ui.window().has_active_animations()).unwrap_or(false);
        crate::winui::cloak(true);
        if let Some(ui) = weak.upgrade() {
            ui.window().set_maximized(true);
        }
        let weak = weak.clone();
        Timer::single_shot(Duration::from_millis(400), move || {
            let maxed = crate::winui::window_style().unwrap_or(0);
            if let Some(ui) = weak.upgrade() {
                ui.window().set_maximized(false);
            }
            Timer::single_shot(Duration::from_millis(400), move || {
                let back = crate::winui::window_style().unwrap_or(0);
                let _ = fs::write(&path, format!("sysmenu before: {} after: {} maximized: {} restored: {}
backdrop applied: {}
animations: {}
", before & 0x80000 != 0, after & 0x80000 != 0, maxed & 0x80000 != 0, back & 0x80000 != 0, ok, transparent));
                let _ = slint::quit_event_loop();
            });
        });
    });
    slint::run_event_loop().unwrap();
}

pub fn perf(file: &str) {
    use slint::{GraphicsAPI, RenderingState};
    use std::cell::{Cell, RefCell};
    use std::time::Instant;
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        .with_winit_window_attributes_hook(|attrs| attrs.with_position(PhysicalPosition::new(-32000, -32000)).with_skip_taskbar(true))
        .select()
        .unwrap();
    crate::fonts::install();
    let ui = MainWindow::new().unwrap();
    app::install_text(&ui);
    let core = std::sync::Arc::new(std::sync::Mutex::new(sample(true)));
    let ctx = app::Ctx::new(ui.as_weak(), core.clone());
    crate::pages::install(&ctx);
    crate::views::push_all(&ui, &core.lock().unwrap());
    ui.global::<AppState>().set_page(3);
    ui.global::<Theme>().set_shown_page(3);
    ui.window().set_size(PhysicalSize::new(1620, 1110));
    crate::motion::sync_scale(&ui);
    let t0 = Instant::now();
    let frames: Rc<RefCell<Vec<(f64, f64)>>> = Rc::new(RefCell::new(Vec::new()));
    let gl: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let begin: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    {
        let frames = frames.clone();
        let gl = gl.clone();
        let begin = begin.clone();
        ui.window()
            .set_rendering_notifier(move |state, api| match state {
                RenderingState::RenderingSetup => {
                    if let GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                        let f = get_proc_address(c"glGetString");
                        if !f.is_null() {
                            let get: extern "system" fn(u32) -> *const std::ffi::c_char = unsafe { std::mem::transmute(f) };
                            for code in [0x1F00u32, 0x1F01, 0x1F02] {
                                let p = get(code);
                                if !p.is_null() {
                                    let s = unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().to_string();
                                    gl.borrow_mut().push_str(&format!("{} | ", s));
                                }
                            }
                        } else {
                            gl.borrow_mut().push_str("glGetString unavailable");
                        }
                    }
                }
                RenderingState::BeforeRendering => begin.set(Some(Instant::now())),
                RenderingState::AfterRendering => {
                    let now = Instant::now();
                    let cost = begin.get().map(|b| (now - b).as_secs_f64() * 1000.0).unwrap_or(0.0);
                    frames.borrow_mut().push(((now - t0).as_secs_f64() * 1000.0, cost));
                    crate::motion::after_frame();
                }
                _ => {}
            })
            .unwrap();
    }
    ui.show().unwrap();
    let pages = [6, 7, 0, 3, 2, 6, 3, 7];
    let step = Rc::new(Cell::new(0usize));
    let marks: Rc<RefCell<Vec<f64>>> = Rc::new(RefCell::new(Vec::new()));
    let timer = Rc::new(Timer::default());
    let keep = timer.clone();
    let path = file.to_string();
    timer.start(TimerMode::Repeated, Duration::from_millis(1600), move || {
        let i = step.get();
        if i < pages.len() {
            marks.borrow_mut().push(t0.elapsed().as_secs_f64() * 1000.0);
            crate::motion::begin_switch(&ctx, pages[i]);
            step.set(i + 1);
            return;
        }
        keep.stop();
        let f = frames.borrow();
        let mut out = format!("gl: {}\nscale: {}\nframes: {}\n", gl.borrow(), ui.window().scale_factor(), f.len());
        let mut gaps = Vec::new();
        let mut costs = Vec::new();
        for m in marks.borrow().iter() {
            let window: Vec<&(f64, f64)> = f.iter().filter(|x| x.0 >= *m && x.0 <= m + 1500.0).collect();
            let n = window.len();
            let span = if n > 1 { window[n - 1].0 - window[0].0 } else { 0.0 };
            for w in window.windows(2) {
                gaps.push(w[1].0 - w[0].0);
            }
            costs.extend(window.iter().map(|x| x.1));
            out.push_str(&format!("switch at {:.0} ms: {} frames over {:.0} ms ({:.0} fps)\n", m, n, span, if span > 0.0 { (n as f64 - 1.0) * 1000.0 / span } else { 0.0 }));
        }
        let pct = |v: &mut Vec<f64>, p: f64| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if v.is_empty() { 0.0 } else { v[((v.len() - 1) as f64 * p) as usize] }
        };
        out.push_str(&format!("frame gap ms p50 {:.2} p90 {:.2} p99 {:.2} max {:.2}\n", pct(&mut gaps, 0.5), pct(&mut gaps, 0.9), pct(&mut gaps, 0.99), pct(&mut gaps, 1.0)));
        out.push_str(&format!("render cost ms p50 {:.2} p90 {:.2} p99 {:.2} max {:.2}\n", pct(&mut costs, 0.5), pct(&mut costs, 0.9), pct(&mut costs, 0.99), pct(&mut costs, 1.0)));
        for m in marks.borrow().iter() {
            out.push_str(&format!("mark {:.1}
", m));
        }
        for x in f.iter() {
            out.push_str(&format!("f {:.2} {:.2}
", x.0, x.1));
        }
        let _ = fs::write(&path, out);
        let _ = slint::quit_event_loop();
    });
    slint::run_event_loop().unwrap();
}
