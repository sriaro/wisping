use std::cell::RefCell;

use slint::ComponentHandle;

use crate::app::{confirm, show_error, toast, Ctx};
use crate::engine::{data, tools};
use crate::i18n;
use crate::{MainWindow, ToolItem, ToolsState};

thread_local! {
    static BUSY: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn title_of(id: &str) -> String {
    i18n::get(&format!("tools.{}.title", id))
}

fn push(ui: &MainWindow) {
    let busy = BUSY.with(|b| b.borrow().clone());
    let admin = ui.global::<crate::AppState>().get_is_admin();
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let items = build(busy, admin);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let st = ui.global::<ToolsState>();
            if let Some(model) = crate::views::sync_model(&st.get_items(), items) {
                st.set_items(model);
            }
        });
    });
}

fn build(busy: Option<String>, admin: bool) -> Vec<ToolItem> {
    tools::TOOLS
        .iter()
        .map(|t| {
            let count = tools::detail(t.id);
            let state = match t.id {
                "notify" => i18n::fmt("tools.state.apps", &[&count]),
                "tray" => i18n::fmt("tools.state.icons", &[&count]),
                "telemetry" if count == "limited" => i18n::get("tools.telemetry.limited"),
                _ => String::new(),
            };
            let locked = t.admin && !admin;
            ToolItem {
                locked,
                id: t.id.into(),
                title: title_of(t.id).into(),
                desc: i18n::get(&format!("tools.{}.desc", t.id)).into(),
                state: if locked { i18n::get("tools.needadmin") } else { state }.into(),
                icon: t.icon.into(),
                tint: t.tint,
                toggle: t.toggle,
                on: t.toggle && tools::is_on(t.id),
                busy: busy.as_deref() == Some(t.id),
            }
        })
        .collect()
}

fn set_busy(c: &Ctx, id: Option<&str>) {
    BUSY.with(|b| *b.borrow_mut() = id.map(str::to_string));
    if let Some(ui) = c.weak.upgrade() {
        push(&ui);
    }
}

fn finish(c: &Ctx, id: &str, result: std::io::Result<()>) {
    set_busy(c, None);
    match result {
        Ok(()) if id == "tray" => {
            let c2 = c.clone();
            confirm(c, i18n::get("tools.restart.title"), i18n::get("tools.restart.text"), i18n::get("tools.restart.btn"), true, Box::new(move || {
                tools::restart_pc();
                toast(&c2, i18n::get("tools.done"));
            }));
        }
        Ok(()) => toast(c, i18n::get("tools.done")),
        Err(e) => show_error(c, e.to_string()),
    }
}

fn start(c: &Ctx, id: &'static str) {
    if BUSY.with(|b| b.borrow().is_some()) {
        return;
    }
    set_busy(c, Some(id));
    let weak = c.weak.clone();
    let core = c.core.clone();
    std::thread::spawn(move || {
        let result = data::new_backup_dir().and_then(|dir| tools::run(id, &dir));
        let _ = weak.upgrade_in_event_loop(move |ui| {
            crate::winui::trim_soon();
            let c = Ctx::new(ui.as_weak(), core);
            finish(&c, id, result);
            push(&ui);
        });
    });
}

pub fn on_show(c: &Ctx) {
    if let Some(ui) = c.weak.upgrade() {
        push(&ui);
    }
}

pub fn install(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<ToolsState>();
    st.on_switched({
        let c = c.clone();
        move |id, on| {
            match tools::set_on(id.as_str(), on) {
                Ok(()) => toast(&c, i18n::get(&format!("tools.{}.{}", id, if on { "on" } else { "off" }))),
                Err(e) => show_error(&c, e.to_string()),
            }
            if let Some(ui) = c.weak.upgrade() {
                push(&ui);
            }
        }
    });
    st.on_run({
        let c = c.clone();
        move |id| {
            let Some(info) = tools::TOOLS.iter().find(|t| t.id == id.as_str()) else { return };
            let tid = info.id;
            let c2 = c.clone();
            confirm(&c, i18n::get(&format!("tools.{}.confirm.title", tid)), i18n::get(&format!("tools.{}.confirm.text", tid)), i18n::get("tools.btn"), true, Box::new(move || start(&c2, tid)));
        }
    });
}

pub fn preview(ui: &MainWindow) {
    push(ui);
}

pub fn refresh(ui: &MainWindow) {
    push(ui);
}
