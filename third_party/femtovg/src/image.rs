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
}
