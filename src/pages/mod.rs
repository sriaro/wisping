pub mod backups;
pub mod disk;
pub mod tools;
pub mod uninstall;

use crate::app::Ctx;

pub fn install(c: &Ctx) {
    uninstall::install(c);
    disk::install(c);
    tools::install(c);
    backups::install(c);
}

thread_local! {
    static STALE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

fn bit(page: i32) -> u8 {
    if (4..=7).contains(&page) { 1 << page } else { 0 }
}

fn refresh_page(ui: &crate::MainWindow, page: i32) {
    match page {
        4 => uninstall::refresh(ui),
        5 => disk::relocate(ui),
        6 => tools::refresh(ui),
        7 => backups::refresh(ui),
        _ => {}
    }
}

pub fn on_page(c: &Ctx, page: i32) {
    if STALE.with(|s| s.get()) & bit(page) != 0 {
        STALE.with(|s| s.set(s.get() & !bit(page)));
        if let Some(ui) = c.weak.upgrade() {
            refresh_page(&ui, page);
        }
    }
    match page {
        4 => uninstall::on_show(c),
        5 => disk::on_show(c),
        6 => tools::on_show(c),
        7 => backups::on_show(c),
        _ => {}
    }
}

pub fn relang(c: &Ctx, page: i32) {
    let Some(ui) = c.weak.upgrade() else { return };
    STALE.with(|s| s.set(0b1111_0000 & !bit(page)));
    refresh_page(&ui, page);
}
