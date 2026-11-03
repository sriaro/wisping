use std::collections::BTreeMap;

use crate::{
    image::ImageStore,
    paint::{GradientStop, MultiStopGradient},
    Color, ErrorKind, ImageFlags, ImageId, ImageInfo, ImageSource, Renderer,
};

const IDLE_FLUSHES_BEFORE_RELEASE: u8 = 2;
const SPARE_TEXTURES: usize = 16;

#[derive(Debug)]
pub struct GradientStore {
    gradients: BTreeMap<MultiStopGradient, (ImageId, u8)>,
    spare: Vec<ImageId>,
}

impl GradientStore {
    pub fn new() -> Self {
        Self {
            gradients: BTreeMap::new(),
            spare: Vec::new(),
        }
    }

    pub fn lookup_or_add<R: Renderer>(
        &mut self,
        colors: &MultiStopGradient,
        images: &mut ImageStore<R::Image>,
        renderer: &mut R,
    ) -> Result<ImageId, ErrorKind> {
        if let Some((gradient_image_id, idle_flushes)) = self.gradients.get_mut(colors) {
            *idle_flushes = 0;
            return Ok(*gradient_image_id);
        }
        let image = linear_gradient_stops(colors);
        let gradient_image_id = match self.spare.pop() {
            Some(gradient_image_id) => gradient_image_id,
            None => {
                let info = ImageInfo::new(ImageFlags::REPEAT_Y, 256, 1, crate::PixelFormat::Rgba8);
                images.alloc(renderer, info)?
            }
        };
        if let Err(error) = images.update(renderer, gradient_image_id, ImageSource::Rgba(image.as_ref()), 0, 0) {
            images.remove(renderer, gradient_image_id);
            return Err(error);
        }
        self.gradients.insert(colors.clone(), (gradient_image_id, 0));
        Ok(gradient_image_id)
    }

    pub fn release_old_gradients<R: Renderer>(&mut self, images: &mut ImageStore<R::Image>, renderer: &mut R) {
        let spare = &mut self.spare;
        self.gradients.retain(|_, (gradient_image_id, idle_flushes)| {
            *idle_flushes += 1;
            if *idle_flushes <= IDLE_FLUSHES_BEFORE_RELEASE {
                return true;
            }
            if spare.len() < SPARE_TEXTURES {
                spare.push(*gradient_image_id);
            } else {
                images.remove(renderer, *gradient_image_id);
            }
            false
        });
    }
}

#[allow(clippy::many_single_char_names)]
// Gradient filling, adapted from https://github.com/lieff/lvg/blob/master/render/common.c#L147
fn gradient_span(dest: &mut [rgb::RGBA8; 256], color0: Color, color1: Color, offset0: f32, offset1: f32) {
    let s0o = offset0.clamp(0.0, 1.0);
    let s1o = offset1.clamp(0.0, 1.0);

    if s1o < s0o {
        return;
    }

    let s = (s0o * 256.0) as usize;
    let e = (s1o * 256.0) as usize;

    let mut r = color0.r;
    let mut g = color0.g;
    let mut b = color0.b;
    let mut a = color0.a;

    let steps = (e - s) as f32;

    let dr = (color1.r - r) / steps;
    let dg = (color1.g - g) / steps;
    let db = (color1.b - b) / steps;
    let da = (color1.a - a) / steps;

    #[allow(clippy::needless_range_loop)]
    for i in s..e {
        dest[i] = rgb::RGBA8::new(
            (r * a * 255.0) as u8,
            (g * a * 255.0) as u8,
            (b * a * 255.0) as u8,
            (a * 255.0) as u8,
        );
        r += dr;
        g += dg;
        b += db;
        a += da;
    }
}
fn linear_gradient_stops(gradient: &MultiStopGradient) -> imgref::Img<Vec<rgb::RGBA8>> {
    let mut dest = [rgb::RGBA8::new(0, 0, 0, 0); 256];

    let first_stop = gradient.get(0);
    if first_stop.0 > 0.0 {
        let s0 = first_stop.0;
        let color0 = first_stop.1;
        gradient_span(&mut dest, color0, color0, 0.0, s0);
    }

    for [GradientStop(s0, color0), GradientStop(s1, color1)] in gradient.pairs() {
        if s0 < 1.0 && s1 > 1.0 {
            gradient_span(&mut dest, color0, color0, s0, 1.0);
        } else {
            gradient_span(&mut dest, color0, color1, s0, s1);
        }

        if s0 > 1.0 {
            break;
        }
    }
    imgref::Img::new(dest.to_vec(), 256, 1)
}
