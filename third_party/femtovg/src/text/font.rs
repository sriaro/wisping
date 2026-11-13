use fnv::FnvHashMap;
use std::cell::{Ref, RefCell};
use std::collections::hash_map::Entry;
use std::fmt;
use std::rc::Rc;

use crate::{ErrorKind, Path};

#[derive(Clone, Debug)]
pub struct GlyphMetrics {
    pub width: f32,
    pub height: f32,
    pub bearing_x: f32,
    pub bearing_y: f32,
}

#[derive(Debug)]
pub struct Glyph {
    pub path: Path,
    pub metrics: GlyphMetrics,
}

pub enum GlyphRendering<'a> {
    RenderAsPath(Ref<'a, Path>),
}

impl fmt::Debug for Font {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Font")
            .field("data", &format_args!(".."))
            .field("face_index", &self.face_index)
            .field("units_per_em", &self.units_per_em)
            .field("glyphs", &self.glyphs)
            .finish()
    }
}

type GlyphCacheKey = (u16, u64);

pub struct Font {
    data: Box<dyn AsRef<[u8]>>,
    face_index: u32,
    units_per_em: u16,
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

        let units_per_em = font_ref.metrics(&[]).units_per_em;

        Ok(Self {
            data: Box::new(data),
            face_index,
            units_per_em,
            glyphs: RefCell::default(),
            swash_scale_context: text_context.swash_scale_context(),
        })
    }

    pub(crate) fn swash_font_ref(&self) -> Option<swash::FontRef<'_>> {
        swash::FontRef::from_index(self.data.as_ref().as_ref(), self.face_index as usize)
    }

    fn swash_scale_context(&self) -> &RefCell<swash::scale::ScaleContext> {
        &self.swash_scale_context
    }

    pub fn scale(&self, size: f32) -> f32 {
        size / self.units_per_em as f32
    }

    fn hash_normalized_coords(coords: &[i16]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = fnv::FnvHasher::default();
        coords.hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn glyph(
        &self,
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
                    path,
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
        codepoint: u16,
        normalized_coords: &[i16],
    ) -> Option<GlyphRendering<'_>> {
        self.glyph(codepoint, normalized_coords)
            .map(|glyph| GlyphRendering::RenderAsPath(Ref::map(glyph, |glyph| &glyph.path)))
    }
}
