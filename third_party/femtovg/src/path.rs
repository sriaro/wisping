use std::{
    cell::{RefCell, RefMut},
    slice,
};

use crate::geometry::{Position, Transform2D, Vector};

mod cache;
pub use cache::{Convexity, PathCache};

const KAPPA90: f32 = 0.552_284_8;

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Default)]
pub enum Solidity {
    #[default]
    Solid = 1,
    Hole = 2,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PackedVerb {
    MoveTo,
    LineTo,
    BezierTo,
    Solid,
    Hole,
    Close,
}

#[derive(Copy, Clone, Debug)]
pub enum Verb {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    BezierTo(f32, f32, f32, f32, f32, f32),
    Solid,
    Hole,
    Close,
}

impl Verb {
    fn num_coordinates(&self) -> usize {
        match *self {
            Self::MoveTo(..) => 1,
            Self::LineTo(..) => 1,
            Self::BezierTo(..) => 3,
            Self::Solid => 0,
            Self::Hole => 0,
            Self::Close => 0,
        }
    }

    fn from_packed(packed: &PackedVerb, coords: &[Position]) -> Self {
        match *packed {
            PackedVerb::MoveTo => Self::MoveTo(coords[0].x, coords[0].y),
            PackedVerb::LineTo => Self::LineTo(coords[0].x, coords[0].y),
            PackedVerb::BezierTo => Self::BezierTo(
                coords[0].x,
                coords[0].y,
                coords[1].x,
                coords[1].y,
                coords[2].x,
                coords[2].y,
            ),
            PackedVerb::Solid => Self::Solid,
            PackedVerb::Hole => Self::Hole,
            PackedVerb::Close => Self::Close,
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct Path {
    verbs: Vec<PackedVerb>,
    coords: Vec<Position>,
    last_pos: Position,
    dist_tol: f32,
    pub(crate) cache: RefCell<Option<(u64, PathCache)>>,
}

impl Path {
    pub fn new() -> Self {
        Self {
            dist_tol: 0.01,
            ..Default::default()
        }
    }

    pub fn size(&self) -> usize {
        std::mem::size_of::<PackedVerb>() * self.verbs.len() + std::mem::size_of::<f32>() * self.coords.len()
    }

    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    pub fn verbs(&self) -> PathIter<'_> {
        PathIter {
            verbs: self.verbs.iter(),
            coords: &self.coords,
        }
    }

    pub(crate) fn cache<'a>(&'a self, transform: &Transform2D, tess_tol: f32, dist_tol: f32) -> RefMut<'a, PathCache> {
        let key = transform.cache_key();

        let needs_rebuild = if let Some((transform_cache_key, _cache)) = &*self.cache.borrow() {
            key != *transform_cache_key
        } else {
            true
        };

        if needs_rebuild {
            let path_cache = PathCache::new(self.verbs(), transform, tess_tol, dist_tol);
            *self.cache.borrow_mut() = Some((key, path_cache));
        }

        RefMut::map(self.cache.borrow_mut(), |cache| &mut cache.as_mut().unwrap().1)
    }

    pub(crate) fn dashed_with_tolerance(&self, dash: &[f32], offset: f32, tess_tol: f32) -> Self {
        let dash = normalize_dash_pattern(dash);
        if dash.is_empty() {
            return self.clone();
        }

        let contours = self.flattened_contours(tess_tol, self.dist_tol);
        let mut dashed = Self::new();
        dashed.dist_tol = self.dist_tol;

        for contour in contours {
            if contour.points.len() < 2 {
                continue;
            }

            let mut cursor = DashCursor::new(&dash, offset);
            for segment in contour.points.windows(2) {
                if !dash_line_segment(&mut dashed, segment[0], segment[1], &mut cursor, self.dist_tol) {
                    return self.clone();
                }
            }

            if contour.closed {
                if !dash_line_segment(
                    &mut dashed,
                    *contour.points.last().unwrap(),
                    contour.points[0],
                    &mut cursor,
                    self.dist_tol,
                ) {
                    return self.clone();
                }
            }
        }

        dashed
    }

    fn flattened_contours(&self, tess_tol: f32, dist_tol: f32) -> Vec<FlattenedContour> {
        let mut contours = Vec::new();
        let mut current = FlattenedContour::default();
        let mut current_pos = Position::default();
        let mut first_pos = None;

        let finish_current = |contours: &mut Vec<FlattenedContour>, current: &mut FlattenedContour| {
            if current.points.len() >= 2 {
                contours.push(std::mem::take(current));
            } else {
                current.points.clear();
                current.closed = false;
            }
        };

        for verb in self.verbs() {
            match verb {
                Verb::MoveTo(x, y) => {
                    finish_current(&mut contours, &mut current);
                    current_pos = Position { x, y };
                    first_pos = Some(current_pos);
                    current.points.push(current_pos);
                }
                Verb::LineTo(x, y) => {
                    current_pos = Position { x, y };
                    push_flattened_point(&mut current.points, current_pos, dist_tol);
                }
                Verb::BezierTo(c1x, c1y, c2x, c2y, x, y) => {
                    let c1 = Position { x: c1x, y: c1y };
                    let c2 = Position { x: c2x, y: c2y };
                    let end = Position { x, y };
                    flatten_bezier(&mut current.points, current_pos, c1, c2, end, 0, tess_tol, dist_tol);
                    current_pos = end;
                }
                Verb::Close => {
                    if let Some(first) = first_pos {
                        current.closed = true;
                        current_pos = first;
                    }
                }
                Verb::Solid | Verb::Hole => {}
            }
        }

        finish_current(&mut contours, &mut current);
        contours
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.append(&[PackedVerb::MoveTo], &[Position { x, y }]);
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        self.append(&[PackedVerb::LineTo], &[Position { x, y }]);
    }

    pub fn bezier_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        self.append(
            &[PackedVerb::BezierTo],
            &[
                Position { x: c1x, y: c1y },
                Position { x: c2x, y: c2y },
                Position { x, y },
            ],
        );
    }

    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let pos0 = self.last_pos;
        let cpos = Position { x: cx, y: cy };
        let pos = Position { x, y };
        let pos1 = pos0 + (cpos - pos0) * (2.0 / 3.0);
        let pos2 = pos + (cpos - pos) * (2.0 / 3.0);

        self.append(&[PackedVerb::BezierTo], &[pos1, pos2, pos]);
    }

    pub fn close(&mut self) {
        self.append(&[PackedVerb::Close], &[]);
    }

    pub fn solidity(&mut self, solidity: Solidity) {
        match solidity {
            Solidity::Solid => self.append(&[PackedVerb::Solid], &[]),
            Solidity::Hole => self.append(&[PackedVerb::Hole], &[]),
        }
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.append(
            &[
                PackedVerb::MoveTo,
                PackedVerb::LineTo,
                PackedVerb::LineTo,
                PackedVerb::LineTo,
                PackedVerb::Close,
            ],
            &{
                let hoffset = Vector::x(w);
                let voffset = Vector::y(h);

                let tl = Position { x, y };
                let tr = tl + hoffset;
                let br = tr + voffset;
                let bl = tl + voffset;

                [tl, bl, br, tr]
            },
        );
    }

    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        self.rounded_rect_varying(x, y, w, h, r, r, r, r);
    }

    pub fn rounded_rect_varying(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rad_top_left: f32,
        rad_top_right: f32,
        rad_bottom_right: f32,
        rad_bottom_left: f32,
    ) {
        let usable = |r: f32| {
            if r.is_nan() || r <= 0.0 {
                0.0
            } else if r.is_infinite() {
                f32::MAX
            } else {
                r
            }
        };

        let mut tl = usable(rad_top_left);
        let mut tr = usable(rad_top_right);
        let mut br = usable(rad_bottom_right);
        let mut bl = usable(rad_bottom_left);

        if tl == 0.0 && tr == 0.0 && br == 0.0 && bl == 0.0 {
            self.rect(x, y, w, h);
        } else {
            let (width, height) = (f64::from(w.abs()), f64::from(h.abs()));
            let (tl64, tr64) = (f64::from(tl), f64::from(tr));
            let (br64, bl64) = (f64::from(br), f64::from(bl));

            let scale = [
                (width, tl64 + tr64),
                (width, bl64 + br64),
                (height, tl64 + bl64),
                (height, tr64 + br64),
            ]
            .into_iter()
            .filter(|&(_, sum)| sum > 0.0)
            .map(|(side, sum)| side / sum)
            .fold(1.0f64, f64::min);

            if scale < 1.0 {
                tl = (tl64 * scale) as f32;
                tr = (tr64 * scale) as f32;
                br = (br64 * scale) as f32;
                bl = (bl64 * scale) as f32;
            }

            let (sign_x, sign_y) = (w.signum(), h.signum());

            let rx_bl = bl * sign_x;
            let ry_bl = bl * sign_y;

            let rx_br = br * sign_x;
            let ry_br = br * sign_y;

            let rx_tr = tr * sign_x;
            let ry_tr = tr * sign_y;

            let rx_tl = tl * sign_x;
            let ry_tl = tl * sign_y;

            self.append(
                &[
                    PackedVerb::MoveTo,
                    PackedVerb::LineTo,
                    PackedVerb::BezierTo,
                    PackedVerb::LineTo,
                    PackedVerb::BezierTo,
                    PackedVerb::LineTo,
                    PackedVerb::BezierTo,
                    PackedVerb::LineTo,
                    PackedVerb::BezierTo,
                    PackedVerb::Close,
                ],
                &[
                    Position { x, y: y + ry_tl },
                    Position { x, y: y + h - ry_bl },
                    Position {
                        x,
                        y: y + h - ry_bl * (1.0 - KAPPA90),
                    },
                    Position {
                        x: x + rx_bl * (1.0 - KAPPA90),
                        y: y + h,
                    },
                    Position { x: x + rx_bl, y: y + h },
                    Position {
                        x: x + w - rx_br,
                        y: y + h,
                    },
                    Position {
                        x: x + w - rx_br * (1.0 - KAPPA90),
                        y: y + h,
                    },
                    Position {
                        x: x + w,
                        y: y + h - ry_br * (1.0 - KAPPA90),
                    },
                    Position {
                        x: x + w,
                        y: y + h - ry_br,
                    },
                    Position { x: x + w, y: y + ry_tr },
                    Position {
                        x: x + w,
                        y: y + ry_tr * (1.0 - KAPPA90),
                    },
                    Position {
                        x: x + w - rx_tr * (1.0 - KAPPA90),
                        y,
                    },
                    Position { x: x + w - rx_tr, y },
                    Position { x: x + rx_tl, y },
                    Position {
                        x: x + rx_tl * (1.0 - KAPPA90),
                        y,
                    },
                    Position {
                        x,
                        y: y + ry_tl * (1.0 - KAPPA90),
                    },
                    Position { x, y: y + ry_tl },
                ],
            );
        }
    }

    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32) {
        self.append(
            &[
                PackedVerb::MoveTo,
                PackedVerb::BezierTo,
                PackedVerb::BezierTo,
                PackedVerb::BezierTo,
                PackedVerb::BezierTo,
                PackedVerb::Close,
            ],
            &{
                let cpos = Position { x: cx, y: cy };
                let hoffset = Vector::x(rx);
                let voffset = Vector::y(ry);
                [
                    cpos - hoffset,
                    cpos - hoffset + voffset * KAPPA90,
                    cpos - hoffset * KAPPA90 + voffset,
                    cpos + voffset,
                    cpos + hoffset * KAPPA90 + voffset,
                    cpos + hoffset + voffset * KAPPA90,
                    cpos + hoffset,
                    cpos + hoffset - voffset * KAPPA90,
                    cpos + hoffset * KAPPA90 - voffset,
                    cpos - voffset,
                    cpos - hoffset * KAPPA90 - voffset,
                    cpos - hoffset - voffset * KAPPA90,
                    cpos - hoffset,
                ]
            },
        );
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32) {
        self.ellipse(cx, cy, r, r);
    }

    fn append(&mut self, verbs: &[PackedVerb], coords: &[Position]) {
        if !coords.is_empty() {
            self.last_pos = coords[coords.len() - 1];
        }

        self.verbs.extend_from_slice(verbs);
        self.coords.extend_from_slice(coords);
    }
}

#[derive(Default)]
struct FlattenedContour {
    points: Vec<Position>,
    closed: bool,
}

struct DashCursor<'a> {
    dash: &'a [f32],
    index: usize,
    remaining: f32,
    drawing: bool,
}

impl<'a> DashCursor<'a> {
    fn new(dash: &'a [f32], offset: f32) -> Self {
        let total = dash.iter().sum::<f32>();
        let mut normalized_offset = if offset.is_finite() { offset % total } else { 0.0 };
        if normalized_offset < 0.0 {
            normalized_offset += total;
        }

        let mut index = 0;
        for (dash_index, interval) in dash.iter().copied().enumerate() {
            if normalized_offset > interval || (normalized_offset == interval && interval > 0.0) {
                normalized_offset -= interval;
                index = (dash_index + 1) % dash.len();
            } else {
                index = dash_index;
                break;
            }
        }

        let mut cursor = Self {
            dash,
            index,
            remaining: (dash[index] - normalized_offset).max(0.0),
            drawing: index % 2 == 0,
        };
        cursor.skip_empty_entries();
        cursor
    }

    fn advance(&mut self) {
        self.index = (self.index + 1) % self.dash.len();
        self.remaining = self.dash[self.index];
        self.drawing = self.index % 2 == 0;
        self.skip_empty_entries();
    }

    fn skip_empty_entries(&mut self) {
        for _ in 0..self.dash.len() {
            if self.remaining > f32::EPSILON {
                break;
            }
            self.index = (self.index + 1) % self.dash.len();
            self.remaining = self.dash[self.index];
            self.drawing = self.index % 2 == 0;
        }
    }
}

fn normalize_dash_pattern(dash: &[f32]) -> Vec<f32> {
    if dash.is_empty() || dash.iter().any(|value| !value.is_finite() || *value < 0.0) {
        return Vec::new();
    }

    let sum = dash.iter().sum::<f32>();
    if !sum.is_finite() || sum <= f32::EPSILON || !dash.iter().any(|value| *value > f32::EPSILON) {
        return Vec::new();
    }

    let mut normalized = dash.to_vec();
    if normalized.len() % 2 == 1 {
        normalized.extend_from_slice(dash);
    }

    if !normalized.iter().sum::<f32>().is_finite() {
        return Vec::new();
    }

    normalized
}

fn push_flattened_point(points: &mut Vec<Position>, point: Position, dist_tol: f32) {
    if points
        .last()
        .is_some_and(|last| Position::equals(*last, point, dist_tol))
    {
        return;
    }

    points.push(point);
}

#[allow(clippy::too_many_arguments)]
fn flatten_bezier(
    points: &mut Vec<Position>,
    p0: Position,
    p1: Position,
    p2: Position,
    p3: Position,
    level: usize,
    tess_tol: f32,
    dist_tol: f32,
) {
    if level > 10 {
        push_flattened_point(points, p3, dist_tol);
        return;
    }

    let p01 = Position {
        x: (p0.x + p1.x) * 0.5,
        y: (p0.y + p1.y) * 0.5,
    };
    let p12 = Position {
        x: (p1.x + p2.x) * 0.5,
        y: (p1.y + p2.y) * 0.5,
    };
    let p23 = Position {
        x: (p2.x + p3.x) * 0.5,
        y: (p2.y + p3.y) * 0.5,
    };
    let p012 = Position {
        x: (p01.x + p12.x) * 0.5,
        y: (p01.y + p12.y) * 0.5,
    };
    let p123 = Position {
        x: (p12.x + p23.x) * 0.5,
        y: (p12.y + p23.y) * 0.5,
    };
    let p0123 = Position {
        x: (p012.x + p123.x) * 0.5,
        y: (p012.y + p123.y) * 0.5,
    };

    let dx = p3.x - p0.x;
    let dy = p3.y - p0.y;
    let d1 = ((p1.x - p3.x) * dy - (p1.y - p3.y) * dx).abs();
    let d2 = ((p2.x - p3.x) * dy - (p2.y - p3.y) * dx).abs();

    if (d1 + d2) * (d1 + d2) < tess_tol * (dx * dx + dy * dy) {
        push_flattened_point(points, p3, dist_tol);
        return;
    }

    flatten_bezier(points, p0, p01, p012, p0123, level + 1, tess_tol, dist_tol);
    flatten_bezier(points, p0123, p123, p23, p3, level + 1, tess_tol, dist_tol);
}

fn dash_line_segment(path: &mut Path, start: Position, end: Position, cursor: &mut DashCursor<'_>, dist_tol: f32) -> bool {
    let delta = end - start;
    let length = delta.mag2().sqrt();
    if !length.is_finite() {
        return false;
    }
    if length <= dist_tol {
        return true;
    }

    let direction = delta * (1.0 / length);
    let mut travelled = 0.0;
    let mut stalled_at = None;
    while travelled < length {
        if cursor.remaining <= f32::EPSILON {
            cursor.advance();
            continue;
        }

        let step = cursor.remaining.min(length - travelled);
        let next = travelled + step;
        if next <= travelled {
            cursor.remaining -= step;
            if stalled_at == Some(cursor.index) { return false; }
            stalled_at.get_or_insert(cursor.index);
            continue;
        }
        stalled_at = None;
        if cursor.drawing && step > dist_tol {
            let dash_start = start + direction * travelled;
            let dash_end = start + direction * (travelled + step);
            path.move_to(dash_start.x, dash_start.y);
            path.line_to(dash_end.x, dash_end.y);
        }

        travelled = next;
        cursor.remaining -= step;
    }
    true
}

#[derive(Debug)]
pub struct PathIter<'a> {
    verbs: slice::Iter<'a, PackedVerb>,
    coords: &'a [Position],
}

impl Iterator for PathIter<'_> {
    type Item = Verb;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(verb) = self.verbs.next() {
            let verb = Verb::from_packed(verb, self.coords);
            let num_coords = verb.num_coordinates();
            self.coords = &self.coords[num_coords..];
            Some(verb)
        } else {
            None
        }
    }
}
