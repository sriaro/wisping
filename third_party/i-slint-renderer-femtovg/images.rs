// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

use std::collections::HashMap;
use std::rc::Rc;

use i_slint_core::graphics::euclid;
use i_slint_core::graphics::{ImageCacheKey, IntSize, SharedImageBuffer, SharedPixelBuffer};
use i_slint_core::items::ImageTiling;
use i_slint_core::lengths::PhysicalPx;
use i_slint_core::{ImageInner, items::ImageRendering};

use super::itemrenderer::CanvasRc;

pub struct Texture<R: femtovg::Renderer> {
    pub id: femtovg::ImageId,
    canvas: CanvasRc<R>,
}

impl<R: femtovg::Renderer> Texture<R> {
    pub fn size(&self) -> Option<IntSize> {
        self.canvas
            .borrow()
            .image_info(self.id)
            .map(|info| [info.width() as u32, info.height() as u32].into())
            .ok()
    }

    pub fn as_render_target(&self) -> femtovg::RenderTarget {
        femtovg::RenderTarget::Image(self.id)
    }

    pub fn adopt(canvas: &CanvasRc<R>, image_id: femtovg::ImageId) -> Rc<Texture<R>> {
        Texture { id: image_id, canvas: canvas.clone() }.into()
    }

    pub fn new_empty_on_gpu(
        canvas: &CanvasRc<R>,
        width: u32,
        height: u32,
    ) -> Option<Rc<Texture<R>>> {
        if width == 0 || height == 0 {
            return None;
        }
        let image_id = canvas
            .borrow_mut()
            .create_image_empty(
                width as usize,
                height as usize,
                femtovg::PixelFormat::Rgba8,
                femtovg::ImageFlags::PREMULTIPLIED | femtovg::ImageFlags::FLIP_Y,
            )
            .unwrap();
        Some(Self { canvas: canvas.clone(), id: image_id }.into())
    }

    pub(crate) fn filter(&self, filter: femtovg::ImageFilter) -> Rc<Self> {
        let size = self.size().unwrap();
        let filtered_image = Self::new_empty_on_gpu(&self.canvas, size.width, size.height).expect(
            "internal error: this can only fail if the filtered image was zero width or height",
        );

        self.canvas.borrow_mut().filter_image(filtered_image.id, filter, self.id);

        filtered_image
    }

    pub fn as_paint(&self) -> femtovg::Paint {
        self.as_paint_with_alpha(1.0)
    }

    pub fn as_paint_with_alpha(&self, alpha_tint: f32) -> femtovg::Paint {
        let size = self
            .size()
            .expect("internal error: CachedImage::as_paint() called on zero-sized texture");
        femtovg::Paint::image(
            self.id,
            0.,
            0.,
            size.width as f32,
            size.height as f32,
            0.,
            alpha_tint,
        )
    }

    pub fn id(&self) -> femtovg::ImageId {
        self.id
    }

    pub fn new_from_image(
        image: &ImageInner,
        canvas: &CanvasRc<R>,
        target_size_for_scalable_source: Option<euclid::Size2D<u32, PhysicalPx>>,
        scaling: ImageRendering,
        tiling: (ImageTiling, ImageTiling),
    ) -> Option<Rc<Self>> {
        let image_flags = base_image_flags(scaling, tiling);

        let buffer = image.render_to_buffer(target_size_for_scalable_source)?;
        let (image_source, flags) = image_buffer_to_image_source(&buffer);
        let mipmaps = if target_size_for_scalable_source.is_none()
            && !image_flags.contains(femtovg::ImageFlags::NEAREST)
        {
            femtovg::ImageFlags::GENERATE_MIPMAPS
        } else {
            femtovg::ImageFlags::empty()
        };
        let image_id = canvas.borrow_mut().create_image(image_source, image_flags | flags | mipmaps).unwrap();

        Some(Self::adopt(canvas, image_id))
    }
}

impl<R: femtovg::Renderer> Drop for Texture<R> {
    fn drop(&mut self) {
        self.canvas.borrow_mut().delete_image(self.id);
    }
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct TextureCacheKey {
    source_key: ImageCacheKey,
    target_size_for_scalable_source: Option<euclid::Size2D<u32, PhysicalPx>>,
    gpu_image_flags: ImageRendering,
    gpu_image_tiling: (ImageTiling, ImageTiling),
}

impl TextureCacheKey {
    pub fn new(
        resource: &ImageInner,
        target_size_for_scalable_source: Option<euclid::Size2D<u32, PhysicalPx>>,
        gpu_image_flags: ImageRendering,
        gpu_image_tiling: (ImageTiling, ImageTiling),
    ) -> Option<Self> {
        ImageCacheKey::new(resource).map(|source_key| Self {
            source_key,
            target_size_for_scalable_source,
            gpu_image_flags,
            gpu_image_tiling,
        })
    }
}

pub struct TextureCache<R: femtovg::Renderer>(
    HashMap<TextureCacheKey, Rc<Texture<R>>>,
);

impl<R: femtovg::Renderer> Default for TextureCache<R> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<R: femtovg::Renderer> TextureCache<R> {
    pub(crate) fn lookup_image_in_cache_or_create(
        &mut self,
        cache_key: TextureCacheKey,
        image_create_fn: impl Fn() -> Option<Rc<Texture<R>>>,
    ) -> Option<Rc<Texture<R>>> {
        Some(match self.0.entry(cache_key) {
            std::collections::hash_map::Entry::Occupied(existing_entry) => {
                existing_entry.get().clone()
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                let new_image = image_create_fn()?;
                vacant_entry.insert(new_image.clone());
                new_image
            }
        })
    }

    pub(crate) fn drain(&mut self) {
        self.0.retain(|_, cached_image| {
            Rc::strong_count(cached_image) > 1 || cached_image.size().is_none()
        });
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

fn image_buffer_to_image_source(
    buffer: &SharedImageBuffer,
) -> (femtovg::ImageSource<'_>, femtovg::ImageFlags) {
    fn image_source<Pixel: Clone>(buffer: &SharedPixelBuffer<Pixel>) -> imgref::ImgRef<'_, Pixel> {
        let pixels = buffer.as_slice();
        let read = buffer.width() as u64 * buffer.height() as u64;
        assert!(pixels.len() as u64 >= read);
        imgref::ImgRef::new(&pixels[..read as usize], buffer.width() as _, buffer.height() as _)
    }

    match buffer {
        SharedImageBuffer::RGB8(buffer) => {
            (image_source(buffer).into(), femtovg::ImageFlags::empty())
        }
        SharedImageBuffer::RGBA8(buffer) => {
            (image_source(buffer).into(), femtovg::ImageFlags::empty())
        }
        SharedImageBuffer::RGBA8Premultiplied(buffer) => {
            (image_source(buffer).into(), femtovg::ImageFlags::PREMULTIPLIED)
        }
    }
}

pub fn base_image_flags(
    scaling: ImageRendering,
    tiling: (ImageTiling, ImageTiling),
) -> femtovg::ImageFlags {
    (match scaling {
        ImageRendering::Pixelated => femtovg::ImageFlags::NEAREST,
        ImageRendering::Smooth | _ => femtovg::ImageFlags::empty(),
    } | match tiling.0 {
        ImageTiling::Repeat | ImageTiling::Round => femtovg::ImageFlags::REPEAT_X,
        ImageTiling::None | _ => femtovg::ImageFlags::empty(),
    } | match tiling.1 {
        ImageTiling::Repeat | ImageTiling::Round => femtovg::ImageFlags::REPEAT_Y,
        ImageTiling::None | _ => femtovg::ImageFlags::empty(),
    })
}
