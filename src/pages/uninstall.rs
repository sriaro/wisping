use std::cell::RefCell;

use slint::ComponentHandle;

use crate::app::{confirm, show_error, toast, Ctx};
use crate::engine::data;
use crate::engine::programs::{self, Program};
use crate::i18n;
use crate::{MainWindow, ProgItem, UninstallState};

#[derive(Default)]
struct State {
    all: Vec<Program>,
    filter: String,
    sort: i32,
    loaded: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn tint_of(name: &str) -> i32 {
    let h = name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    (h % 7) as i32
}

fn push(ui: &MainWindow) {
    STATE.with(|s| {
        let s = s.borrow();
        let needle = s.filter.to_lowercase();
        let mut rows: Vec<(usize, &Program)> = s
            .all
            .iter()
            .enumerate()
            .filter(|(_, p)| needle.is_empty() || p.name.to_lowercase().contains(&needle) || p.publisher.to_lowercase().contains(&needle))
            .collect();
        match s.sort {
            1 => rows.sort_by(|a, b| b.1.size.cmp(&a.1.size).then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))),
            2 => rows.sort_by(|a, b| b.1.date.cmp(&a.1.date).then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))),
            _ => rows.sort_by_key(|(_, p)| p.name.to_lowercase()),
        }
        let total: u64 = rows.iter().map(|(_, p)| p.size).sum();
        let items: Vec<ProgItem> = rows
            .iter()
            .map(|(i, p)| ProgItem {
                idx: *i as i32,
                name: crate::views::ltr(&p.name).into(),
                sub: crate::views::ltr(&[p.publisher.as_str(), p.version.as_str()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join("  ·  ")).into(),
                size: if p.size > 0 { data::format_size(p.size) } else { String::new() }.into(),
                date: p.date.as_str().into(),
                tint: tint_of(&p.name),
                can_open: !p.location.is_empty() && std::path::Path::new(&p.location).is_dir(),
            })
            .collect();
        let st = ui.global::<UninstallState>();
        st.set_count_text(i18n::fmt("uninst.count", &[&items.len().to_string(), &data::format_size(total)]).into());
        st.set_sort(s.sort);
        if let Some(model) = crate::views::sync_model(&st.get_programs(), items) {
            st.set_programs(model);
        }
    });
}

fn load(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<UninstallState>();
    if st.get_loading() {
        return;
    }
    st.set_loading(true);
    let weak = c.weak.clone();
    std::thread::spawn(move || {
        let list = programs::list();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            crate::winui::trim_soon();
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.all = list;
                s.loaded = true;
            });
            ui.global::<UninstallState>().set_loading(false);
            push(&ui);
        });
    });
}

pub fn on_show(c: &Ctx) {
    if !STATE.with(|s| s.borrow().loaded) {
        load(c);
    }
}

fn program_at(idx: i32) -> Option<Program> {
    STATE.with(|s| s.borrow().all.get(idx as usize).cloned())
}

pub fn install(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<UninstallState>();
    st.on_set_sort({
        let c = c.clone();
        move |i| {
            STATE.with(|s| s.borrow_mut().sort = i);
            if let Some(ui) = c.weak.upgrade() {
                push(&ui);
            }
        }
    });
    st.on_set_filter({
        let c = c.clone();
        move |t| {
            STATE.with(|s| s.borrow_mut().filter = t.to_string());
            if let Some(ui) = c.weak.upgrade() {
                push(&ui);
            }
        }
    });
    st.on_refresh({
        let c = c.clone();
        move || load(&c)
    });
    st.on_open_folder(|idx| {
        if let Some(p) = program_at(idx) {
            let b = p.location.as_bytes();
            if b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' &&std::path::Path::new(&p.location).is_dir() {
                let _ = std::process::Command::new(crate::engine::sys::system_tool("explorer.exe")).arg(&p.location).spawn();
            }
        }
    });
    st.on_uninstall({
        let c = c.clone();
        move |idx| {
            let Some(p) = program_at(idx) else { return };
            if programs::refused_elevated(&p, crate::engine::sys::is_admin()) {
                show_error(&c, i18n::fmt("uninst.elevated", &[&crate::views::ltr(&p.name)]));
                return;
            }
            let c2 = c.clone();
            confirm(
                &c,
                i18n::fmt("uninst.confirm.title", &[&crate::views::ltr(&p.name)]),
                i18n::fmt("uninst.confirm.text", &[&crate::views::ltr(&p.command)]),
                i18n::get("uninst.btn"),
                true,
                Box::new(move || match programs::launch_uninstaller(&p) {
                    Ok(()) => {
                        data::log(&format!("uninstaller started {}", p.name));
                        toast(&c2, i18n::get("uninst.started"));
                    }
                    Err(e) if e.to_string() == programs::ELEVATED_USER_ENTRY => show_error(&c2, i18n::fmt("uninst.elevated", &[&crate::views::ltr(&p.name)])),
                    Err(e) => show_error(&c2, e.to_string()),
                }),
            );
        }
    });
}

pub fn preview(ui: &MainWindow) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.all = programs::list();
        s.loaded = true;
    });
    push(ui);
}

pub fn refresh(ui: &MainWindow) {
    if STATE.with(|s| s.borrow().loaded) {
        push(ui);
    }
}
