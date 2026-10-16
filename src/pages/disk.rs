use std::cell::RefCell;
use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

use crate::app::Ctx;
use crate::engine::data;
use crate::engine::diskmap::dups::{self, Group};
use crate::engine::diskmap::owners::{self, Owners};
use crate::engine::diskmap::treemap;
use crate::engine::diskmap::{self, category_of, Kind, Progress, Tree};
use crate::i18n;
use crate::{DiskRow, DiskState, LegendItem, MainWindow, TileItem};

const MAX_TILES: usize = 90;
const MAX_ROWS: usize = 250;

#[derive(Default)]
struct State {
    locations: Vec<(String, PathBuf)>,
    location: usize,
    tree: Option<Arc<Tree>>,
    owners: Option<Arc<Owners>>,
    groups: Arc<Vec<Group>>,
    group_of: HashMap<u32, usize>,
    dups_ready: bool,
    largest: Vec<u32>,
    current: u32,
    selected: Option<u32>,
    area: (f64, f64),
    list_mode: i32,
    dup_open: Option<usize>,
    owner_open: Option<usize>,
    crumb_ids: Vec<u32>,
    progress: Option<Arc<Progress>>,
    dup_cancel: Arc<AtomicBool>,
    scan_id: u64,
    scanning: bool,
    partial: bool,
    poll: Option<Timer>,
}

type ListKey = (i32, Option<usize>, Option<usize>, u32, u64, usize, bool);

thread_local! {
    static SHOWN_LIST: std::cell::Cell<Option<ListKey>> = const { std::cell::Cell::new(None) };
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { area: (900.0, 520.0), ..Default::default() });
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn locations() -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{}:\\", letter);
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let kind = unsafe { GetDriveTypeW(windows::core::PCWSTR(wide.as_ptr())) };
        if kind == 2 || kind == 3 {
            let system = std::env::var("SystemDrive").is_ok_and(|d| d.to_ascii_uppercase().starts_with(letter));
            let name = if system { i18n::get("disk.loc.system") } else { i18n::fmt("disk.loc.fixed", &[&letter.to_string()]) };
            v.push((name, PathBuf::from(root)));
        }
    }
    if let Ok(p) = std::env::var("USERPROFILE") {
        v.push((i18n::get("disk.loc.user"), PathBuf::from(p)));
    }
    if let Ok(p) = std::env::var("LOCALAPPDATA") {
        v.push((i18n::get("disk.loc.appdata"), PathBuf::from(p)));
    }
    v
}

fn pct(part: u64, whole: u64) -> String {
    if whole == 0 { "0%".to_string() } else { format!("{:.1}%", part as f64 * 100.0 / whole as f64) }
}

fn node_label(tree: &Tree, idx: u32) -> String {
    tree.name_of(idx).to_string()
}

fn tint_of(tree: &Tree, idx: u32) -> i32 {
    let n = &tree.nodes[idx as usize];
    match n.kind {
        Kind::Dir => tree.dominant(idx) as i32,
        Kind::File => category_of(tree.name(n)) as i32,
        Kind::Small => 7,
    }
}

fn kind_code(k: Kind) -> i32 {
    match k {
        Kind::Dir => 0,
        Kind::File => 1,
        Kind::Small => 2,
    }
}

const MIN_TILE: f64 = 6.0;

struct MapCtx<'a> {
    tree: &'a Tree,
    area: (f64, f64),
    selected: Option<u32>,
    tiles: Vec<TileItem>,
}

impl MapCtx<'_> {
    fn fill(&mut self, node: u32, rect: treemap::Rect, depth: usize) {
        let tree = self.tree;
        let limit = if depth == 0 { MAX_TILES } else { 28 };
        let kids: Vec<u32> = tree.sorted_children(node).into_iter().filter(|&c| tree.nodes[c as usize].size > 0).collect();
        let shown = kids.len().min(limit);
        let mut sizes: Vec<u64> = kids[..shown].iter().map(|&c| tree.nodes[c as usize].size).collect();
        let rest: u64 = kids[shown..].iter().map(|&c| tree.nodes[c as usize].size).sum();
        if rest > 0 {
            sizes.push(rest);
        }
        let rects = treemap::layout(&sizes, rect.w, rect.h);
        for (i, r) in rects.iter().enumerate() {
            if r.w < MIN_TILE || r.h < MIN_TILE {
                continue;
            }
            let abs = treemap::Rect { x: rect.x + r.x, y: rect.y + r.y, w: r.w, h: r.h };
            let (idx, name, size, tint, kind) = if i < shown {
                let c = kids[i];
                let n = &tree.nodes[c as usize];
                (c as i32, tree.name(n).to_string(), n.size, tint_of(tree, c), kind_code(n.kind))
            } else {
                (-1, i18n::fmt("disk.other", &[&(kids.len() - shown).to_string()]), rest, 7, 3)
            };
            let slot = self.tiles.len();
            self.tiles.push(TileItem {
                idx,
                x: (abs.x / self.area.0) as f32,
                y: (abs.y / self.area.1) as f32,
                w: (abs.w / self.area.0) as f32,
                h: (abs.h / self.area.1) as f32,
                name: crate::views::ltr(&name).into(),
                size: data::format_size(size).into(),
                tint,
                kind,
                selected: self.selected == Some(idx as u32) && idx >= 0,
                nested: false,
            });
            let is_dir = idx >= 0 && tree.nodes[idx as usize].kind == Kind::Dir;
            if is_dir && depth < 3 && abs.w > 120.0 && abs.h > 76.0 && self.tiles.len() < 900 {
                let inner = treemap::Rect { x: abs.x + 4.0, y: abs.y + 22.0, w: abs.w - 8.0, h: abs.h - 26.0 };
                self.fill(idx as u32, inner, depth + 1);
                if self.tiles.len() > slot + 1 {
                    self.tiles[slot].nested = true;
                }
            }
        }
    }
}

fn push_map(ui: &MainWindow, s: &State, fresh: bool) {
    let st = ui.global::<DiskState>();
    let Some(tree) = &s.tree else {
        st.set_tiles(ModelRc::new(VecModel::from(Vec::<TileItem>::new())));
        return;
    };
    let mut m = MapCtx { tree, area: s.area, selected: s.selected, tiles: Vec::new() };
    m.fill(s.current, treemap::Rect { x: 0.0, y: 0.0, w: s.area.0, h: s.area.1 }, 0);
    if fresh {
        use slint::Model;
        let next = st.get_epoch().row_data(0).unwrap_or(0).wrapping_add(1);
        st.set_epoch(ModelRc::new(VecModel::from(vec![next])));
        st.set_tiles(ModelRc::new(VecModel::from(m.tiles)));
    } else if let Some(model) = crate::views::sync_model(&st.get_tiles(), m.tiles) {
        st.set_tiles(model);
    }
}

fn root_label(locations: &[(String, PathBuf)], current: usize, tree: &diskmap::Tree) -> String {
    let root = tree.root.to_string_lossy();
    let trimmed = root.trim_end_matches(char::from(92));
    let is_drive = trimmed.len() == 2 && trimmed.ends_with(char::from(58));
    match locations.get(current) {
        Some((name, path)) if is_drive && *path == tree.root => name.clone(),
        _ => trimmed.to_string(),
    }
}

fn push_crumbs(ui: &MainWindow, s: &mut State) {
    let Some(tree) = &s.tree else {
        s.crumb_ids.clear();
        ui.global::<DiskState>().set_crumbs(ModelRc::new(VecModel::from(Vec::<slint::SharedString>::new())));
        return;
    };
    let chain = tree.ancestors(s.current);
    let skip = chain.len().saturating_sub(5);
    s.crumb_ids = chain[skip..].to_vec();
    let names: Vec<slint::SharedString> = s
        .crumb_ids
        .iter()
        .map(|&c| if c == 0 { root_label(&s.locations, s.location, tree) } else { node_label(tree, c) })
        .map(|n| crate::views::ltr(&n).into())
        .collect();
    ui.global::<DiskState>().set_crumbs(ModelRc::new(VecModel::from(names)));
}

fn push_legend(ui: &MainWindow, s: &State) {
    let Some(tree) = &s.tree else { return };
    let cats = tree.cats(s.current);
    let mut order: Vec<usize> = (0..diskmap::CATS).filter(|&i| cats[i] > 0).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(cats[i]));
    let items: Vec<LegendItem> = order
        .into_iter()
        .take(5)
        .map(|i| LegendItem { tint: i as i32, name: i18n::get(&format!("cat.{}", i)).into(), size: data::format_size(cats[i]).into() })
        .collect();
    ui.global::<DiskState>().set_legend(ModelRc::new(VecModel::from(items)));
}

fn push_folder_text(ui: &MainWindow, s: &State) {
    let Some(tree) = &s.tree else { return };
    let n = &tree.nodes[s.current as usize];
    let mut text = i18n::fmt("disk.folder", &[&data::format_size(n.size), &n.files.to_string()]);
    if s.partial {
        text.push_str("  ·  ");
        text.push_str(&i18n::get("disk.partial"));
    }
    ui.global::<DiskState>().set_folder_text(text.into());
}

fn push_selection(ui: &MainWindow, s: &State) {
    let st = ui.global::<DiskState>();
    let (Some(tree), Some(idx)) = (&s.tree, s.selected) else {
        st.set_sel_valid(false);
        return;
    };
    let n = &tree.nodes[idx as usize];
    let parent_size = tree.nodes[n.parent.min(tree.nodes.len() as u32 - 1) as usize].size;
    let tint = tint_of(tree, idx);
    st.set_sel_valid(true);
    st.set_sel_title(crate::views::ltr(tree.name(n)).into());
    st.set_sel_path(crate::views::ltr(&tree.path_of(idx).to_string_lossy()).into());
    st.set_sel_dir(n.kind == Kind::Dir);
    st.set_sel_tint(tint);
    st.set_sel_type(if n.kind == Kind::Dir { i18n::get("disk.type.folder") } else { i18n::get(&format!("cat.{}", tint)) }.into());
    let meta = match n.kind {
        Kind::Dir => i18n::fmt("disk.meta.dir", &[&data::format_size(n.size), &n.files.to_string(), &pct(n.size, parent_size)]),
        _ => i18n::fmt("disk.meta.file", &[&data::format_size(n.size), &pct(n.size, parent_size)]),
    };
    st.set_sel_meta(meta.into());
    let owner = s.owners.as_ref().and_then(|o| o.of(tree, idx).map(|i| o.list[i].name.clone())).unwrap_or_default();
    st.set_sel_owner(crate::views::ltr(&owner).into());
    let copies = s.group_of.get(&idx).map(|g| i18n::fmt("disk.copies", &[&s.groups[*g].members.len().to_string()])).unwrap_or_default();
    st.set_sel_copies(copies.into());
}

fn row(idx: i32, title: String, sub: String, size: u64, frac: f64, tint: i32, kind: i32) -> DiskRow {
    DiskRow { idx, title: crate::views::ltr(&title).into(), sub: crate::views::ltr(&sub).into(), size: data::format_size(size).into(), frac: frac.clamp(0.0, 1.0) as f32, tint, kind }
}

fn push_list(ui: &MainWindow, s: &State) {
    let st = ui.global::<DiskState>();
    let Some(tree) = &s.tree else {
        st.set_rows(ModelRc::new(VecModel::from(Vec::<DiskRow>::new())));
        st.set_list_back("".into());
        return;
    };
    let mut back = String::new();
    let mut rows: Vec<DiskRow> = Vec::new();
    let parent_dir = |c: u32| tree.path_of(tree.nodes[c as usize].parent).to_string_lossy().to_string();
    match s.list_mode {
        0 => {
            let total = tree.nodes[s.current as usize].size.max(1);
            for c in tree.sorted_children(s.current).into_iter().take(MAX_ROWS) {
                let n = &tree.nodes[c as usize];
                if n.size == 0 {
                    continue;
                }
                let sub = match n.kind {
                    Kind::Dir => i18n::fmt("disk.files", &[&n.files.to_string()]),
                    Kind::File => i18n::get(&format!("cat.{}", category_of(tree.name(n)))),
                    Kind::Small => i18n::get("cat.7"),
                };
                rows.push(row(c as i32, tree.name(n).to_string(), sub, n.size, n.size as f64 / total as f64, tint_of(tree, c), kind_code(n.kind)));
            }
        }
        1 => {
            let top = s.largest.first().map(|&c| tree.nodes[c as usize].size).unwrap_or(1).max(1);
            for &c in s.largest.iter().take(MAX_ROWS) {
                let n = &tree.nodes[c as usize];
                rows.push(row(c as i32, tree.name(n).to_string(), parent_dir(c), n.size, n.size as f64 / top as f64, tint_of(tree, c), 1));
            }
        }
        2 => match s.dup_open {
            None => {
                if !s.dups_ready {
                    rows.push(row(-1, i18n::get("disk.finding"), String::new(), 0, 0.0, 7, 3));
                }
                let top = s.groups.first().map(|g| g.wasted()).unwrap_or(1).max(1);
                for (gi, g) in s.groups.iter().enumerate().take(MAX_ROWS) {
                    let first = g.members[0];
                    rows.push(row(
                        gi as i32,
                        node_label(tree, first),
                        i18n::fmt("disk.copies.row", &[&g.members.len().to_string(), &data::format_size(g.wasted())]),
                        g.wasted(),
                        g.wasted() as f64 / top as f64,
                        tint_of(tree, first),
                        1,
                    ));
                }
                if s.dups_ready && s.groups.is_empty() {
                    rows.push(row(-1, i18n::get("disk.dups.none"), String::new(), 0, 0.0, 7, 3));
                }
            }
            Some(g) => {
                back = i18n::get("disk.back.dups");
                if let Some(group) = s.groups.get(g) {
                    for &m in &group.members {
                        rows.push(row(m as i32, node_label(tree, m), parent_dir(m), group.size, 1.0, tint_of(tree, m), 1));
                    }
                }
            }
        },
        _ => {
            let Some(owners) = &s.owners else {
                st.set_rows(ModelRc::new(VecModel::from(rows)));
                st.set_list_back("".into());
                return;
            };
            match s.owner_open {
                None => {
                    let top = owners.list.first().map(|o| o.size).unwrap_or(1).max(1);
                    for (oi, o) in owners.list.iter().enumerate().take(MAX_ROWS) {
                        rows.push(row(oi as i32, o.name.clone(), i18n::fmt("disk.folders", &[&o.dirs.len().to_string()]), o.size, o.size as f64 / top as f64, (oi % 7) as i32, 0));
                    }
                }
                Some(oi) => {
                    back = i18n::get("disk.back.apps");
                    if let Some(o) = owners.list.get(oi) {
                        let mut dirs = o.dirs.clone();
                        dirs.sort_by_key(|&d| std::cmp::Reverse(tree.nodes[d as usize].size));
                        let top = dirs.first().map(|&d| tree.nodes[d as usize].size).unwrap_or(1).max(1);
                        for d in dirs {
                            let n = &tree.nodes[d as usize];
                            rows.push(row(d as i32, tree.path_of(d).to_string_lossy().to_string(), i18n::fmt("disk.files", &[&n.files.to_string()]), n.size, n.size as f64 / top as f64, (oi % 7) as i32, 0));
                        }
                    }
                }
            }
        }
    }
    st.set_list_mode(s.list_mode);
    st.set_list_back(back.into());
    let key = (s.list_mode, s.dup_open, s.owner_open, s.current, s.scan_id, s.location, s.list_mode == 2 && s.dups_ready);
    if SHOWN_LIST.with(|k| k.replace(Some(key))) != Some(key) {
        use slint::Model;
        let next = st.get_list_epoch().row_data(0).unwrap_or(0).wrapping_add(1);
        st.set_list_epoch(ModelRc::new(VecModel::from(vec![next])));
    }
    st.set_rows(ModelRc::new(VecModel::from(rows)));
}

fn push_all(ui: &MainWindow, s: &mut State) {
    let st = ui.global::<DiskState>();
    st.set_has_tree(s.tree.is_some());
    st.set_location(s.location as i32);
    st.set_scanning(s.scanning);
    push_crumbs(ui, s);
    push_map(ui, s, true);
    push_legend(ui, s);
    push_folder_text(ui, s);
    push_selection(ui, s);
    push_list(ui, s);
}

fn go_to(ui: &MainWindow, s: &mut State, node: u32) {
    let Some(tree) = &s.tree else { return };
    let Some(n) = tree.nodes.get(node as usize) else { return };
    if n.kind == Kind::Dir {
        s.current = node;
        s.selected = None;
    } else {
        s.current = n.parent;
        s.selected = Some(node);
    }
    s.dup_open = None;
    s.owner_open = None;
    if s.list_mode == 2 || s.list_mode == 3 {
        s.list_mode = 0;
    }
    push_all(ui, s);
}

fn select(ui: &MainWindow, s: &mut State, node: u32) {
    if s.tree.as_ref().is_none_or(|t| node as usize >= t.nodes.len()) {
        return;
    }
    s.selected = Some(node);
    mark_selected(ui, node as i32);
    push_selection(ui, s);
}

fn mark_selected(ui: &MainWindow, node: i32) {
    use slint::Model;
    let tiles = ui.global::<DiskState>().get_tiles();
    for i in 0..tiles.row_count() {
        if let Some(mut t) = tiles.row_data(i) {
            let want = t.idx == node;
            if t.selected != want {
                t.selected = want;
                tiles.set_row_data(i, t);
            }
        }
    }
}

fn start_scan(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    let Some((root, id, progress)) = with(|s| -> Option<(PathBuf, u64, Arc<Progress>)> {
        if s.scanning {
            return None;
        }
        let root = s.locations.get(s.location)?.1.clone();
        s.scan_id += 1;
        s.scanning = true;
        s.partial = false;
        s.dup_cancel.store(true, Ordering::Relaxed);
        s.dup_cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::default());
        s.progress = Some(progress.clone());
        Some((root, s.scan_id, progress))
    }) else {
        return;
    };
    let st = ui.global::<DiskState>();
    st.set_scanning(true);
    st.set_status(i18n::fmt("disk.status.scanning", &["0", "0 B"]).into());
    let weak = c.weak.clone();
    let poll = Timer::default();
    {
        let weak = weak.clone();
        let progress = progress.clone();
        poll.start(TimerMode::Repeated, Duration::from_millis(200), move || {
            if let Some(ui) = weak.upgrade() {
                let text = i18n::fmt("disk.status.scanning", &[&progress.files.load(Ordering::Relaxed).to_string(), &data::format_size(progress.bytes.load(Ordering::Relaxed))]);
                ui.global::<DiskState>().set_status(text.into());
            }
        });
    }
    with(|s| s.poll = Some(poll));
    std::thread::spawn(move || {
        let tree = diskmap::scan(&root, &progress);
        let cancelled = progress.cancel.load(Ordering::Relaxed);
        let owners = (!cancelled).then(|| owners::build(&tree));
        let tree = Arc::new(tree);
        let largest = tree.largest_files(MAX_ROWS);
        let for_dups = tree.clone();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            crate::winui::trim_soon();
            let cancel = with(|s| {
                if s.scan_id != id {
                    return None;
                }
                s.scanning = false;
                s.poll = None;
                if !cancelled {
                    s.tree = Some(tree.clone());
                    s.owners = owners.map(Arc::new);
                    s.largest = largest;
                    s.groups = Arc::new(Vec::new());
                    s.group_of.clear();
                    s.dups_ready = false;
                    s.current = 0;
                    s.selected = None;
                    s.dup_open = None;
                    s.owner_open = None;
                    s.partial = !crate::engine::sys::is_admin();
                }
                push_all(&ui, s);
                Some(s.dup_cancel.clone())
            });
            let Some(cancel) = cancel else { return };
            if cancelled {
                return;
            }
            let weak2 = ui.as_weak();
            std::thread::spawn(move || {
                let groups = dups::find(&for_dups, &cancel);
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let _ = weak2.upgrade_in_event_loop(move |ui| {
                    crate::winui::trim_soon();
                    with(|s| {
                        if s.scan_id != id {
                            return;
                        }
                        s.group_of = groups.iter().enumerate().flat_map(|(gi, g)| g.members.iter().map(move |&m| (m, gi))).collect();
                        s.groups = Arc::new(groups);
                        s.dups_ready = true;
                        push_selection(&ui, s);
                        push_list(&ui, s);
                    });
                });
            });
        });
    });
}

pub fn on_show(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    with(|s| {
        if s.locations.is_empty() {
            s.locations = locations();
            let names: Vec<slint::SharedString> = s.locations.iter().map(|(n, _)| n.as_str().into()).collect();
            ui.global::<DiskState>().set_locations(ModelRc::new(VecModel::from(names)));
        }
    });
}

pub fn reveal(path: &std::path::Path, is_dir: bool) {
    let mut cmd = std::process::Command::new(crate::engine::sys::system_tool("explorer.exe"));
    let quoted = path.to_string_lossy().contains('"');
    if is_dir || quoted {
        cmd.arg(if is_dir { path } else { path.parent().unwrap_or(path) });
    } else {
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    }
    let _ = cmd.spawn();
}

pub fn install(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    let st = ui.global::<DiskState>();
    st.on_choose_location({
        let c = c.clone();
        move |i| {
            let Some(ui) = c.weak.upgrade() else { return };
            with(|s| {
                if s.scanning {
                    return;
                }
                s.location = i as usize;
                s.tree = None;
                s.owners = None;
                s.selected = None;
                push_all(&ui, s);
            });
            ui.global::<DiskState>().set_hover_text("".into());
        }
    });
    st.on_scan({
        let c = c.clone();
        move || start_scan(&c)
    });
    st.on_cancel(|| {
        with(|s| {
            if let Some(p) = &s.progress {
                p.cancel.store(true, Ordering::Relaxed);
            }
        });
    });
    st.on_crumb({
        let c = c.clone();
        move |i| {
            let Some(ui) = c.weak.upgrade() else { return };
            with(|s| {
                if let Some(&id) = s.crumb_ids.get(i as usize) {
                    go_to(&ui, s, id);
                }
            });
        }
    });
    st.on_tile_click({
        let c = c.clone();
        move |idx| {
            let Some(ui) = c.weak.upgrade() else { return };
            if idx >= 0 {
                with(|s| select(&ui, s, idx as u32));
            }
        }
    });
    st.on_tile_open({
        let c = c.clone();
        move |idx| {
            let Some(ui) = c.weak.upgrade() else { return };
            if idx < 0 {
                return;
            }
            with(|s| {
                if s.tree.as_ref().is_some_and(|t| t.nodes.get(idx as usize).is_some_and(|n| n.kind == Kind::Dir)) {
                    go_to(&ui, s, idx as u32);
                }
            });
        }
    });
    st.on_tile_hover({
        let c = c.clone();
        move |idx| {
            let Some(ui) = c.weak.upgrade() else { return };
            let text = with(|s| {
                let tree = s.tree.as_ref()?;
                if idx < 0 {
                    return None;
                }
                let n = tree.nodes.get(idx as usize)?;
                let parent = tree.nodes.get(s.current as usize)?.size;
                Some(format!("{}  ·  {}  ·  {}", crate::views::ltr(tree.name(n)), data::format_size(n.size), pct(n.size, parent)))
            });
            ui.global::<DiskState>().set_hover_text(text.unwrap_or_default().into());
        }
    });
    st.on_row_click({
        let c = c.clone();
        move |idx| {
            let Some(ui) = c.weak.upgrade() else { return };
            if idx < 0 {
                return;
            }
            with(|s| match s.list_mode {
                0 => select(&ui, s, idx as u32),
                1 => go_to(&ui, s, idx as u32),
                2 => {
                    if s.dup_open.is_none() {
                        s.dup_open = Some(idx as usize);
                        push_list(&ui, s);
                    } else {
                        go_to(&ui, s, idx as u32);
                    }
                }
                _ => {
                    if s.owner_open.is_none() {
                        s.owner_open = Some(idx as usize);
                        push_list(&ui, s);
                    } else {
                        go_to(&ui, s, idx as u32);
                    }
                }
            });
        }
    });
    st.on_row_open({
        let c = c.clone();
        move |idx| {
            let Some(ui) = c.weak.upgrade() else { return };
            with(|s| {
                if s.list_mode == 0 && idx >= 0 && s.tree.as_ref().is_some_and(|t| t.nodes.get(idx as usize).is_some_and(|n| n.kind == Kind::Dir)) {
                    go_to(&ui, s, idx as u32);
                }
            });
        }
    });
    st.on_set_list_mode({
        let c = c.clone();
        move |m| {
            let Some(ui) = c.weak.upgrade() else { return };
            with(|s| {
                s.list_mode = m;
                s.dup_open = None;
                s.owner_open = None;
                push_list(&ui, s);
            });
        }
    });
    st.on_area_changed({
        let c = c.clone();
        move |w, h| {
            let Some(ui) = c.weak.upgrade() else { return };
            if w < 40.0 || h < 40.0 {
                return;
            }
            let area = ((w - 8.0) as f64, (h - 8.0) as f64);
            with(|s| {
                if (area.0 / s.area.0 - 1.0).abs() > 0.02 || (area.1 / s.area.1 - 1.0).abs() > 0.02 {
                    s.area = area;
                    push_map(&ui, s, false);
                }
            });
        }
    });
    st.on_open_selected({
        let c = c.clone();
        move || {
            let Some(ui) = c.weak.upgrade() else { return };
            with(|s| {
                if let Some(sel) = s.selected
                    && s.tree.as_ref().is_some_and(|t| t.nodes.get(sel as usize).is_some_and(|n| n.kind == Kind::Dir)) {
                        go_to(&ui, s, sel);
                    }
            });
        }
    });
    st.on_reveal_selected(|| {
        let target = with(|s| {
            let tree = s.tree.as_ref()?;
            let idx = s.selected.unwrap_or(s.current);
            let is_dir = tree.nodes.get(idx as usize)?.kind == Kind::Dir;
            Some((tree.path_of(idx), is_dir))
        });
        if let Some((p, dir)) = target {
            reveal(&p, dir);
        }
    });
    st.on_show_copies({
        let c = c.clone();
        move || {
            let Some(ui) = c.weak.upgrade() else { return };
            with(|s| {
                if let Some(g) = s.selected.and_then(|n| s.group_of.get(&n).copied()) {
                    s.list_mode = 2;
                    s.dup_open = Some(g);
                    push_list(&ui, s);
                }
            });
        }
    });
    st.on_show_owner({
        let c = c.clone();
        move || {
            let Some(ui) = c.weak.upgrade() else { return };
            with(|s| {
                let found = match (&s.tree, &s.owners, s.selected) {
                    (Some(t), Some(o), Some(sel)) => o.of(t, sel),
                    _ => None,
                };
                if let Some(oi) = found {
                    s.list_mode = 3;
                    s.owner_open = Some(oi);
                    push_list(&ui, s);
                }
            });
        }
    });
    st.on_go_back({
        let c = c.clone();
        move || {
            let Some(ui) = c.weak.upgrade() else { return };
            with(|s| {
                s.dup_open = None;
                s.owner_open = None;
                push_list(&ui, s);
            });
        }
    });
}

pub fn preview(ui: &MainWindow, root: PathBuf, select_largest: bool) {
    let progress = Progress::default();
    let tree = Arc::new(diskmap::scan(&root, &progress));
    let owners = Arc::new(owners::build(&tree));
    let groups = dups::find(&tree, &AtomicBool::new(false));
    with(|s| {
        s.locations = locations();
        let names: Vec<slint::SharedString> = s.locations.iter().map(|(n, _)| n.as_str().into()).collect();
        ui.global::<DiskState>().set_locations(ModelRc::new(VecModel::from(names)));
        s.location = 0;
        s.largest = tree.largest_files(MAX_ROWS);
        s.group_of = groups.iter().enumerate().flat_map(|(gi, g)| g.members.iter().map(move |&m| (m, gi))).collect();
        s.groups = Arc::new(groups);
        s.dups_ready = true;
        s.owners = Some(owners);
        s.tree = Some(tree);
        s.current = 0;
        s.selected = if select_largest { s.largest.first().copied() } else { None };
        push_all(ui, s);
    });
}

pub fn relocate(ui: &MainWindow) {
    with(|s| {
        if !s.locations.is_empty() {
            s.locations = locations();
            let names: Vec<slint::SharedString> = s.locations.iter().map(|(n, _)| n.as_str().into()).collect();
            ui.global::<DiskState>().set_locations(ModelRc::new(VecModel::from(names)));
        }
    });
}
