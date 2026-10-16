use std::fs;
use std::path::PathBuf;

use tiny_skia::{Color, FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap, Point, RadialGradient, SpreadMode, Transform};

const SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba8(r, g, b, (a * 255.0).round() as u8)
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let k = r * 0.5523;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().unwrap()
}

fn sparkle(cx: f32, cy: f32, radius: f32, pinch: f32) -> Path {
    let c = radius * pinch;
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy - radius);
    pb.quad_to(cx + c, cy - c, cx + radius, cy);
    pb.quad_to(cx + c, cy + c, cx, cy + radius);
    pb.quad_to(cx - c, cy + c, cx - radius, cy);
    pb.quad_to(cx - c, cy - c, cx, cy - radius);
    pb.close();
    pb.finish().unwrap()
}

fn circle(cx: f32, cy: f32, r: f32) -> Path {
    PathBuilder::from_circle(cx, cy, r).unwrap()
}

fn solid(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

fn gradient(a: (f32, f32), b: (f32, f32), stops: &[(f32, Color)]) -> Paint<'static> {
    let stops: Vec<GradientStop> = stops.iter().map(|(p, c)| GradientStop::new(*p, *c)).collect();
    let mut p = Paint::default();
    p.shader = LinearGradient::new(Point::from_xy(a.0, a.1), Point::from_xy(b.0, b.1), stops, SpreadMode::Pad, Transform::identity()).unwrap();
    p.anti_alias = true;
    p
}

fn render(px: u32) -> Pixmap {
    let mut pm = Pixmap::new(px, px).unwrap();
    let s = px as f32 / 256.0;
    let t = Transform::from_scale(s, s);

    let body = rounded_rect(10.0, 10.0, 236.0, 236.0, 60.0);
    let base = gradient((20.0, 14.0), (236.0, 246.0), &[(0.0, rgba(0x2b, 0xe0, 0xc9, 1.0)), (0.55, rgba(0x2f, 0x9b, 0xe8, 1.0)), (1.0, rgba(0x3b, 0x5b, 0xf0, 1.0))]);
    pm.fill_path(&body, &base, FillRule::Winding, t, None);

    let mut glow = Paint::default();
    glow.shader = RadialGradient::new(
        Point::from_xy(70.0, 40.0),
        0.0,
        Point::from_xy(70.0, 40.0),
        190.0,
        vec![GradientStop::new(0.0, rgba(255, 255, 255, 0.42)), GradientStop::new(1.0, rgba(255, 255, 255, 0.0))],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap();
    glow.anti_alias = true;
    pm.fill_path(&body, &glow, FillRule::Winding, t, None);

    let sheen = rounded_rect(14.0, 14.0, 228.0, 118.0, 56.0);
    let sheen_paint = gradient((0.0, 14.0), (0.0, 132.0), &[(0.0, rgba(255, 255, 255, 0.30)), (1.0, rgba(255, 255, 255, 0.0))]);
    pm.fill_path(&sheen, &sheen_paint, FillRule::Winding, t, None);

    let rim = rounded_rect(11.5, 11.5, 233.0, 233.0, 58.5);
    let mut stroke = tiny_skia::Stroke::default();
    stroke.width = 3.0;
    let rim_paint = gradient((0.0, 10.0), (0.0, 246.0), &[(0.0, rgba(255, 255, 255, 0.70)), (0.5, rgba(255, 255, 255, 0.10)), (1.0, rgba(255, 255, 255, 0.28))]);
    pm.stroke_path(&rim, &rim_paint, &stroke, t, None);

    let shadow = solid(rgba(8, 30, 90, 0.28));
    pm.fill_path(&sparkle(118.0, 140.0, 80.0, 0.17), &shadow, FillRule::Winding, t, None);
    let white = solid(rgba(255, 255, 255, 1.0));
    pm.fill_path(&sparkle(116.0, 130.0, 80.0, 0.17), &white, FillRule::Winding, t, None);
    let small = solid(rgba(255, 255, 255, 0.96));
    pm.fill_path(&sparkle(188.0, 64.0, 32.0, 0.17), &small, FillRule::Winding, t, None);
    let dot = solid(rgba(255, 255, 255, 0.82));
    pm.fill_path(&circle(190.0, 196.0, 9.0), &dot, FillRule::Winding, t, None);
    pm
}

fn accent_paint(w: f32, h: f32) -> Paint<'static> {
    gradient((0.0, 0.0), (w, h), &[(0.0, rgba(0x1b, 0xc8, 0xb4, 1.0)), (1.0, rgba(0x33, 0x6e, 0xf2, 1.0))])
}

fn dark_accent_paint(w: f32, h: f32) -> Paint<'static> {
    gradient((0.0, 0.0), (w, h), &[(0.0, rgba(0x2a, 0x9a, 0x90, 1.0)), (1.0, rgba(0x4d, 0x82, 0xd6, 1.0))])
}

fn ring_pixmap(arc_degrees: Option<f32>, dark: bool) -> Pixmap {
    let mut pm = Pixmap::new(440, 440).unwrap();
    let t = Transform::from_scale(2.0, 2.0);
    let mut stroke = tiny_skia::Stroke::default();
    stroke.width = 14.0;
    stroke.line_cap = tiny_skia::LineCap::Round;
    let paint = if dark { dark_accent_paint(220.0, 220.0) } else { accent_paint(220.0, 220.0) };
    let path = match arc_degrees {
        None => circle(110.0, 110.0, 103.0),
        Some(deg) => {
            let mut pb = PathBuilder::new();
            let steps = (deg / 2.0).ceil() as i32;
            for i in 0..=steps {
                let a = (-90.0 + deg * i as f32 / steps as f32).to_radians();
                let (x, y) = (110.0 + 103.0 * a.cos(), 110.0 + 103.0 * a.sin());
                if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
            }
            pb.finish().unwrap()
        }
    };
    pm.stroke_path(&path, &paint, &stroke, t, None);
    pm
}

struct ShadowSpec {
    name: &'static str,
    corner: f32,
    blur: f32,
}

const SHADOWS: [ShadowSpec; 7] = [
    ShadowSpec { name: "shadow-0.png", corner: 17.5, blur: 14.0 },
    ShadowSpec { name: "shadow-1.png", corner: 24.0, blur: 14.0 },
    ShadowSpec { name: "shadow-2.png", corner: 20.0, blur: 14.0 },
    ShadowSpec { name: "shadow-3.png", corner: 24.0, blur: 40.0 },
    ShadowSpec { name: "shadow-4.png", corner: 24.0, blur: 24.0 },
    ShadowSpec { name: "shadow-5.png", corner: 16.0, blur: 8.0 },
    ShadowSpec { name: "shadow-6.png", corner: 16.0, blur: 16.0 },
];

fn blur_axis(src: &[f32], n: usize, kernel: &[f32], horizontal: bool) -> Vec<f32> {
    let r = (kernel.len() / 2) as isize;
    let mut out = vec![0.0f32; n * n];
    for y in 0..n {
        for x in 0..n {
            let mut acc = 0.0;
            for (k, w) in kernel.iter().enumerate() {
                let d = k as isize - r;
                let (sx, sy) = if horizontal { (x as isize + d, y as isize) } else { (x as isize, y as isize + d) };
                if sx >= 0 && sy >= 0 && (sx as usize) < n && (sy as usize) < n {
                    acc += src[sy as usize * n + sx as usize] * w;
                }
            }
            out[y * n + x] = acc;
        }
    }
    out
}

fn shadow_pixmap(spec: &ShadowSpec) -> (Pixmap, u32, u32) {
    let sigma = spec.blur * 0.4;
    let reach = (spec.blur * 1.25).ceil() as u32;
    let slice = reach + spec.corner.ceil().max((2.0 * sigma).ceil()) as u32;
    let size = 2 * slice + 2;
    let pad = (3.0 * sigma).ceil() as u32 + 4;
    let big = (size + 2 * pad) as usize;
    let mut mask = Pixmap::new(big as u32, big as u32).unwrap();
    let side = (size - 2 * reach + 2 * pad) as f32;
    let path = rounded_rect(reach as f32, reach as f32, side, side, spec.corner);
    mask.fill_path(&path, &solid(rgba(0, 0, 0, 1.0)), FillRule::Winding, Transform::identity(), None);
    let alpha: Vec<f32> = mask.data().chunks(4).map(|p| p[3] as f32 / 255.0).collect();
    let radius = (3.0 * sigma).ceil() as usize;
    let mut kernel: Vec<f32> = (0..=2 * radius).map(|i| (-((i as f32 - radius as f32).powi(2)) / (2.0 * sigma * sigma)).exp()).collect();
    let total: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|w| *w /= total);
    let blurred = blur_axis(&blur_axis(&alpha, big, &kernel, true), big, &kernel, false);
    let map = |i: u32| -> usize {
        if i < slice {
            i as usize
        } else if i >= size - slice {
            (i + 2 * pad) as usize
        } else {
            (slice + pad) as usize
        }
    };
    let mut out = Pixmap::new(size, size).unwrap();
    let mut seed: u32 = 0x2545_f491;
    let mut noise = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / 16777216.0
    };
    let data = out.data_mut();
    for y in 0..size {
        for x in 0..size {
            let v = blurred[map(y) * big + map(x)];
            let level = if v > 0.002 { v * 255.0 + noise() - noise() } else { 0.0 };
            data[((y * size + x) * 4 + 3) as usize] = level.round().clamp(0.0, 255.0) as u8;
        }
    }
    (out, slice, reach)
}

fn write_shadows(dir: &PathBuf) {
    for spec in &SHADOWS {
        let (pm, slice, reach) = shadow_pixmap(spec);
        println!("{} slice {} reach {}", spec.name, slice, reach);
        fs::write(dir.join(spec.name), pm.encode_png().unwrap()).unwrap();
    }
}

fn write_art(dir: &PathBuf) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("ring-full.png"), ring_pixmap(None, false).encode_png().unwrap()).unwrap();
    fs::write(dir.join("ring-arc.png"), ring_pixmap(Some(110.0), false).encode_png().unwrap()).unwrap();
    fs::write(dir.join("ring-full-dark.png"), ring_pixmap(None, true).encode_png().unwrap()).unwrap();
    fs::write(dir.join("ring-arc-dark.png"), ring_pixmap(Some(110.0), true).encode_png().unwrap()).unwrap();
}

fn main() {
    let out: PathBuf = std::env::args().nth(1).map(PathBuf::from).expect("usage: make-icon <assets dir> <art dir>");
    let art: PathBuf = std::env::args().nth(2).map(PathBuf::from).expect("usage: make-icon <assets dir> <art dir>");
    write_art(&art);
    write_shadows(&art);
    fs::create_dir_all(&out).unwrap();
    let mut images: Vec<(u32, Vec<u8>)> = Vec::new();
    for px in SIZES {
        let png = render(px).encode_png().unwrap();
        if px == 256 {
            fs::write(out.join("app-256.png"), &png).unwrap();
        }
        images.push((px, png));
    }
    let mut ico: Vec<u8> = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (px, png) in &images {
        let dim = if *px >= 256 { 0u8 } else { *px as u8 };
        ico.push(dim);
        ico.push(dim);
        ico.push(0);
        ico.push(0);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &images {
        ico.extend_from_slice(png);
    }
    fs::write(out.join("app.ico"), ico).unwrap();
}
