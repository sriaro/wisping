
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::{geometry::Position, Align, Baseline, Color, FillRule, FontId, ImageId, LineCap, LineJoin};

const MAX_FONT_VARIATIONS: usize = 4;

#[derive(Clone, Debug)]
pub(crate) struct FontVariations {
    entries: [(u32, f32); MAX_FONT_VARIATIONS],
    len: u8,
}

impl Default for FontVariations {
    fn default() -> Self {
        Self {
            entries: [(0, 0.0); MAX_FONT_VARIATIONS],
            len: 0,
        }
    }
}

impl Hash for FontVariations {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let len = self.len as usize;
        let mut sorted = self.entries;
        sorted[..len].sort_unstable_by_key(|&(tag, _)| tag);
        for &(tag, val) in &sorted[..len] {
            tag.hash(state);
            val.to_bits().hash(state);
        }
    }
}

impl FontVariations {
    pub(crate) fn tag_to_u32(tag: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*tag)
    }

    pub(crate) fn set(&mut self, tag: u32, value: f32) {
        for i in 0..self.len as usize {
            if self.entries[i].0 == tag {
                self.entries[i].1 = value;
                return;
            }
        }
        assert!(
            (self.len as usize) < MAX_FONT_VARIATIONS,
            "FontVariations: too many axes (max {})",
            MAX_FONT_VARIATIONS
        );
        self.entries[self.len as usize] = (tag, value);
        self.len += 1;
    }

    pub(crate) fn get(&self, tag: u32) -> Option<f32> {
        for i in 0..self.len as usize {
            if self.entries[i].0 == tag {
                return Some(self.entries[i].1);
            }
        }
        None
    }

    pub(crate) fn remove(&mut self, tag: u32) -> bool {
        for i in 0..self.len as usize {
            if self.entries[i].0 == tag {
                let last = self.len as usize - 1;
                self.entries[i] = self.entries[last];
                self.entries[last] = (0, 0.0);
                self.len -= 1;
                return true;
            }
        }
        false
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, f32)> + '_ {
        self.entries[..self.len as usize].iter().copied()
    }

    pub(crate) fn clear(&mut self) {
        self.len = 0;
    }
}

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
    BoxGradient {
        pos: Position,
        width: f32,
        height: f32,
        radius: f32,
        feather: f32,
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
            Self::BoxGradient { colors, .. } => {
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
            Self::BoxGradient { colors, .. } => Some(colors),
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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TextDecoration {
    pub underline: bool,
    pub strikethrough: bool,
    pub overline: bool,
}

impl TextDecoration {
    #[inline]
    pub fn is_none(&self) -> bool {
        !self.underline && !self.strikethrough && !self.overline
    }
}

#[derive(Clone, Debug)]
pub struct TextSettings {
    pub(crate) font_ids: [Option<FontId>; 8],
    pub(crate) font_size: f32,
    pub(crate) letter_spacing: f32,
    pub(crate) text_baseline: Baseline,
    pub(crate) text_align: Align,
    pub(crate) font_variations: FontVariations,
    pub(crate) text_decoration: TextDecoration,
}

impl Default for TextSettings {
    fn default() -> Self {
        Self {
            font_ids: Default::default(),
            font_size: 16.0,
            letter_spacing: 0.0,
            text_baseline: Baseline::default(),
            text_align: Align::default(),
            font_variations: FontVariations::default(),
            text_decoration: TextDecoration::default(),
        }
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
    pub const FONT_WEIGHT_THIN: f32 = 100.0;
    pub const FONT_WEIGHT_EXTRA_LIGHT: f32 = 200.0;
    pub const FONT_WEIGHT_LIGHT: f32 = 300.0;
    pub const FONT_WEIGHT_REGULAR: f32 = 400.0;
    pub const FONT_WEIGHT_MEDIUM: f32 = 500.0;
    pub const FONT_WEIGHT_SEMI_BOLD: f32 = 600.0;
    pub const FONT_WEIGHT_BOLD: f32 = 700.0;
    pub const FONT_WEIGHT_EXTRA_BOLD: f32 = 800.0;
    pub const FONT_WEIGHT_BLACK: f32 = 900.0;

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

    pub fn linear_gradient(
        start_x: f32,
        start_y: f32,
        end_x: f32,
        end_y: f32,
        start_color: Color,
        end_color: Color,
    ) -> Self {
        Self::with_flavor(PaintFlavor::LinearGradient {
            start: Position { x: start_x, y: start_y },
            end: Position { x: end_x, y: end_y },
            colors: GradientColors::TwoStop { start_color, end_color },
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

    #[allow(clippy::too_many_arguments)]
    pub fn box_gradient(
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius: f32,
        feather: f32,
        inner_color: Color,
        outer_color: Color,
    ) -> Self {
        Self::with_flavor(PaintFlavor::BoxGradient {
            pos: Position { x, y },
            width,
            height,
            radius,
            feather,
            colors: GradientColors::TwoStop {
                start_color: inner_color,
                end_color: outer_color,
            },
        })
    }

    pub fn radial_gradient(
        cx: f32,
        cy: f32,
        in_radius: f32,
        out_radius: f32,
        inner_color: Color,
        outer_color: Color,
    ) -> Self {
        Self::with_flavor(PaintFlavor::RadialGradient {
            center: Position { x: cx, y: cy },
            in_radius: (in_radius, in_radius),
            out_radius: (out_radius, out_radius),
            colors: GradientColors::TwoStop {
                start_color: inner_color,
                end_color: outer_color,
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn elliptical_gradient(
        cx: f32,
        cy: f32,
        in_rx: f32,
        in_ry: f32,
        out_rx: f32,
        out_ry: f32,
        inner_color: Color,
        outer_color: Color,
    ) -> Self {
        Self::with_flavor(PaintFlavor::RadialGradient {
            center: Position { x: cx, y: cy },
            in_radius: (in_rx, in_ry),
            out_radius: (out_rx, out_ry),
            colors: GradientColors::TwoStop {
                start_color: inner_color,
                end_color: outer_color,
            },
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

    pub fn elliptical_gradient_stops(
        cx: f32,
        cy: f32,
        in_rx: f32,
        in_ry: f32,
        out_rx: f32,
        out_ry: f32,
        stops: impl IntoIterator<Item = (f32, Color)>,
    ) -> Self {
        Self::with_flavor(PaintFlavor::RadialGradient {
            center: Position { x: cx, y: cy },
            in_radius: (in_rx, in_ry),
            out_radius: (out_rx, out_ry),
            colors: GradientColors::from_stops(stops),
        })
    }

    pub fn conic_gradient(cx: f32, cy: f32, start_color: Color, end_color: Color) -> Self {
        Self::conic_gradient_with_angle(cx, cy, 0.0, start_color, end_color)
    }

    pub fn conic_gradient_with_angle(cx: f32, cy: f32, start_angle: f32, start_color: Color, end_color: Color) -> Self {
        Self::with_flavor(PaintFlavor::ConicGradient {
            center: Position { x: cx, y: cy },
            start_angle: Self::finite_start_angle_or_zero(start_angle),
            colors: GradientColors::TwoStop { start_color, end_color },
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

    pub fn set_color(&mut self, color: Color) {
        self.flavor = PaintFlavor::Color(color);
    }

    #[inline]
    pub fn with_color(mut self, color: Color) -> Self {
        self.set_color(color);
        self
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
    pub fn set_stencil_strokes(&mut self, value: bool) {
        self.stroke.stencil_strokes = value;
    }

    #[inline]
    pub fn with_stencil_strokes(mut self, value: bool) -> Self {
        self.set_stencil_strokes(value);
        self
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
    pub fn with_line_width(mut self, width: f32) -> Self {
        self.set_line_width(width);
        self
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
    pub fn with_miter_limit(mut self, limit: f32) -> Self {
        self.set_miter_limit(limit);
        self
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
    pub fn with_line_cap(mut self, cap: LineCap) -> Self {
        self.set_line_cap(cap);
        self
    }

    #[inline]
    pub fn set_line_cap_start(&mut self, cap: LineCap) {
        self.stroke.line_cap_start = cap;
    }

    #[inline]
    pub fn with_line_cap_start(mut self, cap: LineCap) -> Self {
        self.set_line_cap_start(cap);
        self
    }

    #[inline]
    pub fn set_line_cap_end(&mut self, cap: LineCap) {
        self.stroke.line_cap_end = cap;
    }

    #[inline]
    pub fn with_line_cap_end(mut self, cap: LineCap) -> Self {
        self.set_line_cap_end(cap);
        self
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
    pub fn with_line_join(mut self, join: LineJoin) -> Self {
        self.set_line_join(join);
        self
    }

    #[inline]
    pub fn line_dash(&self) -> &[f32] {
        &self.stroke.line_dash
    }

    pub fn set_line_dash(&mut self, dash: &[f32]) {
        if dash.iter().any(|value| !value.is_finite() || *value < 0.0) {
            return;
        }

        self.stroke.line_dash.clear();
        if dash.is_empty() {
            return;
        }

        self.stroke.line_dash.extend_from_slice(dash);
        if dash.len() % 2 == 1 {
            self.stroke.line_dash.extend_from_slice(dash);
        }
    }

    #[inline]
    pub fn with_line_dash(mut self, dash: &[f32]) -> Self {
        self.set_line_dash(dash);
        self
    }

    #[inline]
    pub fn line_dash_offset(&self) -> f32 {
        self.stroke.line_dash_offset
    }

    #[inline]
    pub fn set_line_dash_offset(&mut self, offset: f32) {
        if offset.is_finite() {
            self.stroke.line_dash_offset = offset;
        }
    }

    #[inline]
    pub fn with_line_dash_offset(mut self, offset: f32) -> Self {
        self.set_line_dash_offset(offset);
        self
    }

    pub fn set_font(&mut self, font_ids: &[FontId]) {
        self.text.font_ids = Default::default();

        for (i, id) in font_ids.iter().take(8).enumerate() {
            self.text.font_ids[i] = Some(*id);
        }
    }

    #[inline]
    pub fn with_font(mut self, font_ids: &[FontId]) -> Self {
        self.set_font(font_ids);
        self
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
    pub fn with_font_size(mut self, size: f32) -> Self {
        self.set_font_size(size);
        self
    }

    #[inline]
    pub fn letter_spacing(&self) -> f32 {
        self.text.letter_spacing
    }

    #[inline]
    pub fn set_letter_spacing(&mut self, spacing: f32) {
        self.text.letter_spacing = spacing;
    }

    #[inline]
    pub fn with_letter_spacing(mut self, spacing: f32) -> Self {
        self.set_letter_spacing(spacing);
        self
    }

    #[inline]
    pub fn text_baseline(&self) -> Baseline {
        self.text.text_baseline
    }

    #[inline]
    pub fn set_text_baseline(&mut self, align: Baseline) {
        self.text.text_baseline = align;
    }

    #[inline]
    pub fn with_text_baseline(mut self, align: Baseline) -> Self {
        self.set_text_baseline(align);
        self
    }

    #[inline]
    pub fn text_align(&self) -> Align {
        self.text.text_align
    }

    #[inline]
    pub fn set_text_align(&mut self, align: Align) {
        self.text.text_align = align;
    }

    #[inline]
    pub fn with_text_align(mut self, align: Align) -> Self {
        self.set_text_align(align);
        self
    }

    #[inline]
    pub fn text_decoration(&self) -> TextDecoration {
        self.text.text_decoration
    }

    #[inline]
    pub fn set_text_decoration(&mut self, decoration: TextDecoration) {
        self.text.text_decoration = decoration;
    }

    #[inline]
    pub fn with_text_decoration(mut self, decoration: TextDecoration) -> Self {
        self.set_text_decoration(decoration);
        self
    }

    #[inline]
    pub fn font_weight(&self) -> Option<f32> {
        self.text.font_variations.get(FontVariations::tag_to_u32(b"wght"))
    }

    #[inline]
    pub fn set_font_weight(&mut self, weight: f32) {
        self.text
            .font_variations
            .set(FontVariations::tag_to_u32(b"wght"), weight);
    }

    #[inline]
    pub fn with_font_weight(mut self, weight: f32) -> Self {
        self.set_font_weight(weight);
        self
    }

    #[inline]
    pub fn clear_font_weight(&mut self) {
        self.text.font_variations.remove(FontVariations::tag_to_u32(b"wght"));
    }

    #[inline]
    pub fn font_italic(&self) -> Option<bool> {
        self.text
            .font_variations
            .get(FontVariations::tag_to_u32(b"ital"))
            .map(|v| v >= 0.5)
    }

    #[inline]
    pub fn set_font_italic(&mut self, italic: bool) {
        self.text
            .font_variations
            .set(FontVariations::tag_to_u32(b"ital"), if italic { 1.0 } else { 0.0 });
        self.text
            .font_variations
            .set(FontVariations::tag_to_u32(b"slnt"), if italic { -12.0 } else { 0.0 });
    }

    #[inline]
    pub fn with_font_italic(mut self, italic: bool) -> Self {
        self.set_font_italic(italic);
        self
    }

    #[inline]
    pub fn clear_font_italic(&mut self) {
        self.text.font_variations.remove(FontVariations::tag_to_u32(b"ital"));
        self.text.font_variations.remove(FontVariations::tag_to_u32(b"slnt"));
    }

    #[inline]
    pub fn font_slant(&self) -> Option<f32> {
        self.text.font_variations.get(FontVariations::tag_to_u32(b"slnt"))
    }

    #[inline]
    pub fn set_font_slant(&mut self, degrees: f32) {
        self.text
            .font_variations
            .set(FontVariations::tag_to_u32(b"slnt"), degrees);
    }

    #[inline]
    pub fn with_font_slant(mut self, degrees: f32) -> Self {
        self.set_font_slant(degrees);
        self
    }

    #[inline]
    pub fn clear_font_slant(&mut self) {
        self.text.font_variations.remove(FontVariations::tag_to_u32(b"slnt"));
    }

    #[inline]
    pub fn font_variation(&self, tag: &[u8; 4]) -> Option<f32> {
        self.text.font_variations.get(FontVariations::tag_to_u32(tag))
    }

    #[inline]
    pub fn set_font_variation(&mut self, tag: &[u8; 4], value: f32) {
        self.text.font_variations.set(FontVariations::tag_to_u32(tag), value);
    }

    #[inline]
    pub fn with_font_variation(mut self, tag: &[u8; 4], value: f32) -> Self {
        self.set_font_variation(tag, value);
        self
    }

    #[inline]
    pub fn clear_font_variation(&mut self, tag: &[u8; 4]) {
        self.text.font_variations.remove(FontVariations::tag_to_u32(tag));
    }

    #[inline]
    pub fn clear_font_variations(&mut self) {
        self.text.font_variations.clear();
    }

    #[inline]
    pub fn fill_rule(&self) -> FillRule {
        self.fill_rule
    }

    #[inline]
    pub fn set_fill_rule(&mut self, rule: FillRule) {
        self.fill_rule = rule;
    }

    #[inline]
    pub fn with_fill_rule(mut self, rule: FillRule) -> Self {
        self.set_fill_rule(rule);
        self
    }
}
