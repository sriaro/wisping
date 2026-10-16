#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

fn worst(row: &[f64], side: f64) -> f64 {
    let sum: f64 = row.iter().sum();
    let max = row.iter().cloned().fold(0.0, f64::max);
    let min = row.iter().cloned().fold(f64::MAX, f64::min);
    let s2 = side * side;
    ((s2 * max) / (sum * sum)).max((sum * sum) / (s2 * min))
}

fn place(row: &[f64], area: &mut Rect, out: &mut Vec<Rect>) {
    let sum: f64 = row.iter().sum();
    if area.w >= area.h {
        let w = sum / area.h;
        let mut y = area.y;
        for a in row {
            let h = a / w;
            out.push(Rect { x: area.x, y, w, h });
            y += h;
        }
        area.x += w;
        area.w -= w;
    } else {
        let h = sum / area.w;
        let mut x = area.x;
        for a in row {
            let w = a / h;
            out.push(Rect { x, y: area.y, w, h });
            x += w;
        }
        area.y += h;
        area.h -= h;
    }
}

pub fn layout(sizes: &[u64], w: f64, h: f64) -> Vec<Rect> {
    let total: f64 = sizes.iter().map(|s| *s as f64).sum();
    if total <= 0.0 || sizes.is_empty() || w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let scale = w * h / total;
    let areas: Vec<f64> = sizes.iter().map(|s| (*s as f64).max(1.0) * scale).collect();
    let mut area = Rect { x: 0.0, y: 0.0, w, h };
    let mut out = Vec::with_capacity(areas.len());
    let mut row: Vec<f64> = Vec::new();
    let mut i = 0;
    while i < areas.len() {
        let side = area.w.min(area.h);
        let a = areas[i];
        let mut with = row.clone();
        with.push(a);
        if row.is_empty() || worst(&with, side) <= worst(&row, side) {
            row = with;
            i += 1;
        } else {
            place(&row, &mut area, &mut out);
            row.clear();
        }
    }
    if !row.is_empty() {
        place(&row, &mut area, &mut out);
    }
    out
}
