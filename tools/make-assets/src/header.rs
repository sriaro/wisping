use std::f32::consts::PI;
use std::fs;
use std::io::BufWriter;
use std::path::Path as FsPath;

use tiny_skia::{Color, FillRule, FilterQuality, GradientStop, Paint, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, SpreadMode, Transform};
use ttf_parser::{Face, OutlineBuilder, Tag};

use super::{gradient, render, rgba, sparkle};

const SCALE: f32 = 2.0;
const HEIGHT: f32 = 152.0;
const ICON: f32 = 120.0;
const GAP: f32 = 26.0;
const FONT_SIZE: f32 = 92.0;
const TRACKING: f32 = -0.012;
const TAIL: f32 = 72.0;
const MARGIN: f32 = 10.0;
const FPS: u16 = 50;
const TEAL: (u8, u8, u8) = (0x1b, 0xc8, 0xb4);
const BLUE: (u8, u8, u8) = (0x33, 0x6e, 0xf2);

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
    glyphs: Vec<tiny_skia::Path>,
    width: f32,
    cap: f32,
}

fn wordmark(face: &Face, text: &str) -> Wordmark {
    let s = FONT_SIZE / face.units_per_em() as f32;
    let mut glyphs = Vec::new();
    let mut pen = 0.0;
    for ch in text.chars() {
        let id = face.glyph_index(ch).expect("glyph");
        let mut o = Outline { pb: PathBuilder::new(), x: pen, y: 0.0, s };
        face.outline_glyph(id, &mut o);
        glyphs.extend(o.pb.finish());
        pen += face.glyph_hor_advance(id).unwrap_or(0) as f32 * s + TRACKING * FONT_SIZE;
    }
    let cap = face.capital_height().unwrap_or(700) as f32 * s;
    Wordmark { glyphs, width: pen - TRACKING * FONT_SIZE, cap }
}

fn clamp01(u: f32) -> f32 {
    u.clamp(0.0, 1.0)
}

fn out_cubic(u: f32) -> f32 {
    1.0 - (1.0 - clamp01(u)).powi(3)
}

fn in_out(u: f32) -> f32 {
    0.5 - 0.5 * (PI * clamp01(u)).cos()
}

fn back(u: f32) -> f32 {
    let v = clamp01(u) - 1.0;
    1.0 + 2.70158 * v.powi(3) + 1.70158 * v.powi(2)
}

fn lerp(a: f32, b: f32, u: f32) -> f32 {
    a + (b - a) * u
}

fn tone(u: f32, a: f32) -> Color {
    let u = clamp01(u);
    let mix = |p: u8, q: u8| lerp(p as f32, q as f32, u).round() as u8;
    rgba(mix(TEAL.0, BLUE.0), mix(TEAL.1, BLUE.1), mix(TEAL.2, BLUE.2), a)
}

fn solid(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

struct Scene<'a> {
    mark: &'a Wordmark,
    ink: Color,
    icon: &'a Pixmap,
    width: f32,
}

impl Scene<'_> {
    fn canvas(&self) -> Pixmap {
        Pixmap::new((self.width * SCALE) as u32, (HEIGHT * SCALE) as u32).unwrap()
    }

    fn text_x(&self) -> f32 {
        MARGIN + ICON + GAP
    }

    fn baseline(&self) -> f32 {
        HEIGHT * 0.5 + self.mark.cap * 0.5
    }

    fn end(&self) -> f32 {
        self.text_x() + self.mark.width
    }

    fn stars(&self) -> [(f32, f32, f32); 3] {
        let (ex, sky) = (self.end(), self.baseline() - self.mark.cap);
        [(ex + 30.0, sky + 10.0, 17.0), (ex + 54.0, sky + 40.0, 9.0), (ex + 24.0, sky + 52.0, 5.5)]
    }

    fn shade(&self, x: f32) -> f32 {
        (x - self.text_x()) / (self.width - self.text_x())
    }

    fn icon_at(&self, pm: &mut Pixmap, scale: f32, alpha: f32) {
        if alpha <= 0.0 || scale <= 0.0 {
            return;
        }
        let half = ICON * SCALE * 0.5;
        let quality = if (scale - 1.0).abs() < 1e-4 { FilterQuality::Nearest } else { FilterQuality::Bicubic };
        let paint = PixmapPaint { opacity: alpha, quality, ..PixmapPaint::default() };
        let at = Transform::from_translate(-half, -half).post_scale(scale, scale).post_translate((MARGIN + ICON * 0.5) * SCALE, HEIGHT * 0.5 * SCALE);
        pm.draw_pixmap(0, 0, self.icon.as_ref(), &paint, at, None);
    }

    fn glyph(&self, pm: &mut Pixmap, i: usize, dy: f32, alpha: f32) {
        let mut ink = self.ink;
        ink.apply_opacity(alpha);
        let at = Transform::from_translate(self.text_x(), self.baseline() + dy).post_scale(SCALE, SCALE);
        pm.fill_path(&self.mark.glyphs[i], &solid(ink), FillRule::Winding, at, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn star(&self, pm: &mut Pixmap, (cx, cy, r): (f32, f32, f32), round: bool, scale: f32, spin: f32, alpha: f32, glow: f32) {
        if scale <= 0.0 || alpha <= 0.0 {
            return;
        }
        let screen = Transform::from_scale(SCALE, SCALE);
        let r = r * scale;
        if glow > 0.0 {
            let mut halo = Paint::default();
            halo.shader = RadialGradient::new(
                Point::from_xy(cx, cy),
                0.0,
                Point::from_xy(cx, cy),
                r * 1.9,
                vec![GradientStop::new(0.0, tone(self.shade(cx), 0.36 * glow * alpha)), GradientStop::new(1.0, tone(self.shade(cx), 0.0))],
                SpreadMode::Pad,
                Transform::identity(),
            )
            .unwrap();
            halo.anti_alias = true;
            pm.fill_path(&PathBuilder::from_circle(cx, cy, r * 1.9).unwrap(), &halo, FillRule::Winding, screen, None);
        }
        let ex = self.end();
        let paint = gradient((ex, 0.0), (ex + TAIL, HEIGHT), &[(0.0, tone(0.0, alpha)), (1.0, tone(1.0, alpha))]);
        if round {
            pm.fill_path(&PathBuilder::from_circle(cx, cy, r * 0.62).unwrap(), &paint, FillRule::Winding, screen, None);
        } else {
            let at = Transform::from_rotate_at(spin, cx, cy).post_scale(SCALE, SCALE);
            pm.fill_path(&sparkle(cx, cy, r, 0.17), &paint, FillRule::Winding, at, None);
        }
    }
}

fn faded(pm: &Pixmap, into: &mut Pixmap, alpha: f32) {
    let paint = PixmapPaint { opacity: alpha, ..PixmapPaint::default() };
    into.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::identity(), None);
}

const LOOP: f32 = 6.0;

fn frame(sc: &Scene, t: f32) -> Pixmap {
    let mut pm = sc.canvas();
    let u = t / 0.55;
    sc.icon_at(&mut pm, 0.6 + 0.4 * back(u), clamp01(u / 0.35));
    for i in 0..sc.mark.glyphs.len() {
        let u = (t - 0.3 - 0.065 * i as f32) / 0.5;
        if u > 0.0 {
            sc.glyph(&mut pm, i, 26.0 * (1.0 - out_cubic(u)), clamp01(u * 1.6));
        }
    }
    for (k, s) in sc.stars().into_iter().enumerate() {
        let u = clamp01((t - 1.05 - 0.12 * k as f32) / 0.5);
        if u > 0.0 {
            sc.star(&mut pm, s, k == 2, back(u), -90.0 * (1.0 - out_cubic(u)), clamp01(u * 2.0), (PI * u).sin());
        }
    }
    let fade = clamp01((t - (LOOP - 0.45)) / 0.45);
    if fade > 0.0 {
        let mut out = sc.canvas();
        faded(&pm, &mut out, 1.0 - in_out(fade));
        return out;
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

fn write_apng(path: &FsPath, still: &Pixmap, frames: &[Pixmap]) {
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
    enc.set_sep_def_img(true).unwrap();
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(&straight(still)).unwrap();
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
        let sc = Scene { mark: &mark, ink, icon: &icon, width };
        let count = (LOOP * FPS as f32).round() as u32;
        let frames: Vec<Pixmap> = (0..count).map(|i| frame(&sc, i as f32 / FPS as f32)).collect();
        write_apng(&dir.join(name), &frame(&sc, LOOP - 1.0), &frames);
    }
}
