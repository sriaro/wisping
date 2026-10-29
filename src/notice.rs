use std::cell::RefCell;
use std::ffi::c_void;
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, DEFAULT_CHARSET,
    DEFAULT_GUI_FONT, DEFAULT_PITCH, DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetMonitorInfoW, GetStockObject, GetTextExtentPoint32W, GetTextFaceW, GetTextMetricsW, HBITMAP, HDC, HFONT, HGDIOBJ, LOGFONTW,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TEXTMETRICW, TRANSPARENT, TextOutW,
};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_CONTROL, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, GetCursorPos, GetMessageW, GetWindowRect, HICON, HTCAPTION, HTCLIENT,
    HWND_NOTOPMOST, IDC_ARROW, KillTimer, LoadCursorW, LoadIconW, MSG, RegisterClassExW, SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST, SPI_GETSCREENREADER, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, SystemParametersInfoW, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WM_ACTIVATE, WM_CLOSE, WM_DESTROY, WM_DPICHANGED,
    WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, w};

const W: f32 = 448.0;
const PAD: f32 = 24.0;
const RADIUS: f32 = 8.0;
const SHADOW: f32 = 28.0;
const BLUR: f32 = 22.0;
const DROP: f32 = 6.0;
const BADGE: f32 = 24.0;
const GAP: f32 = 12.0;
const HEAD_PX: f32 = 20.0;
const BODY_PX: f32 = 14.0;
const BTN_H: f32 = 32.0;
const BTN_GAP: f32 = 8.0;
const FOOT_H: f32 = 80.0;
const HEAD_LINES: usize = 3;
const BODY_LINES: usize = 12;
const ANIM_MS: f32 = 160.0;
const RISE: f32 = 8.0;
const ANIM_TIMER: usize = 1;
const KEY_GRACE_MS: u128 = 350;
const CF_UNICODETEXT: u32 = 13;

#[derive(Clone, Copy, PartialEq)]
enum Act {
    Copy,
    Close,
}

struct Btn {
    rect: [f32; 4],
    act: Act,
    label: String,
    primary: bool,
}

struct Line {
    x: f32,
    y: f32,
    text: String,
    head: bool,
}

struct Plan {
    w: f32,
    h: f32,
    foot_y: f32,
    badge: [f32; 4],
    lines: Vec<Line>,
    buttons: Vec<Btn>,
}

#[derive(Default, Clone, Copy)]
struct View {
    hover: Option<usize>,
    press: Option<usize>,
    focus: usize,
    kbd: bool,
}

struct Pal {
    body: [u8; 3],
    foot: [u8; 3],
    divider: [u8; 3],
    edge: [u8; 3],
    ink: [u8; 3],
    soft: [u8; 3],
    btn: [u8; 3],
    btn_hover: [u8; 3],
    btn_press: [u8; 3],
    btn_edge: [u8; 3],
    accent_a: [u8; 3],
    accent_b: [u8; 3],
    on_accent: [u8; 3],
    critical: [u8; 3],
    shadow: f32,
}

fn palette(dark: bool) -> Pal {
    if dark {
        Pal {
            body: [0x1b, 0x23, 0x35],
            foot: [0x15, 0x1c, 0x2c],
            divider: [0x2b, 0x33, 0x46],
            edge: [0x34, 0x3d, 0x52],
            ink: [0xef, 0xf3, 0xfa],
            soft: [0xa3, 0xb0, 0xc4],
            btn: [0x2c, 0x34, 0x47],
            btn_hover: [0x35, 0x3e, 0x53],
            btn_press: [0x25, 0x2d, 0x3e],
            btn_edge: [0x40, 0x49, 0x5e],
            accent_a: [0x12, 0x85, 0x7c],
            accent_b: [0x2c, 0x58, 0xc4],
            on_accent: [0xff, 0xff, 0xff],
            critical: [0xe5, 0x69, 0x6e],
            shadow: 0.55,
        }
    } else {
        Pal {
            body: [0xff, 0xff, 0xff],
            foot: [0xf3, 0xf6, 0xfb],
            divider: [0xe4, 0xe8, 0xef],
            edge: [0xd3, 0xd9, 0xe3],
            ink: [0x14, 0x1b, 0x2b],
            soft: [0x4f, 0x5b, 0x70],
            btn: [0xff, 0xff, 0xff],
            btn_hover: [0xee, 0xf2, 0xf8],
            btn_press: [0xe4, 0xe9, 0xf2],
            btn_edge: [0xd5, 0xdb, 0xe5],
            accent_a: [0x17, 0xbf, 0xb2],
            accent_b: [0x2f, 0x6f, 0xf0],
            on_accent: [0xff, 0xff, 0xff],
            critical: [0xd1, 0x2f, 0x36],
            shadow: 0.22,
        }
    }
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3134F))
}

struct Fonts {
    dc: HDC,
    head: HFONT,
    body: HFONT,
}

impl Fonts {
    fn new(dc: HDC, s: f32, han: bool) -> Fonts {
        let han_face = crate::fonts::han_face();
        let head: &[&str] = if han { &[han_face, "Segoe UI"] } else { &["Segoe UI Variable Display Semib", "Segoe UI Variable Text Semibold", "Segoe UI Semibold", "Segoe UI"] };
        let body: &[&str] = if han { &[han_face, "Segoe UI"] } else { &["Segoe UI Variable Text", "Segoe UI"] };
        Fonts { dc, head: make_font(dc, head, HEAD_PX * s, 600), body: make_font(dc, body, BODY_PX * s, 400) }
    }

    fn font(&self, head: bool) -> HFONT {
        if head { self.head } else { self.body }
    }

    fn advance(&self, head: bool, text: &str) -> f32 {
        let units = wide(text);
        if units.is_empty() {
            return 0.0;
        }
        let mut size = SIZE::default();
        unsafe {
            let old = SelectObject(self.dc, HGDIOBJ(self.font(head).0));
            let _ = GetTextExtentPoint32W(self.dc, &units, &mut size);
            SelectObject(self.dc, old);
        }
        size.cx as f32
    }

    fn line_height(&self, head: bool) -> f32 {
        let mut tm = TEXTMETRICW::default();
        unsafe {
            let old = SelectObject(self.dc, HGDIOBJ(self.font(head).0));
            let _ = GetTextMetricsW(self.dc, &mut tm);
            SelectObject(self.dc, old);
        }
        (tm.tmHeight + tm.tmExternalLeading) as f32
    }

    fn free(&mut self) {
        unsafe {
            SelectObject(self.dc, GetStockObject(DEFAULT_GUI_FONT));
            let _ = DeleteObject(HGDIOBJ(self.head.0));
            let _ = DeleteObject(HGDIOBJ(self.body.0));
        }
    }
}

fn make_font(dc: HDC, faces: &[&str], px: f32, weight: i32) -> HFONT {
    for (i, face) in faces.iter().enumerate() {
        let mut lf = LOGFONTW {
            lfHeight: -(px.round() as i32),
            lfWeight: weight,
            lfCharSet: DEFAULT_CHARSET,
            lfOutPrecision: OUT_DEFAULT_PRECIS,
            lfClipPrecision: CLIP_DEFAULT_PRECIS,
            lfQuality: CLEARTYPE_QUALITY,
            lfPitchAndFamily: DEFAULT_PITCH.0,
            ..Default::default()
        };
        for (k, u) in face.encode_utf16().take(31).enumerate() {
            lf.lfFaceName[k] = u;
        }
        let font = unsafe { CreateFontIndirectW(&lf) };
        if font.is_invalid() {
            continue;
        }
        if i + 1 == faces.len() {
            return font;
        }
        let mut name = [0u16; 32];
        let got = unsafe {
            let old = SelectObject(dc, HGDIOBJ(font.0));
            let n = GetTextFaceW(dc, Some(&mut name));
            SelectObject(dc, old);
            String::from_utf16_lossy(&name[..n.max(0) as usize]).trim_end_matches('\0').to_string()
        };
        if got.eq_ignore_ascii_case(face) {
            return font;
        }
        unsafe {
            let _ = DeleteObject(HGDIOBJ(font.0));
        }
    }
    HFONT::default()
}

fn pieces(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev_space = false;
    for (i, c) in s.char_indices() {
        let space = c == ' ';
        if i > start && prev_space && !space {
            out.push(&s[start..i]);
            start = i;
        }
        if c == '\\' || c == '/' {
            let end = i + c.len_utf8();
            out.push(&s[start..end]);
            start = end;
            prev_space = false;
            continue;
        }
        prev_space = space;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn longest_prefix(measure: &dyn Fn(&str) -> f32, s: &str, max_w: f32) -> usize {
    let mut best = 0;
    for (i, c) in s.char_indices() {
        let end = i + c.len_utf8();
        if best > 0 && measure(&s[..end]) > max_w {
            break;
        }
        best = end;
    }
    best.max(s.chars().next().map_or(0, char::len_utf8))
}

fn wrap(measure: &dyn Fn(&str) -> f32, text: &str, max_w: f32, most: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    'outer: for raw in text.split('\n') {
        let mut line = String::new();
        for mut piece in pieces(raw) {
            loop {
                let joined = format!("{line}{piece}");
                if measure(joined.trim_end()) <= max_w {
                    line = joined;
                    break;
                }
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line).trim_end().to_string());
                    if out.len() >= most {
                        break 'outer;
                    }
                    continue;
                }
                let cut = longest_prefix(measure, piece, max_w);
                out.push(piece[..cut].to_string());
                if out.len() >= most {
                    break 'outer;
                }
                piece = &piece[cut..];
                if piece.is_empty() {
                    break;
                }
            }
        }
        out.push(line.trim_end().to_string());
        if out.len() >= most {
            break;
        }
    }
    out
}

fn cap(measure: &dyn Fn(&str) -> f32, mut lines: Vec<String>, n: usize, max_w: f32) -> Vec<String> {
    if lines.len() <= n {
        return lines;
    }
    lines.truncate(n.max(1));
    let Some(last) = lines.last_mut() else { return lines };
    let mut s = last.trim_end().to_string();
    while !s.is_empty() && measure(&format!("{s}…")) > max_w {
        s.pop();
        s = s.trim_end().to_string();
    }
    *last = format!("{s}…");
    lines
}

fn plan(heading: &str, text: &str, f: &Fonts, s: f32, copied: bool) -> Plan {
    let w = (W * s).round();
    let x0 = ((PAD + BADGE + GAP) * s).round();
    let col = w - x0 - PAD * s;
    let mut lines = Vec::new();
    let mut y = (PAD * s).round();
    let head_lh = f.line_height(true).round();
    let measure_head = |t: &str| f.advance(true, t);
    let head_lines = cap(&measure_head, wrap(&measure_head, heading, col, HEAD_LINES + 1), HEAD_LINES, col);
    let badge = [(PAD * s).round(), y + ((head_lh - BADGE * s) / 2.0).round(), (BADGE * s).round(), (BADGE * s).round()];
    for t in head_lines {
        lines.push(Line { x: x0, y, text: t, head: true });
        y += head_lh;
    }
    y = y.max(badge[1] + badge[3]) + (GAP * s).round();
    let body_lh = f.line_height(false).round();
    let measure_body = |t: &str| f.advance(false, t);
    for t in cap(&measure_body, wrap(&measure_body, text, col, BODY_LINES + 1), BODY_LINES, col) {
        lines.push(Line { x: x0, y, text: t, head: false });
        y += body_lh;
    }
    let foot_y = (y + PAD * s).ceil();
    let h = (foot_y + FOOT_H * s).ceil();
    let bw = ((w - 2.0 * PAD * s - BTN_GAP * s) / 2.0).floor();
    let bh = (BTN_H * s).round();
    let by = (foot_y + (FOOT_H * s - bh) / 2.0).round();
    let bx = (PAD * s).round();
    let copy = crate::i18n::get(if copied { "fatal.copied" } else { "fatal.copy" });
    let buttons = vec![
        Btn { rect: [bx, by, bw, bh], act: Act::Copy, label: copy, primary: false },
        Btn { rect: [bx + bw + (BTN_GAP * s).round(), by, bw, bh], act: Act::Close, label: crate::i18n::get("fatal.close"), primary: true },
    ];
    Plan { w, h, foot_y, badge, lines, buttons }
}

fn hit_at(p: &Plan, x: f32, y: f32) -> Option<usize> {
    p.buttons.iter().position(|b| x >= b.rect[0] && x < b.rect[0] + b.rect[2] && y >= b.rect[1] && y < b.rect[1] + b.rect[3])
}

fn on_body(p: &Plan, x: f32, y: f32) -> bool {
    x >= 0.0 && x < p.w && y >= 0.0 && y < p.h
}

fn sdf(x: f32, y: f32, r: [f32; 4], radius: f32) -> f32 {
    let radius = radius.min(r[2] / 2.0).min(r[3] / 2.0).max(0.0);
    let qx = (x - (r[0] + r[2] / 2.0)).abs() - (r[2] / 2.0 - radius);
    let qy = (y - (r[1] + r[3] / 2.0)).abs() - (r[3] / 2.0 - radius);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

struct Canvas<'a> {
    px: &'a mut [u8],
    w: usize,
    h: usize,
}

impl Canvas<'_> {
    fn bounds(&self, r: [f32; 4], pad: f32) -> (usize, usize, usize, usize) {
        let x0 = (r[0] - pad).floor().max(0.0) as usize;
        let y0 = (r[1] - pad).floor().max(0.0) as usize;
        let x1 = ((r[0] + r[2] + pad).ceil().max(0.0) as usize).min(self.w);
        let y1 = ((r[1] + r[3] + pad).ceil().max(0.0) as usize).min(self.h);
        (x0, y0, x1, y1)
    }

    fn blend(&mut self, i: usize, c: [u8; 3], a: f32) {
        if a <= 0.0 {
            return;
        }
        let a = a.min(1.0);
        let p = &mut self.px[i * 4..i * 4 + 4];
        let keep = 1.0 - a;
        p[0] = (c[2] as f32 * a + p[0] as f32 * keep).round() as u8;
        p[1] = (c[1] as f32 * a + p[1] as f32 * keep).round() as u8;
        p[2] = (c[0] as f32 * a + p[2] as f32 * keep).round() as u8;
        p[3] = (255.0 * a + p[3] as f32 * keep).round() as u8;
    }

    fn fill(&mut self, r: [f32; 4], radius: f32, clip: Option<&[f32]>, color: impl Fn(f32, f32) -> ([u8; 3], f32)) {
        let (x0, y0, x1, y1) = self.bounds(r, 1.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let mut a = coverage(sdf(fx, fy, r, radius));
                if let Some(m) = clip {
                    a *= m[y * self.w + x];
                }
                if a > 0.0 {
                    let (c, ca) = color(fx, fy);
                    self.blend(y * self.w + x, c, a * ca);
                }
            }
        }
    }

    fn rrect(&mut self, r: [f32; 4], radius: f32, c: [u8; 3], a: f32, clip: Option<&[f32]>) {
        self.fill(r, radius, clip, |_, _| (c, a));
    }

    fn ring(&mut self, r: [f32; 4], radius: f32, width: f32, c: [u8; 3], a: f32) {
        let (x0, y0, x1, y1) = self.bounds(r, 1.0);
        let inner = [r[0] + width, r[1] + width, r[2] - 2.0 * width, r[3] - 2.0 * width];
        for y in y0..y1 {
            for x in x0..x1 {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let cov = coverage(sdf(fx, fy, r, radius)) - coverage(sdf(fx, fy, inner, (radius - width).max(0.0)));
                if cov > 0.0 {
                    self.blend(y * self.w + x, c, cov * a);
                }
            }
        }
    }

    fn shadow(&mut self, r: [f32; 4], radius: f32, blur: f32, peak: f32) {
        let (x0, y0, x1, y1) = self.bounds(r, blur + 1.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = sdf(x as f32 + 0.5, y as f32 + 0.5, r, radius).max(0.0);
                if d < blur {
                    let t = 1.0 - d / blur;
                    self.blend(y * self.w + x, [0, 0, 0], peak * t * t * t);
                }
            }
        }
    }

    fn mask(&self, r: [f32; 4], radius: f32) -> Vec<f32> {
        let mut m = vec![0.0f32; self.w * self.h];
        let (x0, y0, x1, y1) = self.bounds(r, 1.0);
        for y in y0..y1 {
            for x in x0..x1 {
                m[y * self.w + x] = coverage(sdf(x as f32 + 0.5, y as f32 + 0.5, r, radius));
            }
        }
        m
    }
}

fn rgb(c: [u8; 3]) -> COLORREF {
    COLORREF(c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16)
}

fn text_out(dc: HDC, font: HFONT, x: f32, y: f32, text: &str, c: [u8; 3]) {
    let units = wide(text);
    if units.is_empty() {
        return;
    }
    unsafe {
        SelectObject(dc, HGDIOBJ(font.0));
        SetTextColor(dc, rgb(c));
        let _ = TextOutW(dc, x.round() as i32, y.round() as i32, &units);
    }
}

fn draw(d: &mut Dlg) {
    let s = d.s;
    let pal = palette(d.dark);
    let p = &d.plan;
    let m = (SHADOW * s).round();
    let (ww, wh) = (d.size.0 as usize, d.size.1 as usize);
    let px = unsafe { std::slice::from_raw_parts_mut(d.bits as *mut u8, ww * wh * 4) };
    px.fill(0);
    let mut cv = Canvas { px, w: ww, h: wh };
    let radius = RADIUS * s;
    let outer = [m, m, p.w, p.h];
    cv.shadow([m, m + DROP * s, p.w, p.h], radius, BLUR * s, pal.shadow);
    cv.rrect(outer, radius, pal.edge, 1.0, None);
    let one = s.max(1.0).round();
    let inner = [m + one, m + one, p.w - 2.0 * one, p.h - 2.0 * one];
    let clip = cv.mask(inner, radius - one);
    cv.rrect(inner, radius - one, pal.body, 1.0, None);
    cv.rrect([m, m + p.foot_y, p.w, p.h - p.foot_y], 0.0, pal.foot, 1.0, Some(&clip));
    cv.rrect([m, m + p.foot_y, p.w, one], 0.0, pal.divider, 1.0, Some(&clip));

    let b = [p.badge[0] + m, p.badge[1] + m, p.badge[2], p.badge[3]];
    cv.rrect(b, b[2] / 2.0, pal.critical, 1.0, None);
    let (cx, size) = (b[0] + b[2] / 2.0, b[2]);
    let bar_w = (0.11 * size).max(2.0);
    cv.rrect([cx - bar_w / 2.0, b[1] + 0.24 * size, bar_w, 0.34 * size], bar_w / 2.0, [255, 255, 255], 1.0, None);
    let dot = (0.13 * size).max(2.4);
    cv.rrect([cx - dot / 2.0, b[1] + 0.68 * size, dot, dot], dot / 2.0, [255, 255, 255], 1.0, None);

    let btn_radius = 4.0 * s;
    for (i, btn) in p.buttons.iter().enumerate() {
        let r = [btn.rect[0] + m, btn.rect[1] + m, btn.rect[2], btn.rect[3]];
        let hover = d.view.hover == Some(i) && d.view.press.is_none_or(|k| k == i);
        let press = d.view.press == Some(i) && d.view.hover == Some(i);
        if btn.primary {
            let (a, bcol) = (pal.accent_a, pal.accent_b);
            let span = r[2] + r[3];
            let (r0, r1) = (r[0], r[1]);
            let tone = if press { 0.12 } else { 0.0 };
            let lift = if hover && !press { 0.08 } else { 0.0 };
            cv.fill(r, btn_radius, None, move |x, y| {
                let t = ((x - r0) + (y - r1)) / span;
                let c = mix(a, bcol, t.clamp(0.0, 1.0));
                (mix(mix(c, [0, 0, 0], tone), [255, 255, 255], lift), 1.0)
            });
        } else {
            let fill = if press {
                pal.btn_press
            } else if hover {
                pal.btn_hover
            } else {
                pal.btn
            };
            cv.rrect(r, btn_radius, fill, 1.0, None);
            cv.ring(r, btn_radius, one, pal.btn_edge, 1.0);
        }
        if d.view.kbd && d.view.focus == i {
            let g = 3.0 * s;
            cv.ring([r[0] - g, r[1] - g, r[2] + 2.0 * g, r[3] + 2.0 * g], btn_radius + g, (2.0 * s).round().max(2.0), pal.ink, 1.0);
        }
    }

    unsafe {
        SetBkMode(d.dc, TRANSPARENT);
    }
    for line in &p.lines {
        text_out(d.dc, d.fonts.font(line.head), line.x + m, line.y + m, &line.text, if line.head { pal.ink } else { pal.soft });
    }
    let lh = d.fonts.line_height(false);
    for (i, btn) in p.buttons.iter().enumerate() {
        let adv = d.fonts.advance(false, &btn.label);
        let x = btn.rect[0] + m + ((btn.rect[2] - adv) / 2.0).round();
        let y = btn.rect[1] + m + ((btn.rect[3] - lh) / 2.0).round();
        let press = d.view.press == Some(i) && d.view.hover == Some(i);
        let c = if btn.primary {
            pal.on_accent
        } else if press {
            pal.soft
        } else {
            pal.ink
        };
        text_out(d.dc, d.fonts.body, x, y, &btn.label, c);
    }
    unsafe {
        let _ = GdiFlush();
    }
    let outer_mask = cv.mask(outer, radius);
    for (i, &c) in outer_mask.iter().enumerate() {
        if c >= 1.0 {
            cv.px[i * 4 + 3] = 255;
        }
    }
}

struct Dlg {
    hwnd: HWND,
    dc: HDC,
    bmp: HBITMAP,
    bits: *mut c_void,
    size: (i32, i32),
    origin: (i32, i32),
    heading: String,
    text: String,
    plan: Plan,
    fonts: Fonts,
    dark: bool,
    s: f32,
    view: View,
    copied: bool,
    tracking: bool,
    done: bool,
    born: Instant,
    animate: bool,
    placed: bool,
    replace: bool,
    space_down: bool,
}

impl Drop for Dlg {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
            self.fonts.free();
            let _ = DeleteDC(self.dc);
            let _ = DeleteObject(HGDIOBJ(self.bmp.0));
        }
    }
}

thread_local! {
    static STATE: RefCell<Option<Dlg>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Dlg) -> R) -> Option<R> {
    STATE
        .try_with(|c| {
            let mut g = c.try_borrow_mut().ok()?;
            g.as_mut().map(f)
        })
        .ok()
        .flatten()
}

enum Outcome {
    Shown,
    Busy,
    Failed,
}

pub fn fatal(text: &str) {
    let heading = crate::i18n::get("fatal.title");
    let text = text.replace("\r\n", "\n").replace('\r', "\n").trim().to_string();
    if assistive() {
        crate::winui::fatal(&heading, &text);
        return;
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&heading, &text))).unwrap_or(Outcome::Failed);
    if matches!(outcome, Outcome::Failed) {
        crate::winui::fatal(&heading, &text);
    }
}

fn assistive() -> bool {
    #[repr(C)]
    struct HighContrast {
        size: u32,
        flags: u32,
        scheme: *mut u16,
    }
    const HCF_HIGHCONTRASTON: u32 = 1;
    let mut reader = BOOL(0);
    let mut contrast = HighContrast { size: size_of::<HighContrast>() as u32, flags: 0, scheme: std::ptr::null_mut() };
    unsafe {
        let r = SystemParametersInfoW(SPI_GETSCREENREADER, 0, Some(&mut reader as *mut BOOL as *mut c_void), Default::default()).is_ok() && reader.as_bool();
        let h =
            SystemParametersInfoW(SPI_GETHIGHCONTRAST, contrast.size, Some(&mut contrast as *mut HighContrast as *mut c_void), Default::default()).is_ok() && contrast.flags & HCF_HIGHCONTRASTON != 0;
        r || h
    }
}

fn animations_on() -> bool {
    let mut on = BOOL(1);
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut BOOL as *mut c_void), Default::default()).is_err() || on.as_bool() }
}

struct Leave;

impl Drop for Leave {
    fn drop(&mut self) {
        STATE.with(|c| {
            if let Ok(mut g) = c.try_borrow_mut() {
                drop(g.take());
            }
        });
    }
}

fn run(heading: &str, text: &str) -> Outcome {
    if STATE.with(|c| c.try_borrow().map_or(true, |g| g.is_some())) {
        return Outcome::Busy;
    }
    let _leave = Leave;
    if !open(heading, text) {
        return Outcome::Failed;
    }
    pump();
    Outcome::Shown
}

fn pump() {
    let mut msg = MSG::default();
    while !with(|d| d.done).unwrap_or(true) {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn register_class() -> Option<HINSTANCE> {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    let hinst = unsafe { GetModuleHandleW(None) }.ok().map(|h| HINSTANCE(h.0))?;
    ONCE.call_once(|| unsafe {
        let icon = LoadIconW(Some(hinst), PCWSTR(std::ptr::without_provenance(1))).unwrap_or(HICON::default());
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: icon,
            hIconSm: icon,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("WispingNotice"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    });
    Some(hinst)
}

fn cursor_monitor() -> ([i32; 4], f32) {
    let mut cur = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cur);
    }
    let mon = unsafe { MonitorFromPoint(cur, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    let work = if unsafe { GetMonitorInfoW(mon, &mut info) }.as_bool() { [info.rcWork.left, info.rcWork.top, info.rcWork.right, info.rcWork.bottom] } else { [0, 0, 1280, 720] };
    let (mut dx, mut dy) = (96u32, 96u32);
    let ok = unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_ok();
    (work, if ok && dx >= 96 { (dx as f32 / 96.0).clamp(1.0, 4.0) } else { 1.0 })
}

fn make_dib(dc: HDC, w: i32, h: i32) -> Option<(HBITMAP, *mut c_void)> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bmp = unsafe { CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
    if bits.is_null() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(bmp.0));
        }
        return None;
    }
    Some((bmp, bits))
}

fn open(heading: &str, text: &str) -> bool {
    let Some(hinst) = register_class() else {
        return false;
    };
    let dark = crate::app::detect_dark();
    let (work, s) = cursor_monitor();
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        return false;
    }
    let fonts = Fonts::new(dc, s, has_cjk(heading) || has_cjk(text));
    let plan = plan(heading, text, &fonts, s, false);
    let m = (SHADOW * s).round() as i32;
    let (bw, bh) = (plan.w as i32, plan.h as i32);
    let (ww, wh) = (bw + 2 * m, bh + 2 * m);
    let bx = (work[0] + (work[2] - work[0] - bw) / 2).min(work[2] - bw).max(work[0]);
    let by = (work[1] + (work[3] - work[1] - bh) * 9 / 20).min(work[3] - bh).max(work[1]);
    let title: Vec<u16> = heading.encode_utf16().chain(Some(0)).collect();
    let Ok(hwnd) = (unsafe { CreateWindowExW(WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_APPWINDOW, w!("WispingNotice"), PCWSTR(title.as_ptr()), WS_POPUP, bx - m, by - m, ww, wh, None, None, Some(hinst), None) })
    else {
        let mut fonts = fonts;
        fonts.free();
        unsafe {
            let _ = DeleteDC(dc);
        }
        return false;
    };
    let Some((bmp, bits)) = make_dib(dc, ww, wh) else {
        let mut fonts = fonts;
        fonts.free();
        unsafe {
            let _ = DestroyWindow(hwnd);
            let _ = DeleteDC(dc);
        }
        return false;
    };
    unsafe {
        SelectObject(dc, HGDIOBJ(bmp.0));
    }
    let focus = plan.buttons.iter().position(|b| b.act == Act::Close).unwrap_or(0);
    let animate = animations_on();
    let dlg = Dlg {
        hwnd,
        dc,
        bmp,
        bits,
        size: (ww, wh),
        origin: (bx - m, by - m),
        heading: heading.to_string(),
        text: text.to_string(),
        plan,
        fonts,
        dark,
        s,
        view: View { focus, ..View::default() },
        copied: false,
        tracking: false,
        done: false,
        born: Instant::now(),
        animate,
        placed: false,
        replace: false,
        space_down: false,
    };
    STATE.with(|c| *c.borrow_mut() = Some(dlg));
    if !redraw() {
        return false;
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        if !SetForegroundWindow(hwnd).as_bool() {
            let flash = FLASHWINFO { cbSize: size_of::<FLASHWINFO>() as u32, hwnd, dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG, uCount: 3, dwTimeout: 0 };
            let _ = FlashWindowEx(&flash);
        }
        if animate {
            SetTimer(Some(hwnd), ANIM_TIMER, 15, None);
        }
    }
    true
}

fn rescale(dpi: u32, to: Option<RECT>) {
    let s = (dpi as f32 / 96.0).clamp(1.0, 4.0);
    let changed = with(|d| {
        if (s - d.s).abs() < 0.01 {
            return false;
        }
        let mut fonts = Fonts::new(d.dc, s, has_cjk(&d.heading) || has_cjk(&d.text));
        let plan = plan(&d.heading, &d.text, &fonts, s, d.copied);
        let m = (SHADOW * s).round() as i32;
        let (ww, wh) = (plan.w as i32 + 2 * m, plan.h as i32 + 2 * m);
        let Some((bmp, bits)) = make_dib(d.dc, ww, wh) else {
            fonts.free();
            return false;
        };
        unsafe {
            SelectObject(d.dc, HGDIOBJ(bmp.0));
            let _ = DeleteObject(HGDIOBJ(d.bmp.0));
        }
        d.fonts.free();
        (d.bmp, d.bits, d.size, d.plan, d.s, d.fonts) = (bmp, bits, (ww, wh), plan, s, fonts);
        if let Some(r) = to {
            d.origin = (r.left, r.top);
        }
        d.replace = true;
        true
    });
    if changed == Some(true) {
        redraw();
    }
}

fn redraw() -> bool {
    with(draw);
    present()
}

fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}

fn present() -> bool {
    let work = with(|d| {
        let elapsed = d.born.elapsed().as_secs_f32() * 1000.0;
        let t = if !d.animate {
            1.0
        } else if !d.placed {
            0.0
        } else {
            (elapsed / ANIM_MS).min(1.0)
        };
        let e = ease(t);
        let rise = ((1.0 - e) * RISE * d.s).round() as i32;
        let dst = (!d.placed || d.replace || t < 1.0).then_some(POINT { x: d.origin.0, y: d.origin.1 + rise });
        if !d.placed {
            d.born = Instant::now();
        }
        d.placed = true;
        d.replace = false;
        (d.hwnd, d.dc, dst, SIZE { cx: d.size.0, cy: d.size.1 }, (e * 255.0).round() as u8, t >= 1.0)
    });
    let Some((hwnd, dc, dst, full, alpha, settled)) = work else {
        return false;
    };
    let src = POINT::default();
    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
    let pushed = unsafe { UpdateLayeredWindow(hwnd, None, dst.as_ref().map(|p| p as *const POINT), Some(&full), Some(dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA) }.is_ok();
    if settled {
        unsafe {
            let _ = KillTimer(Some(hwnd), ANIM_TIMER);
        }
    }
    pushed
}

fn close() {
    let hwnd = with(|d| {
        d.done = true;
        std::mem::take(&mut d.hwnd)
    });
    if let Some(h) = hwnd.filter(|h| !h.is_invalid()) {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

fn copy_to_clipboard(hwnd: HWND, text: &str) -> bool {
    let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let ok = EmptyClipboard().is_ok()
            && (|| {
                let h = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2).ok()?;
                let p = GlobalLock(h) as *mut u16;
                if p.is_null() {
                    return None;
                }
                std::ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
                let _ = GlobalUnlock(h);
                SetClipboardData(CF_UNICODETEXT, Some(HANDLE(h.0))).ok()
            })()
            .is_some();
        let _ = CloseClipboard();
        ok
    }
}

fn copy_text() {
    let Some((hwnd, text)) = with(|d| (d.hwnd, format!("{}\n\n{}", d.heading, d.text))) else {
        return;
    };
    if copy_to_clipboard(hwnd, &text) {
        with(|d| {
            d.copied = true;
            d.plan = plan(&d.heading, &d.text, &d.fonts, d.s, true);
        });
        redraw();
    }
}

fn activate(i: usize) {
    match with(|d| d.plan.buttons.get(i).map(|b| b.act)).flatten() {
        Some(Act::Close) => close(),
        Some(Act::Copy) => copy_text(),
        None => {}
    }
}

fn signed(lp: LPARAM) -> (f32, f32) {
    ((lp.0 & 0xFFFF) as u16 as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as f32)
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(hwnd, msg, wp, lp))).unwrap_or_else(|_| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

fn handle(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_NCHITTEST => {
            let mut r = RECT::default();
            let rect_ok = unsafe { GetWindowRect(hwnd, &mut r) }.is_ok();
            let (sx, sy) = signed(lp);
            let on = rect_ok
                && with(|d| {
                    let m = (SHADOW * d.s).round();
                    let (x, y) = (sx - r.left as f32 - m, sy - r.top as f32 - m);
                    on_body(&d.plan, x, y) && hit_at(&d.plan, x, y).is_none()
                })
                .unwrap_or(false);
            return LRESULT(if on { HTCAPTION } else { HTCLIENT } as isize);
        }
        WM_MOUSEMOVE => {
            let (x, y) = signed(lp);
            let mut track = false;
            let changed = with(|d| {
                let m = (SHADOW * d.s).round();
                let hit = hit_at(&d.plan, x - m, y - m);
                track = !d.tracking;
                d.tracking = true;
                std::mem::replace(&mut d.view.hover, hit) != hit
            })
            .unwrap_or(false);
            if track {
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                unsafe {
                    let _ = TrackMouseEvent(&mut t);
                }
            }
            if changed {
                redraw();
            }
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            let changed = with(|d| {
                d.tracking = false;
                d.view.hover.take().is_some()
            })
            .unwrap_or(false);
            if changed {
                redraw();
            }
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            let (x, y) = signed(lp);
            let hit = with(|d| {
                let m = (SHADOW * d.s).round();
                let hit = hit_at(&d.plan, x - m, y - m);
                d.view.press = hit;
                d.view.kbd = false;
                hit
            })
            .flatten();
            if hit.is_some() {
                unsafe {
                    SetCapture(hwnd);
                }
                redraw();
            }
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let (x, y) = signed(lp);
            unsafe {
                let _ = ReleaseCapture();
            }
            let pressed = with(|d| {
                let m = (SHADOW * d.s).round();
                let up = hit_at(&d.plan, x - m, y - m);
                let down = d.view.press.take();
                d.view.hover = up;
                if up.is_some() && up == down { up } else { None }
            })
            .flatten();
            match pressed {
                Some(i) => activate(i),
                None => {
                    redraw();
                }
            }
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let key = wp.0 as u16;
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            let fresh = (lp.0 >> 30) & 1 == 0 && with(|d| d.born.elapsed().as_millis() >= KEY_GRACE_MS) == Some(true);
            if key == VK_ESCAPE.0 {
                close();
            } else if key == VK_RETURN.0 {
                if let Some(i) = with(|d| d.view.focus).filter(|_| fresh) {
                    activate(i);
                }
            } else if key == VK_SPACE.0 {
                if fresh {
                    with(|d| d.space_down = true);
                }
            } else if key == VK_TAB.0 || key == VK_LEFT.0 || key == VK_RIGHT.0 {
                let back = key == VK_LEFT.0 || (key == VK_TAB.0 && shift);
                with(|d| {
                    let n = d.plan.buttons.len().max(1);
                    d.view.focus = if back { (d.view.focus + n - 1) % n } else { (d.view.focus + 1) % n };
                    d.view.kbd = true;
                });
                redraw();
            } else if ctrl && key == b'C' as u16 {
                copy_text();
            }
            return LRESULT(0);
        }
        WM_KEYUP => {
            if wp.0 as u16 == VK_SPACE.0
                && let Some(i) = with(|d| std::mem::take(&mut d.space_down).then_some(d.view.focus)).flatten()
            {
                activate(i);
            }
            return LRESULT(0);
        }
        WM_TIMER if wp.0 == ANIM_TIMER => {
            present();
            return LRESULT(0);
        }
        WM_ACTIVATE => {
            if wp.0 & 0xFFFF == 0 {
                unsafe {
                    let _ = SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
        }
        WM_DPICHANGED => {
            let to = unsafe { (lp.0 as *const RECT).as_ref() }.copied();
            rescale((wp.0 & 0xFFFF) as u32, to);
            return LRESULT(0);
        }
        WM_SETTINGCHANGE => {
            let named = lp.0 != 0 && unsafe { PCWSTR(lp.0 as *const u16).to_string() }.is_ok_and(|n| n == "ImmersiveColorSet");
            if named {
                let dark = crate::app::detect_dark();
                if with(|d| std::mem::replace(&mut d.dark, dark) != dark) == Some(true) {
                    redraw();
                }
            }
        }
        WM_CLOSE => {
            close();
            return LRESULT(0);
        }
        WM_DESTROY => {
            with(|d| {
                d.done = true;
                d.hwnd = HWND::default();
            });
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(s: &str) -> f32 {
        s.chars().count() as f32 * 10.0
    }

    #[test]
    fn pieces_keep_spaces_and_split_paths() {
        assert_eq!(pieces("ab  cd e"), vec!["ab  ", "cd ", "e"]);
        assert_eq!(pieces("C:\\a\\bb/c.rs"), vec!["C:\\", "a\\", "bb/", "c.rs"]);
        assert_eq!(pieces(""), Vec::<&str>::new());
    }

    #[test]
    fn wrapped_lines_fit_and_keep_every_character() {
        let samples = ["the quick brown fox jumps over the lazy dog again and again", "averyveryverylongwordwithoutanybreakopportunityatall", "panicked at src\\engine\\cleaner.rs:12:5:\nindex out of bounds", "a\n\nb", ""];
        for s in samples {
            let lines = wrap(&width, s, 100.0, usize::MAX);
            assert!(!lines.is_empty());
            for l in &lines {
                assert!(width(l) <= 100.0, "{l:?}");
            }
            let strip = |t: &str| t.chars().filter(|c| !c.is_whitespace()).collect::<String>();
            assert_eq!(strip(&lines.concat()), strip(s), "{s:?}");
        }
        assert_eq!(wrap(&width, "a\nb", 100.0, usize::MAX), vec!["a", "b"]);
        assert_eq!(wrap(&width, "a\n\nb", 100.0, usize::MAX), vec!["a", "", "b"]);
    }

    #[test]
    fn long_text_is_capped_with_an_ellipsis() {
        let long = "word ".repeat(100);
        let lines = cap(&width, wrap(&width, &long, 100.0, 4), 3, 100.0);
        assert_eq!(lines.len(), 3);
        assert!(lines[2].ends_with('…') && width(&lines[2]) <= 100.0);
        assert_eq!(cap(&width, vec!["a".into()], 3, 100.0), vec!["a".to_string()]);
    }

    #[test]
    fn rounded_rect_coverage_is_full_inside_and_empty_outside() {
        let r = [10.0, 10.0, 100.0, 50.0];
        assert_eq!(coverage(sdf(60.0, 35.0, r, 8.0)), 1.0);
        assert_eq!(coverage(sdf(10.5, 10.5, r, 8.0)), 0.0);
        assert_eq!(coverage(sdf(200.0, 35.0, r, 8.0)), 0.0);
        assert!(coverage(sdf(12.0, 12.0, r, 8.0)) < 1.0);
        assert_eq!(coverage(sdf(60.0, 35.0, r, 0.0)), 1.0);
    }

    #[test]
    fn cjk_is_detected() {
        assert!(has_cjk("操作意外中断"));
        assert!(!has_cjk("panicked at src/app.rs"));
    }

    #[test]
    fn fonts_resolve_to_real_faces() {
        let dc = unsafe { CreateCompatibleDC(None) };
        for han in [false, true] {
            let mut f = Fonts::new(dc, 1.5, han);
            assert!(!f.head.is_invalid() && !f.body.is_invalid());
            assert!(f.line_height(true) > f.line_height(false) && f.line_height(false) > 0.0);
            assert!(f.advance(false, "Wisping") > 0.0);
            f.free();
        }
        unsafe {
            let _ = DeleteDC(dc);
        }
    }

}
