use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::winit_030::winit::platform::windows::WindowExtWindows;
use slint::winit_030::winit::window::ResizeDirection;
use slint::winit_030::{winit, WinitWindowAccessor};
use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel, Weak};

use crate::engine::cleaner::{self, CleanTarget};
use crate::engine::data;
use crate::engine::regbackup::Hive;
use crate::engine::registry::{self, RegIssue};
use crate::engine::report;
use crate::engine::startup::{self, StartupEntry};
use crate::engine::sys;
use crate::i18n;
use crate::winui;
use crate::motion::{begin_switch, crossfade, switch_language};
use crate::views::{fill_details, set_row, cat_style, issue_locked, lock_issues, ltr, push_all, push_clean_summary, push_home, push_issues, push_reg_summary, push_startups};
use crate::{AppState, MainWindow, T, Theme};

#[derive(Default)]
pub struct Core {
    pub targets: Vec<CleanTarget>,
    pub issues: Vec<RegIssue>,
    pub startups: Vec<StartupEntry>,
    pub reg_scanned: bool,
    pub all_scanned: bool,
    pub admin: bool,
    pub last_report: Option<std::path::PathBuf>,
}

pub type Shared = Arc<Mutex<Core>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    All,
    Targets,
    Registry,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

fn ctx() -> Ctx {
    CTX.with(|c| c.borrow().clone().expect("ui context"))
}

type Action = Box<dyn FnOnce()>;

#[derive(Clone)]
pub struct Ctx {
    pub weak: Weak<MainWindow>,
    pub core: Shared,
    pending: Rc<RefCell<Option<Action>>>,
    toast: Rc<Timer>,
}

impl Ctx {
    pub fn new(weak: Weak<MainWindow>, core: Shared) -> Ctx {
        Ctx { weak, core, pending: Rc::new(RefCell::new(None)), toast: Rc::new(Timer::default()) }
    }
}

pub fn toast(c: &Ctx, text: String) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<AppState>();
    st.set_toast_text(text.into());
    st.set_toast_open(true);
    let weak = c.weak.clone();
    c.toast.start(TimerMode::SingleShot, Duration::from_millis(3400), move || {
        if let Some(ui) = weak.upgrade() {
            ui.global::<AppState>().set_toast_open(false);
        }
    });
}

pub fn confirm(c: &Ctx, title: String, text: String, ok: String, danger: bool, action: Box<dyn FnOnce()>) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<AppState>();
    st.set_dialog_title(title.into());
    st.set_dialog_text(text.into());
    st.set_dialog_ok(ok.into());
    st.set_dialog_danger(danger);
    st.set_dialog_single(false);
    *c.pending.borrow_mut() = Some(action);
    st.set_dialog_open(true);
}

pub fn show_error(c: &Ctx, text: String) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<AppState>();
    st.set_dialog_title(i18n::get("error.title").into());
    st.set_dialog_text(text.into());
    st.set_dialog_danger(false);
    st.set_dialog_single(true);
    *c.pending.borrow_mut() = None;
    st.set_dialog_open(true);
}

fn run_parallel(targets: &mut [CleanTarget], only: Option<&[usize]>, work: fn(&mut CleanTarget)) {
    let jobs: Vec<usize> = match only {
        Some(o) => o.to_vec(),
        None => (0..targets.len()).collect(),
    };
    if jobs.is_empty() {
        return;
    }
    let cells: Vec<Mutex<CleanTarget>> = jobs.iter().map(|&i| Mutex::new(std::mem::take(&mut targets[i]))).collect();
    let next = AtomicUsize::new(0);
    let workers = cells.len().min(cleaner::worker_count());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                crate::engine::cpu::urgent();
                loop {
                    let k = next.fetch_add(1, Ordering::SeqCst);
                    if k >= cells.len() {
                        break;
                    }
                    let mut guard = cells[k].lock().unwrap();
                    work(&mut guard);
                }
            });
        }
    });
    for (k, cell) in cells.into_iter().enumerate() {
        targets[jobs[k]] = cell.into_inner().unwrap();
    }
}

fn start_scan(c: &Ctx, scope: Scope) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<AppState>();
    if st.get_busy() {
        return;
    }
    st.set_busy(true);
    let status_key = match scope {
        Scope::Targets => "status.analyzing",
        _ => "status.scanning",
    };
    st.set_status(i18n::get(status_key).into());
    if scope == Scope::All {
        st.set_ring_mode(1);
        st.set_hero_title(i18n::get("hero.busy.title").into());
        st.set_hero_text(i18n::get("hero.busy.text").into());
        st.set_ring_big("".into());
        st.set_ring_small(i18n::get("ring.busy").into());
    }
    let core = c.core.clone();
    let weak = c.weak.clone();
    std::thread::spawn(move || {
        let reg_job = (scope != Scope::Targets).then(|| std::thread::spawn(registry::run));
        let start_job = (scope == Scope::All).then(|| std::thread::spawn(startup::list));
        if scope != Scope::Registry {
            let mut targets = core.lock().unwrap().targets.clone();
            run_parallel(&mut targets, None, cleaner::scan);
            store_targets(&mut core.lock().unwrap(), targets);
        }
        if let Some(job) = reg_job {
            let issues = job.join().unwrap_or_default();
            let mut g = core.lock().unwrap();
            g.issues = issues;
            lock_issues(&mut g);
            g.reg_scanned = true;
        }
        if let Some(job) = start_job {
            let list = job.join().unwrap_or_default();
            let mut g = core.lock().unwrap();
            g.startups = list;
            g.all_scanned = true;
        }
        if scope == Scope::Targets {
            core.lock().unwrap().all_scanned = true;
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            crate::winui::trim_soon();
            ui.global::<AppState>().set_busy(false);
            push_all(&ui, &core.lock().unwrap());
        });
    });
}

fn store_targets(g: &mut Core, mut fresh: Vec<CleanTarget>) {
    for (new, old) in fresh.iter_mut().zip(g.targets.iter()) {
        if new.id == old.id {
            new.selected = old.selected;
        }
    }
    g.targets = fresh;
}

fn start_clean(c: &Ctx) {
    let (count, bytes, recycle) = {
        let g = c.core.lock().unwrap();
        let sel: Vec<&CleanTarget> = g.targets.iter().filter(|t| t.selected && !(t.needs_admin && !g.admin)).collect();
        (sel.len(), sel.iter().map(|t| t.bytes).sum::<u64>(), sel.iter().any(|t| t.recycle))
    };
    if count == 0 {
        toast(c, i18n::get("msg.nothing"));
        return;
    }
    let mut text = i18n::fmt("confirm.clean.text", &[&data::format_size(bytes)]);
    if !cleaner::keep_recent() {
        text.push('\n');
        text.push_str(&i18n::get("confirm.allages"));
    }
    if recycle {
        text.push('\n');
        text.push_str(&i18n::get("confirm.recycle"));
    }
    let c2 = c.clone();
    confirm(c, i18n::get("confirm.clean.title"), text, i18n::get("confirm.clean.ok"), false, Box::new(move || run_clean(&c2)));
}

fn guard_with_restore_point(weak: &Weak<MainWindow>, admin: bool, desc_key: &'static str, status_key: &'static str) {
    if !admin || !crate::engine::settings::restore_point_on() {
        return;
    }
    let _ = weak.upgrade_in_event_loop(|ui| ui.global::<AppState>().set_status(i18n::get("status.restorepoint").into()));
    crate::engine::restorepoint::create(&i18n::get(desc_key));
    let _ = weak.upgrade_in_event_loop(move |ui| ui.global::<AppState>().set_status(i18n::get(status_key).into()));
}

fn run_clean(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<AppState>();
    if st.get_busy() {
        return;
    }
    st.set_busy(true);
    st.set_status(i18n::get("status.cleaning").into());
    let core = c.core.clone();
    let weak = c.weak.clone();
    std::thread::spawn(move || {
        let (mut targets, admin) = {
            let g = core.lock().unwrap();
            (g.targets.clone(), g.admin)
        };
        guard_with_restore_point(&weak, admin, "restore.desc.clean", "status.cleaning");
        let idx: Vec<usize> = (0..targets.len()).filter(|&i| targets[i].selected && !(targets[i].needs_admin && !admin)).collect();
        let before: Vec<CleanTarget> = idx.iter().map(|&i| targets[i].clone()).collect();
        run_parallel(&mut targets, Some(&idx), cleaner::clean);
        run_parallel(&mut targets, Some(&idx), cleaner::scan);
        let freed: u64 = idx.iter().map(|&i| targets[i].freed).sum();
        let failed: u64 = idx.iter().map(|&i| targets[i].failed).sum();
        let left: u64 = idx.iter().map(|&i| targets[i].bytes).sum();
        crate::engine::history::record("clean", freed, failed, idx.len() as u64);
        let lines: Vec<report::Line> = idx.iter().enumerate().map(|(n, &i)| report::Line { name: i18n::get(&format!("t.{}", targets[i].id)), before: &before[n], after: &targets[i] }).collect();
        let path = report::write_clean(&lines);
        drop(lines);
        {
            let mut g = core.lock().unwrap();
            store_targets(&mut g, targets);
            g.last_report = path;
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            crate::winui::trim_soon();
            let st = ui.global::<AppState>();
            st.set_busy(false);
            let g = core.lock().unwrap();
            st.set_has_report(g.last_report.is_some());
            push_all(&ui, &g);
            drop(g);
            let (size, rest) = (data::format_size(freed), data::format_size(left));
            let text = if failed > 0 { i18n::fmt("toast.freed.failed", &[&size, &failed.to_string(), &rest]) } else { i18n::fmt("toast.freed", &[&size, &rest]) };
            toast(&ctx(), text);
        });
    });
}

fn start_fix(c: &Ctx) {
    let count = c.core.lock().unwrap().issues.iter().filter(|i| i.selected).count();
    if count == 0 {
        toast(c, i18n::get("msg.nothing"));
        return;
    }
    let risky = c.core.lock().unwrap().issues.iter().any(|i| i.selected && i.kind == "riskyleft");
    let c2 = c.clone();
    let text = if risky { format!("{}\n\n{}", i18n::get("confirm.fix.text"), i18n::get("confirm.fix.risky")) } else { i18n::get("confirm.fix.text") };
    confirm(c, i18n::fmt("confirm.fix.title", &[&count.to_string()]), text, i18n::get("confirm.fix.ok"), risky, Box::new(move || run_fix(&c2)));
}

fn run_fix(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<AppState>();
    if st.get_busy() {
        return;
    }
    let backup = match data::new_backup_dir() {
        Ok(d) => d,
        Err(e) => {
            show_error(c, e.to_string());
            return;
        }
    };
    st.set_busy(true);
    st.set_status(i18n::get("status.fixing").into());
    let core = c.core.clone();
    let weak = c.weak.clone();
    std::thread::spawn(move || {
        guard_with_restore_point(&weak, crate::engine::sys::is_admin(), "restore.desc.fix", "status.fixing");
        let chosen: Vec<RegIssue> = {
            let g = core.lock().unwrap();
            g.issues.iter().filter(|i| i.selected && !issue_locked(i, g.admin)).cloned().collect()
        };
        let mut errors = Vec::new();
        let fixed = registry::fix(&chosen, &backup, &mut errors);
        let _ = std::fs::remove_dir(&backup);
        let old = core.lock().unwrap().issues.clone();
        let issues = registry::rescan(&old, &chosen);
        {
            let mut g = core.lock().unwrap();
            g.issues = issues;
            lock_issues(&mut g);
            g.reg_scanned = true;
        }
        let total = chosen.len();
        crate::engine::history::record("fix", fixed as u64, total as u64, 0);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            crate::winui::trim_soon();
            ui.global::<AppState>().set_busy(false);
            push_all(&ui, &core.lock().unwrap());
            if errors.is_empty() {
                toast(&ctx(), i18n::fmt("toast.fixed", &[&fixed.to_string(), &total.to_string()]));
            } else {
                let mut text = i18n::fmt("msg.errors", &[&errors.len().to_string()]);
                for line in errors.iter().take(5) {
                    text.push('\n');
                    text.push_str(&ltr(line));
                }
                show_error(&ctx(), text);
            }
        });
    });
}

fn toggle_start(c: &Ctx, i: usize, value: bool) {
    let Some(ui) = c.weak.upgrade() else { return };
    if ui.global::<AppState>().get_busy() {
        return;
    }
    let result = {
        let mut g = c.core.lock().unwrap();
        match g.startups.get_mut(i) {
            Some(e) => startup::set_enabled(e, value).map_err(|err| (err, true)),
            None => return,
        }
    };
    if let Err((err, machine)) = result {
        let text = if err.to_string() == "policy" {
            i18n::get("msg.policy")
        } else if err.to_string() == "system" {
            i18n::get("msg.system")
        } else if machine && err.kind() == std::io::ErrorKind::PermissionDenied {
            i18n::get("msg.needAdmin")
        } else {
            err.to_string()
        };
        show_error(c, text);
    }
    let g = c.core.lock().unwrap();
    push_startups(&ui, &g);
    push_home(&ui, &g);
}

fn delete_start(c: &Ctx, i: usize) {
    if c.weak.upgrade().is_none_or(|ui| ui.global::<AppState>().get_busy()) {
        return;
    }
    let Some(entry) = c.core.lock().unwrap().startups.get(i).cloned() else { return };
    let c2 = c.clone();
    confirm(
        c,
        i18n::fmt("confirm.delete.title", &[&ltr(&entry.name)]),
        i18n::get("confirm.delete.text"),
        i18n::get("confirm.delete.ok"),
        true,
        Box::new(move || {
            let outcome = data::new_backup_dir().and_then(|dir| startup::delete(&entry, &dir));
            match outcome {
                Ok(()) => reload_startups(&c2, Some(i18n::get("toast.removed"))),
                Err(e) => {
                    let text = match e.to_string().as_str() {
                        "nodelete" => i18n::get("msg.nodelete"),
                        "system" => i18n::get("msg.system"),
                        _ if e.kind() == std::io::ErrorKind::PermissionDenied => i18n::get("msg.needAdmin"),
                        other => other.to_string(),
                    };
                    show_error(&c2, text);
                }
            }
        }),
    );
}

fn reload_startups(c: &Ctx, done: Option<String>) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<AppState>();
    let owns_busy = !st.get_busy();
    if owns_busy {
        st.set_busy(true);
        st.set_status(i18n::get("status.scanning").into());
    }
    let core = c.core.clone();
    let weak = c.weak.clone();
    std::thread::spawn(move || {
        let list = startup::list();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            crate::winui::trim_soon();
            if owns_busy {
                ui.global::<AppState>().set_busy(false);
            }
            let mut g = core.lock().unwrap();
            g.startups = list;
            push_startups(&ui, &g);
            push_home(&ui, &g);
            drop(g);
            if let Some(text) = done {
                toast(&ctx(), text);
            }
        });
    });
}

fn detect_dark() -> bool {
    if let Some(v) = data::read_setting("theme.txt") {
        return v == "dark";
    }
    match Hive::User.open("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize") {
        Ok(key) => crate::engine::regbackup::read_dword(&key, "AppsUseLightTheme") == Some(0),
        Err(_) => false,
    }
}

pub fn make_core(admin: bool) -> Core {
    Core { targets: cleaner::targets(), startups: startup::list(), admin, ..Default::default() }
}

thread_local! {
    static LOW_DISK_DISMISSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn low_disk_text() -> String {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let Some(d) = crate::engine::drives::fixed().into_iter().find(|d| d.name.eq_ignore_ascii_case(&drive)) else { return String::new() };
    let free_pct = d.free as f64 / d.total as f64 * 100.0;
    if free_pct < 10.0 || d.free < 10 * 1024 * 1024 * 1024 {
        i18n::fmt("lowdisk.text", &[&d.name, &data::format_size(d.free), &format!("{:.0}%", free_pct)])
    } else {
        String::new()
    }
}

pub fn push_low_disk(ui: &MainWindow) {
    let shown = crate::engine::settings::low_disk_on() && !LOW_DISK_DISMISSED.with(|d| d.get());
    ui.global::<AppState>().set_low_disk_text(if shown { low_disk_text() } else { String::new() }.into());
}

pub fn reset_low_disk_dismissal() {
    LOW_DISK_DISMISSED.with(|d| d.set(false));
}

pub fn apply_fonts(ui: &MainWindow) {
    ui.global::<Theme>().set_ui_font(if i18n::zh() { "Microsoft YaHei UI" } else { "Segoe UI" }.into());
    ui.global::<Theme>().set_semi(if i18n::zh() { 500 } else { 600 });
}

pub fn enter_page(c: &Ctx, page: i32) {
    let (targets_done, reg_done) = {
        let g = c.core.lock().unwrap();
        (g.targets.iter().any(|t| t.scanned), g.reg_scanned)
    };
    match page {
        1 if !targets_done => start_scan(c, Scope::Targets),
        2 if !reg_done => start_scan(c, Scope::Registry),
        _ => {}
    }
    crate::pages::on_page(c, page);
}

pub fn install_text(ui: &MainWindow) {
    crate::geo::install(ui);
    apply_fonts(ui);
    let t = ui.global::<T>();
    t.on_t(|key, _| i18n::get(&key).into());
    t.on_f(|key, a, b, c, _| {
        let args: Vec<&str> = [a.as_str(), b.as_str(), c.as_str()].into_iter().collect();
        i18n::fmt(&key, &args).into()
    });
}

pub fn setup(ui: &MainWindow) {
    crate::engine::settings::load();
    install_window_controls(ui);
    install_text(ui);
    let dark = detect_dark();
    ui.global::<Theme>().set_dark(dark);
    let keep = data::read_setting("keep-recent.txt").map(|v| v != "all").unwrap_or(true);
    cleaner::set_keep_recent(keep);
    ui.global::<AppState>().set_keep_recent(keep);
    ui.global::<AppState>().set_glass_enabled(!std::env::args().any(|a| a == "--opaque"));

    let admin = sys::is_admin();
    ui.global::<AppState>().set_is_admin(admin);
    let core: Shared = Arc::new(Mutex::new(make_core(admin)));
    let ctx = Ctx::new(ui.as_weak(), core.clone());
    CTX.with(|x| *x.borrow_mut() = Some(ctx.clone()));
    push_all(ui, &core.lock().unwrap());
    push_low_disk(ui);
    ui.global::<AppState>().set_status(i18n::get("status.idle").into());

    let st = ui.global::<AppState>();
    st.on_scan_all({
        let c = ctx.clone();
        move || start_scan(&c, Scope::All)
    });
    st.on_page_moved({
        let c = ctx.clone();
        move |page| begin_switch(&c, page)
    });
    crate::pages::install(&ctx);
    st.on_analyze({
        let c = ctx.clone();
        move || start_scan(&c, Scope::All)
    });
    st.on_scan_reg({
        let c = ctx.clone();
        move || start_scan(&c, Scope::Registry)
    });
    st.on_clean({
        let c = ctx.clone();
        move || start_clean(&c)
    });
    st.on_fix_reg({
        let c = ctx.clone();
        move || start_fix(&c)
    });
    st.on_refresh_start({
        let c = ctx.clone();
        move || {
            if c.weak.upgrade().is_some_and(|ui| !ui.global::<AppState>().get_busy()) {
                reload_startups(&c, None);
            }
        }
    });
    st.on_select_start_tab({
        let c = ctx.clone();
        move |tab| {
            let Some(ui) = c.weak.upgrade() else { return };
            ui.global::<AppState>().set_start_tab(tab);
            push_startups(&ui, &c.core.lock().unwrap());
        }
    });
    st.on_set_show_system({
        let c = ctx.clone();
        move |on| {
            let Some(ui) = c.weak.upgrade() else { return };
            ui.global::<AppState>().set_start_show_system(on);
            push_startups(&ui, &c.core.lock().unwrap());
        }
    });
    st.on_select_safe({
        let c = ctx.clone();
        move || {
            let Some(ui) = c.weak.upgrade() else { return };
            let mut g = c.core.lock().unwrap();
            let admin = g.admin;
            let mut flags = Vec::new();
            for t in g.targets.iter_mut() {
                t.selected = cleaner::risk_of(&t.id) == 0 && (!t.needs_admin || admin);
                flags.push(t.selected);
            }
            let cats = ui.global::<AppState>().get_cats();
            for (i, on) in flags.into_iter().enumerate() {
                set_row(cats.clone(), i, |row| row.selected = on);
            }
            push_clean_summary(&ui, &g);
            push_home(&ui, &g);
        }
    });
    st.on_toggle_cat({
        let c = ctx.clone();
        move |i| {
            let Some(ui) = c.weak.upgrade() else { return };
            let i = i as usize;
            let mut g = c.core.lock().unwrap();
            let admin = g.admin;
            let Some(t) = g.targets.get_mut(i) else { return };
            if t.needs_admin && !admin {
                return;
            }
            t.selected = !t.selected;
            let selected = t.selected;
            set_row(ui.global::<AppState>().get_cats(), i, |row| row.selected = selected);
            push_clean_summary(&ui, &g);
            push_home(&ui, &g);
        }
    });
    st.on_show_details({
        let c = ctx.clone();
        move |i| {
            let Some(ui) = c.weak.upgrade() else { return };
            let Some(t) = c.core.lock().unwrap().targets.get(i as usize).cloned() else { return };
            let title = i18n::get(&format!("t.{}", t.id));
            let st = ui.global::<AppState>();
            let (icon, tint) = cat_style(&t.id);
            st.set_details_title(title.as_str().into());
            st.set_details_icon(icon.into());
            st.set_details_tint(tint);
            st.set_details_total("".into());
            st.set_details_sub("".into());
            st.set_details_rows(ModelRc::new(VecModel::from(Vec::<crate::DetailRow>::new())));
            st.set_details_loading(true);
            st.set_details_open(true);
            let weak = c.weak.clone();
            std::thread::spawn(move || {
                let rows = cleaner::details(&t);
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    crate::winui::trim_soon();
                    let st = ui.global::<AppState>();
                    if !st.get_details_open() || st.get_details_title().as_str() != title {
                        return;
                    }
                    fill_details(&ui, &rows);
                });
            });
        }
    });
    st.on_open_location(|path| {
        let path = path.as_str();
        if path == "shell:RecycleBinFolder" {
            let _ = std::process::Command::new(sys::system_tool("explorer.exe")).arg(path).spawn();
            return;
        }
        let p = std::path::Path::new(path);
        if p.is_absolute() && p.exists() {
            crate::pages::disk::reveal(p, p.is_dir());
        }
    });
    st.on_close_details({
        let c = ctx.clone();
        move || {
            if let Some(ui) = c.weak.upgrade() {
                ui.global::<AppState>().set_details_open(false);
            }
        }
    });
    st.on_toggle_issue({
        let c = ctx.clone();
        move |i| {
            let Some(ui) = c.weak.upgrade() else { return };
            if ui.global::<AppState>().get_busy() {
                return;
            }
            let i = i as usize;
            let mut g = c.core.lock().unwrap();
            let admin = g.admin;
            let Some(issue) = g.issues.get_mut(i) else { return };
            if issue_locked(issue, admin) {
                drop(g);
                toast(&c, i18n::get("msg.needAdmin"));
                return;
            }
            issue.selected = !issue.selected;
            let selected = issue.selected;
            set_row(ui.global::<AppState>().get_issues(), i, |row| row.selected = selected);
            push_reg_summary(&ui, &g);
        }
    });
    st.on_select_all_issues({
        let c = ctx.clone();
        move || {
            let Some(ui) = c.weak.upgrade() else { return };
            if ui.global::<AppState>().get_busy() {
                return;
            }
            let mut g = c.core.lock().unwrap();
            let admin = g.admin;
            let all = g.issues.iter().filter(|i| !issue_locked(i, admin) && !registry::is_leftover(&i.kind)).all(|i| i.selected);
            for i in g.issues.iter_mut().filter(|i| !issue_locked(i, admin) && !registry::is_leftover(&i.kind)) {
                i.selected = !all;
            }
            push_issues(&ui, &g);
        }
    });
    st.on_toggle_start({
        let c = ctx.clone();
        move |i, v| toggle_start(&c, i as usize, v)
    });
    st.on_delete_start({
        let c = ctx.clone();
        move |i| delete_start(&c, i as usize)
    });
    st.on_open_report({
        let c = ctx.clone();
        move || {
            let path = c.core.lock().unwrap().last_report.clone();
            if let Some(p) = path {
                let _ = std::process::Command::new(sys::system_tool("notepad.exe")).arg(p).spawn();
            }
        }
    });
    st.on_open_backups(|| {
        let dir = data::backup_root();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::process::Command::new(sys::system_tool("explorer.exe")).arg(&dir).spawn();
    });
    st.on_restart_admin(|| {
        if winui::restart_elevated() {
            let _ = slint::quit_event_loop();
        }
    });
    st.on_toggle_keep_recent({
        let c = ctx.clone();
        move |keep| {
            cleaner::set_keep_recent(keep);
            data::write_setting("keep-recent.txt", if keep { "recent" } else { "all" });
            let Some(ui) = c.weak.upgrade() else { return };
            ui.global::<AppState>().set_keep_recent(keep);
            let scanned = c.core.lock().unwrap().targets.iter().any(|t| t.scanned);
            if scanned {
                start_scan(&c, Scope::Targets);
            }
        }
    });
    st.on_dismiss_low_disk({
        let c = ctx.clone();
        move || {
            LOW_DISK_DISMISSED.with(|d| d.set(true));
            if let Some(ui) = c.weak.upgrade() {
                push_low_disk(&ui);
            }
        }
    });
    st.on_toggle_lang({
        let c = ctx.clone();
        move || switch_language(&c)
    });
    st.on_toggle_theme({
        let c = ctx.clone();
        move || {
            let Some(ui) = c.weak.upgrade() else { return };
            let now = !ui.global::<Theme>().get_dark();
            crossfade(&ui, || {
                ui.global::<Theme>().set_dark(now);
                data::write_setting("theme.txt", if now { "dark" } else { "light" });
                if ui.global::<AppState>().get_glass_active() {
                    winui::set_dark(now);
                }
            });
        }
    });
    st.on_dialog_accept({
        let c = ctx.clone();
        move || {
            let Some(ui) = c.weak.upgrade() else { return };
            ui.global::<AppState>().set_dialog_open(false);
            let action = c.pending.borrow_mut().take();
            if let Some(a) = action {
                a();
            }
        }
    });
    st.on_dialog_cancel({
        let c = ctx.clone();
        move || {
            let Some(ui) = c.weak.upgrade() else { return };
            ui.global::<AppState>().set_dialog_open(false);
            c.pending.borrow_mut().take();
        }
    });
}

fn with_window<R>(ui: &MainWindow, f: impl FnOnce(&winit::window::Window) -> R) -> Option<R> {
    ui.window().with_winit_window(f)
}

fn install_window_controls(ui: &MainWindow) {
    let st = ui.global::<AppState>();
    let weak = ui.as_weak();
    st.on_window_drag({
        let weak = weak.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                with_window(&ui, |w| {
                    let _ = w.drag_window();
                });
            }
        }
    });
    st.on_window_minimize({
        let weak = weak.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                with_window(&ui, |w| w.set_minimized(true));
            }
        }
    });
    st.on_window_maximize({
        let weak = weak.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                with_window(&ui, |w| w.set_maximized(!w.is_maximized()));
                sync_maximized(&ui);
            }
        }
    });
    st.on_window_close(|| {
        let _ = slint::quit_event_loop();
    });
    st.on_window_resize({
        let weak = weak.clone();
        move |dir| {
            let Some(ui) = weak.upgrade() else { return };
            let direction = match dir {
                0 => ResizeDirection::North,
                1 => ResizeDirection::South,
                2 => ResizeDirection::West,
                3 => ResizeDirection::East,
                4 => ResizeDirection::NorthWest,
                5 => ResizeDirection::NorthEast,
                6 => ResizeDirection::SouthWest,
                _ => ResizeDirection::SouthEast,
            };
            with_window(&ui, |w| {
                let _ = w.drag_resize_window(direction);
            });
        }
    });
}

fn sync_maximized(ui: &MainWindow) {
    if let Some(m) = with_window(ui, |w| w.is_maximized()) {
        let st = ui.global::<AppState>();
        if st.get_maximized() != m {
            st.set_maximized(m);
        }
    }
}

thread_local! {
    static REVEAL: std::cell::Cell<(bool, u8, bool)> = const { std::cell::Cell::new((false, 0, false)) };
}

const REVEAL_FRAMES: u8 = 3;

fn reveal_now() {
    let done = REVEAL.with(|r| {
        let (styled, frames, done) = r.get();
        r.set((styled, frames, true));
        done
    });
    if !done {
        winui::center_window();
        Timer::single_shot(Duration::from_millis(40), || winui::cloak(false));
    }
}

fn count_frame() {
    let reveal = REVEAL.with(|r| {
        let (styled, frames, done) = r.get();
        if done || !styled || frames >= REVEAL_FRAMES {
            return false;
        }
        r.set((styled, frames + 1, done));
        frames + 1 == REVEAL_FRAMES
    });
    if reveal {
        let _ = slint::invoke_from_event_loop(|| Timer::single_shot(Duration::from_millis(24), reveal_now));
    }
}

pub fn style_window_later(ui: &MainWindow) {
    let _ = ui.window().set_rendering_notifier(|state, _| {
        if matches!(state, slint::RenderingState::AfterRendering) {
            crate::motion::after_frame();
            count_frame();
        }
    });
    let weak_style = ui.as_weak();
    Timer::single_shot(Duration::from_millis(30), move || {
        let Some(ui) = weak_style.upgrade() else { return };
        with_window(&ui, |w| w.set_undecorated_shadow(true));
        winui::round_corners();
        if ui.global::<AppState>().get_glass_enabled() {
            let ok = winui::apply_backdrop(ui.global::<Theme>().get_dark());
            winui::strip_caption_buttons();
            ui.global::<AppState>().set_glass_active(ok);
        }
        REVEAL.with(|r| {
            let (_, _, done) = r.get();
            r.set((true, 0, done));
        });
    });
    Timer::single_shot(Duration::from_millis(700), reveal_now);
    let weak_place = ui.as_weak();
    Timer::single_shot(Duration::from_millis(1100), move || {
        winui::cloak(false);
        if !winui::is_centered()
            && let Some(ui) = weak_place.upgrade() {
                ui.window().set_position(slint::PhysicalPosition::new(120, 120));
            }
    });
    Timer::single_shot(Duration::from_millis(2500), winui::trim_memory);
}

pub fn restore_state(ui: &MainWindow) {
    crate::motion::sync_scale(ui);
    let Some(state) = crate::engine::settings::load_state() else { return };
    let scale = ui.window().scale_factor().max(0.5);
    let (mut w, mut h) = (state.width as f32, state.height as f32);
    if let Some((max_w, max_h)) = winui::work_size() {
        w = w.min(max_w as f32 / scale);
        h = h.min(max_h as f32 / scale);
    }
    ui.window().set_size(slint::LogicalSize::new(w.max(820.0), h.max(560.0)));
    ui.global::<AppState>().set_page(state.page);
    ui.global::<Theme>().set_shown_page(state.page);
    enter_page(&ctx(), state.page);
}

pub fn save_state(ui: &MainWindow) {
    use crate::engine::settings::{self, WindowState};
    let previous = settings::load_state();
    let scale = ui.window().scale_factor().max(0.5);
    let size = ui.window().size();
    let keep_size = ui.window().is_maximized() || ui.window().is_fullscreen() || size.width == 0 || size.height == 0;
    let (width, height) = match (keep_size, previous) {
        (true, Some(p)) => (p.width, p.height),
        (true, None) => return,
        (false, _) => ((size.width as f32 / scale).round() as i32, (size.height as f32 / scale).round() as i32),
    };
    settings::save_state(WindowState { page: ui.global::<AppState>().get_page(), width, height });
}

fn try_bind(ui: &MainWindow) -> Option<isize> {
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let hwnd = with_window(ui, |w| match w.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => Some(h.hwnd.get()),
        _ => None,
    })??;
    winui::bind(hwnd);
    Some(hwnd)
}

fn bind_later(weak: slint::Weak<MainWindow>, tries: u8) {
    Timer::single_shot(Duration::from_millis(5), move || {
        let Some(ui) = weak.upgrade() else { return };
        if try_bind(&ui).is_none() && tries > 1 {
            bind_later(weak, tries - 1);
        }
    });
}

pub fn bind_window(ui: &MainWindow) {
    if try_bind(ui).is_some() {
        winui::cloak(true);
    } else {
        bind_later(ui.as_weak(), 200);
    }
    ui.global::<AppState>().on_window_changed({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                sync_maximized(&ui);
            }
        }
    });
}

