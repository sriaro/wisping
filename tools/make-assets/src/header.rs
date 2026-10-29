use std::f32::consts::PI;
use std::fs;
use std::io::BufWriter;
use std::path::Path as FsPath;

use tiny_skia::{Color, FillRule, GradientStop, Mask, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, SpreadMode, Transform};
use ttf_parser::{Face, OutlineBuilder, Tag};

use super::{gradient, render, rgba, rounded_rect, sparkle};

const SCALE: f32 = 2.0;
const HEIGHT: f32 = 152.0;
const ICON: f32 = 120.0;
const GAP: f32 = 26.0;
const FONT_SIZE: f32 = 92.0;
const TRACKING: f32 = -0.012;
const TAIL: f32 = 72.0;
const MARGIN: f32 = 10.0;
const FRAMES: u32 = 120;
const FPS: u16 = 30;
const SWEEP: f32 = 0.55;
const TWINKLE: f32 = 0.3;
const BAND: f32 = 90.0;
const ANGLE: f32 = 0.35;

struct Outline {
    pb: PathBuilder,
    x: f32,
    y: f32,
    s: f32,
}

impl OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb.move_to(self.x + x * self.s, self.y - y * self.s);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pb.line_to(self.x + x * self.s, self.y - y * self.s);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.pb.quad_to(self.x + x1 * self.s, self.y - y1 * self.s, self.x + x * self.s, self.y - y * self.s);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.pb.cubic_to(
            self.x + x1 * self.s,
            self.y - y1 * self.s,
            self.x + x2 * self.s,
            self.y - y2 * self.s,
            self.x + x * self.s,
            self.y - y * self.s,
        );
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

struct Wordmark {
    path: tiny_skia::Path,
    width: f32,
    cap: f32,
}

fn wordmark(face: &Face, text: &str) -> Wordmark {
    let s = FONT_SIZE / face.units_per_em() as f32;
    let mut pb = PathBuilder::new();
    let mut pen = 0.0;
    for ch in text.chars() {
        let id = face.glyph_index(ch).expect("glyph");
        let mut o = Outline { pb, x: pen, y: 0.0, s };
        face.outline_glyph(id, &mut o);
        pb = o.pb;
        pen += face.glyph_hor_advance(id).unwrap_or(0) as f32 * s + TRACKING * FONT_SIZE;
    }
    let cap = face.capital_height().unwrap_or(700) as f32 * s;
    Wordmark { path: pb.finish().expect("outline"), width: pen - TRACKING * FONT_SIZE, cap }
}

fn bump(t: f32, from: f32, to: f32) -> f32 {
    let u = (t - from) / (to - from);
    if (0.0..1.0).contains(&u) { (PI * u).sin().powi(2) } else { 0.0 }
}

fn ease(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    0.5 - 0.5 * (PI * u).cos()
}

fn band(c: f32, stops: &[(f32, Color)]) -> tiny_skia::Paint<'static> {
    let (dx, dy) = (BAND * ANGLE.cos(), BAND * ANGLE.sin());
    let y = HEIGHT * 0.5;
    gradient((c - dx, y - dy), (c + dx, y + dy), stops)
}

fn frame(t: f32, mark: &Wordmark, ink: Color, icon: &Pixmap, width: f32) -> Pixmap {
    let mut pm = Pixmap::new((width * SCALE) as u32, (HEIGHT * SCALE) as u32).unwrap();
    let top = (HEIGHT - ICON) * 0.5;
    pm.draw_pixmap((MARGIN * SCALE) as i32, (top * SCALE) as i32, icon.as_ref(), &PixmapPaint::default(), Transform::identity(), None);

    let reach = BAND * 1.6;
    let c = -reach + ease(t / SWEEP) * (width + 2.0 * reach);
    let sweeping = t < SWEEP;
    let screen = Transform::from_scale(SCALE, SCALE);

    if sweeping {
        let k = ICON / 256.0;
        let at = Transform::from_scale(k, k).post_translate(MARGIN, top).post_scale(SCALE, SCALE);
        let mut mask = Mask::new(pm.width(), pm.height()).unwrap();
        mask.fill_path(&rounded_rect(10.0, 10.0, 236.0, 236.0, 60.0), FillRule::Winding, true, at);
        let light = band(c, &[(0.0, rgba(255, 255, 255, 0.0)), (0.5, rgba(255, 255, 255, 0.62)), (1.0, rgba(255, 255, 255, 0.0))]);
        let full = rounded_rect(0.0, 0.0, width, HEIGHT, 1.0);
        pm.fill_path(&full, &light, FillRule::Winding, screen, Some(&mask));
    }

    let x = MARGIN + ICON + GAP;
    let baseline = HEIGHT * 0.5 + mark.cap * 0.5;
    let at = Transform::from_translate(x, baseline).post_scale(SCALE, SCALE);
    let mut ink_paint = tiny_skia::Paint::default();
    ink_paint.set_color(ink);
    ink_paint.anti_alias = true;
    pm.fill_path(&mark.path, &ink_paint, FillRule::Winding, at, None);
    if sweeping {
        let mut mask = Mask::new(pm.width(), pm.height()).unwrap();
        mask.fill_path(&mark.path, FillRule::Winding, true, at);
        let glow = band(
            c,
            &[
                (0.0, rgba(0x1b, 0xc8, 0xb4, 0.0)),
                (0.3, rgba(0x1b, 0xc8, 0xb4, 0.9)),
                (0.5, rgba(0x2f, 0x9b, 0xe8, 1.0)),
                (0.7, rgba(0x33, 0x6e, 0xf2, 0.9)),
                (1.0, rgba(0x33, 0x6e, 0xf2, 0.0)),
            ],
        );
        let full = rounded_rect(0.0, 0.0, width, HEIGHT, 1.0);
        pm.fill_path(&full, &glow, FillRule::Winding, screen, Some(&mask));
    }

    let ex = x + mark.width;
    let accent = |a: f32| gradient((ex, 0.0), (ex + TAIL, HEIGHT), &[(0.0, rgba(0x1b, 0xc8, 0xb4, a)), (1.0, rgba(0x33, 0x6e, 0xf2, a))]);
    let sky = baseline - mark.cap;
    let stars = [(ex + 30.0, sky + 10.0, 17.0, 0.44), (ex + 54.0, sky + 40.0, 9.0, 0.53), (ex + 24.0, sky + 52.0, 5.5, 0.62)];
    for (i, (cx, cy, r, from)) in stars.into_iter().enumerate() {
        let p = bump(t, from, from + TWINKLE);
        let a = 0.78 + 0.22 * p;
        let r = r * (0.86 + 0.42 * p);
        let spin = Transform::from_rotate_at(90.0 * ease((t - from) / TWINKLE), cx, cy).post_scale(SCALE, SCALE);
        if p > 0.0 {
            let mut halo = tiny_skia::Paint::default();
            halo.shader = RadialGradient::new(
                Point::from_xy(cx, cy),
                0.0,
                Point::from_xy(cx, cy),
                r * 1.5,
                vec![GradientStop::new(0.0, rgba(0x2f, 0x9b, 0xe8, 0.32 * p)), GradientStop::new(1.0, rgba(0x2f, 0x9b, 0xe8, 0.0))],
                SpreadMode::Pad,
                Transform::identity(),
            )
            .unwrap();
            halo.anti_alias = true;
            pm.fill_path(&PathBuilder::from_circle(cx, cy, r * 1.5).unwrap(), &halo, FillRule::Winding, screen, None);
        }
        if i == 2 {
            pm.fill_path(&PathBuilder::from_circle(cx, cy, r * 0.62).unwrap(), &accent(a), FillRule::Winding, screen, None);
        } else {
            pm.fill_path(&sparkle(cx, cy, r, 0.17), &accent(a), FillRule::Winding, spin, None);
        }
    }
    pm
}

fn straight(pm: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pm.data().len());
    for p in pm.pixels() {
        let c = p.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

struct Shot {
    rect: (u32, u32, u32, u32),
    data: Vec<u8>,
    delay: u16,
}

fn changed(a: &[u8], b: &[u8], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if a[i..i + 4] != b[i..i + 4] {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > x0).then_some((x0, y0, x1 - x0, y1 - y0))
}

fn crop(data: &[u8], w: u32, (x, y, cw, ch): (u32, u32, u32, u32)) -> Vec<u8> {
    let mut out = Vec::with_capacity((cw * ch * 4) as usize);
    for row in y..y + ch {
        let i = ((row * w + x) * 4) as usize;
        out.extend_from_slice(&data[i..i + (cw * 4) as usize]);
    }
    out
}

fn write_apng(path: &FsPath, frames: &[Pixmap]) {
    let (w, h) = (frames[0].width(), frames[0].height());
    let raw: Vec<Vec<u8>> = frames.iter().map(straight).collect();
    let mut shots = vec![Shot { rect: (0, 0, w, h), data: raw[0].clone(), delay: 1 }];
    for i in 1..raw.len() {
        match changed(&raw[i - 1], &raw[i], w, h) {
            Some(rect) => shots.push(Shot { rect, data: crop(&raw[i], w, rect), delay: 1 }),
            None => shots.last_mut().unwrap().delay += 1,
        }
    }
    let file = BufWriter::new(fs::File::create(path).unwrap());
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    enc.set_animated(shots.len() as u32, 0).unwrap();
    let mut writer = enc.write_header().unwrap();
    for shot in &shots {
        let (x, y, cw, ch) = shot.rect;
        writer.set_frame_position(0, 0).unwrap();
        writer.set_frame_dimension(cw, ch).unwrap();
        writer.set_frame_position(x, y).unwrap();
        writer.set_blend_op(png::BlendOp::Source).unwrap();
        writer.set_dispose_op(png::DisposeOp::None).unwrap();
        writer.set_frame_delay(shot.delay, FPS).unwrap();
        writer.write_image_data(&shot.data).unwrap();
    }
    writer.finish().unwrap();
}

pub fn write(dir: &FsPath) {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:/Windows".into());
    let data = fs::read(FsPath::new(&windir).join("Fonts").join("SegUIVar.ttf")).expect("Segoe UI Variable");
    let mut face = Face::parse(&data, 0).expect("font");
    face.set_variation(Tag::from_bytes(b"wght"), 600.0);
    face.set_variation(Tag::from_bytes(b"opsz"), 36.0);
    let mark = wordmark(&face, "Wisping");
    let width = (MARGIN + ICON + GAP + mark.width + TAIL + MARGIN).ceil();
    let icon = render((ICON * SCALE) as u32);
    fs::create_dir_all(dir).unwrap();
    for (name, ink) in [("header-light.png", rgba(0x1f, 0x23, 0x28, 1.0)), ("header-dark.png", rgba(0xf0, 0xf6, 0xfc, 1.0))] {
        let frames: Vec<Pixmap> = (0..FRAMES).map(|i| frame(i as f32 / FRAMES as f32, &mark, ink, &icon, width)).collect();
        write_apng(&dir.join(name), &frames);
    }
}
