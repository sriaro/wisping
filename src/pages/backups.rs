use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::{confirm, error_text, show_error, toast, Ctx};
use crate::engine::{backups, data, settings, sys};
use crate::i18n;
use crate::{BackupsState, DataItem, MainWindow};

static SHOWN_TAB: AtomicI32 = AtomicI32::new(-1);
static BUSY: AtomicBool = AtomicBool::new(false);
static LIST_GEN: AtomicU32 = AtomicU32::new(0);
static FACTS_GEN: AtomicU32 = AtomicU32::new(0);

struct Facts {
    history: Vec<crate::engine::history::Entry>,
    freed: u64,
    backups: usize,
    logged: bool,
}

thread_local! {
    static LISTED: RefCell<Option<Vec<DataItem>>> = const { RefCell::new(None) };
    static FACTS: RefCell<Option<Facts>> = const { RefCell::new(None) };
}

fn facts() -> Facts {
    let (history, freed) = crate::engine::history::load();
    Facts { history, freed, backups: backups::count(), logged: crate::engine::fsx::plain_file(&data::log_path()) == Some(true) }
}

fn known<R>(read: impl FnOnce(&Facts) -> R) -> R {
    FACTS.with(|f| read(f.borrow_mut().get_or_insert_with(facts)))
}

fn forget() {
    FACTS_GEN.fetch_add(1, Ordering::SeqCst);
    FACTS.with(|f| *f.borrow_mut() = None);
}

fn revalidate(ui: &MainWindow) {
    let generation = FACTS_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let weak = ui.as_weak();
    crate::engine::pool::spawn(move || {
        let fresh = facts();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if FACTS_GEN.load(Ordering::SeqCst) != generation {
                return;
            }
            FACTS.with(|f| *f.borrow_mut() = Some(fresh));
            if ui.global::<BackupsState>().get_tab() != 0 {
                push(&ui);
            }
        });
    });
}

fn row(id: &str, title: String, desc: String, icon: &str, tint: i32) -> DataItem {
    DataItem { id: id.into(), title: title.into(), desc: desc.into(), state: "".into(), icon: icon.into(), tint, toggle: false, on: false, button: "".into(), enabled: true, deletable: false }
}

fn backup_rows() -> Vec<DataItem> {
    backups::list()
        .into_iter()
        .map(|b| {
            let mut keys = b.keys.join("  ");
            if b.files > b.keys.len() {
                keys.push_str("  ...");
            }
            let mut r = row(&b.id, b.when, keys, "archive", 1);
            r.state = if b.files == 1 { i18n::get("backups.file.one") } else { i18n::fmt("backups.files", &[&b.files.to_string()]) }.into();
            if b.restorable {
                r.button = i18n::get("backups.restore").into();
            }
            r.deletable = true;
            r
        })
        .collect()
}

fn history_rows(list: &[crate::engine::history::Entry]) -> Vec<DataItem> {
    list.iter()
        .map(|e| {
            if e.kind == "fix" {
                let mut r = row("h", e.when.clone(), i18n::fmt("history.fix.desc", &[&e.a.to_string(), &e.b.to_string()]), "wrench", 3);
                r.state = i18n::get("history.fix").into();
                r
            } else {
                let mut desc = i18n::fmt("history.clean.desc", &[&e.c.to_string()]);
                if e.b > 0 {
                    desc.push_str("  ·  ");
                    desc.push_str(&i18n::fmt("history.failed", &[&data::format_size(e.b)]));
                }
                let mut r = row("h", e.when.clone(), desc, "trash", 0);
                r.state = data::format_size(e.a).into();
                r
            }
        })
        .collect()
}

fn data_rows(facts: &Facts) -> Vec<DataItem> {
    let mut log = row("log", i18n::get("backups.log.title"), i18n::get("backups.log.desc"), "file", 5);
    log.toggle = true;
    log.on = settings::log_on();
    let mut remember = row("remember", i18n::get("backups.remember.title"), i18n::get("backups.remember.desc"), "check", 3);
    remember.toggle = true;
    remember.on = settings::remember_on();
    let mut history = row("history", i18n::get("backups.history.title"), i18n::get("backups.history.desc"), "archive", 1);
    history.toggle = true;
    history.on = settings::history_on();
    let mut restorepoint = row("restorepoint", i18n::get("backups.restorepoint.title"), i18n::get("backups.restorepoint.desc"), "shield", 4);
    restorepoint.toggle = true;
    restorepoint.on = settings::restore_point_on();
    let mut lowdisk = row("lowdisk", i18n::get("backups.lowdisk.title"), i18n::get("backups.lowdisk.desc"), "alert", 3);
    lowdisk.toggle = true;
    lowdisk.on = settings::low_disk_on();
    let mut folder = row("folder", i18n::get("backups.folder.title"), data::root().display().to_string(), "folder", 2);
    folder.button = i18n::get("backups.folder.change").into();
    if !data::is_custom() {
        folder.state = i18n::get("backups.folder.default").into();
    }
    let count = facts.backups;
    let mut clear = row("clear", i18n::get("backups.clear.title"), i18n::fmt("backups.clear.desc", &[&count.to_string()]), "trash", 6);
    clear.button = i18n::get("backups.clear.btn").into();
    clear.enabled = count > 0;
    let mut clearhist = row("clearhist", i18n::get("backups.clearhist.title"), i18n::get("backups.clearhist.desc"), "trash", 6);
    clearhist.button = i18n::get("backups.clearhist.btn").into();
    clearhist.enabled = !facts.history.is_empty();
    let mut rows = vec![log, history, remember, restorepoint, lowdisk, folder];
    if data::is_custom() {
        let mut reset = row("reset", i18n::get("backups.reset.title"), data::default_root().display().to_string(), "refresh", 1);
        reset.button = i18n::get("backups.reset.btn").into();
        rows.push(reset);
    }
    let mut open = row("open", i18n::get("backups.open.title"), i18n::get("backups.open.desc"), "download", 4);
    open.button = i18n::get("backups.open.btn").into();
    let mut view = row("viewlog", i18n::get("backups.viewlog.title"), i18n::get("backups.viewlog.desc"), "file", 6);
    view.button = i18n::get("backups.open.btn").into();
    view.enabled = facts.logged;
    if !view.enabled {
        view.desc = i18n::get("backups.viewlog.none").into();
    }
    rows.push(clearhist);
    rows.push(clear);
    rows.push(open);
    rows.push(view);
    rows
}

fn push(ui: &MainWindow) {
    let st = ui.global::<BackupsState>();
    let tab = st.get_tab();
    st.set_tab_names(ModelRc::new(VecModel::from(vec![
        i18n::get("backups.tab.list").into(),
        i18n::get("backups.tab.history").into(),
        i18n::get("backups.tab.data").into(),
    ])));
    let sub = match tab {
        0 => i18n::get("backups.sub.list"),
        1 => i18n::fmt("backups.sub.history", &[&data::format_size(known(|f| f.freed))]),
        _ => i18n::get("backups.sub.data"),
    };
    st.set_sub(sub.into());
    let keep = SHOWN_TAB.swap(tab, Ordering::SeqCst) == tab;
    if tab != 0 {
        st.set_empty(i18n::get(if tab != 1 { "backups.empty" } else if settings::history_on() { "history.empty" } else { "history.off" }).into());
        show_items(&st, known(|f| if tab == 1 { history_rows(&f.history) } else { data_rows(f) }), keep);
        return;
    }
    let listed = LISTED.with(|l| l.borrow().clone());
    st.set_empty(if listed.is_some() { i18n::get("backups.empty") } else { String::new() }.into());
    match listed {
        Some(items) => show_items(&st, items, keep),
        None if !keep => st.set_items(ModelRc::new(VecModel::from(Vec::<DataItem>::new()))),
        None => {}
    }
    let generation = LIST_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let weak = ui.as_weak();
    crate::engine::pool::spawn(move || {
        let items = backup_rows();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if LIST_GEN.load(Ordering::SeqCst) != generation {
                return;
            }
            LISTED.with(|l| *l.borrow_mut() = Some(items.clone()));
            let st = ui.global::<BackupsState>();
            if st.get_tab() != 0 {
                return;
            }
            st.set_empty(i18n::get("backups.empty").into());
            show_items(&st, items, true);
        });
    });
}

fn show_items(st: &BackupsState, items: Vec<DataItem>, keep: bool) {
    if keep {
        if let Some(model) = crate::views::sync_model(&st.get_items(), items) {
            st.set_items(model);
        }
    } else {
        st.set_items(ModelRc::new(VecModel::from(items)));
    }
}

fn restore(c: &Ctx, id: String) {
    in_background(c, true, move || backups::restore(&id).map(|_| i18n::get("backups.restored")).map_err(|e| error_text(&e)));
}

fn in_background(c: &Ctx, reload: bool, work: impl FnOnce() -> Result<String, String> + Send + 'static) {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(ui) = c.weak.upgrade() {
        ui.global::<BackupsState>().set_busy(true);
    }
    let weak = c.weak.clone();
    let core = c.core.clone();
    crate::engine::pool::spawn(move || {
        let _reset = crate::app::panic_reset(&weak, |ui| {
            BUSY.store(false, Ordering::SeqCst);
            ui.global::<BackupsState>().set_busy(false);
            forget();
            push(ui);
        });
        let result = work();
        BUSY.store(false, Ordering::SeqCst);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            crate::winui::trim_soon();
            ui.global::<BackupsState>().set_busy(false);
            let c = Ctx::new(ui.as_weak(), core);
            if reload {
                crate::app::reload_startups(&c, None);
            }
            match result {
                Ok(text) => toast(&c, text),
                Err(e) => show_error(&c, e),
            }
            forget();
            push(&ui);
        });
    });
}

fn open_path(path: &std::path::Path) {
    let _ = sys::launch("explorer.exe", &sys::quoted(path));
}

fn act(c: &Ctx, id: &str) {
    match id {
        "folder" => {
            if let Some(dir) = crate::winui::pick_folder() {
                in_background(c, false, move || data::set_location(&dir).map(|()| i18n::get("backups.folder.moved")).map_err(|e| error_text(&e)));
            }
        }
        "reset" => in_background(c, false, || data::reset_location().map(|()| i18n::get("backups.folder.reset")).map_err(|e| error_text(&e))),
        "open" => {
            let dir = data::ensure_root();
            open_path(&dir);
        }
        "clearhist" => {
            crate::engine::history::clear();
            forget();
            toast(c, i18n::get("backups.clearhist.done"));
        }
        "clear" => {
            let count = known(|f| f.backups).to_string();
            let c2 = c.clone();
            confirm(
                c,
                i18n::get("backups.clear.confirm.title"),
                i18n::fmt("backups.clear.confirm.text", &[&count]),
                i18n::get("backups.clear.btn"),
                true,
                Box::new(move || {
                    in_background(&c2, false, || {
                        let report = backups::delete_all();
                        if report.failed == 0 {
                            Ok(i18n::fmt("backups.deleted.many", &[&report.removed.to_string()]))
                        } else if report.needs_admin {
                            Err(i18n::fmt("backups.deleted.admin", &[&report.removed.to_string(), &report.failed.to_string()]))
                        } else {
                            Err(i18n::fmt("backups.deleted.partial", &[&report.removed.to_string(), &report.failed.to_string()]))
                        }
                    });
                }),
            );
        }
        "viewlog" => {
            let _ = sys::launch("notepad.exe", &sys::quoted(&data::log_path()));
        }
        _ => {
            let when = backups::when_of(id);
            let owned = id.to_string();
            let c2 = c.clone();
            confirm(c, i18n::get("backups.restore.title"), i18n::fmt("backups.restore.text", &[&when]), i18n::get("backups.restore"), false, Box::new(move || restore(&c2, owned)));
        }
    }
    if let Some(ui) = c.weak.upgrade() {
        push(&ui);
    }
}

pub fn on_show(c: &Ctx) {
    SHOWN_TAB.store(-1, Ordering::SeqCst);
    if let Some(ui) = c.weak.upgrade() {
        push(&ui);
        revalidate(&ui);
    }
}

pub fn install(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<BackupsState>();
    st.on_select_tab({
        let c = c.clone();
        move |i| {
            if let Some(ui) = c.weak.upgrade() {
                ui.global::<BackupsState>().set_tab(i);
                push(&ui);
            }
        }
    });
    st.on_switched({
        let c = c.clone();
        move |id, on| {
            match id.as_str() {
                "log" => {
                    settings::set_log(on);
                    toast(&c, i18n::get(if on { "backups.log.on" } else { "backups.log.off" }));
                }
                "history" => {
                    settings::set_history(on);
                    toast(&c, i18n::get(if on { "backups.history.on" } else { "backups.history.off" }));
                }
                "restorepoint" => {
                    settings::set_restore_point(on);
                    toast(&c, i18n::get(if on { "backups.restorepoint.on" } else { "backups.restorepoint.off" }));
                }
                "lowdisk" => {
                    settings::set_low_disk(on);
                    crate::app::reset_low_disk_dismissal();
                    if let Some(ui) = c.weak.upgrade() {
                        crate::app::push_low_disk(&ui);
                    }
                    toast(&c, i18n::get(if on { "backups.lowdisk.on" } else { "backups.lowdisk.off" }));
                }
                "remember" => {
                    settings::set_remember(on);
                    toast(&c, i18n::get(if on { "backups.remember.on" } else { "backups.remember.off" }));
                }
                _ => {}
            }
            if let Some(ui) = c.weak.upgrade() {
                push(&ui);
            }
        }
    });
    st.on_remove({
        let c = c.clone();
        move |id| {
            let when = backups::when_of(&id);
            let owned = id.to_string();
            let c2 = c.clone();
            confirm(
                &c,
                i18n::get("backups.delete.title"),
                i18n::fmt("backups.delete.text", &[&when]),
                i18n::get("backups.delete.btn"),
                true,
                Box::new(move || {
                    in_background(&c2, false, move || backups::delete(&owned).map(|()| i18n::get("backups.deleted")).map_err(|e| error_text(&e)));
                }),
            );
        }
    });
    st.on_act({
        let c = c.clone();
        move |id| act(&c, id.as_str())
    });
}

pub fn refresh(ui: &MainWindow) {
    push(ui);
}

pub fn changed(ui: &MainWindow) {
    if ui.global::<crate::AppState>().get_page() != 7 {
        forget();
        return;
    }
    revalidate(ui);
    if ui.global::<BackupsState>().get_tab() == 0 {
        push(ui);
    }
}
