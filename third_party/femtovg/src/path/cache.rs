use std::{cmp::Ordering, f32::consts::PI, ops::Range};

use bitflags::bitflags;

use crate::{
    geometry::{Bounds, Position, Transform2D, Vector},
    renderer::Vertex,
    FillRule, LineCap, LineJoin, Solidity,
};

use super::Verb;

bitflags! {
    #[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct PointFlags: u8 {
        const CORNER        = 0x01;
        const LEFT          = 0x02;
        const BEVEL         = 0x04;
        const INNERBEVEL    = 0x08;
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Point {
    pos: Position,
    dpos: Vector,
    len: f32,
    dmpos: Vector,
    flags: PointFlags,
}

impl Point {
    pub fn new(x: f32, y: f32, flags: PointFlags) -> Self {
        Self {
            pos: Position { x, y },
            flags,
            ..Default::default()
        }
    }

    pub fn is_left(p0: &Self, p1: &Self, x: f32, y: f32) -> f32 {
        let pos = Position { x, y };
        (p1.pos - p0.pos).dot((pos - p0.pos).orthogonal())
    }

    pub fn approx_eq(&self, other: &Self, tolerance: f32) -> bool {
        (other.pos - self.pos).mag2() < tolerance * tolerance
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
pub enum Convexity {
    Concave,
    Convex,
    #[default]
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Contour {
    point_range: Range<usize>,
    closed: bool,
    bevel: usize,
    solidity: Option<Solidity>,
    pub(crate) fill: Vec<Vertex>,
    pub(crate) stroke: Vec<Vertex>,
    pub(crate) convexity: Convexity,
}

impl Default for Contour {
    fn default() -> Self {
        Self {
            point_range: 0..0,
            closed: Default::default(),
            bevel: Default::default(),
            solidity: None,
            fill: Vec::new(),
            stroke: Vec::new(),
            convexity: Convexity::default(),
        }
    }
}

impl Contour {
    fn point_pairs<'a>(&self, points: &'a [Point]) -> impl Iterator<Item = (&'a Point, &'a Point)> {
        PointPairsIter {
            curr: 0,
            points: &points[self.point_range.clone()],
        }
    }

    fn polygon_area(points: &[Point]) -> f32 {
        let mut area = 0.0;

        for (p0, p1) in (PointPairsIter { curr: 0, points }) {
            area += (p1.pos.x - p0.pos.x) * (p1.pos.y + p0.pos.y);
        }

        area * 0.5
    }

    fn point_count(&self) -> usize {
        self.point_range.end - self.point_range.start
    }
}

struct PointPairsIter<'a> {
    curr: usize,
    points: &'a [Point],
}

impl<'a> Iterator for PointPairsIter<'a> {
    type Item = (&'a Point, &'a Point);

    fn next(&mut self) -> Option<Self::Item> {
        let curr = self.points.get(self.curr);

        let prev = if self.curr == 0 {
            self.points.last()
        } else {
            self.points.get(self.curr - 1)
        };

        self.curr += 1;

        curr.and_then(|some_curr| prev.map(|some_prev| (some_prev, some_curr)))
    }
}

#[derive(Clone, Debug, Default)]
pub struct PathCache {
    pub(crate) contours: Vec<Contour>,
    pub(crate) bounds: Bounds,
    points: Vec<Point>,
}

impl PathCache {
    pub fn new(verbs: impl Iterator<Item = Verb>, transform: &Transform2D, tess_tol: f32, dist_tol: f32) -> Self {
        let mut cache = Self::default();

        for verb in verbs {
            match verb {
                Verb::MoveTo(x, y) => {
                    cache.add_contour();
                    let (x, y) = transform.transform_point(x, y);
                    cache.add_point(x, y, PointFlags::CORNER, dist_tol);
                }
                Verb::LineTo(x, y) => {
                    let (x, y) = transform.transform_point(x, y);
                    cache.add_point(x, y, PointFlags::CORNER, dist_tol);
                }
                Verb::BezierTo(c1x, c1y, c2x, c2y, x, y) => {
                    if let Some(last) = cache.points.last().copied() {
                        let (c1x, c1y) = transform.transform_point(c1x, c1y);
                        let (c2x, c2y) = transform.transform_point(c2x, c2y);
                        let (x, y) = transform.transform_point(x, y);

                        cache.tesselate_bezier(
                            last.pos.x,
                            last.pos.y,
                            c1x,
                            c1y,
                            c2x,
                            c2y,
                            x,
                            y,
                            0,
                            PointFlags::CORNER,
                            tess_tol,
                            dist_tol,
                        );
                    }
                }
                Verb::Close => {
                    if let Some(contour) = cache.contours.last_mut() {
                        contour.closed = true;
                    }
                }
                Verb::Solid => {
                    if let Some(contour) = cache.contours.last_mut() {
                        contour.solidity = Some(Solidity::Solid);
                    }
                }
                Verb::Hole => {
                    if let Some(contour) = cache.contours.last_mut() {
                        contour.solidity = Some(Solidity::Hole);
                    }
                }
            }
        }

        let all_points = &mut cache.points;
        let bounds = &mut cache.bounds;

        cache.contours.retain_mut(|contour| {
            let mut points = &mut all_points[contour.point_range.clone()];

            if let (Some(p0), Some(p1)) = (points.last(), points.first()) {
                if p0.approx_eq(p1, dist_tol) {
                    contour.point_range.end -= 1;
                    contour.closed = true;
                    points = &mut all_points[contour.point_range.clone()];
                }
            }

            if points.len() < 2 {
                return false;
            }

            if let Some(solidity) = contour.solidity {
                let area = Contour::polygon_area(points);

                if solidity == Solidity::Solid && area < 0.0 {
                    points.reverse();
                }

                if solidity == Solidity::Hole && area > 0.0 {
                    points.reverse();
                }
            }

            for i in 0..contour.point_count() {
                let p1 = points.get(i).copied().unwrap();

                let p0 = if i == 0 {
                    points.last_mut().unwrap()
                } else {
                    points.get_mut(i - 1).unwrap()
                };

                p0.dpos = p1.pos - p0.pos;
                p0.len = p0.dpos.normalize();

                bounds.minx = bounds.minx.min(p0.pos.x);
                bounds.miny = bounds.miny.min(p0.pos.y);
                bounds.maxx = bounds.maxx.max(p0.pos.x);
                bounds.maxy = bounds.maxy.max(p0.pos.y);
            }

            true
        });

        cache
    }

    fn add_contour(&mut self) {
        let mut contour = Contour::default();

        contour.point_range.start = self.points.len();
        contour.point_range.end = self.points.len();

        self.contours.push(contour);
    }

    fn add_point(&mut self, x: f32, y: f32, flags: PointFlags, dist_tol: f32) {
        if let Some(contour) = self.contours.last_mut() {
            let new_point = Point::new(x, y, flags);

            if let Some(last_point) = self.points.get_mut(contour.point_range.end) {
                if last_point.approx_eq(&new_point, dist_tol) {
                    last_point.flags |= new_point.flags;
                    return;
                }
            }

            self.points.push(new_point);
            contour.point_range.end += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tesselate_bezier(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x3: f32,
        y3: f32,
        x4: f32,
        y4: f32,
        level: usize,
        flags: PointFlags,
        tess_tol: f32,
        dist_tol: f32,
    ) {
        if level > 10 {
            return;
        }

        let x12 = (x1 + x2) * 0.5;
        let y12 = (y1 + y2) * 0.5;
        let x23 = (x2 + x3) * 0.5;
        let y23 = (y2 + y3) * 0.5;
        let x34 = (x3 + x4) * 0.5;
        let y34 = (y3 + y4) * 0.5;
        let x123 = (x12 + x23) * 0.5;
        let y123 = (y12 + y23) * 0.5;

        let dx = x4 - x1;
        let dy = y4 - y1;
        let d2 = ((x2 - x4) * dy - (y2 - y4) * dx).abs();
        let d3 = ((x3 - x4) * dy - (y3 - y4) * dx).abs();

        if (d2 + d3) * (d2 + d3) < tess_tol * (dx * dx + dy * dy) {
            self.add_point(x4, y4, flags, dist_tol);
            return;
        }

        let x234 = (x23 + x34) * 0.5;
        let y234 = (y23 + y34) * 0.5;
        let x1234 = (x123 + x234) * 0.5;
        let y1234 = (y123 + y234) * 0.5;

        self.tesselate_bezier(
            x1,
            y1,
            x12,
            y12,
            x123,
            y123,
            x1234,
            y1234,
            level + 1,
            PointFlags::empty(),
            tess_tol,
            dist_tol,
        );
        self.tesselate_bezier(
            x1234,
            y1234,
            x234,
            y234,
            x34,
            y34,
            x4,
            y4,
            level + 1,
            flags,
            tess_tol,
            dist_tol,
        );
    }

    pub fn contains_point(&self, x: f32, y: f32, fill_rule: FillRule) -> bool {
        if !self.bounds.contains(x, y) {
            return false;
        }

        if fill_rule == FillRule::EvenOdd {
            for contour in &self.contours {
                let mut crossing = false;

                for (p0, p1) in contour.point_pairs(&self.points) {
                    if (p1.pos.y > y) != (p0.pos.y > y)
                        && (x < (p0.pos.x - p1.pos.x) * (y - p1.pos.y) / (p0.pos.y - p1.pos.y) + p1.pos.x)
                    {
                        crossing = !crossing;
                    }
                }

                if crossing {
                    return true;
                }
            }
        } else {
            for contour in &self.contours {
                let mut winding_number: i32 = 0;

                for (p0, p1) in contour.point_pairs(&self.points) {
                    if p0.pos.y <= y {
                        if p1.pos.y > y && Point::is_left(p0, p1, x, y) > 0.0 {
                            winding_number = winding_number.wrapping_add(1);
                        }
                    } else if p1.pos.y <= y && Point::is_left(p0, p1, x, y) < 0.0 {
                        winding_number = winding_number.wrapping_sub(1);
                    }
                }

                if winding_number != 0 {
                    return true;
                }
            }
        }

        false
    }

    pub(crate) fn expand_fill(&mut self, fringe_width: f32, line_join: LineJoin, miter_limit: f32) {
        let has_fringe = fringe_width > 0.0;

        self.calculate_joins(fringe_width, line_join, miter_limit);

        for contour in &mut self.contours {
            let point_count = contour.point_count();
            let mut vertex_count = point_count + contour.bevel + 1;

            if has_fringe {
                vertex_count += (point_count + contour.bevel * 5 + 1) * 2;
                contour.stroke.reserve(vertex_count);
            }

            contour.fill.reserve(vertex_count);
        }

        let convex = self.contours.len() == 1 && self.contours[0].convexity == Convexity::Convex;

        for contour in &mut self.contours {
            contour.stroke.clear();
            contour.fill.clear();

            let triangle_count = (contour.fill.capacity() - 2) * 3;
            let mut triangle_fan_fill = Vec::with_capacity(triangle_count);

            let woff = 0.5 * fringe_width;
            let foff = woff - 0.05 * fringe_width;

            if has_fringe {
                for (p0, p1) in contour.point_pairs(&self.points) {
                    if p1.flags.contains(PointFlags::BEVEL) {
                        if p1.flags.contains(PointFlags::LEFT) {
                            let lpos = p1.pos + p1.dmpos * foff;
                            triangle_fan_fill.push(Vertex::pos(lpos, 0.5, 1.0));
                        } else {
                            let lpos0 = p1.pos + p0.dpos.orthogonal() * foff;
                            let lpos1 = p1.pos + p1.dpos.orthogonal() * foff;
                            triangle_fan_fill.push(Vertex::pos(lpos0, 0.5, 1.0));
                            triangle_fan_fill.push(Vertex::pos(lpos1, 0.5, 1.0));
                        }
                    } else {
                        triangle_fan_fill.push(Vertex::pos(p1.pos + p1.dmpos * foff, 0.5, 1.0));
                    }
                }
            } else {
                let points = &self.points[contour.point_range.clone()];

                for point in points {
                    triangle_fan_fill.push(Vertex::pos(point.pos, 0.5, 1.0));
                }
            }

            if triangle_fan_fill.len() > 2 {
                let n = triangle_fan_fill.len() as f32;
                let (sx, sy) = triangle_fan_fill.iter().fold((0.0, 0.0), |(x, y), v| (x + v.x, y + v.y));
                let center = Vertex::new(sx / n, sy / n, 0.5, 1.0);
                let count = triangle_fan_fill.len();
                contour.fill = (0..count)
                    .flat_map(|i| IntoIterator::into_iter([center, triangle_fan_fill[i], triangle_fan_fill[(i + 1) % count]]))
                    .collect();
            }

            if has_fringe {
                let rw = fringe_width - woff;
                let ru = 1.0;

                let (lw, lu) = if convex {
                    (woff, 0.5)
                } else {
                    (fringe_width + woff, 0.0)
                };

                for (p0, p1) in contour.point_pairs(&self.points) {
                    if p1.flags.contains(PointFlags::BEVEL | PointFlags::INNERBEVEL) {
                        bevel_join(&mut contour.stroke, p0, p1, lw, rw, lu, ru);
                    } else {
                        contour.stroke.push(Vertex::pos(p1.pos + p1.dmpos * lw, lu, 1.0));
                        contour.stroke.push(Vertex::pos(p1.pos - p1.dmpos * rw, ru, 1.0));
                    }
                }

                let p0 = contour.stroke[0];
                let p1 = contour.stroke[1];
                contour.stroke.push(Vertex::new(p0.x, p0.y, lu, 1.0));
                contour.stroke.push(Vertex::new(p1.x, p1.y, ru, 1.0));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn expand_stroke(
        &mut self,
        stroke_width: f32,
        fringe_width: f32,
        line_cap_start: LineCap,
        line_cap_end: LineCap,
        line_join: LineJoin,
        miter_limit: f32,
        tess_tol: f32,
    ) {
        let ncap = curve_divisions(stroke_width, PI, tess_tol);

        let stroke_width = stroke_width + (fringe_width * 0.5);

        let (u0, u1) = if fringe_width == 0.0 { (0.5, 0.5) } else { (0.0, 1.0) };

        self.calculate_joins(stroke_width, line_join, miter_limit);

        for contour in &mut self.contours {
            contour.stroke.clear();

            for (i, (p0, p1)) in contour.point_pairs(&self.points).enumerate() {
                if !contour.closed && i == 1 {
                    match line_cap_start {
                        LineCap::Butt => butt_cap_start(
                            &mut contour.stroke,
                            p0,
                            p0,
                            stroke_width,
                            -fringe_width * 0.5,
                            fringe_width,
                            u0,
                            u1,
                        ),
                        LineCap::Square => butt_cap_start(
                            &mut contour.stroke,
                            p0,
                            p0,
                            stroke_width,
                            stroke_width - fringe_width,
                            fringe_width,
                            u0,
                            u1,
                        ),
                        LineCap::Round => {
                            round_cap_start(&mut contour.stroke, p0, p0, stroke_width, ncap as usize, u0, u1)
                        }
                    }
                }

                if (i > 0 && i < contour.point_count() - 1) || contour.closed {
                    if p1.flags.contains(PointFlags::BEVEL) || p1.flags.contains(PointFlags::INNERBEVEL) {
                        if line_join == LineJoin::Round {
                            round_join(
                                &mut contour.stroke,
                                p0,
                                p1,
                                stroke_width,
                                stroke_width,
                                u0,
                                u1,
                                ncap as usize,
                            );
                        } else {
                            bevel_join(&mut contour.stroke, p0, p1, stroke_width, stroke_width, u0, u1);
                        }
                    } else {
                        contour
                            .stroke
                            .push(Vertex::pos(p1.pos + p1.dmpos * stroke_width, u0, 1.0));
                        contour
                            .stroke
                            .push(Vertex::pos(p1.pos - p1.dmpos * stroke_width, u1, 1.0));
                    }
                }

                if !contour.closed && i == contour.point_count() - 1 {
                    match line_cap_end {
                        LineCap::Butt => butt_cap_end(
                            &mut contour.stroke,
                            p1,
                            p0,
                            stroke_width,
                            -fringe_width * 0.5,
                            fringe_width,
                            u0,
                            u1,
                        ),
                        LineCap::Square => butt_cap_end(
                            &mut contour.stroke,
                            p1,
                            p0,
                            stroke_width,
                            stroke_width - fringe_width,
                            fringe_width,
                            u0,
                            u1,
                        ),
                        LineCap::Round => {
                            round_cap_end(&mut contour.stroke, p1, p0, stroke_width, ncap as usize, u0, u1)
                        }
                    }
                }
            }

            if contour.closed {
                contour
                    .stroke
                    .push(Vertex::new(contour.stroke[0].x, contour.stroke[0].y, u0, 1.0));
                contour
                    .stroke
                    .push(Vertex::new(contour.stroke[1].x, contour.stroke[1].y, u1, 1.0));
            }
        }
    }

    fn calculate_joins(&mut self, stroke_width: f32, line_join: LineJoin, miter_limit: f32) {
        let inv_stroke_width = if stroke_width > 0.0 { 1.0 / stroke_width } else { 0.0 };

        for contour in &mut self.contours {
            let points = &mut self.points[contour.point_range.clone()];
            let mut nleft = 0;

            contour.bevel = 0;

            let mut x_sign = 0;
            let mut y_sign = 0;
            let mut x_first_sign = 0;
            let mut y_first_sign = 0;
            let mut x_flips = 0;
            let mut y_flips = 0;

            for i in 0..points.len() {
                let p0 = if i == 0 {
                    points.last().copied().unwrap()
                } else {
                    points.get(i - 1).copied().unwrap()
                };

                let p1 = points.get_mut(i).unwrap();

                let dlpos0 = p0.dpos.orthogonal();
                let dlpos1 = p1.dpos.orthogonal();

                p1.dmpos = (dlpos0 + dlpos1) * 0.5;
                let dmr2 = p1.dmpos.mag2();

                if dmr2 > 0.000_001 {
                    let scale = (1.0 / dmr2).min(600.0);

                    p1.dmpos *= scale;
                }

                p1.flags = if p1.flags.contains(PointFlags::CORNER) {
                    PointFlags::CORNER
                } else {
                    PointFlags::empty()
                };

                let cross = p0.dpos.cross(p1.dpos);

                if cross > 0.0 {
                    nleft += 1;
                    p1.flags |= PointFlags::LEFT;
                }

                match p1.dpos.x.partial_cmp(&0.0) {
                    Some(Ordering::Greater) => {
                        match x_sign.cmp(&0) {
                            Ordering::Equal => x_first_sign = 1,
                            Ordering::Less => x_flips += 1,
                            Ordering::Greater => (),
                        }

                        x_sign = 1;
                    }
                    Some(Ordering::Less) => {
                        match x_sign.cmp(&0) {
                            Ordering::Equal => x_first_sign = -1,
                            Ordering::Greater => x_flips += 1,
                            Ordering::Less => (),
                        }

                        x_sign = -1;
                    }
                    _ => (),
                }

                match p1.dpos.y.partial_cmp(&0.0) {
                    Some(Ordering::Greater) => {
                        match y_sign.cmp(&0) {
                            Ordering::Equal => y_first_sign = 1,
                            Ordering::Less => y_flips += 1,
                            Ordering::Greater => (),
                        }

                        y_sign = 1;
                    }
                    Some(Ordering::Less) => {
                        match y_sign.cmp(&0) {
                            Ordering::Equal => y_first_sign = -1,
                            Ordering::Greater => y_flips += 1,
                            Ordering::Less => (),
                        }

                        y_sign = -1;
                    }
                    _ => (),
                }

                let limit = (p0.len.min(p1.len) * inv_stroke_width).max(1.01);

                if (dmr2 * limit * limit) < 1.0 {
                    p1.flags |= PointFlags::INNERBEVEL;
                }

                #[allow(clippy::collapsible_if)]
                if p1.flags.contains(PointFlags::CORNER) {
                    if (dmr2 * miter_limit * miter_limit) < 1.0
                        || line_join == LineJoin::Bevel
                        || line_join == LineJoin::Round
                    {
                        p1.flags |= PointFlags::BEVEL;
                    }
                }

                if p1.flags.contains(PointFlags::BEVEL | PointFlags::INNERBEVEL) {
                    contour.bevel += 1;
                }
            }

            if x_sign != 0 && x_first_sign != 0 && x_sign != x_first_sign {
                x_flips += 1;
            }

            if y_sign != 0 && y_first_sign != 0 && y_sign != y_first_sign {
                y_flips += 1;
            }

            let convex = x_flips == 2 && y_flips == 2;

            contour.convexity = if nleft == points.len() && convex {
                Convexity::Convex
            } else {
                Convexity::Concave
            };
        }
    }

    pub(crate) fn path_fill_is_rect(&self) -> Option<crate::Rect> {
        if self.contours.len() != 1 {
            return None;
        }

        let points = &self.points[self.contours[0].point_range.clone()];
        if points.len() != 4 || points.iter().any(|p| !p.pos.x.is_finite() || !p.pos.y.is_finite()) {
            return None;
        }
        let [p0, p1, p2, p3] = [points[0].pos, points[1].pos, points[2].pos, points[3].pos];
        let width = (p2.x - p0.x).abs();
        let height = (p2.y - p0.y).abs();
        if p0.x != p2.x
            && p0.y != p2.y
            && width.is_finite()
            && height.is_finite()
            && ((p0.x == p1.x && p1.y == p2.y && p2.x == p3.x && p3.y == p0.y)
                || (p0.y == p1.y && p1.x == p2.x && p2.y == p3.y && p3.x == p0.x))
        {
            Some(crate::Rect::new(p0.x.min(p2.x), p0.y.min(p2.y), width, height))
        } else {
            None
        }
    }
}

fn curve_divisions(radius: f32, arc: f32, tol: f32) -> u32 {
    let da = (radius / (radius + tol)).acos() * 2.0;

    ((arc / da).ceil() as u32).clamp(2, 1024)
}

#[allow(clippy::too_many_arguments)]
fn butt_cap_start(verts: &mut Vec<Vertex>, p0: &Point, p1: &Point, w: f32, d: f32, aa: f32, u0: f32, u1: f32) {
    let ppos = p0.pos - p1.dpos * d;
    let dlpos = p1.dpos.orthogonal();

    verts.push(Vertex::pos(ppos + dlpos * w - p1.dpos * aa, u0, 0.0));
    verts.push(Vertex::pos(ppos - dlpos * w - p1.dpos * aa, u1, 0.0));
    verts.push(Vertex::pos(ppos + dlpos * w, u0, 1.0));
    verts.push(Vertex::pos(ppos - dlpos * w, u1, 1.0));
}

#[allow(clippy::too_many_arguments)]
fn butt_cap_end(verts: &mut Vec<Vertex>, p0: &Point, p1: &Point, w: f32, d: f32, aa: f32, u0: f32, u1: f32) {
    let ppos = p0.pos + p1.dpos * d;
    let dlpos = p1.dpos.orthogonal();

    verts.push(Vertex::pos(ppos + dlpos * w, u0, 1.0));
    verts.push(Vertex::pos(ppos - dlpos * w, u1, 1.0));
    verts.push(Vertex::pos(ppos + dlpos * w + p1.dpos * aa, u0, 0.0));
    verts.push(Vertex::pos(ppos - dlpos * w + p1.dpos * aa, u1, 0.0));
}

fn round_cap_start(verts: &mut Vec<Vertex>, p0: &Point, p1: &Point, w: f32, ncap: usize, u0: f32, u1: f32) {
    let ppos = p0.pos;
    let dlpos = p1.dpos.orthogonal();

    for i in 0..ncap {
        let a = i as f32 / (ncap as f32 - 1.0) * PI;
        let offset = Vector::from_angle(a).with_basis(-dlpos, -p1.dpos) * w;

        verts.push(Vertex::pos(ppos + offset, u0, 1.0));
        verts.push(Vertex::pos(ppos, 0.5, 1.0));
    }

    verts.push(Vertex::pos(ppos + dlpos * w, u0, 1.0));
    verts.push(Vertex::pos(ppos - dlpos * w, u1, 1.0));
}

fn round_cap_end(verts: &mut Vec<Vertex>, p0: &Point, p1: &Point, w: f32, ncap: usize, u0: f32, u1: f32) {
    let ppos = p0.pos;
    let dlpos = p1.dpos.orthogonal();

    verts.push(Vertex::pos(ppos + dlpos * w, u0, 1.0));
    verts.push(Vertex::pos(ppos - dlpos * w, u1, 1.0));

    for i in 0..ncap {
        let a = i as f32 / (ncap as f32 - 1.0) * PI;
        let offset = Vector::from_angle(a).with_basis(-dlpos, p1.dpos) * w;

        verts.push(Vertex::pos(ppos, 0.5, 1.0));
        verts.push(Vertex::pos(ppos + offset, u0, 1.0));
    }
}

fn choose_bevel(bevel: bool, p0: &Point, p1: &Point, w: f32) -> (Position, Position) {
    if bevel {
        (p1.pos + p0.dpos.orthogonal() * w, p1.pos + p1.dpos.orthogonal() * w)
    } else {
        let pos = p1.pos + p1.dmpos * w;
        (pos, pos)
    }
}

#[allow(clippy::too_many_arguments)]
fn round_join(verts: &mut Vec<Vertex>, p0: &Point, p1: &Point, lw: f32, rw: f32, lu: f32, ru: f32, ncap: usize) {
    let dlpos0 = p0.dpos.orthogonal();
    let dlpos1 = p1.dpos.orthogonal();

    let a0;
    let mut a1;

    if p1.flags.contains(PointFlags::LEFT) {
        let (lpos0, lpos1) = choose_bevel(p1.flags.contains(PointFlags::INNERBEVEL), p0, p1, lw);
        a0 = (-dlpos0).angle();
        a1 = (-dlpos1).angle();

        if a1 > a0 {
            a1 -= PI * 2.0;
        }

        verts.push(Vertex::pos(lpos0, lu, 1.0));
        verts.push(Vertex::pos(p1.pos - dlpos0 * rw, ru, 1.0));

        let n = ((((a0 - a1) / PI) * ncap as f32).ceil() as usize).max(2).min(ncap);

        for i in 0..n {
            let u = i as f32 / (n - 1) as f32;
            let a = a0 + u * (a1 - a0);
            let rpos = p1.pos + Vector::from_angle(a) * rw;

            verts.push(Vertex::pos(p1.pos, 0.5, 1.0));
            verts.push(Vertex::pos(rpos, ru, 1.0));
        }

        verts.push(Vertex::pos(lpos1, lu, 1.0));
        verts.push(Vertex::pos(p1.pos - dlpos1 * rw, ru, 1.0));
    } else {
        let (rpos0, rpos1) = choose_bevel(p1.flags.contains(PointFlags::INNERBEVEL), p0, p1, -rw);
        a0 = dlpos0.angle();
        a1 = dlpos1.angle();

        if a1 < a0 {
            a1 += PI * 2.0;
        }

        verts.push(Vertex::pos(p1.pos + dlpos0 * rw, lu, 1.0));
        verts.push(Vertex::pos(rpos0, ru, 1.0));

        let n = ((((a1 - a0) / PI) * ncap as f32).ceil() as usize).max(2).min(ncap);

        for i in 0..n {
            let u = i as f32 / (n - 1) as f32;
            let a = a0 + u * (a1 - a0);
            let lpos = p1.pos + Vector::from_angle(a) * lw;

            verts.push(Vertex::pos(lpos, lu, 1.0));
            verts.push(Vertex::pos(p1.pos, 0.5, 1.0));
        }

        verts.push(Vertex::pos(p1.pos + dlpos1 * rw, lu, 1.0));
        verts.push(Vertex::pos(rpos1, ru, 1.0));
    }
}

#[allow(clippy::branches_sharing_code)]
fn bevel_join(verts: &mut Vec<Vertex>, p0: &Point, p1: &Point, lw: f32, rw: f32, lu: f32, ru: f32) {
    let dlpos0 = p0.dpos.orthogonal();
    let dlpos1 = p1.dpos.orthogonal();

    if p1.flags.contains(PointFlags::LEFT) {
        let (lpos0, lpos1) = choose_bevel(p1.flags.contains(PointFlags::INNERBEVEL), p0, p1, lw);

        verts.push(Vertex::pos(lpos0, lu, 1.0));
        verts.push(Vertex::pos(p1.pos - dlpos0 * rw, ru, 1.0));

        if p1.flags.contains(PointFlags::BEVEL) {
            verts.push(Vertex::pos(lpos0, lu, 1.0));
            verts.push(Vertex::pos(p1.pos - dlpos0 * rw, ru, 1.0));

            verts.push(Vertex::pos(lpos1, lu, 1.0));
            verts.push(Vertex::pos(p1.pos - dlpos1 * rw, ru, 1.0));
        } else {
            let rpos0 = p1.pos - p1.dmpos * rw;

            verts.push(Vertex::pos(p1.pos, 0.5, 1.0));
            verts.push(Vertex::pos(p1.pos - dlpos0 * rw, ru, 1.0));

            verts.push(Vertex::pos(rpos0, ru, 1.0));
            verts.push(Vertex::pos(rpos0, ru, 1.0));

            verts.push(Vertex::pos(p1.pos, 0.5, 1.0));
            verts.push(Vertex::pos(p1.pos - dlpos1 * rw, ru, 1.0));
        }

        verts.push(Vertex::pos(lpos1, lu, 1.0));
        verts.push(Vertex::pos(p1.pos - dlpos1 * rw, ru, 1.0));
    } else {
        let (rpos0, rpos1) = choose_bevel(p1.flags.contains(PointFlags::INNERBEVEL), p0, p1, -rw);

        verts.push(Vertex::pos(p1.pos + dlpos0 * lw, lu, 1.0));
        verts.push(Vertex::pos(rpos0, ru, 1.0));

        if p1.flags.contains(PointFlags::BEVEL) {
            verts.push(Vertex::pos(p1.pos + dlpos0 * lw, lu, 1.0));
            verts.push(Vertex::pos(rpos0, ru, 1.0));

            verts.push(Vertex::pos(p1.pos + dlpos1 * lw, lu, 1.0));
            verts.push(Vertex::pos(rpos1, ru, 1.0));
        } else {
            let lpos0 = p1.pos + p1.dmpos * lw;

            verts.push(Vertex::pos(p1.pos + dlpos0 * lw, lu, 1.0));
            verts.push(Vertex::pos(p1.pos, 0.5, 1.0));

            verts.push(Vertex::pos(lpos0, lu, 1.0));
            verts.push(Vertex::pos(lpos0, lu, 1.0));

            verts.push(Vertex::pos(p1.pos + dlpos1 * lw, lu, 1.0));
            verts.push(Vertex::pos(p1.pos, 0.5, 1.0));
        }

        verts.push(Vertex::pos(p1.pos + dlpos1 * lw, lu, 1.0));
        verts.push(Vertex::pos(rpos1, ru, 1.0));
    }
}
