use std::sync::atomic::{AtomicI32, Ordering};

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::{confirm, show_error, toast, Ctx};
use crate::engine::{backups, data, settings, sys};
use crate::i18n;
use crate::{BackupsState, DataItem, MainWindow};

static SHOWN_TAB: AtomicI32 = AtomicI32::new(-1);

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
            r.button = i18n::get("backups.restore").into();
            r.deletable = true;
            r
        })
        .collect()
}

fn history_rows() -> Vec<DataItem> {
    crate::engine::history::list()
        .into_iter()
        .map(|e| {
            if e.kind == "fix" {
                let mut r = row("h", e.when, i18n::fmt("history.fix.desc", &[&e.a.to_string(), &e.b.to_string()]), "wrench", 3);
                r.state = i18n::get("history.fix").into();
                r
            } else {
                let mut desc = i18n::fmt("history.clean.desc", &[&e.c.to_string()]);
                if e.b > 0 {
                    desc.push_str("  ·  ");
                    desc.push_str(&i18n::fmt("history.failed", &[&data::format_size(e.b)]));
                }
                let mut r = row("h", e.when, desc, "trash", 0);
                r.state = data::format_size(e.a).into();
                r
            }
        })
        .collect()
}

fn data_rows() -> Vec<DataItem> {
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
    let count = backups::list().len();
    let mut clear = row("clear", i18n::get("backups.clear.title"), i18n::fmt("backups.clear.desc", &[&count.to_string()]), "trash", 6);
    clear.button = i18n::get("backups.clear.btn").into();
    clear.enabled = count > 0;
    let hist = crate::engine::history::list().len();
    let mut clearhist = row("clearhist", i18n::get("backups.clearhist.title"), i18n::get("backups.clearhist.desc"), "trash", 6);
    clearhist.button = i18n::get("backups.clearhist.btn").into();
    clearhist.enabled = hist > 0;
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
    view.enabled = data::log_path().is_file();
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
        1 => i18n::fmt("backups.sub.history", &[&data::format_size(crate::engine::history::total_freed())]),
        _ => i18n::get("backups.sub.data"),
    };
    st.set_sub(sub.into());
    st.set_empty(
        i18n::get(match (tab, settings::history_on()) {
            (1, true) => "history.empty",
            (1, false) => "history.off",
            _ => "backups.empty",
        })
        .into(),
    );
    let items = match tab {
        0 => backup_rows(),
        1 => history_rows(),
        _ => data_rows(),
    };
    let keep = SHOWN_TAB.swap(tab, Ordering::SeqCst) == tab;
    if keep {
        if let Some(model) = crate::views::sync_model(&st.get_items(), items) {
            st.set_items(model);
        }
    } else {
        st.set_items(ModelRc::new(VecModel::from(items)));
    }
}

fn restore(c: &Ctx, id: String) {
    let weak = c.weak.clone();
    let core = c.core.clone();
    std::thread::spawn(move || {
        let result = backups::restore(&id);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            crate::winui::trim_soon();
            let c = Ctx::new(ui.as_weak(), core);
            match result {
                Ok(_) => toast(&c, i18n::get("backups.restored")),
                Err(e) => show_error(&c, e.to_string()),
            }
        });
    });
}

fn open_path(path: &std::path::Path) {
    let _ = std::process::Command::new(sys::system_tool("explorer.exe")).arg(path).spawn();
}

fn act(c: &Ctx, id: &str) {
    match id {
        "folder" => {
            if let Some(dir) = crate::winui::pick_folder() {
                match data::set_location(&dir) {
                    Ok(()) => toast(c, i18n::get("backups.folder.moved")),
                    Err(e) => show_error(c, e.to_string()),
                }
            }
        }
        "reset" => match data::reset_location() {
            Ok(()) => toast(c, i18n::get("backups.folder.reset")),
            Err(e) => show_error(c, e.to_string()),
        },
        "open" => {
            let dir = data::ensure_root();
            open_path(dir);
        }
        "clearhist" => {
            crate::engine::history::clear();
            toast(c, i18n::get("backups.clearhist.done"));
        }
        "clear" => {
            let count = backups::list().len().to_string();
            let c2 = c.clone();
            confirm(
                c,
                i18n::get("backups.clear.confirm.title"),
                i18n::fmt("backups.clear.confirm.text", &[&count]),
                i18n::get("backups.clear.btn"),
                true,
                Box::new(move || {
                    let n = backups::delete_all();
                    toast(&c2, i18n::fmt("backups.deleted.many", &[&n.to_string()]));
                    if let Some(ui) = c2.weak.upgrade() {
                        push(&ui);
                    }
                }),
            );
        }
        "viewlog" => {
            let _ = std::process::Command::new(sys::system_tool("notepad.exe")).arg(data::log_path()).spawn();
        }
        _ => {
            let when = backups::list().into_iter().find(|b| b.id == id).map(|b| b.when).unwrap_or_else(|| id.to_string());
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
            let when = backups::list().into_iter().find(|b| b.id == id.as_str()).map(|b| b.when).unwrap_or_else(|| id.to_string());
            let owned = id.to_string();
            let c2 = c.clone();
            confirm(
                &c,
                i18n::get("backups.delete.title"),
                i18n::fmt("backups.delete.text", &[&when]),
                i18n::get("backups.delete.btn"),
                true,
                Box::new(move || {
                    match backups::delete(&owned) {
                        Ok(()) => toast(&c2, i18n::get("backups.deleted")),
                        Err(e) => show_error(&c2, e.to_string()),
                    }
                    if let Some(ui) = c2.weak.upgrade() {
                        push(&ui);
                    }
                }),
            );
        }
    });
    st.on_act({
        let c = c.clone();
        move |id| act(&c, id.as_str())
    });
}

pub fn preview(ui: &MainWindow) {
    push(ui);
}

pub fn refresh(ui: &MainWindow) {
    push(ui);
}
