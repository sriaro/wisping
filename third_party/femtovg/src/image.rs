use bitflags::bitflags;
use imgref::*;
use rgb::alt::Gray;
use rgb::*;
use slotmap::{DefaultKey, SlotMap};

use crate::{ErrorKind, Renderer};

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ImageId(DefaultKey);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum PixelFormat {
    Rgb8,
    Rgba8,
    Gray8,
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ImageFlags: u32 {
        const GENERATE_MIPMAPS = 1;
        const REPEAT_X = 1 << 1;
        const REPEAT_Y = 1 << 2;
        const FLIP_Y = 1 << 3;
        const PREMULTIPLIED = 1 << 4;
        const NEAREST = 1 << 5;
    }
}

#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum ImageSource<'a> {
    Rgb(ImgRef<'a, RGB8>),
    Rgba(ImgRef<'a, RGBA8>),
    Gray(ImgRef<'a, Gray<u8>>),
}

impl ImageSource<'_> {
    pub fn check_data(&self) -> Result<(), ErrorKind> {
        fn check<T>(img: ImgRef<'_, T>) -> Result<(), ErrorKind> {
            let required = img.height().checked_sub(1)
                .and_then(|rows| rows.checked_mul(img.stride()))
                .and_then(|offset| offset.checked_add(img.width()));
            let valid = img.width() > 0 && img.height() > 0 && img.stride() >= img.width()
                && img.width() <= i32::MAX as usize && img.height() <= i32::MAX as usize
                && required.is_some_and(|len| len <= img.buf().len());
            if valid { Ok(()) } else { Err(ErrorKind::InvalidImageData) }
        }
        match self {
            Self::Rgb(img) => check(*img),
            Self::Rgba(img) => check(*img),
            Self::Gray(img) => check(*img),
        }
    }

    pub fn format(&self) -> PixelFormat {
        match self {
            Self::Rgb(_) => PixelFormat::Rgb8,
            Self::Rgba(_) => PixelFormat::Rgba8,
            Self::Gray(_) => PixelFormat::Gray8,
        }
    }

    pub fn dimensions(&self) -> Size {
        match self {
            Self::Rgb(imgref) => Size::new(imgref.width(), imgref.height()),
            Self::Rgba(imgref) => Size::new(imgref.width(), imgref.height()),
            Self::Gray(imgref) => Size::new(imgref.width(), imgref.height()),
        }
    }

    pub fn check_update(&self, info: &ImageInfo, x: usize, y: usize) -> Result<(), ErrorKind> {
        self.check_data()?;
        let size = self.dimensions();

        if !x.checked_add(size.width).is_some_and(|end| end <= info.width())
            || !y.checked_add(size.height).is_some_and(|end| end <= info.height()) {
            return Err(ErrorKind::ImageUpdateOutOfBounds);
        }

        if info.format() != self.format() {
            return Err(ErrorKind::ImageUpdateWithDifferentFormat);
        }

        Ok(())
    }
}

impl<'a> From<ImgRef<'a, RGB8>> for ImageSource<'a> {
    fn from(src: ImgRef<'a, RGB8>) -> Self {
        Self::Rgb(src)
    }
}

impl<'a> From<ImgRef<'a, RGBA8>> for ImageSource<'a> {
    fn from(src: ImgRef<'a, RGBA8>) -> Self {
        Self::Rgba(src)
    }
}

impl<'a> From<ImgRef<'a, Gray<u8>>> for ImageSource<'a> {
    fn from(src: ImgRef<'a, Gray<u8>>) -> Self {
        Self::Gray(src)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Size {
    pub width: usize,
    pub height: usize,
}

impl Size {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct ImageInfo {
    flags: ImageFlags,
    size: Size,
    format: PixelFormat,
}

impl ImageInfo {
    pub fn new(flags: ImageFlags, width: usize, height: usize, format: PixelFormat) -> Self {
        Self {
            flags,
            size: Size { width, height },
            format,
        }
    }

    pub fn flags(&self) -> ImageFlags {
        self.flags
    }

    pub fn width(&self) -> usize {
        self.size.width
    }

    pub fn height(&self) -> usize {
        self.size.height
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn set_format(&mut self, format: PixelFormat) {
        self.format = format;
    }
}

#[derive(Debug)]
pub struct ImageStore<T>(SlotMap<DefaultKey, (ImageInfo, T)>);

impl<T> Default for ImageStore<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ImageStore<T> {
    pub fn new() -> Self {
        Self(SlotMap::new())
    }

    pub fn alloc<R: Renderer<Image = T>>(&mut self, renderer: &mut R, info: ImageInfo) -> Result<ImageId, ErrorKind> {
        let image = renderer.alloc_image(info)?;
        Ok(ImageId(self.0.insert((info, image))))
    }

    pub fn register_native_texture<R: Renderer<Image = T>>(
        &mut self,
        renderer: &mut R,
        texture: R::NativeTexture,
        info: ImageInfo,
    ) -> Result<ImageId, ErrorKind> {
        let image = renderer.create_image_from_native_texture(texture, info)?;
        Ok(ImageId(self.0.insert((info, image))))
    }

    pub fn register_external_texture<R: Renderer<Image = T>>(
        &mut self,
        renderer: &mut R,
        texture: R::ExternalTexture,
        info: ImageInfo,
    ) -> Result<ImageId, ErrorKind> {
        let image = renderer.create_image_from_external_texture(texture, info)?;
        Ok(ImageId(self.0.insert((info, image))))
    }

    pub fn realloc<R: Renderer<Image = T>>(
        &mut self,
        renderer: &mut R,
        id: ImageId,
        info: ImageInfo,
    ) -> Result<(), ErrorKind> {
        if let Some(old) = self.0.get_mut(id.0) {
            let new = renderer.alloc_image(info)?;
            let old = std::mem::replace(old, (info, new));
            renderer.delete_image(old.1, id);
            Ok(())
        } else {
            Err(ErrorKind::ImageIdNotFound)
        }
    }

    pub fn get(&self, id: ImageId) -> Option<&T> {
        self.0.get(id.0).map(|inner| &inner.1)
    }

    pub fn get_mut(&mut self, id: ImageId) -> Option<&mut T> {
        self.0.get_mut(id.0).map(|inner| &mut inner.1)
    }

    pub fn update<R: Renderer<Image = T>>(
        &mut self,
        renderer: &mut R,
        id: ImageId,
        data: ImageSource,
        x: usize,
        y: usize,
    ) -> Result<(), ErrorKind> {
        if let Some(image) = self.0.get_mut(id.0) {
            data.check_update(&image.0, x, y)?;
            renderer.update_image(&mut image.1, data, x, y)?;
            Ok(())
        } else {
            Err(ErrorKind::ImageIdNotFound)
        }
    }

    pub fn info(&self, id: ImageId) -> Option<ImageInfo> {
        self.0.get(id.0).map(|inner| inner.0)
    }

    pub fn remove<R: Renderer<Image = T>>(&mut self, renderer: &mut R, id: ImageId) {
        if let Some(image) = self.0.remove(id.0) {
            renderer.delete_image(image.1, id);
        }
    }

    pub fn clear<R: Renderer<Image = T>>(&mut self, renderer: &mut R) {
        for (idx, image) in self.0.drain() {
            renderer.delete_image(image.1, ImageId(idx));
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum ImageFilter {
    GaussianBlur {
        sigma: f32,
    },
    ColorMatrix {
        matrix: [f32; 20],
    },
}

impl ImageFilter {
    pub const IDENTITY_MATRIX: [f32; 20] = [
        1.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];

    const LR: f32 = 0.2126;
    const LG: f32 = 0.7152;
    const LB: f32 = 0.0722;

    pub fn grayscale(amount: f32) -> Self {
        let a = amount.clamp(0.0, 1.0);
        let inv = 1.0 - a;
        let (lr, lg, lb) = (Self::LR, Self::LG, Self::LB);
        Self::ColorMatrix {
            matrix: [
                lr + 0.7874 * inv,
                lg - lg * inv,
                lb - lb * inv,
                0.0,
                0.0,
                lr - lr * inv,
                lg + 0.2848 * inv,
                lb - lb * inv,
                0.0,
                0.0,
                lr - lr * inv,
                lg - lg * inv,
                lb + 0.9278 * inv,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
            ],
        }
    }

    pub fn sepia(amount: f32) -> Self {
        let inv = 1.0 - amount.clamp(0.0, 1.0);
        Self::ColorMatrix {
            matrix: [
                0.393 + 0.607 * inv,
                0.769 - 0.769 * inv,
                0.189 - 0.189 * inv,
                0.0,
                0.0,
                0.349 - 0.349 * inv,
                0.686 + 0.314 * inv,
                0.168 - 0.168 * inv,
                0.0,
                0.0,
                0.272 - 0.272 * inv,
                0.534 - 0.534 * inv,
                0.131 + 0.869 * inv,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
            ],
        }
    }

    pub fn saturate(amount: f32) -> Self {
        let s = amount.max(0.0);
        let (lr, lg, lb) = (0.213f32, 0.715f32, 0.072f32);
        Self::ColorMatrix {
            matrix: [
                lr + 0.787 * s,
                lg - lg * s,
                lb - lb * s,
                0.0,
                0.0,
                lr - lr * s,
                lg + 0.285 * s,
                lb - lb * s,
                0.0,
                0.0,
                lr - lr * s,
                lg - lg * s,
                lb + 0.928 * s,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
            ],
        }
    }

    pub fn hue_rotate(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        let (lr, lg, lb) = (0.213f32, 0.715f32, 0.072f32);
        Self::ColorMatrix {
            matrix: [
                lr + c * 0.787 - s * 0.213,
                lg - c * lg - s * lg,
                lb - c * lb + s * 0.928,
                0.0,
                0.0,
                lr - c * lr + s * 0.143,
                lg + c * 0.285 + s * 0.140,
                lb - c * lb - s * 0.283,
                0.0,
                0.0,
                lr - c * lr - s * 0.787,
                lg - c * lg + s * lg,
                lb + c * 0.928 + s * lb,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
            ],
        }
    }

    pub fn brightness(amount: f32) -> Self {
        let a = amount.max(0.0);
        Self::ColorMatrix {
            matrix: [
                a, 0.0, 0.0, 0.0, 0.0,
                0.0, a, 0.0, 0.0, 0.0,
                0.0, 0.0, a, 0.0, 0.0,
                0.0, 0.0, 0.0, 1.0, 0.0,
            ],
        }
    }

    pub fn contrast(amount: f32) -> Self {
        let a = amount.max(0.0);
        let b = 0.5 - 0.5 * a;
        Self::ColorMatrix {
            matrix: [
                a, 0.0, 0.0, 0.0, b,
                0.0, a, 0.0, 0.0, b,
                0.0, 0.0, a, 0.0, b,
                0.0, 0.0, 0.0, 1.0, 0.0,
            ],
        }
    }

    pub fn invert(amount: f32) -> Self {
        let a = amount.clamp(0.0, 1.0);
        let d = 1.0 - 2.0 * a;
        Self::ColorMatrix {
            matrix: [
                d, 0.0, 0.0, 0.0, a,
                0.0, d, 0.0, 0.0, a,
                0.0, 0.0, d, 0.0, a,
                0.0, 0.0, 0.0, 1.0, 0.0,
            ],
        }
    }

    pub fn opacity(amount: f32) -> Self {
        let a = amount.clamp(0.0, 1.0);
        Self::ColorMatrix {
            matrix: [
                1.0, 0.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 0.0, a, 0.0,
            ],
        }
    }
}
