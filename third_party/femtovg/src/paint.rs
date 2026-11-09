
use std::rc::Rc;

use crate::{geometry::Position, Color, FillRule, ImageId, LineCap, LineJoin};

#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct GradientStop(pub f32, pub Color);

impl Eq for GradientStop {}
impl Ord for GradientStop {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if (other.0, other.1) < (self.0, self.1) {
            std::cmp::Ordering::Less
        } else if (self.0, self.1) < (other.0, other.1) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }
}

impl PartialOrd for GradientStop {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MultiStopGradient {
    shared_stops: Rc<[GradientStop]>,
    tint: f32,
}

impl MultiStopGradient {
    pub(crate) fn get(&self, index: usize) -> GradientStop {
        let mut stop = self
            .shared_stops
            .get(index)
            .copied()
            .unwrap_or_else(|| GradientStop(2.0, Color::black()));

        stop.1.a *= self.tint;
        stop
    }

    pub(crate) fn pairs(&self) -> impl Iterator<Item = [GradientStop; 2]> + '_ {
        self.shared_stops.as_ref().windows(2).map(move |pair| {
            let mut stops = [pair[0], pair[1]];
            stops[0].1.a *= self.tint;
            stops[1].1.a *= self.tint;
            stops
        })
    }
}

impl Eq for MultiStopGradient {}

impl PartialOrd for MultiStopGradient {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MultiStopGradient {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if (&other.shared_stops, other.tint) < (&self.shared_stops, self.tint) {
            std::cmp::Ordering::Less
        } else if (&self.shared_stops, self.tint) < (&other.shared_stops, other.tint) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum GradientColors {
    TwoStop { start_color: Color, end_color: Color },
    MultiStop { stops: MultiStopGradient },
}
impl GradientColors {
    fn mul_alpha(&mut self, a: f32) {
        match self {
            Self::TwoStop { start_color, end_color } => {
                start_color.a *= a;
                end_color.a *= a;
            }
            Self::MultiStop { stops, .. } => {
                stops.tint *= a;
            }
        }
    }
    fn from_stops<Stops>(stops: Stops) -> Self
    where
        Stops: IntoIterator<Item = (f32, Color)>,
    {
        let mut stops = stops.into_iter();
        let Some(first_stop) = stops.next() else {
            return Self::TwoStop {
                start_color: Color::black(),
                end_color: Color::black(),
            };
        };
        let Some(second_stop) = stops.next() else {
            return Self::TwoStop {
                start_color: first_stop.1,
                end_color: first_stop.1,
            };
        };

        let maybe_third_stop = stops.next();

        if maybe_third_stop.is_none() && first_stop.0 <= 0.0 && second_stop.0 >= 1.0 {
            return Self::TwoStop {
                start_color: first_stop.1,
                end_color: second_stop.1,
            };
        }

        let out_stops = [first_stop, second_stop]
            .into_iter()
            .chain(maybe_third_stop)
            .chain(stops)
            .map(|(stop, color)| GradientStop(stop, color))
            .collect();
        Self::MultiStop {
            stops: MultiStopGradient {
                shared_stops: out_stops,
                tint: 1.0,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub enum PaintFlavor {
    Color(Color),
    Image {
        id: ImageId,
        center: Position,
        width: f32,
        height: f32,
        angle: f32,
        tint: Color,
    },
    LinearGradient {
        start: Position,
        end: Position,
        colors: GradientColors,
    },
    RadialGradient {
        center: Position,
        in_radius: (f32, f32),
        out_radius: (f32, f32),
        colors: GradientColors,
    },
    ConicGradient {
        center: Position,
        start_angle: f32,
        colors: GradientColors,
    },
}

impl PaintFlavor {
    pub(crate) fn mul_alpha(&mut self, a: f32) {
        match self {
            Self::Color(color) => {
                color.a *= a;
            }
            Self::Image { tint, .. } => {
                tint.a *= a;
            }
            Self::LinearGradient { colors, .. } => {
                colors.mul_alpha(a);
            }
            Self::RadialGradient { colors, .. } => {
                colors.mul_alpha(a);
            }
            Self::ConicGradient { colors, .. } => {
                colors.mul_alpha(a);
            }
        }
    }

    pub(crate) fn gradient_colors(&self) -> Option<&GradientColors> {
        match self {
            Self::LinearGradient { colors, .. } => Some(colors),
            Self::RadialGradient { colors, .. } => Some(colors),
            Self::ConicGradient { colors, .. } => Some(colors),
            _ => None,
        }
    }

    pub(crate) fn is_straight_tinted_image(&self, shape_anti_alias: bool) -> bool {
        matches!(self, &Self::Image { angle, .. } if angle == 0.0 && !shape_anti_alias)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum GlyphTexture {
    #[default]
    None,
    AlphaMask(ImageId),
    ColorTexture(ImageId),
}

impl GlyphTexture {
    pub(crate) fn image_id(&self) -> Option<ImageId> {
        match self {
            Self::None => None,
            Self::AlphaMask(image_id) | Self::ColorTexture(image_id) => Some(*image_id),
        }
    }
}

#[derive(Clone, Debug)]
pub struct StrokeSettings {
    pub(crate) stencil_strokes: bool,
    pub(crate) miter_limit: f32,
    pub(crate) line_width: f32,
    pub(crate) line_cap_start: LineCap,
    pub(crate) line_cap_end: LineCap,
    pub(crate) line_join: LineJoin,
    pub(crate) line_dash: Vec<f32>,
    pub(crate) line_dash_offset: f32,
}

impl Default for StrokeSettings {
    fn default() -> Self {
        Self {
            stencil_strokes: true,
            miter_limit: 10.0,
            line_width: 1.0,
            line_cap_start: LineCap::default(),
            line_cap_end: LineCap::default(),
            line_join: LineJoin::default(),
            line_dash: Vec::new(),
            line_dash_offset: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextSettings {
    pub(crate) font_size: f32,
}

impl Default for TextSettings {
    fn default() -> Self {
        Self { font_size: 16.0 }
    }
}

#[derive(Clone, Debug)]
pub struct Paint {
    pub(crate) flavor: PaintFlavor,
    pub(crate) shape_anti_alias: bool,
    pub(crate) stroke: StrokeSettings,
    pub(crate) text: TextSettings,
    pub(crate) fill_rule: FillRule,
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            flavor: PaintFlavor::Color(Color::white()),
            shape_anti_alias: true,
            stroke: StrokeSettings::default(),
            text: TextSettings::default(),
            fill_rule: FillRule::default(),
        }
    }
}

impl Paint {
    pub fn color(color: Color) -> Self {
        Self::with_flavor(PaintFlavor::Color(color))
    }

    fn with_flavor(flavor: PaintFlavor) -> Self {
        Self {
            flavor,
            ..Default::default()
        }
    }

    pub fn image(id: ImageId, cx: f32, cy: f32, width: f32, height: f32, angle: f32, alpha: f32) -> Self {
        Self::with_flavor(PaintFlavor::Image {
            id,
            center: Position { x: cx, y: cy },
            width,
            height,
            angle,
            tint: Color::rgbaf(1.0, 1.0, 1.0, alpha),
        })
    }

    pub fn image_tint(id: ImageId, cx: f32, cy: f32, width: f32, height: f32, angle: f32, tint: Color) -> Self {
        Self::with_flavor(PaintFlavor::Image {
            id,
            center: Position { x: cx, y: cy },
            width,
            height,
            angle,
            tint,
        })
    }

    pub fn linear_gradient_stops(
        start_x: f32,
        start_y: f32,
        end_x: f32,
        end_y: f32,
        stops: impl IntoIterator<Item = (f32, Color)>,
    ) -> Self {
        Self::with_flavor(PaintFlavor::LinearGradient {
            start: Position { x: start_x, y: start_y },
            end: Position { x: end_x, y: end_y },
            colors: GradientColors::from_stops(stops),
        })
    }

    pub fn radial_gradient_stops(
        cx: f32,
        cy: f32,
        in_radius: f32,
        out_radius: f32,
        stops: impl IntoIterator<Item = (f32, Color)>,
    ) -> Self {
        Self::with_flavor(PaintFlavor::RadialGradient {
            center: Position { x: cx, y: cy },
            in_radius: (in_radius, in_radius),
            out_radius: (out_radius, out_radius),
            colors: GradientColors::from_stops(stops),
        })
    }

    pub fn conic_gradient_stops(cx: f32, cy: f32, stops: impl IntoIterator<Item = (f32, Color)>) -> Self {
        Self::conic_gradient_stops_with_angle(cx, cy, 0.0, stops)
    }

    pub fn conic_gradient_stops_with_angle(
        cx: f32,
        cy: f32,
        start_angle: f32,
        stops: impl IntoIterator<Item = (f32, Color)>,
    ) -> Self {
        Self::with_flavor(PaintFlavor::ConicGradient {
            center: Position { x: cx, y: cy },
            start_angle: Self::finite_start_angle_or_zero(start_angle),
            colors: GradientColors::from_stops(stops),
        })
    }

    fn finite_start_angle_or_zero(start_angle: f32) -> f32 {
        if start_angle.is_finite() {
            start_angle
        } else {
            0.0
        }
    }

    #[inline]
    pub fn anti_alias(&self) -> bool {
        self.shape_anti_alias
    }

    #[inline]
    pub fn set_anti_alias(&mut self, value: bool) {
        self.shape_anti_alias = value;
    }

    #[inline]
    pub fn with_anti_alias(mut self, value: bool) -> Self {
        self.set_anti_alias(value);
        self
    }

    #[inline]
    pub fn stencil_strokes(&self) -> bool {
        self.stroke.stencil_strokes
    }

    #[inline]
    pub fn line_width(&self) -> f32 {
        self.stroke.line_width
    }

    #[inline]
    pub fn set_line_width(&mut self, width: f32) {
        self.stroke.line_width = width;
    }

    #[inline]
    pub fn miter_limit(&self) -> f32 {
        self.stroke.miter_limit
    }

    #[inline]
    pub fn set_miter_limit(&mut self, limit: f32) {
        self.stroke.miter_limit = limit;
    }

    #[inline]
    pub fn line_cap_start(&self) -> LineCap {
        self.stroke.line_cap_start
    }

    #[inline]
    pub fn line_cap_end(&self) -> LineCap {
        self.stroke.line_cap_end
    }

    pub fn set_line_cap(&mut self, cap: LineCap) {
        self.stroke.line_cap_start = cap;
        self.stroke.line_cap_end = cap;
    }

    #[inline]
    pub fn line_join(&self) -> LineJoin {
        self.stroke.line_join
    }

    #[inline]
    pub fn set_line_join(&mut self, join: LineJoin) {
        self.stroke.line_join = join;
    }

    #[inline]
    pub fn line_dash(&self) -> &[f32] {
        &self.stroke.line_dash
    }

    #[inline]
    pub fn line_dash_offset(&self) -> f32 {
        self.stroke.line_dash_offset
    }

    #[inline]
    pub fn font_size(&self) -> f32 {
        self.text.font_size
    }

    #[inline]
    pub fn set_font_size(&mut self, size: f32) {
        self.text.font_size = size;
    }

    #[inline]
    pub fn fill_rule(&self) -> FillRule {
        self.fill_rule
    }

    #[inline]
    pub fn set_fill_rule(&mut self, rule: FillRule) {
        self.fill_rule = rule;
    }
}
