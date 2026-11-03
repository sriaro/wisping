use fnv::FnvHashMap;
use std::cell::{Ref, RefCell};
use std::collections::hash_map::Entry;
use std::fmt;
use std::rc::Rc;

use crate::{paint::FontVariations, ErrorKind, Path};

pub(crate) struct FontFaceRef<'a>(std::marker::PhantomData<&'a ()>);

#[derive(Clone, Debug)]
pub struct VariationAxisInfo {
    pub tag: [u8; 4],
    pub min_value: f32,
    pub def_value: f32,
    pub max_value: f32,
    pub name_id: u16,
    pub hidden: bool,
}

#[derive(Clone, Debug)]
pub struct GlyphMetrics {
    pub width: f32,
    pub height: f32,
    pub bearing_x: f32,
    pub bearing_y: f32,
}

#[derive(Debug)]
pub struct Glyph {
    pub path: Option<Path>,
    pub metrics: GlyphMetrics,
}

pub enum GlyphRendering<'a> {
    RenderAsPath(Ref<'a, Path>),
}

#[derive(Copy, Clone, Default, Debug)]
struct FontFlags(u8);

impl FontFlags {
    fn new(regular: bool, italic: bool, bold: bool, oblique: bool, variable: bool) -> Self {
        let mut flags = 0;
        if regular {
            flags |= 0x1;
        }
        if italic {
            flags |= 0x2;
        }
        if bold {
            flags |= 0x4;
        }
        if oblique {
            flags |= 0x8;
        }
        if variable {
            flags |= 0x10;
        }
        Self(flags)
    }

    #[inline]
    fn regular(&self) -> bool {
        self.0 & 0x1 > 0
    }

    #[inline]
    fn italic(&self) -> bool {
        self.0 & 0x2 > 0
    }

    #[inline]
    fn bold(&self) -> bool {
        self.0 & 0x4 > 0
    }

    #[inline]
    fn oblique(&self) -> bool {
        self.0 & 0x8 > 0
    }

    #[inline]
    fn variable(&self) -> bool {
        self.0 & 0x10 > 0
    }
}

mod fallback {
    pub const SCRIPT_SIZE: f32 = 0.65;
    pub const SUBSCRIPT_DROP: f32 = 0.14;
    pub const SUPERSCRIPT_RISE: f32 = 0.48;
    pub const X_HEIGHT: f32 = 0.5;
    pub const CAP_HEIGHT: f32 = 0.7;
}

#[derive(Copy, Clone, Default, Debug)]
pub struct FontMetrics {
    ascender: f32,
    descender: f32,
    height: f32,
    underline_position: f32,
    underline_thickness: f32,
    strikeout_position: f32,
    strikeout_thickness: f32,
    subscript_size: (f32, f32),
    subscript_offset: (f32, f32),
    superscript_size: (f32, f32),
    superscript_offset: (f32, f32),
    x_height: f32,
    cap_height: f32,
    line_gap: f32,
    flags: FontFlags,
    weight: u16,
    width: u16,
}

impl FontMetrics {
    fn scale(&mut self, scale: f32) {
        self.ascender *= scale;
        self.descender *= scale;
        self.height *= scale;
        self.underline_position *= scale;
        self.underline_thickness *= scale;
        self.strikeout_position *= scale;
        self.strikeout_thickness *= scale;
        self.subscript_size.0 *= scale;
        self.subscript_size.1 *= scale;
        self.subscript_offset.0 *= scale;
        self.subscript_offset.1 *= scale;
        self.superscript_size.0 *= scale;
        self.superscript_size.1 *= scale;
        self.superscript_offset.0 *= scale;
        self.superscript_offset.1 *= scale;
        self.x_height *= scale;
        self.cap_height *= scale;
        self.line_gap *= scale;
    }

    pub fn ascender(&self) -> f32 {
        self.ascender
    }

    pub fn descender(&self) -> f32 {
        self.descender
    }

    pub fn height(&self) -> f32 {
        self.height.round()
    }

    pub fn underline_position(&self) -> f32 {
        self.underline_position
    }

    pub fn underline_thickness(&self) -> f32 {
        self.underline_thickness
    }

    pub fn strikeout_position(&self) -> f32 {
        self.strikeout_position
    }

    pub fn strikeout_thickness(&self) -> f32 {
        self.strikeout_thickness
    }

    pub fn subscript_size(&self) -> (f32, f32) {
        self.subscript_size
    }

    pub fn subscript_offset(&self) -> (f32, f32) {
        self.subscript_offset
    }

    pub fn superscript_size(&self) -> (f32, f32) {
        self.superscript_size
    }

    pub fn superscript_offset(&self) -> (f32, f32) {
        self.superscript_offset
    }

    pub fn x_height(&self) -> f32 {
        self.x_height
    }

    pub fn cap_height(&self) -> f32 {
        self.cap_height
    }

    pub fn line_gap(&self) -> f32 {
        self.line_gap
    }

    pub fn regular(&self) -> bool {
        self.flags.regular()
    }

    pub fn italic(&self) -> bool {
        self.flags.italic()
    }

    pub fn bold(&self) -> bool {
        self.flags.bold()
    }

    pub fn oblique(&self) -> bool {
        self.flags.oblique()
    }

    pub fn variable(&self) -> bool {
        self.flags.variable()
    }

    pub fn weight(&self) -> u16 {
        self.weight
    }

    pub fn width(&self) -> u16 {
        self.width
    }
}

impl fmt::Debug for Font {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Font")
            .field("data", &format_args!(".."))
            .field("face_index", &self.face_index)
            .field("units_per_em", &self.units_per_em)
            .field("metrics", &self.metrics)
            .field("glyphs", &self.glyphs)
            .finish()
    }
}

type GlyphCacheKey = (u16, u64);

pub struct Font {
    data: Box<dyn AsRef<[u8]>>,
    face_index: u32,
    units_per_em: u16,
    metrics: FontMetrics,
    glyphs: RefCell<FnvHashMap<GlyphCacheKey, Glyph>>,
    swash_scale_context: Rc<RefCell<swash::scale::ScaleContext>>,
}

impl Font {
    pub fn new_with_data<T: AsRef<[u8]> + 'static>(
        data: T,
        face_index: u32,
        text_context: &super::TextContextImpl,
    ) -> Result<Self, ErrorKind> {
        let font_ref =
            swash::FontRef::from_index(data.as_ref(), face_index as usize).ok_or(ErrorKind::FontParseError)?;

        let swash_metrics = font_ref.metrics(&[]);
        let attrs = font_ref.attributes();

        let units_per_em = swash_metrics.units_per_em;

        let weight = attrs.weight().0;
        let stretch = attrs.stretch();
        let style = attrs.style();

        let is_bold = weight >= 700;
        let is_italic = matches!(style, swash::Style::Italic);
        let is_oblique = matches!(style, swash::Style::Oblique(_));
        let is_variable = font_ref.variations().next().is_some();

        let is_regular = weight == 400 && matches!(style, swash::Style::Normal) && stretch == swash::Stretch::NORMAL;

        let width: u16 = match stretch {
            swash::Stretch::ULTRA_CONDENSED => 1,
            swash::Stretch::EXTRA_CONDENSED => 2,
            swash::Stretch::CONDENSED => 3,
            swash::Stretch::SEMI_CONDENSED => 4,
            swash::Stretch::NORMAL => 5,
            swash::Stretch::SEMI_EXPANDED => 6,
            swash::Stretch::EXPANDED => 7,
            swash::Stretch::EXTRA_EXPANDED => 8,
            swash::Stretch::ULTRA_EXPANDED => 9,
            _ => 5,
        };

        let em = units_per_em as f32;
        let default_thickness = em / 14.0;
        let stroke_size = if swash_metrics.stroke_size > 0.0 {
            swash_metrics.stroke_size
        } else {
            default_thickness
        };

        let fallback_script_size = (em * fallback::SCRIPT_SIZE, em * fallback::SCRIPT_SIZE);

        let metrics = FontMetrics {
            ascender: swash_metrics.ascent,
            descender: -swash_metrics.descent,
            height: swash_metrics.ascent + swash_metrics.descent + swash_metrics.leading,
            underline_position: if swash_metrics.underline_offset != 0.0 {
                swash_metrics.underline_offset
            } else {
                -swash_metrics.descent * 0.5
            },
            underline_thickness: stroke_size,
            strikeout_position: if swash_metrics.strikeout_offset != 0.0 {
                swash_metrics.strikeout_offset
            } else {
                swash_metrics.ascent * 0.4
            },
            strikeout_thickness: stroke_size,
            subscript_size: fallback_script_size,
            subscript_offset: (0.0, em * fallback::SUBSCRIPT_DROP),
            superscript_size: fallback_script_size,
            superscript_offset: (0.0, -(em * fallback::SUPERSCRIPT_RISE)),
            x_height: if swash_metrics.x_height != 0.0 {
                swash_metrics.x_height
            } else {
                em * fallback::X_HEIGHT
            },
            cap_height: if swash_metrics.cap_height != 0.0 {
                swash_metrics.cap_height
            } else {
                em * fallback::CAP_HEIGHT
            },
            line_gap: swash_metrics.leading,
            flags: FontFlags::new(is_regular, is_italic, is_bold, is_oblique, is_variable),
            weight,
            width,
        };

        Ok(Self {
            data: Box::new(data),
            face_index,
            units_per_em,
            metrics,
            glyphs: RefCell::default(),
            swash_scale_context: text_context.swash_scale_context(),
        })
    }

    pub(crate) fn face_ref(&self) -> FontFaceRef<'_> {
        FontFaceRef(std::marker::PhantomData)
    }

    pub(crate) fn swash_font_ref(&self) -> Option<swash::FontRef<'_>> {
        swash::FontRef::from_index(self.data.as_ref().as_ref(), self.face_index as usize)
    }

    fn swash_scale_context(&self) -> &RefCell<swash::scale::ScaleContext> {
        &self.swash_scale_context
    }

    pub(crate) fn face_ref_with_normalized_coords(&self, _coords: &[i16]) -> FontFaceRef<'_> {
        self.face_ref()
    }

    pub fn metrics(&self, size: f32) -> FontMetrics {
        let mut metrics = self.metrics;

        metrics.scale(self.scale(size));

        metrics
    }

    pub fn metrics_with_variations(&self, size: f32, variations: &FontVariations) -> FontMetrics {
        if variations.is_empty() {
            return self.metrics(size);
        }
        let Some(font_ref) = self.swash_font_ref() else {
            return self.metrics(size);
        };
        let settings: Vec<swash::Setting<f32>> = variations
            .iter()
            .map(|(tag, value)| swash::Setting {
                tag: swash::tag_from_bytes(&tag.to_be_bytes()),
                value,
            })
            .collect();
        let coords: Vec<swash::NormalizedCoord> = font_ref.variations().normalized_coords(settings).collect();
        let swash_metrics = font_ref.metrics(&coords);
        let mut metrics = FontMetrics {
            ascender: swash_metrics.ascent,
            descender: -swash_metrics.descent,
            height: swash_metrics.ascent + swash_metrics.descent + swash_metrics.leading,
            ..self.metrics
        };
        metrics.scale(self.scale(size));
        metrics
    }

    pub fn scale(&self, size: f32) -> f32 {
        size / self.units_per_em as f32
    }

    pub fn variation_axes(&self) -> Vec<VariationAxisInfo> {
        let Some(font_ref) = self.swash_font_ref() else {
            return Vec::new();
        };
        font_ref
            .variations()
            .map(|v| {
                let name_id = match v.name_id() {
                    swash::StringId::Other(id) => id,
                    _ => 0,
                };
                VariationAxisInfo {
                    tag: v.tag().to_be_bytes(),
                    min_value: v.min_value(),
                    def_value: v.default_value(),
                    max_value: v.max_value(),
                    name_id,
                    hidden: v.is_hidden(),
                }
            })
            .collect()
    }

    fn hash_normalized_coords(coords: &[i16]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = fnv::FnvHasher::default();
        coords.hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn glyph(
        &self,
        _face: &FontFaceRef<'_>,
        codepoint: u16,
        normalized_coords: &[i16],
    ) -> Option<Ref<'_, Glyph>> {
        let cache_key: GlyphCacheKey = (codepoint, Self::hash_normalized_coords(normalized_coords));
        if let Entry::Vacant(entry) = self.glyphs.borrow_mut().entry(cache_key) {
            let font_ref = self.swash_font_ref()?;

            let mut scale_context = self.swash_scale_context().borrow_mut();
            let scaler_builder = scale_context
                .builder(font_ref)
                .size(self.units_per_em as f32)
                .hint(false)
                .normalized_coords(normalized_coords);
            let mut scaler = scaler_builder.build();

            let maybe_glyph = if let Some(outline) = scaler.scale_outline(codepoint) {
                use swash::zeno::{Command, PathData};
                let bounds = outline.bounds();
                let mut path = Path::new();
                for cmd in outline.path().commands() {
                    match cmd {
                        Command::MoveTo(p) => path.move_to(p.x, p.y),
                        Command::LineTo(p) => path.line_to(p.x, p.y),
                        Command::QuadTo(c, p) => path.quad_to(c.x, c.y, p.x, p.y),
                        Command::CurveTo(c1, c2, p) => path.bezier_to(c1.x, c1.y, c2.x, c2.y, p.x, p.y),
                        Command::Close => path.close(),
                    }
                }
                Some(Glyph {
                    path: Some(path),
                    metrics: GlyphMetrics {
                        width: bounds.width(),
                        height: bounds.height(),
                        bearing_x: bounds.min.x,
                        bearing_y: bounds.max.y,
                    },
                })
            } else {
                None
            };

            if let Some(glyph) = maybe_glyph {
                entry.insert(glyph);
            }
        }

        Ref::filter_map(self.glyphs.borrow(), |glyphs| glyphs.get(&cache_key)).ok()
    }

    pub(crate) fn glyph_rendering_representation(
        &self,
        _face: &FontFaceRef<'_>,
        codepoint: u16,
        #[allow(unused_variables)] _pixels_per_em: u16,
        normalized_coords: &[i16],
    ) -> Option<GlyphRendering<'_>> {
        self.glyph(_face, codepoint, normalized_coords).and_then(|glyph| {
            Ref::filter_map(glyph, |glyph| glyph.path.as_ref())
                .ok()
                .map(GlyphRendering::RenderAsPath)
        })
    }
}
