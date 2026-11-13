use std::{
    hash::{Hash, Hasher},
    ops::{Add, AddAssign, Div, DivAssign, Index, IndexMut, Mul, MulAssign, Neg, Sub, SubAssign},
};

use fnv::FnvHasher;

#[derive(Copy, Clone, Debug, Default)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

impl Add<Vector> for Position {
    type Output = Self;

    #[inline]
    fn add(self, other: Vector) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }
}

impl Sub<Vector> for Position {
    type Output = Self;

    #[inline]
    fn sub(self, other: Vector) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }
}

impl Sub for Position {
    type Output = Vector;

    #[inline]
    fn sub(self, other: Self) -> Vector {
        Vector {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Vector {
    pub x: f32,
    pub y: f32,
}

impl Vector {
    pub fn x(x: f32) -> Self {
        Self { x, y: 0.0 }
    }
    pub fn y(y: f32) -> Self {
        Self { x: 0.0, y }
    }

    pub fn with_basis(self, basis_x: Self, basis_y: Self) -> Self {
        basis_x * self.x + basis_y * self.y
    }

    pub fn cross(self, other: Self) -> f32 {
        self.orthogonal().dot(other)
    }

    #[inline]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
    }

    #[inline]
    pub fn mag2(self) -> f32 {
        self.dot(self)
    }

    #[inline]
    pub fn orthogonal(self) -> Self {
        Self { x: self.y, y: -self.x }
    }

    #[inline]
    pub fn from_angle(angle: f32) -> Self {
        let (y, x) = angle.sin_cos();
        Self { x, y }
    }

    #[inline]
    pub fn angle(&self) -> f32 {
        self.y.atan2(self.x)
    }

    pub fn normalize(&mut self) -> f32 {
        let d = self.x.hypot(self.y);

        if d > 1e-6 {
            let id = 1.0 / d;
            self.x *= id;
            self.y *= id;
        }

        d
    }
}

impl Add for Vector {
    type Output = Self;

    #[inline]
    fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }
}

impl Sub for Vector {
    type Output = Self;

    #[inline]
    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }
}

impl Neg for Vector {
    type Output = Self;

    #[inline]
    fn neg(self) -> Self {
        Self { x: -self.x, y: -self.y }
    }
}

impl Mul<f32> for Vector {
    type Output = Self;

    #[inline]
    fn mul(self, other: f32) -> Self {
        Self {
            x: self.x * other,
            y: self.y * other,
        }
    }
}

impl MulAssign<f32> for Vector {
    #[inline]
    fn mul_assign(&mut self, other: f32) {
        self.x *= other;
        self.y *= other;
    }
}

pub fn quantize(a: f32, d: f32) -> f32 {
    (a / d + 0.5).trunc() * d
}

#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Transform2D(pub [f32; 6]);

impl Transform2D {
    pub fn identity() -> Self {
        Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
    }

    pub fn new(a: f32, b: f32, c: f32, d: f32, x: f32, y: f32) -> Self {
        Self([a, b, c, d, x, y])
    }

    pub fn translation(tx: f32, ty: f32) -> Self {
        Self([1.0, 0.0, 0.0, 1.0, tx, ty])
    }

    pub fn rotation(a: f32) -> Self {
        let (sin, cos) = a.sin_cos();

        Self([cos, sin, -sin, cos, 0.0, 0.0])
    }

    pub fn scaling(sx: f32, sy: f32) -> Self {
        Self([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }

    pub fn translate(&mut self, tx: f32, ty: f32) {
        let Self([.., x, y]) = self;

        *x += tx;
        *y += ty;
    }

    #[inline]
    pub fn premultiply(&mut self, other: &Self) {
        *self = *other * *self;
    }

    pub fn inverse(&self) -> Self {
        let &Self([a, b, c, d, x, y]) = self;
        let [a, b, c, d, x, y] = [a as f64, b as f64, c as f64, d as f64, x as f64, y as f64];

        let det = a * d - c * b;

        if det > -1e-6 && det < 1e-6 {
            return Self::identity();
        }

        let invdet = 1.0 / det;

        Self([
            (d * invdet) as f32,
            (-b * invdet) as f32,
            (-c * invdet) as f32,
            (a * invdet) as f32,
            ((c * y - d * x) * invdet) as f32,
            ((b * x - a * y) * invdet) as f32,
        ])
    }

    pub fn transform_point(&self, sx: f32, sy: f32) -> (f32, f32) {
        let &Self([a, b, c, d, x, y]) = self;

        let dx = sx * a + sy * c + x;
        let dy = sx * b + sy * d + y;
        (dx, dy)
    }

    pub fn average_scale(&self) -> f32 {
        let &Self([a, b, c, d, ..]) = self;

        let sx = a.hypot(c);
        let sy = b.hypot(d);

        (sx + sy) * 0.5
    }

    pub(crate) fn as_uniform_scale_translation(&self, epsilon: f32) -> Option<(f32, f32, f32)> {
        let &Self([a, b, c, d, tx, ty]) = self;
        let is_uniform =
            b.abs() < epsilon && c.abs() < epsilon && (a - d).abs() < epsilon * a.abs().max(1.0) && a > 0.0;
        is_uniform.then_some((a, tx, ty))
    }

    pub fn to_mat3x4(self) -> [f32; 12] {
        let Self([a, b, c, d, x, y]) = self;
        [a, b, 0.0, 0.0, c, d, 0.0, 0.0, x, y, 1.0, 0.0]
    }

    pub fn cache_key(&self) -> u64 {
        let mut hasher = FnvHasher::default();

        for i in 0..6 {
            self.0[i].to_bits().hash(&mut hasher);
        }

        hasher.finish()
    }
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::identity()
    }
}

impl Index<usize> for Transform2D {
    type Output = f32;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<usize> for Transform2D {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl Add for Transform2D {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        let Self([a0, b0, c0, d0, x0, y0]) = self;
        let Self([a1, b1, c1, d1, x1, y1]) = other;

        Self([a0 + a1, b0 + b1, c0 + c1, d0 + d1, x0 + x1, y0 + y1])
    }
}

impl AddAssign for Transform2D {
    #[inline]
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Transform2D {
    type Output = Self;

    fn sub(self, other: Self) -> Self::Output {
        let Self([a0, b0, c0, d0, x0, y0]) = self;
        let Self([a1, b1, c1, d1, x1, y1]) = other;

        Self([a0 - a1, b0 - b1, c0 - c1, d0 - d1, x0 - x1, y0 - y1])
    }
}

impl SubAssign for Transform2D {
    #[inline]
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

impl Mul for Transform2D {
    type Output = Self;

    #[inline]
    fn mul(mut self, other: Self) -> Self::Output {
        self *= other;
        self
    }
}

impl MulAssign for Transform2D {
    fn mul_assign(&mut self, other: Self) {
        let Self([a0, b0, c0, d0, x0, y0]) = self;
        let Self([a1, b1, c1, d1, x1, y1]) = other;

        [*a0, *b0] = [*a0 * a1 + *b0 * c1, *a0 * b1 + *b0 * d1];
        [*c0, *d0] = [*c0 * a1 + *d0 * c1, *c0 * b1 + *d0 * d1];
        [*x0, *y0] = [*x0 * a1 + *y0 * c1 + x1, *x0 * b1 + *y0 * d1 + y1];
    }
}

impl Div for Transform2D {
    type Output = Self;

    #[allow(clippy::suspicious_arithmetic_impl)]
    fn div(self, other: Self) -> Self::Output {
        self * other.inverse()
    }
}

impl DivAssign for Transform2D {
    #[inline]
    fn div_assign(&mut self, other: Self) {
        *self = *self / other;
    }
}

#[derive(Copy, Clone, Default, Debug, PartialEq, PartialOrd)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn intersect(&self, other: Self) -> Self {
        let minx = self.x.max(other.x);
        let miny = self.y.max(other.y);
        let maxx = (self.x + self.w).min(other.x + other.w);
        let maxy = (self.y + self.h).min(other.y + other.h);

        Self::new(minx, miny, 0.0f32.max(maxx - minx), 0.0f32.max(maxy - miny))
    }

    pub fn contains_rect(&self, other: &Self) -> bool {
        other.is_empty()
            || (self.x <= other.x
                && other.x + other.w <= self.x + self.w
                && self.y <= other.y
                && other.y + other.h <= self.y + self.h)
    }

    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let w = (self.x + self.w).min(other.x + other.w) - x;
        let h = (self.y + self.h).min(other.y + other.h) - y;

        let result = Self { x, y, w, h };
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0. || self.h <= 0.
    }
}

#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Bounds {
    pub minx: f32,
    pub miny: f32,
    pub maxx: f32,
    pub maxy: f32,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            minx: 1e6,
            miny: 1e6,
            maxx: -1e6,
            maxy: -1e6,
        }
    }
}
