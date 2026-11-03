use std::cell::{Cell, RefCell};
use std::time::Duration;

use slint::{ComponentHandle, Timer};

use crate::app::{apply_fonts, enter_page, push_low_disk, Ctx};
use crate::engine::lock::LockExt;
use crate::views::push_all;
use crate::i18n;
use crate::{AppState, MainWindow, T, Theme};

const FADE_MS: u64 = 320;
const LANG_OUT_MS: u64 = 260;
const LANG_IN_MS: u64 = 500;
const FRAME_WAIT_MS: u64 = 120;

thread_local! {
    static FADE: Timer = Timer::default();
    static LANG_OUT: Timer = Timer::default();
    static LANG_IN: Timer = Timer::default();
    static WAIT: Timer = Timer::default();
    static FADING: Cell<bool> = const { Cell::new(false) };
    static SWITCHING: Cell<bool> = const { Cell::new(false) };
    static ARMED: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

thread_local! {
    static GEN: Cell<u32> = const { Cell::new(0) };
    static OUTGOING: Cell<bool> = const { Cell::new(false) };
    static WANT: Cell<i32> = const { Cell::new(0) };
    static ENTERING: RefCell<Option<(u32, Ctx)>> = const { RefCell::new(None) };
}

pub fn sync_scale(ui: &MainWindow) {
    ui.global::<Theme>().set_scale(ui.window().scale_factor());
}

pub fn after_frame() {
    if ARMED.with(|a| a.borrow().is_some()) {
        let _ = slint::invoke_from_event_loop(fire);
    }
    if ENTERING.with(|e| e.borrow().is_none()) {
        return;
    }
    let _ = slint::invoke_from_event_loop(start_entering);
}

fn arm(f: impl FnOnce() + 'static) {
    ARMED.with(|a| *a.borrow_mut() = Some(Box::new(f)));
    WAIT.with(|t| t.start(slint::TimerMode::SingleShot, Duration::from_millis(FRAME_WAIT_MS), fire));
}

fn fire() {
    WAIT.with(|t| t.stop());
    if let Some(f) = ARMED.with(|a| a.borrow_mut().take()) {
        f();
    }
}

fn start_entering() {
    let Some((epoch, c)) = ENTERING.with(|e| e.borrow_mut().take()) else { return };
    if GEN.get() == epoch {
        fade_in(&c, epoch);
    }
}

fn fade_in(c: &Ctx, epoch: u32) {
    let Some(ui) = c.weak.upgrade() else { return };
    let theme = ui.global::<Theme>();
    theme.set_veil_ms(260.0);
    theme.set_veil_target(0.0);
    let weak = c.weak.clone();
    Timer::single_shot(Duration::from_millis(280), move || {
        if GEN.get() != epoch {
            return;
        }
        if let Some(ui) = weak.upgrade() {
            ui.global::<Theme>().set_settling(false);
        }
    });
}

pub fn begin_switch(c: &Ctx, page: i32) {
    let Some(ui) = c.weak.upgrade() else { return };
    let theme = ui.global::<Theme>();
    WANT.set(page);
    if OUTGOING.get() {
        return;
    }
    let epoch = GEN.get().wrapping_add(1);
    GEN.set(epoch);
    if theme.get_shown_page() == page {
        if theme.get_veil_target() > 0.0 {
            fade_in(c, epoch);
        }
        return;
    }
    OUTGOING.set(true);
    theme.set_settling(true);
    theme.set_veil_ms(100.0);
    theme.set_veil_target(1.0);
    let c = c.clone();
    Timer::single_shot(Duration::from_millis(115), move || {
        OUTGOING.set(false);
        if GEN.get() != epoch {
            return;
        }
        let Some(ui) = c.weak.upgrade() else { return };
        let theme = ui.global::<Theme>();
        let want = WANT.get();
        sync_scale(&ui);
        theme.set_shown_page(want);
        theme.set_enter_tick(theme.get_enter_tick().wrapping_add(1));
        enter_page(&c, want);
        ENTERING.with(|e| *e.borrow_mut() = Some((epoch, c)));
        Timer::single_shot(Duration::from_millis(150), start_entering);
    });
}

fn sync_quiet(theme: &Theme) {
    theme.set_quiet(FADING.get() || SWITCHING.get());
}

fn end_fade(ui: &MainWindow) {
    FADE.with(|f| f.stop());
    let theme = ui.global::<Theme>();
    theme.set_fade_ms(0.0);
    FADING.set(false);
    sync_quiet(&theme);
}

pub fn crossfade(ui: &MainWindow, change: impl FnOnce()) {
    let theme = ui.global::<Theme>();
    theme.set_fade_ms(FADE_MS as f32);
    FADING.set(true);
    sync_quiet(&theme);
    change();
    let weak = ui.as_weak();
    FADE.with(|f| {
        f.start(slint::TimerMode::SingleShot, Duration::from_millis(FADE_MS + 40), move || {
            if let Some(ui) = weak.upgrade() {
                end_fade(&ui);
            }
        })
    });
}

fn swap_language(c: &Ctx, now: bool) {
    let Some(ui) = c.weak.upgrade() else { return };
    let idle = ui.global::<AppState>().get_status() == i18n::get("status.idle");
    i18n::set_zh(now);
    crate::engine::settings::set_lang(if now { 2 } else { 1 });
    apply_fonts(&ui);
    let t = ui.global::<T>();
    t.set_rev(t.get_rev() + 1);
    push_all(&ui, &c.core.locked());
    push_low_disk(&ui);
    if idle {
        ui.global::<AppState>().set_status(i18n::get("status.idle").into());
    }
    crate::pages::relang(c, ui.global::<AppState>().get_page());
}

pub fn switch_language(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    if SWITCHING.get() {
        return;
    }
    SWITCHING.set(true);
    let theme = ui.global::<Theme>();
    sync_quiet(&theme);
    theme.set_content_ms(LANG_OUT_MS as f32);
    theme.set_content_opacity(0.0);
    let now = !i18n::zh();
    let c = c.clone();
    LANG_OUT.with(|f| {
        f.start(slint::TimerMode::SingleShot, Duration::from_millis(LANG_OUT_MS + 20), move || {
            swap_language(&c, now);
            let c = c.clone();
            arm(move || reveal_language(&c));
        })
    });
}

fn reveal_language(c: &Ctx) {
    let Some(ui) = c.weak.upgrade() else { return };
    let theme = ui.global::<Theme>();
    theme.set_content_ms(LANG_IN_MS as f32);
    theme.set_content_opacity(1.0);
    let weak = c.weak.clone();
    LANG_IN.with(|f| {
        f.start(slint::TimerMode::SingleShot, Duration::from_millis(LANG_IN_MS), move || {
            SWITCHING.set(false);
            if let Some(ui) = weak.upgrade() {
                sync_quiet(&ui.global::<Theme>());
            }
        })
    });
}
