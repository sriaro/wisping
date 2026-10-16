use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app::Core;
use crate::engine::classify;
use crate::engine::cleaner::{self, CleanTarget};
use crate::engine::data;
use crate::engine::drives;
use crate::engine::regbackup::Hive;
use crate::engine::registry::RegIssue;
use crate::engine::startup::{StartupEntry, StartupKind};
use crate::i18n;
use crate::{AppState, CatItem, DriveItem, IssueItem, MainWindow, StartItem, TopItem};

pub fn cat_style(id: &str) -> (&'static str, i32) {
    match id {
        "usertemp" => ("folder", 0),
        "wintemp" => ("grid", 1),
        "thumb" => ("image", 2),
        "recent" => ("clock", 7),
        "crash" => ("alert", 3),
        "shader" => ("cpu", 2),
        "wu" => ("download", 1),
        "do" => ("share", 5),
        "logs" => ("file", 7),
        "chrome" => ("globe", 5),
        "edge" => ("globe", 1),
        "brave" => ("globe", 3),
        "firefox" => ("globe", 4),
        "otherbrowsers" => ("globe", 2),
        "history" => ("clock", 4),
        "cookies" => ("globe", 6),
        "explorer" => ("folder", 5),
        "prefetch" => ("zap", 3),
        "sessions" => ("clock", 3),
        "metrics" => ("alert", 7),
        "iecache" => ("globe", 1),
        "iedata" => ("globe", 2),
        "wintools" => ("wrench", 7),
        "rdp" => ("share", 1),
        "defender" => ("shield", 1),
        "steam" => ("zap", 1),
        "onedrive" => ("download", 1),
        "nvidia" => ("cpu", 5),
        "winrar" => ("archive", 3),
        "devcache" => ("box", 2),
        "recycle" => ("trash", 6),
        "aiapps" => ("sparkles", 4),
        "aicli" => ("file", 4),
        "aigpu" => ("cpu", 4),
        "aimodelcache" => ("download", 4),
        "aihistory" => ("clock", 4),
        "aimodels" => ("database", 4),
        _ => ("box", 7),
    }
}

pub fn issue_tint(kind: &str) -> i32 {
    match kind {
        "shareddll" => 1,
        "apppath" => 2,
        "uninstall" => 3,
        "com" => 4,
        "extension" => 5,
        "openwith" => 6,
        "trayicon" => 3,
        "leftover" => 5,
        "riskyleft" => 6,
        _ => 7,
    }
}

pub fn count_label(entries: bool, n: u64) -> String {
    match (n, entries) {
        (1, false) => i18n::get("count.file.one"),
        (1, true) => i18n::get("count.entry.one"),
        (_, false) => i18n::fmt("count.files", &[&n.to_string()]),
        (_, true) => i18n::fmt("count.entries", &[&n.to_string()]),
    }
}

pub fn cat_item(t: &CleanTarget, admin: bool) -> CatItem {
    let (icon, tint) = cat_style(&t.id);
    CatItem {
        id: t.id.as_str().into(),
        group: i18n::get(&format!("g.{}", cleaner::group_of(&t.id))).into(),
        name: i18n::get(&format!("t.{}", t.id)).into(),
        desc: i18n::get(&format!("d.{}", t.id)).into(),
        risk: i18n::get(&format!("risk.{}", cleaner::risk_of(&t.id))).into(),
        risk_tone: cleaner::risk_of(&t.id) as i32,
        icon: icon.into(),
        tint,
        size: if t.reg.is_empty() { data::format_size(t.bytes) } else { String::new() }.into(),
        count: count_label(!t.reg.is_empty(), t.count).into(),
        selected: t.selected,
        locked: t.needs_admin && !admin,
        scanned: t.scanned,
    }
}

pub fn ltr(text: &str) -> String {
    let control = |c: char| matches!(c as u32, 0x061C | 0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069);
    let clean: String = text.chars().map(|c| if control(c) { '\u{FFFD}' } else { c }).collect();
    let rtl = clean.chars().any(|c| matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF));
    if rtl { format!("\u{200E}{}", clean) } else { clean }
}

pub fn mid_ellipsis(s: &str, max: usize) -> String {
    if max < 3 {
        return s.chars().take(max).collect();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let head = max / 3;
    let tail = max - head - 1;
    let a: String = chars[..head].iter().collect();
    let b: String = chars[chars.len() - tail..].iter().collect();
    format!("{}…{}", a, b)
}

pub fn detail_style(name: &str, reg: bool) -> (&'static str, i32) {
    if reg {
        return ("database", 2);
    }
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "gguf" | "safetensors" | "onnx" | "pt" | "pth" | "ckpt" => return ("cpu", 2),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" => return ("image", 4),
        "log" | "dmp" | "etl" | "wer" | "tmp" => return ("alert", 3),
        _ => {}
    }
    if name.contains(':') && !name.contains('\\') && !name.contains('/') {
        return ("cpu", 2);
    }
    if !ext.is_empty() && ext.len() <= 5 && !name.contains('\\') && !name.contains('/') {
        return ("file", 7);
    }
    ("folder", 1)
}

pub fn fill_details(ui: &MainWindow, rows: &[cleaner::Detail]) {
    let st = ui.global::<AppState>();
    let bytes: u64 = rows.iter().map(|d| d.bytes).sum();
    let files: u64 = rows.iter().filter(|d| d.place != "Registry").map(|d| d.count).sum();
    let entries: u64 = rows.iter().filter(|d| d.place == "Registry").map(|d| d.count).sum();
    let sub = if files == 0 && entries > 0 {
        i18n::fmt("details.sub.reg", &[&entries.to_string()])
    } else if rows.is_empty() {
        String::new()
    } else {
        i18n::fmt("details.sub", &[&data::format_size(bytes), &files.to_string()])
    };
    let items: Vec<crate::DetailRow> = rows
        .iter()
        .map(|d| {
            let reg = d.place == "Registry";
            let (icon, tint) = detail_style(&d.name, reg);
            let share = if reg || bytes == 0 { 0.0 } else { d.bytes as f32 / bytes as f32 };
            let percent = if reg || bytes == 0 {
                String::new()
            } else if share < 0.01 {
                "<1%".to_string()
            } else {
                format!("{}%", (share * 100.0).round() as u32)
            };
            crate::DetailRow {
                name: ltr(&d.name).into(),
                place: ltr(&mid_ellipsis(&d.place, 62)).into(),
                size: if reg { String::new() } else { data::format_size(d.bytes) }.into(),
                count: count_label(reg, d.count).into(),
                icon: icon.into(),
                tint,
                share,
                percent: percent.into(),
                path: d.path.as_str().into(),
            }
        })
        .collect();
    st.set_details_total(if bytes > 0 { data::format_size(bytes) } else { String::new() }.into());
    st.set_details_sub(sub.as_str().into());
    st.set_details_rows(ModelRc::new(VecModel::from(items)));
    st.set_details_loading(false);
}

pub fn issue_locked(i: &RegIssue, admin: bool) -> bool {
    i.hive == Hive::Machine && !admin
}

pub fn lock_issues(g: &mut Core) {
    let admin = g.admin;
    for i in g.issues.iter_mut() {
        if issue_locked(i, admin) {
            i.selected = false;
        }
    }
}

pub fn issue_item(i: &RegIssue, admin: bool) -> IssueItem {
    IssueItem {
        kind_name: i18n::get(&format!("r.{}", i.kind)).into(),
        title: ltr(&i.display()).into(),
        detail: ltr(&if i.kind == "riskyleft" { format!("{}  ·  {}", i.detail, i18n::get("risky.note")) } else { i.detail.clone() }).into(),
        tint: issue_tint(&i.kind),
        selected: i.selected,
        locked: issue_locked(i, admin),
    }
}

pub fn needs_admin(e: &StartupEntry, delete: bool) -> bool {
    match e.kind {
        StartupKind::Service => true,
        StartupKind::Task => false,
        StartupKind::Context => e.hive == Hive::Machine || (!delete && !e.value_name.is_empty()),
        _ => e.hive == Hive::Machine,
    }
}

pub fn start_item(e: &StartupEntry, admin: bool, idx: usize) -> StartItem {
    let (kind_key, icon, tint) = match e.kind {
        StartupKind::Run => ("s.run", "power", 1),
        StartupKind::Folder => ("s.folder", "power", 5),
        StartupKind::Packaged => ("s.packaged", "power", 2),
        StartupKind::Task => ("s.task", "clock", 3),
        StartupKind::Context => ("s.context", "wrench", 4),
        StartupKind::Service => ("s.service", "cpu", 6),
    };
    let kind_name = if e.detail.is_empty() { i18n::get(kind_key) } else { e.detail.clone() };
    let toggle_ok = !needs_admin(e, false) || admin;
    let delete_ok = !needs_admin(e, true) || admin;
    let (role, advice) = classify::classify(e);
    StartItem {
        idx: idx as i32,
        icon: icon.into(),
        publisher: ltr(&e.publisher).into(),
        system: e.system,
        name: ltr(&e.name).into(),
        command: ltr(&e.command).into(),
        kind_name: kind_name.into(),
        tint,
        enabled: e.enabled,
        can_toggle: !e.policy && !e.system && toggle_ok,
        can_delete: e.kind != StartupKind::Packaged && !e.system && delete_ok,
        missing: e.missing,
        admin: !e.system && (needs_admin(e, false) || needs_admin(e, true)) && !admin,
        tag: format!("{}  ·  {}", i18n::get(&format!("role.{}", role.key())), i18n::get(&format!("advice.{}", advice.key()))).into(),
        tone: advice.tone(),
    }
}

pub fn selected_bytes(core: &Core) -> u64 {
    core.targets.iter().filter(|t| t.selected && !(t.needs_admin && !core.admin)).map(|t| t.bytes).sum()
}

pub fn push_cats(ui: &MainWindow, core: &Core) {
    let items: Vec<CatItem> = core.targets.iter().map(|t| cat_item(t, core.admin)).collect();
    let st = ui.global::<AppState>();
    if let Some(m) = sync_model(&st.get_cats(), items) {
        st.set_cats(m);
    }
    push_clean_summary(ui, core);
}

pub fn push_clean_summary(ui: &MainWindow, core: &Core) {
    let st = ui.global::<AppState>();
    let any_scanned = core.targets.iter().any(|t| t.scanned);
    let any_selected = core.targets.iter().any(|t| t.selected && !(t.needs_admin && !core.admin));
    st.set_clean_selected_text(if any_scanned { i18n::fmt("clean.selected", &[&data::format_size(selected_bytes(core))]) } else { i18n::get("clean.unscanned") }.into());
    st.set_can_run_clean(any_selected);
}

pub fn push_issues(ui: &MainWindow, core: &Core) {
    let items: Vec<IssueItem> = core.issues.iter().map(|i| issue_item(i, core.admin)).collect();
    let st = ui.global::<AppState>();
    if let Some(m) = sync_model(&st.get_issues(), items) {
        st.set_issues(m);
    }
    st.set_reg_scanned(core.reg_scanned);
    push_reg_summary(ui, core);
}

pub fn push_reg_summary(ui: &MainWindow, core: &Core) {
    let st = ui.global::<AppState>();
    let selected = core.issues.iter().filter(|i| i.selected).count();
    st.set_reg_count_text(i18n::fmt("reg.found", &[&core.issues.len().to_string()]).into());
    st.set_reg_selected_text(i18n::fmt("reg.selected", &[&selected.to_string()]).into());
    st.set_can_fix(selected > 0);
}

pub fn push_startups(ui: &MainWindow, core: &Core) {
    let st = ui.global::<AppState>();
    let tab = st.get_start_tab().clamp(0, 3) as usize;
    let show_system = st.get_start_show_system();
    let shown = |e: &StartupEntry| e.kind.group() == tab && (show_system || !e.system);
    let items: Vec<StartItem> = core.startups.iter().enumerate().filter(|(_, e)| shown(e)).map(|(i, e)| start_item(e, core.admin, i)).collect();
    let on = core.startups.iter().filter(|e| shown(e) && e.enabled).count();
    let total = items.len();
    let names: Vec<slint::SharedString> = (0..4)
        .map(|g| {
            let n = core.startups.iter().filter(|e| e.kind.group() == g && (show_system || !e.system)).count();
            format!("{}  {}", i18n::get(&format!("start.tab.{}", g)), n).into()
        })
        .collect();
    st.set_start_tab_names(ModelRc::new(VecModel::from(names)));
    st.set_start_sub(i18n::get(&format!("start.sub.{}", tab)).into());
    if let Some(m) = sync_model(&st.get_startups(), items) {
        st.set_startups(m);
    }
    let safe = core.startups.iter().filter(|e| shown(e) && e.enabled && classify::classify(e).1 == classify::Advice::Safe && !e.policy).count();
    let key = if safe > 0 { "start.count.safe" } else { "start.count" };
    st.set_start_count_text(i18n::fmt(key, &[&on.to_string(), &total.to_string(), &safe.to_string()]).into());
}

pub fn windows_on(core: &Core) -> usize {
    core.startups.iter().filter(|e| e.kind.group() == 0 && e.enabled).count()
}

pub fn push_overview_cards(st: &AppState, core: &Core) {
    let drives: Vec<DriveItem> = drives::fixed()
        .iter()
        .take(4)
        .map(|d| {
            let used = d.used_fraction();
            DriveItem {
                name: d.name.as_str().into(),
                text: i18n::fmt("home.drive.free", &[&data::format_size(d.free), &data::format_size(d.total)]).into(),
                frac: used,
                tone: if used >= 0.92 { 2 } else if used >= 0.8 { 1 } else { 0 },
            }
        })
        .collect();
    if let Some(m) = sync_model(&st.get_drives(), drives) {
        st.set_drives(m);
    }
    let mut scanned: Vec<&CleanTarget> = core.targets.iter().filter(|t| t.scanned && t.bytes > 0 && t.reg.is_empty()).collect();
    scanned.sort_by_key(|t| std::cmp::Reverse(t.bytes));
    let tops: Vec<TopItem> = scanned
        .iter()
        .take(4)
        .map(|t| TopItem {
            name: i18n::get(&format!("t.{}", t.id)).into(),
            size: data::format_size(t.bytes).into(),
            risk: i18n::get(&format!("risk.{}", cleaner::risk_of(&t.id))).into(),
            risk_tone: cleaner::risk_of(&t.id) as i32,
        })
        .collect();
    if let Some(m) = sync_model(&st.get_tops(), tops) {
        st.set_tops(m);
    }
}

pub fn push_home(ui: &MainWindow, core: &Core) {
    let st = ui.global::<AppState>();
    if st.get_busy() && st.get_ring_mode() == 1 {
        return;
    }
    push_overview_cards(&st, core);
    let on = windows_on(core);
    st.set_can_clean_now(false);
    let everything = core.reg_scanned && core.targets.iter().any(|t| t.scanned);
    if !everything {
        st.set_ring_mode(0);
        st.set_hero_title(i18n::get("hero.idle.title").into());
        st.set_hero_text(i18n::get("hero.idle.text").into());
        st.set_ring_big("".into());
        st.set_ring_small(i18n::get("ring.idle").into());
        st.set_tile_junk("—".into());
        st.set_tile_reg("—".into());
        st.set_tile_start(format!("{}", on).into());
        return;
    }
    let bytes = selected_bytes(core);
    let size = data::format_size(bytes);
    st.set_ring_mode(2);
    st.set_ring_big(size.as_str().into());
    st.set_ring_small(i18n::get("ring.done").into());
    st.set_hero_title(if bytes == 0 { i18n::get("hero.clean.title") } else { i18n::fmt("hero.done.title", &[&size]) }.into());
    st.set_hero_text(i18n::fmt("hero.done.text", &[&core.issues.len().to_string(), &on.to_string(), &core.startups.iter().filter(|e| e.kind.group() == 0).count().to_string()]).into());
    st.set_tile_junk(size.as_str().into());
    st.set_tile_reg(core.issues.len().to_string().into());
    st.set_tile_start(on.to_string().into());
    st.set_can_clean_now(bytes > 0);
}

pub fn push_all(ui: &MainWindow, core: &Core) {
    push_cats(ui, core);
    push_issues(ui, core);
    push_startups(ui, core);
    push_home(ui, core);
}

pub fn sync_model<T: Clone + PartialEq + 'static>(current: &ModelRc<T>, items: Vec<T>) -> Option<ModelRc<T>> {
    if let Some(vm) = current.as_any().downcast_ref::<VecModel<T>>()
        && vm.row_count() == items.len() {
            for (i, item) in items.into_iter().enumerate() {
                if vm.row_data(i).as_ref() != Some(&item) {
                    vm.set_row_data(i, item);
                }
            }
            return None;
        }
    Some(ModelRc::new(VecModel::from(items)))
}

pub fn set_row<M: Clone + 'static>(model: ModelRc<M>, i: usize, edit: impl FnOnce(&mut M)) {
    if let Some(mut row) = model.row_data(i) {
        edit(&mut row);
        model.set_row_data(i, row);
    }
}

