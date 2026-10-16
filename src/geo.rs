use std::fmt::Write;

use crate::{Geo, MainWindow};
use slint::ComponentHandle;

const POWER: f32 = 4.0;
const PILL_POWER: f32 = 3.0;

fn corner_pull(power: f32) -> f32 {
    (0.5f32.powf(1.0 / power) - 0.5) / 0.375
}

pub fn squircle(w: f32, h: f32, extent: f32, inset: f32) -> String {
    let e = extent.min(w / 2.0).min(h / 2.0).max(0.0);
    let full = e >= w.min(h) / 2.0 - 0.01;
    let power = if !full {
        POWER
    } else if (w - h).abs() < 0.5 {
        2.0
    } else {
        PILL_POWER
    };
    let k = e * (1.0 - corner_pull(power));
    let (l, t, r, b) = (inset, inset, w + inset, h + inset);
    let q1 = (r - e + (e - k) / 2.0, t);
    let m = (r - k / 2.0, t + k / 2.0);
    let q2 = ((q1.0 + m.0) / 2.0, (q1.1 + m.1) / 2.0);
    let r2 = (r, t + (k + e) / 2.0);
    let r1 = ((m.0 + r2.0) / 2.0, (m.1 + r2.1) / 2.0);
    let apex = ((q2.0 + r1.0) / 2.0, (q2.1 + r1.1) / 2.0);
    let mut out = String::with_capacity(400);
    let _ = write!(out, "M {:.3} {:.3} C {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} L {:.3} {:.3} ", apex.0, apex.1, q2.0, q2.1, q1.0, q1.1, r - e, t, l + e, t);
    let _ = write!(out, "C {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} L {:.3} {:.3} ", l + k, t, l, t + k, l, t + e, l, b - e);
    let _ = write!(out, "C {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} L {:.3} {:.3} ", l, b - k, l + k, b, l + e, b, r - e, b);
    let _ = write!(out, "C {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} L {:.3} {:.3} ", r - k, b, r, b - k, r, b - e, r, t + e);
    let _ = write!(out, "C {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} Z", r2.0, r2.1, r1.0, r1.1, apex.0, apex.1);
    out
}

pub fn install(ui: &MainWindow) {
    ui.global::<Geo>().on_squircle(|w, h, e, inset| squircle(w, h, e, inset).into());
}
