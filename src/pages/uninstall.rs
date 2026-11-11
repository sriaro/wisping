use std::cell::RefCell;

use slint::ComponentHandle;

use crate::app::{confirm, error_text, show_error, toast, Ctx};
use crate::engine::data;
use crate::engine::fsx;
use crate::engine::programs::{self, Program};
use crate::i18n;
use crate::{MainWindow, ProgItem, UninstallState};

struct Row {
    program: Program,
    name: String,
    publisher: String,
    can_open: bool,
}

fn rows_of(list: Vec<Program>) -> Vec<Row> {
    list.into_iter()
        .map(|program| Row {
            name: program.name.to_lowercase(),
            publisher: program.publisher.to_lowercase(),
            can_open: !program.location.is_empty() && fsx::plain_dir(std::path::Path::new(&program.location)) == Some(true),
            program,
        })
        .collect()
}

#[derive(Default)]
struct State {
    all: Vec<Row>,
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
        let mut rows: Vec<(usize, &Row)> = s.all.iter().enumerate().filter(|(_, r)| needle.is_empty() || r.name.contains(&needle) || r.publisher.contains(&needle)).collect();
        match s.sort {
            1 => rows.sort_by(|a, b| b.1.program.size.cmp(&a.1.program.size).then_with(|| a.1.name.cmp(&b.1.name))),
            2 => rows.sort_by(|a, b| b.1.program.date.cmp(&a.1.program.date).then_with(|| a.1.name.cmp(&b.1.name))),
            _ => rows.sort_by(|a, b| a.1.name.cmp(&b.1.name)),
        }
        let total: u64 = rows.iter().map(|(_, r)| r.program.size).sum();
        let items: Vec<ProgItem> = rows
            .iter()
            .map(|(i, r)| {
                let p = &r.program;
                ProgItem {
                    idx: *i as i32,
                    name: crate::views::ltr(&p.name).into(),
                    sub: crate::views::ltr(&[p.publisher.as_str(), p.version.as_str()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join("  ·  ")).into(),
                    size: if p.size > 0 { data::format_size(p.size) } else { String::new() }.into(),
                    date: p.date.as_str().into(),
                    tint: tint_of(&p.name),
                    can_open: r.can_open,
                }
            })
            .collect();
        let st = ui.global::<UninstallState>();
        st.set_count_text(i18n::fmt("uninst.count", &[&items.len().to_string(), &data::format_size(total)]).into());
        st.set_sort(s.sort);
        if let Some(model) = crate::views::sync_model(&st.get_programs(), items) {
            st.set_programs(model);
            st.set_programs_epoch(crate::views::bump(&st.get_programs_epoch()));
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
    crate::engine::pool::spawn(move || {
        let _reset = crate::app::panic_reset(&weak, |ui| ui.global::<UninstallState>().set_loading(false));
        let list = rows_of(programs::list());
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
    STATE.with(|s| s.borrow().all.get(idx as usize).map(|r| r.program.clone()))
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
            if b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' && fsx::plain_dir(std::path::Path::new(&p.location)) == Some(true) {
                let _ = crate::engine::sys::launch("explorer.exe", &crate::engine::sys::quoted(std::path::Path::new(&p.location)));
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
                    Err(e) => show_error(&c2, error_text(&e)),
                }),
            );
        }
    });
}

pub fn refresh(ui: &MainWindow) {
    if STATE.with(|s| s.borrow().loaded) {
        push(ui);
    }
}
