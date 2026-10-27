use super::*;
use crate::{Canvas, RecordingRenderer};

#[test]
fn security_short_sources_are_rejected_in_every_format() {
    let rgb = [RGB8::new(1, 2, 3)];
    let rgba = [RGBA8::new(1, 2, 3, 4)];
    let gray = [Gray::new(1)];
    let sources = [ImageSource::Rgb(Img::new(&rgb[..], 2, 2)), ImageSource::Rgba(Img::new(&rgba[..], 2, 2)), ImageSource::Gray(Img::new(&gray[..], 2, 2))];
    let mut canvas = Canvas::new(RecordingRenderer::default()).unwrap();
    for src in sources {
        assert!(matches!(src.check_data(), Err(ErrorKind::InvalidImageData)));
        assert!(matches!(canvas.create_image(src, ImageFlags::empty()), Err(ErrorKind::InvalidImageData)));
        let id = canvas.create_image_empty(2, 2, src.format(), ImageFlags::empty()).unwrap();
        assert!(matches!(canvas.update_image(id, src, 0, 0), Err(ErrorKind::InvalidImageData)));
    }
}

#[test]
#[allow(deprecated)]
fn security_stride_and_overflow_are_checked() {
    let pixels = [RGBA8::new(1, 2, 3, 4); 4];
    let mut img = Img::new(&pixels[..], 2, 2);
    img.stride = 1;
    assert!(ImageSource::Rgba(img).check_data().is_err());
    let overflow = Img::new_stride(&pixels[..], 2, 3, usize::MAX);
    assert!(ImageSource::Rgba(overflow).check_data().is_err());
    let zero = Img::new_stride(&pixels[..], 0, 1, 1);
    assert!(ImageSource::Rgba(zero).check_data().is_err());
    let src = ImageSource::Rgba(Img::new(&pixels[..], 2, 2));
    let info = ImageInfo::new(ImageFlags::empty(), usize::MAX, usize::MAX, PixelFormat::Rgba8);
    assert!(matches!(src.check_update(&info, usize::MAX, 0), Err(ErrorKind::ImageUpdateOutOfBounds)));
}

#[test]
fn security_packed_and_padded_sources_remain_valid() {
    let pixels = [RGBA8::new(1, 2, 3, 4); 5];
    let padded = ImageSource::Rgba(Img::new_stride(&pixels[..], 2, 2, 3));
    let mut canvas = Canvas::new(RecordingRenderer::default()).unwrap();
    let id = canvas.create_image(padded, ImageFlags::empty()).unwrap();
    let packed = ImageSource::Rgba(Img::new(&pixels[..], 2, 2));
    canvas.update_image(id, packed, 0, 0).unwrap();
    assert!(matches!(packed.check_update(&ImageInfo::new(ImageFlags::empty(), 2, 2, PixelFormat::Rgb8), 0, 0), Err(ErrorKind::ImageUpdateWithDifferentFormat)));
    assert!(matches!(canvas.update_image(id, packed, 1, 0), Err(ErrorKind::ImageUpdateOutOfBounds)));
}

#[derive(Default)]
struct ImageLifetimeRenderer {
    allocation_attempts: usize,
    allocations: usize,
    deleted: Vec<(usize, ImageId)>,
    live: std::collections::HashSet<usize>,
    fail_next_allocation: bool,
}

impl Renderer for ImageLifetimeRenderer {
    type Image = usize;
    type NativeTexture = ();
    type ExternalTexture = ();
    type RenderOutput = ();
    type CommandBuffer = ();

    fn set_size(&mut self, _: u32, _: u32, _: f32) {}

    fn render(
        &mut self,
        _: impl Into<Self::RenderOutput>,
        _: &mut ImageStore<Self::Image>,
        _: &[crate::renderer::Vertex],
        _: Vec<crate::renderer::Command>,
    ) {}

    fn alloc_image(&mut self, _: ImageInfo) -> Result<Self::Image, ErrorKind> {
        self.allocation_attempts += 1;
        if std::mem::take(&mut self.fail_next_allocation) {
            return Err(ErrorKind::GeneralError("allocation failed".to_owned()));
        }
        self.allocations += 1;
        assert!(self.live.insert(self.allocations));
        Ok(self.allocations)
    }

    fn create_image_from_native_texture(&mut self, _: (), _: ImageInfo) -> Result<Self::Image, ErrorKind> {
        Err(ErrorKind::UnsupportedOperation)
    }

    fn create_image_from_external_texture(&mut self, _: (), _: ImageInfo) -> Result<Self::Image, ErrorKind> {
        Err(ErrorKind::UnsupportedOperation)
    }

    fn update_image(&mut self, _: &mut Self::Image, _: ImageSource, _: usize, _: usize) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn delete_image(&mut self, image: Self::Image, id: ImageId) {
        assert!(self.live.remove(&image));
        self.deleted.push((image, id));
    }

    fn screenshot(&mut self) -> Result<ImgVec<RGBA8>, ErrorKind> {
        Err(ErrorKind::UnsupportedOperation)
    }
}

#[test]
fn security_realloc_releases_each_previous_image_and_keeps_the_id() {
    let mut renderer = ImageLifetimeRenderer::default();
    let mut images = ImageStore::new();
    let info = ImageInfo::new(ImageFlags::empty(), 2, 2, PixelFormat::Rgba8);
    let id = images.alloc(&mut renderer, info).unwrap();

    for width in 3..=10 {
        let previous = *images.get(id).unwrap();
        let info = ImageInfo::new(ImageFlags::NEAREST, width, 4, PixelFormat::Rgb8);
        images.realloc(&mut renderer, id, info).unwrap();
        assert_eq!(images.info(id), Some(info));
        assert_eq!(images.get(id), Some(&renderer.allocations));
        assert_eq!(renderer.deleted.last(), Some(&(previous, id)));
        assert_eq!(renderer.live.len(), 1);
        assert_eq!(renderer.allocations, renderer.deleted.len() + 1);
    }

    images.remove(&mut renderer, id);
    assert!(images.get(id).is_none());
    assert!(renderer.live.is_empty());
    assert_eq!(renderer.allocations, renderer.deleted.len());
}

#[test]
fn security_realloc_failure_preserves_the_previous_image_and_info() {
    let mut renderer = ImageLifetimeRenderer::default();
    let mut images = ImageStore::new();
    let info = ImageInfo::new(ImageFlags::empty(), 2, 2, PixelFormat::Rgba8);
    let id = images.alloc(&mut renderer, info).unwrap();
    let previous = *images.get(id).unwrap();
    renderer.fail_next_allocation = true;

    let replacement = ImageInfo::new(ImageFlags::NEAREST, 4, 5, PixelFormat::Rgb8);
    assert!(matches!(images.realloc(&mut renderer, id, replacement), Err(ErrorKind::GeneralError(_))));
    assert_eq!(images.get(id), Some(&previous));
    assert_eq!(images.info(id), Some(info));
    assert_eq!(renderer.allocation_attempts, 2);
    assert_eq!(renderer.allocations, 1);
    assert!(renderer.deleted.is_empty());
    assert_eq!(renderer.live, std::collections::HashSet::from([previous]));

    images.clear(&mut renderer);
    assert_eq!(renderer.deleted, [(previous, id)]);
    assert!(renderer.live.is_empty());
}

#[test]
fn security_realloc_missing_id_does_not_allocate_or_delete() {
    let mut renderer = ImageLifetimeRenderer::default();
    let mut images = ImageStore::new();
    let info = ImageInfo::new(ImageFlags::empty(), 2, 2, PixelFormat::Rgba8);
    let removed_id = images.alloc(&mut renderer, info).unwrap();
    images.remove(&mut renderer, removed_id);
    let live_id = images.alloc(&mut renderer, info).unwrap();
    let previous = *images.get(live_id).unwrap();

    assert!(matches!(images.realloc(&mut renderer, removed_id, info), Err(ErrorKind::ImageIdNotFound)));
    assert_eq!(renderer.allocation_attempts, 2);
    assert_eq!(renderer.allocations, 2);
    assert_eq!(renderer.deleted, [(1, removed_id)]);
    assert_eq!(renderer.live, std::collections::HashSet::from([previous]));
    assert_eq!(images.get(live_id), Some(&previous));
    assert_eq!(images.info(live_id), Some(info));
}
