use std::cell::Cell;
use std::time::Duration;

use slint::{ComponentHandle, Timer};

use crate::app::{apply_fonts, enter_page, push_low_disk, Ctx};
use crate::views::push_all;
use crate::i18n;
use crate::{AppState, MainWindow, T, Theme};

thread_local! {
    static FADE: Timer = Timer::default();
}

thread_local! {
    static GEN: Cell<u32> = const { Cell::new(0) };
    static OUTGOING: Cell<bool> = const { Cell::new(false) };
    static WANT: Cell<i32> = const { Cell::new(0) };
    static ENTERING: std::cell::RefCell<Option<(u32, Ctx)>> = const { std::cell::RefCell::new(None) };
}

pub fn sync_scale(ui: &MainWindow) {
    ui.global::<Theme>().set_scale(ui.window().scale_factor());
}

pub fn after_frame() {
    if ENTERING.with(|e| e.borrow().is_none()) {
        return;
    }
    let _ = slint::invoke_from_event_loop(start_entering);
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

pub fn crossfade(ui: &MainWindow, change: impl FnOnce()) {
    let theme = ui.global::<Theme>();
    let Ok(shot) = ui.window().take_snapshot() else {
        change();
        return;
    };
    theme.set_fade_image(slint::Image::from_rgba8(shot));
    theme.set_fade_opacity(1.0);
    theme.set_quiet(true);
    change();
    let weak = ui.as_weak();
    let start = std::time::Instant::now();
    FADE.with(|f| {
        f.start(slint::TimerMode::Repeated, Duration::from_millis(8), move || {
            let Some(ui) = weak.upgrade() else { return };
            let k = (start.elapsed().as_secs_f32() / 0.32).min(1.0);
            let theme = ui.global::<Theme>();
            if k >= 1.0 {
                theme.set_fade_opacity(0.0);
                theme.set_fade_image(slint::Image::default());
                theme.set_quiet(false);
                FADE.with(|f| f.stop());
            } else {
                theme.set_fade_opacity((1.0 - k).powi(2));
            }
        });
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
    push_all(&ui, &c.core.lock().unwrap());
    push_low_disk(&ui);
    if idle {
        ui.global::<AppState>().set_status(i18n::get("status.idle").into());
    }
    crate::pages::relang(c, ui.global::<AppState>().get_page());
}

pub fn switch_language(c: &Ctx) {
    const OUT: f32 = 0.26;
    const IN: f32 = 0.50;
    let now = !i18n::zh();
    let c = c.clone();
    if let Some(ui) = c.weak.upgrade() {
        ui.global::<Theme>().set_quiet(true);
    }
    let start = std::time::Instant::now();
    let resumed: std::cell::Cell<Option<std::time::Instant>> = std::cell::Cell::new(None);
    FADE.with(|f| {
        f.start(slint::TimerMode::Repeated, Duration::from_millis(8), move || {
            let Some(ui) = c.weak.upgrade() else { return };
            let theme = ui.global::<Theme>();
            let ease = |k: f32| k * k * (3.0 - 2.0 * k);
            match resumed.get() {
                None => {
                    let k = start.elapsed().as_secs_f32() / OUT;
                    if k < 1.0 {
                        theme.set_content_opacity(1.0 - ease(k));
                    } else {
                        theme.set_content_opacity(0.0);
                        swap_language(&c, now);
                        resumed.set(Some(std::time::Instant::now()));
                    }
                }
                Some(at) => {
                    let k = at.elapsed().as_secs_f32() / IN;
                    if k < 1.0 {
                        theme.set_content_opacity(ease(k));
                    } else {
                        theme.set_content_opacity(1.0);
                        theme.set_quiet(false);
                        FADE.with(|f| f.stop());
                    }
                }
            }
        });
    });
}

