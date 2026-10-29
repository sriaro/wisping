use crate::engine::cleaner::{self, CleanTarget};
use crate::engine::{data, history, report, restorepoint, settings, sys};
use crate::i18n;

pub fn clean() -> i32 {
    std::panic::set_hook(Box::new(|info| data::log(&format!("panic {}", info))));
    if !crate::winui::claim_instance(0) {
        return 3;
    }
    settings::load();
    data::harden_default();
    cleaner::set_keep_recent(data::read_setting("keep-recent.txt").is_none_or(|v| v != "all"));
    let admin = sys::is_admin();
    let mut targets = cleaner::targets();
    if let Some(ids) = settings::clean_list() {
        for t in &mut targets {
            t.selected = ids.contains(&t.id);
        }
    }
    let idx: Vec<usize> = (0..targets.len()).filter(|&i| targets[i].selected && !(targets[i].needs_admin && !admin)).collect();
    if idx.is_empty() {
        data::log("silent clean skipped: nothing selected");
        return 2;
    }
    if admin && settings::restore_point_on() {
        restorepoint::create(&i18n::get("restore.desc.clean"));
    }
    cleaner::run_parallel(&mut targets, Some(&idx), cleaner::scan);
    let before: Vec<CleanTarget> = idx.iter().map(|&i| targets[i].clone()).collect();
    cleaner::run_parallel(&mut targets, Some(&idx), cleaner::clean);
    cleaner::run_parallel(&mut targets, Some(&idx), cleaner::scan);
    let freed: u64 = idx.iter().map(|&i| targets[i].freed).sum();
    let failed: u64 = idx.iter().map(|&i| targets[i].failed).sum();
    history::record("clean", freed, failed, idx.len() as u64);
    let lines: Vec<report::Line> = idx.iter().enumerate().map(|(n, &i)| report::Line { name: i18n::get(&format!("t.{}", targets[i].id)), before: &before[n], after: &targets[i] }).collect();
    report::write_clean(&lines);
    data::log(&format!("silent clean freed {} failed {}", freed, failed));
    if failed > 0 { 1 } else { 0 }
}
