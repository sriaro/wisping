#![warn(missing_debug_implementations)]

use std::{cell::RefCell, rc::Rc};

use imgref::ImgVec;
use rgb::RGBA8;

mod text;

mod error;
pub use error::ErrorKind;

pub use text::{
    Atlas, DrawCommand, FontId, GlyphDrawCommands, Quad, RenderMode,
};

pub use text::TextContext;

use text::{GlyphAtlas, TextContextImpl};

mod image;
use crate::image::ImageStore;
pub use crate::image::{ImageFilter, ImageFlags, ImageId, ImageInfo, ImageSource, PixelFormat};

mod color;
pub use color::Color;

pub mod renderer;
pub use renderer::{RenderTarget, Renderer};

use renderer::{Command, CommandType, Drawable, Params, ShaderType, Vertex};

pub(crate) mod geometry;
pub use geometry::Transform2D;
use geometry::*;

mod paint;
pub use paint::Paint;
use paint::{GlyphTexture, PaintFlavor, StrokeSettings};

mod path;
use path::Convexity;
pub use path::{Path, PathIter, Solidity, Verb};

mod gradient_store;
use gradient_store::GradientStore;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
pub enum FillRule {
    EvenOdd,
    #[default]
    NonZero,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Hash)]
pub enum BlendFactor {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    DstColor,
    OneMinusDstColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    SrcAlphaSaturate,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Hash)]
pub enum CompositeOperation {
    SourceOver,
    SourceIn,
    SourceOut,
    Atop,
    DestinationOver,
    DestinationIn,
    DestinationOut,
    DestinationAtop,
    Lighter,
    Copy,
    Xor,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Hash)]
pub struct CompositeOperationState {
    src_rgb: BlendFactor,
    src_alpha: BlendFactor,
    dst_rgb: BlendFactor,
    dst_alpha: BlendFactor,
}

impl CompositeOperationState {
    pub fn new(op: CompositeOperation) -> Self {
        let (sfactor, dfactor) = match op {
            CompositeOperation::SourceOver => (BlendFactor::One, BlendFactor::OneMinusSrcAlpha),
            CompositeOperation::SourceIn => (BlendFactor::DstAlpha, BlendFactor::Zero),
            CompositeOperation::SourceOut => (BlendFactor::OneMinusDstAlpha, BlendFactor::Zero),
            CompositeOperation::Atop => (BlendFactor::DstAlpha, BlendFactor::OneMinusSrcAlpha),
            CompositeOperation::DestinationOver => (BlendFactor::OneMinusDstAlpha, BlendFactor::One),
            CompositeOperation::DestinationIn => (BlendFactor::Zero, BlendFactor::SrcAlpha),
            CompositeOperation::DestinationOut => (BlendFactor::Zero, BlendFactor::OneMinusSrcAlpha),
            CompositeOperation::DestinationAtop => (BlendFactor::OneMinusDstAlpha, BlendFactor::SrcAlpha),
            CompositeOperation::Lighter => (BlendFactor::One, BlendFactor::One),
            CompositeOperation::Copy => (BlendFactor::One, BlendFactor::Zero),
            CompositeOperation::Xor => (BlendFactor::OneMinusDstAlpha, BlendFactor::OneMinusSrcAlpha),
        };

        Self {
            src_rgb: sfactor,
            src_alpha: sfactor,
            dst_rgb: dfactor,
            dst_alpha: dfactor,
        }
    }
}

impl Default for CompositeOperationState {
    fn default() -> Self {
        Self::new(CompositeOperation::SourceOver)
    }
}

#[derive(Copy, Clone, Debug, Default)]
struct Scissor {
    transform: Transform2D,
    extent: Option<[f32; 2]>,
    radius: f32,
}

impl Scissor {
    fn as_rect(&self, canvas_width: f32, canvas_height: f32) -> Option<Rect> {
        let Some(extent) = self.extent else {
            return Some(Rect::new(0., 0., canvas_width, canvas_height));
        };

        if self.radius > 0.0 {
            return None;
        }

        let Transform2D([a, b, c, d, x, y]) = self.transform;

        if b != 0.0 || c != 0.0 {
            return None;
        }

        if a != 1.0 || d != 1.0 {
            return None;
        }

        let half_width = extent[0];
        let half_height = extent[1];
        Some(Rect::new(
            x - half_width,
            y - half_height,
            half_width * 2.0,
            half_height * 2.0,
        ))
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Default)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Copy, Clone, Debug)]
struct State {
    composite_operation: CompositeOperationState,
    transform: Transform2D,
    scissor: Scissor,
    alpha: f32,
    shadow_color: Color,
    shadow_blur: f32,
    shadow_offset: [f32; 2],
}

impl Default for State {
    fn default() -> Self {
        Self {
            composite_operation: CompositeOperationState::default(),
            transform: Transform2D::identity(),
            scissor: Scissor::default(),
            alpha: 1.0,
            shadow_color: Color::rgbaf(0.0, 0.0, 0.0, 0.0),
            shadow_blur: 0.0,
            shadow_offset: [0.0, 0.0],
        }
    }
}

#[derive(Debug)]
pub struct Canvas<T: Renderer> {
    width: u32,
    height: u32,
    renderer: T,
    text_context: Rc<RefCell<TextContextImpl>>,
    glyph_atlas: Rc<GlyphAtlas>,
    ephemeral_glyph_atlas: Option<Rc<GlyphAtlas>>,
    current_render_target: RenderTarget,
    state_stack: Vec<State>,
    commands: Vec<Command>,
    verts: Vec<Vertex>,
    images: ImageStore<T::Image>,
    fringe_width: f32,
    device_px_ratio: f32,
    tess_tol: f32,
    dist_tol: f32,
    gradients: GradientStore,
    shadow_images: Vec<ImageId>,
}

impl<T> Canvas<T>
where
    T: Renderer,
{
    pub fn new(renderer: T) -> Result<Self, ErrorKind> {
        let text_context = Rc::new(RefCell::new(TextContextImpl::default()));
        let glyph_atlas = Rc::new(GlyphAtlas::new(&text_context));
        let mut canvas = Self {
            width: 0,
            height: 0,
            renderer,
            text_context,
            glyph_atlas,
            ephemeral_glyph_atlas: None,
            current_render_target: RenderTarget::Screen,
            state_stack: Vec::new(),
            commands: Vec::new(),
            verts: Vec::new(),
            images: ImageStore::new(),
            fringe_width: 1.0,
            device_px_ratio: 1.0,
            tess_tol: 0.25,
            dist_tol: 0.01,
            gradients: GradientStore::new(),
            shadow_images: Vec::new(),
        };

        canvas.save();

        Ok(canvas)
    }

    pub fn new_with_text_context(renderer: T, text_context: TextContext) -> Result<Self, ErrorKind> {
        let glyph_atlas = Rc::new(GlyphAtlas::new(&text_context.0));
        let mut canvas = Self {
            width: 0,
            height: 0,
            renderer,
            text_context: text_context.0,
            glyph_atlas,
            ephemeral_glyph_atlas: None,
            current_render_target: RenderTarget::Screen,
            state_stack: Vec::new(),
            commands: Vec::new(),
            verts: Vec::new(),
            images: ImageStore::new(),
            fringe_width: 1.0,
            device_px_ratio: 1.0,
            tess_tol: 0.25,
            dist_tol: 0.01,
            gradients: GradientStore::new(),
            shadow_images: Vec::new(),
        };

        canvas.save();

        Ok(canvas)
    }

    pub fn set_size(&mut self, width: u32, height: u32, dpi: f32) {
        self.width = width;
        self.height = height;
        self.fringe_width = 1.0;
        self.tess_tol = 0.25 / dpi;
        self.dist_tol = 0.01 / dpi;
        self.device_px_ratio = dpi;

        self.renderer.set_size(width, height, dpi);

        self.append_cmd(Command::new(CommandType::SetRenderTarget(RenderTarget::Screen)));
    }

    pub fn clear_rect(&mut self, x: u32, y: u32, width: u32, height: u32, color: Color) {
        let mut cmd = Command::new(CommandType::ClearRect { color });
        cmd.composite_operation = self.state().composite_operation;

        let x0 = x as f32;
        let y0 = y as f32;
        let x1 = x0 + width as f32;
        let y1 = y0 + height as f32;

        let (p0, p1) = (x0, y0);
        let (p2, p3) = (x1, y0);
        let (p4, p5) = (x1, y1);
        let (p6, p7) = (x0, y1);

        let verts = [
            Vertex::new(p0, p1, 0.0, 0.0),
            Vertex::new(p4, p5, 0.0, 0.0),
            Vertex::new(p2, p3, 0.0, 0.0),
            Vertex::new(p0, p1, 0.0, 0.0),
            Vertex::new(p6, p7, 0.0, 0.0),
            Vertex::new(p4, p5, 0.0, 0.0),
        ];

        cmd.triangles_verts = Some((self.verts.len(), verts.len()));
        self.append_cmd(cmd);

        self.verts.extend_from_slice(&verts);
    }

    pub fn width(&self) -> u32 {
        match self.current_render_target {
            RenderTarget::Image(id) => self.image_info(id).map(|info| info.width() as u32).unwrap_or(0),
            RenderTarget::Screen => self.width,
        }
    }

    pub fn height(&self) -> u32 {
        match self.current_render_target {
            RenderTarget::Image(id) => self.image_info(id).map(|info| info.height() as u32).unwrap_or(0),
            RenderTarget::Screen => self.height,
        }
    }

    pub fn flush_to_output(&mut self, output: impl Into<T::RenderOutput>) -> T::CommandBuffer {
        let command_buffer = self.renderer.render(
            output,
            &mut self.images,
            &self.verts,
            std::mem::take(&mut self.commands),
        );
        self.verts.clear();
        self.gradients
            .release_old_gradients(&mut self.images, &mut self.renderer);
        self.release_shadow_images();
        if let Some(atlas) = self.ephemeral_glyph_atlas.take() {
            atlas.clear(self);
        }
        command_buffer
    }

    pub fn screenshot(&mut self) -> Result<ImgVec<RGBA8>, ErrorKind> {
        self.renderer.screenshot()
    }

    pub fn save(&mut self) {
        let state = self.state_stack.last().map_or_else(State::default, |state| *state);

        self.state_stack.push(state);
    }

    pub fn restore(&mut self) {
        if self.state_stack.len() > 1 {
            self.state_stack.pop();
        } else {
            self.reset();
        }
    }

    pub fn reset(&mut self) {
        *self.state_mut() = State::default();
    }

    pub fn save_with(&mut self, mut callback: impl FnMut(&mut Self)) {
        self.save();

        callback(self);

        self.restore();
    }

    pub fn set_global_alpha(&mut self, alpha: f32) {
        self.state_mut().alpha = alpha;
    }

    pub fn global_composite_operation(&mut self, op: CompositeOperation) {
        self.state_mut().composite_operation = CompositeOperationState::new(op);
    }

    pub fn global_composite_blend_func(&mut self, src_factor: BlendFactor, dst_factor: BlendFactor) {
        self.global_composite_blend_func_separate(src_factor, dst_factor, src_factor, dst_factor);
    }

    pub fn global_composite_blend_func_separate(
        &mut self,
        src_rgb: BlendFactor,
        dst_rgb: BlendFactor,
        src_alpha: BlendFactor,
        dst_alpha: BlendFactor,
    ) {
        self.state_mut().composite_operation = CompositeOperationState {
            src_rgb,
            src_alpha,
            dst_rgb,
            dst_alpha,
        }
    }

    pub fn set_render_target(&mut self, target: RenderTarget) {
        if self.current_render_target != target {
            self.append_cmd(Command::new(CommandType::SetRenderTarget(target)));
            self.current_render_target = target;
        }
    }

    fn append_cmd(&mut self, cmd: Command) {
        self.commands.push(cmd);
    }

    pub fn create_image_empty(
        &mut self,
        width: usize,
        height: usize,
        format: PixelFormat,
        flags: ImageFlags,
    ) -> Result<ImageId, ErrorKind> {
        let info = ImageInfo::new(flags, width, height, format);

        self.images.alloc(&mut self.renderer, info)
    }

    pub fn create_image_from_native_texture(
        &mut self,
        texture: T::NativeTexture,
        info: ImageInfo,
    ) -> Result<ImageId, ErrorKind> {
        self.images.register_native_texture(&mut self.renderer, texture, info)
    }

    pub fn create_image<'a, S: Into<ImageSource<'a>>>(
        &mut self,
        src: S,
        flags: ImageFlags,
    ) -> Result<ImageId, ErrorKind> {
        let src = src.into();
        src.check_data()?;
        let size = src.dimensions();
        let id = self.create_image_empty(size.width, size.height, src.format(), flags)?;
        self.images.update(&mut self.renderer, id, src, 0, 0)?;
        Ok(id)
    }

    pub fn get_native_texture(&self, id: ImageId) -> Result<T::NativeTexture, ErrorKind> {
        self.get_image(id)
            .ok_or(ErrorKind::ImageIdNotFound)
            .and_then(|image| self.renderer.get_native_texture(image))
    }

    pub fn get_image(&self, id: ImageId) -> Option<&T::Image> {
        self.images.get(id)
    }

    pub fn update_image<'a, S: Into<ImageSource<'a>>>(
        &mut self,
        id: ImageId,
        src: S,
        x: usize,
        y: usize,
    ) -> Result<(), ErrorKind> {
        self.images.update(&mut self.renderer, id, src.into(), x, y)
    }

    pub fn delete_image(&mut self, id: ImageId) {
        self.images.remove(&mut self.renderer, id);
    }

    pub fn image_info(&self, id: ImageId) -> Result<ImageInfo, ErrorKind> {
        if let Some(info) = self.images.info(id) {
            Ok(info)
        } else {
            Err(ErrorKind::ImageIdNotFound)
        }
    }

    pub fn image_size(&self, id: ImageId) -> Result<(usize, usize), ErrorKind> {
        let info = self.image_info(id)?;
        Ok((info.width(), info.height()))
    }

    pub fn filter_image(&mut self, target_image: ImageId, filter: ImageFilter, source_image: ImageId) {
        let Ok((image_width, image_height)) = self.image_size(source_image) else {
            return;
        };

        let mut cmd = Command::new(CommandType::RenderFilteredImage { target_image, filter });
        cmd.image = Some(source_image);

        let vertex_offset = self.verts.len();

        let image_width = image_width as f32;
        let image_height = image_height as f32;

        let quad_x0 = 0.0;
        let quad_y0 = -image_height;
        let quad_x1 = image_width;
        let quad_y1 = image_height;

        let texture_x0 = -(image_width / 2.);
        let texture_y0 = -(image_height / 2.);
        let texture_x1 = (image_width) / 2.;
        let texture_y1 = (image_height) / 2.;

        self.verts.push(Vertex::new(quad_x0, quad_y0, texture_x0, texture_y0));
        self.verts.push(Vertex::new(quad_x1, quad_y1, texture_x1, texture_y1));
        self.verts.push(Vertex::new(quad_x1, quad_y0, texture_x1, texture_y0));
        self.verts.push(Vertex::new(quad_x0, quad_y0, texture_x0, texture_y0));
        self.verts.push(Vertex::new(quad_x0, quad_y1, texture_x0, texture_y1));
        self.verts.push(Vertex::new(quad_x1, quad_y1, texture_x1, texture_y1));

        cmd.triangles_verts = Some((vertex_offset, 6));

        self.append_cmd(cmd)
    }

    pub fn reset_transform(&mut self) {
        self.state_mut().transform = Transform2D::identity();
    }

    #[allow(clippy::many_single_char_names)]
    pub fn set_transform(&mut self, transform: &Transform2D) {
        self.state_mut().transform.premultiply(transform);
    }

    pub fn translate(&mut self, x: f32, y: f32) {
        let t = Transform2D::translation(x, y);
        self.state_mut().transform.premultiply(&t);
    }

    pub fn rotate(&mut self, angle: f32) {
        let t = Transform2D::rotation(angle);
        self.state_mut().transform.premultiply(&t);
    }

    pub fn scale(&mut self, x: f32, y: f32) {
        let t = Transform2D::scaling(x, y);
        self.state_mut().transform.premultiply(&t);
    }

    pub fn transform(&self) -> Transform2D {
        self.state().transform
    }

    pub fn scissor(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.rounded_scissor(x, y, w, h, 0.0);
    }

    pub fn rounded_scissor(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let state = self.state_mut();

        let w = w.max(0.0);
        let h = h.max(0.0);

        let mut transform = Transform2D::translation(x + w * 0.5, y + h * 0.5);
        transform *= state.transform;
        state.scissor.transform = transform;

        state.scissor.extent = Some([w * 0.5, h * 0.5]);
        state.scissor.radius = r.max(0.0).min(w * 0.5).min(h * 0.5);
    }

    pub fn intersect_scissor(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.intersect_rounded_scissor(x, y, w, h, 0.0);
    }

    pub fn intersect_rounded_scissor(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let tolerance = self.dist_tol;
        let state = self.state_mut();

        if state.scissor.extent.is_none() {
            self.rounded_scissor(x, y, w, h, r);
            return;
        }

        let extent = state.scissor.extent.unwrap();

        let Transform2D([a, b, c, d, tx, ty]) = state.scissor.transform / state.transform;

        let ex = extent[0];
        let ey = extent[1];

        let tex = ex * a.abs() + ey * c.abs();
        let tey = ex * b.abs() + ey * d.abs();

        let rect = Rect::new(tx - tex, ty - tey, tex * 2.0, tey * 2.0);
        let res = rect.intersect(Rect::new(x, y, w, h));

        let requested = Rect::new(x, y, w, h);
        let requested_contains_existing = requested.x <= rect.x + tolerance
            && requested.y <= rect.y + tolerance
            && rect.x + rect.w <= requested.x + requested.w + tolerance
            && rect.y + rect.h <= requested.y + requested.h + tolerance;
        if r <= 0.0 && state.scissor.radius > 0.0 && requested_contains_existing {
            return;
        }

        if r <= 0.0 && state.scissor.radius > 0.0 {
            let radius = state.scissor.radius;
            let contains_point = |x: f32, y: f32| {
                let left = rect.x + radius;
                let right = rect.x + rect.w - radius;
                let top = rect.y + radius;
                let bottom = rect.y + rect.h - radius;
                let dx = if x < left {
                    left - x
                } else if x > right {
                    x - right
                } else {
                    0.0
                };
                let dy = if y < top {
                    top - y
                } else if y > bottom {
                    y - bottom
                } else {
                    0.0
                };
                dx * dx + dy * dy <= (radius + tolerance) * (radius + tolerance)
            };
            let rounded_contains_requested = contains_point(requested.x, requested.y)
                && contains_point(requested.x + requested.w, requested.y)
                && contains_point(requested.x, requested.y + requested.h)
                && contains_point(requested.x + requested.w, requested.y + requested.h);
            if rounded_contains_requested {
                self.scissor(res.x, res.y, res.w, res.h);
            } else {
                self.rounded_scissor(res.x, res.y, res.w, res.h, radius);
            }
            return;
        }

        let contains_requested = rect.x <= requested.x + tolerance
            && rect.y <= requested.y + tolerance
            && requested.x + requested.w <= rect.x + rect.w + tolerance
            && requested.y + requested.h <= rect.y + rect.h + tolerance;
        if contains_requested {
            self.rounded_scissor(requested.x, requested.y, requested.w, requested.h, r);
        } else {
            self.scissor(res.x, res.y, res.w, res.h);
        }
    }

    pub fn contains_point(&self, path: &Path, x: f32, y: f32, fill_rule: FillRule) -> bool {
        let transform = self.state().transform;

        let path_cache = path.cache(&transform, self.tess_tol, self.dist_tol);

        if path_cache.bounds.maxx < 0.0
            || path_cache.bounds.minx > self.width() as f32
            || path_cache.bounds.maxy < 0.0
            || path_cache.bounds.miny > self.height() as f32
        {
            return false;
        }

        path_cache.contains_point(x, y, fill_rule)
    }

    pub fn fill_path(&mut self, path: &Path, paint: &Paint) {
        self.fill_path_internal(path, &paint.flavor, paint.shape_anti_alias, paint.fill_rule);
    }

    fn fill_path_internal(&mut self, path: &Path, paint_flavor: &PaintFlavor, anti_alias: bool, fill_rule: FillRule) {
        let mut paint_flavor = paint_flavor.clone();
        let transform = self.state().transform;

        let canvas_width = self.width();
        let canvas_height = self.height();

        if self.shadow_enabled() {
            let bounds = {
                let cache = path.cache(&transform, self.tess_tol, self.dist_tol);
                cache.bounds
            };
            if self.shadow_could_be_visible(bounds) {
                let path = path.clone();
                let shadow_flavor = paint_flavor.clone();
                self.render_shadow(bounds, move |canvas| {
                    canvas.fill_path_internal(&path, &shadow_flavor, anti_alias, fill_rule);
                });
            }
        }

        let mut path_cache = path.cache(&transform, self.tess_tol, self.dist_tol);

        if path_cache.bounds.maxx < 0.0
            || path_cache.bounds.minx > canvas_width as f32
            || path_cache.bounds.maxy < 0.0
            || path_cache.bounds.miny > canvas_height as f32
        {
            return;
        }

        paint_flavor.mul_alpha(self.state().alpha);

        let scissor = self.state().scissor;

        let fringe_width = if anti_alias { self.fringe_width } else { 0.0 };
        path_cache.expand_fill(fringe_width, LineJoin::Miter, 2.4);

        if let (Some(path_rect), Some(scissor_rect), true) = (
            path_cache.path_fill_is_rect(),
            scissor.as_rect(canvas_width as f32, canvas_height as f32),
            paint_flavor.is_straight_tinted_image(anti_alias),
        ) {
            if scissor_rect.contains_rect(&path_rect) {
                self.render_unclipped_image_blit(&path_rect, &transform, &paint_flavor);
            } else if let Some(intersection) = path_rect.intersection(&scissor_rect) {
                self.render_unclipped_image_blit(&intersection, &transform, &paint_flavor);
            }

            return;
        }

        let flavor = if path_cache.contours.len() == 1 && path_cache.contours[0].convexity == Convexity::Convex {
            let params = Params::new(
                &self.images,
                &transform,
                &paint_flavor,
                &GlyphTexture::default(),
                &scissor,
                self.fringe_width,
                self.fringe_width,
                -1.0,
            );

            CommandType::ConvexFill { params }
        } else {
            let stencil_params = Params {
                stroke_thr: -1.0,
                shader_type: ShaderType::Stencil,
                ..Params::default()
            };

            let fill_params = Params::new(
                &self.images,
                &transform,
                &paint_flavor,
                &GlyphTexture::default(),
                &scissor,
                self.fringe_width,
                self.fringe_width,
                -1.0,
            );

            CommandType::ConcaveFill {
                stencil_params,
                fill_params,
            }
        };

        let mut cmd = Command::new(flavor);
        cmd.fill_rule = fill_rule;
        cmd.composite_operation = self.state().composite_operation;

        if let PaintFlavor::Image { id, .. } = paint_flavor {
            cmd.image = Some(id);
        } else if let Some(paint::GradientColors::MultiStop { stops }) = paint_flavor.gradient_colors() {
            cmd.image = self
                .gradients
                .lookup_or_add(stops, &mut self.images, &mut self.renderer)
                .ok();
        }

        let mut offset = self.verts.len();

        cmd.drawables.reserve_exact(path_cache.contours.len());
        for contour in &path_cache.contours {
            let mut drawable = Drawable::default();

            if !contour.fill.is_empty() {
                drawable.fill_verts = Some((offset, contour.fill.len()));
                self.verts.extend_from_slice(&contour.fill);
                offset += contour.fill.len();
            }

            if !contour.stroke.is_empty() {
                drawable.stroke_verts = Some((offset, contour.stroke.len()));
                self.verts.extend_from_slice(&contour.stroke);
                offset += contour.stroke.len();
            }

            cmd.drawables.push(drawable);
        }

        if let CommandType::ConcaveFill { .. } = cmd.cmd_type {
            self.verts.push(Vertex::new(
                path_cache.bounds.maxx + fringe_width,
                path_cache.bounds.maxy + fringe_width,
                0.5,
                1.0,
            ));
            self.verts.push(Vertex::new(
                path_cache.bounds.maxx + fringe_width,
                path_cache.bounds.miny - fringe_width,
                0.5,
                1.0,
            ));
            self.verts.push(Vertex::new(
                path_cache.bounds.minx - fringe_width,
                path_cache.bounds.maxy + fringe_width,
                0.5,
                1.0,
            ));
            self.verts.push(Vertex::new(
                path_cache.bounds.minx - fringe_width,
                path_cache.bounds.miny,
                0.5,
                1.0,
            ));

            cmd.triangles_verts = Some((offset, 4));
        }

        self.append_cmd(cmd);
    }

    pub fn stroke_path(&mut self, path: &Path, paint: &Paint) {
        self.stroke_path_internal(path, &paint.flavor, paint.shape_anti_alias, &paint.stroke);
    }

    fn stroke_path_internal(
        &mut self,
        path: &Path,
        paint_flavor: &PaintFlavor,
        anti_alias: bool,
        stroke: &StrokeSettings,
    ) {
        let mut paint_flavor = paint_flavor.clone();
        let transform = self.state().transform;

        if !stroke.line_dash.is_empty() {
            let dashed_path = path.dashed_with_tolerance(&stroke.line_dash, stroke.line_dash_offset, self.tess_tol);
            if dashed_path.is_empty() {
                return;
            }

            let mut solid_stroke = stroke.clone();
            solid_stroke.line_dash.clear();
            solid_stroke.line_dash_offset = 0.0;
            self.stroke_path_internal(&dashed_path, &paint_flavor, anti_alias, &solid_stroke);
            return;
        }

        if self.shadow_enabled() {
            let centerline = {
                let cache = path.cache(&transform, self.tess_tol, self.dist_tol);
                cache.bounds
            };
            let half = (stroke.line_width * transform.average_scale()).max(self.fringe_width) * 0.5;
            let mut bounds = centerline;
            bounds.minx -= half;
            bounds.miny -= half;
            bounds.maxx += half;
            bounds.maxy += half;
            if self.shadow_could_be_visible(bounds) {
                let path = path.clone();
                let stroke = stroke.clone();
                let shadow_flavor = paint_flavor.clone();
                self.render_shadow(bounds, move |canvas| {
                    canvas.stroke_path_internal(&path, &shadow_flavor, anti_alias, &stroke);
                });
            }
        }

        let mut path_cache = path.cache(&transform, self.tess_tol, self.dist_tol);

        if path_cache.bounds.maxx < 0.0
            || path_cache.bounds.minx > self.width() as f32
            || path_cache.bounds.maxy < 0.0
            || path_cache.bounds.miny > self.height() as f32
        {
            return;
        }

        let scissor = self.state().scissor;

        let mut line_width = (stroke.line_width * transform.average_scale()).max(0.0);

        if line_width < self.fringe_width {
            let alpha = (line_width / self.fringe_width).clamp(0.0, 1.0);

            paint_flavor.mul_alpha(alpha * alpha);
            line_width = self.fringe_width;
        }

        paint_flavor.mul_alpha(self.state().alpha);

        let fringe_with = if anti_alias { self.fringe_width } else { 0.0 };
        path_cache.expand_stroke(
            line_width * 0.5,
            fringe_with,
            stroke.line_cap_start,
            stroke.line_cap_end,
            stroke.line_join,
            stroke.miter_limit,
            self.tess_tol,
        );

        let params = Params::new(
            &self.images,
            &transform,
            &paint_flavor,
            &GlyphTexture::default(),
            &scissor,
            line_width,
            self.fringe_width,
            -1.0,
        );

        let flavor = if stroke.stencil_strokes {
            let params2 = Params::new(
                &self.images,
                &transform,
                &paint_flavor,
                &GlyphTexture::default(),
                &scissor,
                line_width,
                self.fringe_width,
                1.0 - 0.5 / 255.0,
            );

            CommandType::StencilStroke {
                params1: params,
                params2,
            }
        } else {
            CommandType::Stroke { params }
        };

        let mut cmd = Command::new(flavor);
        cmd.composite_operation = self.state().composite_operation;

        if let PaintFlavor::Image { id, .. } = paint_flavor {
            cmd.image = Some(id);
        } else if let Some(paint::GradientColors::MultiStop { stops }) = paint_flavor.gradient_colors() {
            cmd.image = self
                .gradients
                .lookup_or_add(stops, &mut self.images, &mut self.renderer)
                .ok();
        }

        let mut offset = self.verts.len();

        cmd.drawables.reserve_exact(path_cache.contours.len());
        for contour in &path_cache.contours {
            let mut drawable = Drawable::default();

            if !contour.stroke.is_empty() {
                drawable.stroke_verts = Some((offset, contour.stroke.len()));
                self.verts.extend_from_slice(&contour.stroke);
                offset += contour.stroke.len();
            }

            cmd.drawables.push(drawable);
        }

        self.append_cmd(cmd);
    }

    fn shadow_enabled(&self) -> bool {
        let state = self.state();
        state.shadow_color.a > 0.0
            && (state.shadow_blur != 0.0 || state.shadow_offset[0] != 0.0 || state.shadow_offset[1] != 0.0)
    }

    fn shadow_could_be_visible(&self, shape_bounds: Bounds) -> bool {
        let state = self.state();
        let [ox, oy] = state.shadow_offset;
        let spread = (state.shadow_blur / 2.0 * 3.0).ceil();
        let minx = shape_bounds.minx + ox - spread;
        let miny = shape_bounds.miny + oy - spread;
        let maxx = shape_bounds.maxx + ox + spread;
        let maxy = shape_bounds.maxy + oy + spread;
        maxx >= 0.0 && minx <= self.width() as f32 && maxy >= 0.0 && miny <= self.height() as f32
    }

    fn render_shadow(&mut self, shape_bounds: Bounds, draw_coverage: impl FnOnce(&mut Self)) {
        if shape_bounds.maxx <= shape_bounds.minx || shape_bounds.maxy <= shape_bounds.miny {
            return;
        }

        let state = *self.state();
        let shadow_color = state.shadow_color;

        let sigma = state.shadow_blur / 2.0;

        let [txx, txy] = state.shadow_offset;

        let pad = (sigma * 3.0).ceil() + 2.0;

        let minx = (shape_bounds.minx - pad).floor();
        let miny = (shape_bounds.miny - pad).floor();
        let maxx = (shape_bounds.maxx + pad).ceil();
        let maxy = (shape_bounds.maxy + pad).ceil();

        let width = (maxx - minx) as usize;
        let height = (maxy - miny) as usize;

        if width == 0 || height == 0 || width > 8192 || height > 8192 {
            return;
        }

        let image_flags = ImageFlags::PREMULTIPLIED | ImageFlags::FLIP_Y;
        let Ok(coverage_image) = self.create_image_empty(width, height, PixelFormat::Rgba8, image_flags) else {
            return;
        };
        let blurred_image = if sigma >= 0.01 {
            match self.create_image_empty(width, height, PixelFormat::Rgba8, image_flags) {
                Ok(image) => Some(image),
                Err(_) => {
                    self.delete_image(coverage_image);
                    return;
                }
            }
        } else {
            None
        };

        let previous_target = self.current_render_target;

        self.save();
        self.set_render_target(RenderTarget::Image(coverage_image));
        self.clear_rect(0, 0, width as u32, height as u32, Color::rgbaf(0.0, 0.0, 0.0, 0.0));

        let mut coverage_transform = Transform2D::translation(-minx, -miny);
        coverage_transform.premultiply(&state.transform);
        self.state_mut().transform = coverage_transform;
        self.state_mut().alpha = 1.0;
        self.state_mut().scissor = Scissor::default();
        self.state_mut().composite_operation = CompositeOperationState::default();
        self.state_mut().shadow_color = Color::rgbaf(0.0, 0.0, 0.0, 0.0);

        draw_coverage(self);

        self.state_mut().transform = Transform2D::identity();
        self.state_mut().composite_operation = CompositeOperationState::new(CompositeOperation::SourceIn);
        let mut mask_rect = Path::new();
        mask_rect.rect(0.0, 0.0, width as f32, height as f32);
        self.fill_path_internal(&mask_rect, &PaintFlavor::Color(shadow_color), false, FillRule::NonZero);

        self.restore();

        let source_image = if let Some(blurred_image) = blurred_image {
            self.filter_image(blurred_image, ImageFilter::GaussianBlur { sigma }, coverage_image);
            blurred_image
        } else {
            coverage_image
        };

        self.set_render_target(previous_target);

        let dst_x = minx + txx;
        let dst_y = miny + txy;

        let tint = Color::rgbaf(1.0, 1.0, 1.0, state.alpha);
        let mut shadow_paint = Paint::image_tint(source_image, dst_x, dst_y, width as f32, height as f32, 0.0, tint);
        shadow_paint.set_anti_alias(false);

        let saved_transform = self.state().transform;
        let saved_alpha = self.state().alpha;
        self.state_mut().transform = Transform2D::identity();
        self.state_mut().alpha = 1.0;
        self.state_mut().shadow_color = Color::rgbaf(0.0, 0.0, 0.0, 0.0);

        let mut shadow_rect = Path::new();
        shadow_rect.rect(dst_x, dst_y, width as f32, height as f32);
        self.fill_path_internal(&shadow_rect, &shadow_paint.flavor, false, FillRule::NonZero);

        self.state_mut().transform = saved_transform;
        self.state_mut().alpha = saved_alpha;
        self.state_mut().shadow_color = shadow_color;

        self.shadow_images.push(coverage_image);
        if let Some(blurred_image) = blurred_image {
            self.shadow_images.push(blurred_image);
        }
    }

    fn release_shadow_images(&mut self) {
        for id in std::mem::take(&mut self.shadow_images) {
            self.images.remove(&mut self.renderer, id);
        }
    }

    fn render_unclipped_image_blit(&mut self, target_rect: &Rect, transform: &Transform2D, paint_flavor: &PaintFlavor) {
        let scissor = self.state().scissor;

        let mut params = Params::new(
            &self.images,
            transform,
            paint_flavor,
            &GlyphTexture::default(),
            &scissor,
            0.,
            0.,
            -1.0,
        );
        params.shader_type = ShaderType::TextureCopyUnclipped;

        let mut cmd = Command::new(CommandType::Triangles { params });
        cmd.composite_operation = self.state().composite_operation;

        let x0 = target_rect.x;
        let y0 = target_rect.y;
        let x1 = x0 + target_rect.w;
        let y1 = y0 + target_rect.h;

        let (p0, p1) = (x0, y0);
        let (p2, p3) = (x1, y0);
        let (p4, p5) = (x1, y1);
        let (p6, p7) = (x0, y1);

        let mut to_texture_space_transform = Transform2D::scaling(1. / params.extent[0], 1. / params.extent[1]);
        to_texture_space_transform.premultiply(&Transform2D([
            params.paint_mat[0],
            params.paint_mat[1],
            params.paint_mat[4],
            params.paint_mat[5],
            params.paint_mat[8],
            params.paint_mat[9],
        ]));

        let verts = [(p0, p1), (p4, p5), (p2, p3), (p0, p1), (p6, p7), (p4, p5)].map(|(x, y)| {
            let (s, t) = to_texture_space_transform.transform_point(x, y);
            Vertex::new(x, y, s, t)
        });

        if let &PaintFlavor::Image { id, .. } = paint_flavor {
            cmd.image = Some(id);
        }

        cmd.triangles_verts = Some((self.verts.len(), verts.len()));
        self.append_cmd(cmd);

        self.verts.extend_from_slice(&verts);
    }

    pub fn fill_glyph_run(
        &mut self,
        font_id: FontId,
        normalized_coords: &[i16],
        glyphs: impl IntoIterator<Item = PositionedGlyph>,
        paint: &Paint,
    ) -> Result<(), ErrorKind> {
        self.draw_glyph_run(glyphs, paint, font_id, normalized_coords, RenderMode::Fill)
    }

    pub fn stroke_glyph_run(
        &mut self,
        font_id: FontId,
        normalized_coords: &[i16],
        glyphs: impl IntoIterator<Item = PositionedGlyph>,
        paint: &Paint,
    ) -> Result<(), ErrorKind> {
        self.draw_glyph_run(glyphs, paint, font_id, normalized_coords, RenderMode::Stroke)
    }

    pub fn draw_glyph_commands(&mut self, draw_commands: GlyphDrawCommands, paint: &Paint) {
        let transform = self.state().transform;
        let create_vertices = |quads: &Vec<text::Quad>| {
            let mut verts = Vec::with_capacity(quads.len() * 6);

            for quad in quads {
                let left = quad.x0;
                let right = quad.x1;
                let top = quad.y0;
                let bottom = quad.y1;

                let (p0, p1) = transform.transform_point(left, top);
                let (p2, p3) = transform.transform_point(right, top);
                let (p4, p5) = transform.transform_point(right, bottom);
                let (p6, p7) = transform.transform_point(left, bottom);

                verts.push(Vertex::new(p0, p1, quad.s0, quad.t0));
                verts.push(Vertex::new(p4, p5, quad.s1, quad.t1));
                verts.push(Vertex::new(p2, p3, quad.s1, quad.t0));
                verts.push(Vertex::new(p0, p1, quad.s0, quad.t0));
                verts.push(Vertex::new(p6, p7, quad.s0, quad.t1));
                verts.push(Vertex::new(p4, p5, quad.s1, quad.t1));
            }
            verts
        };

        let mut paint_flavor = paint.flavor.clone();
        paint_flavor.mul_alpha(self.state().alpha);

        for cmd in draw_commands.alpha_glyphs {
            let verts = create_vertices(&cmd.quads);

            self.render_triangles(&verts, &transform, &paint_flavor, GlyphTexture::AlphaMask(cmd.image_id));
        }

        for cmd in draw_commands.color_glyphs {
            let verts = create_vertices(&cmd.quads);

            self.render_triangles(
                &verts,
                &transform,
                &paint_flavor,
                GlyphTexture::ColorTexture(cmd.image_id),
            );
        }
    }

    fn draw_glyph_run(
        &mut self,
        glyphs: impl IntoIterator<Item = PositionedGlyph>,
        paint: &Paint,
        font_id: FontId,
        normalized_coords: &[i16],
        render_mode: RenderMode,
    ) -> Result<(), ErrorKind> {
        let text_context = self.text_context.clone();
        let mut text_context = text_context.borrow_mut();

        #[derive(Clone, Copy)]
        enum Rasterization {
            Path,
            Atlas,
            ScaledAtlas {
                scale: f32,
                true_scale: f32,
                translation: (f32, f32),
            },
        }

        let rasterization = match self.state().transform.as_uniform_scale_translation(1e-3) {
            None => Rasterization::Path,
            Some((true_scale, tx, ty)) => {
                let scale = geometry::quantize(true_scale, 1.0 / 16.0).max(1.0 / 16.0);
                if paint.text.font_size * scale > 92.0 {
                    Rasterization::Path
                } else if scale == 1.0 {
                    Rasterization::Atlas
                } else if matches!(paint.flavor, PaintFlavor::Color(_)) {
                    Rasterization::ScaledAtlas {
                        scale,
                        true_scale,
                        translation: (tx, ty),
                    }
                } else {
                    Rasterization::Path
                }
            }
        };

        let need_direct_rendering = matches!(rasterization, Rasterization::Path);
        let effective_scale = match rasterization {
            Rasterization::ScaledAtlas { scale, .. } => scale,
            _ => 1.0,
        };
        let effective_font_size = paint.text.font_size * effective_scale;

        let mut stroke = paint.stroke.clone();
        stroke.line_width *= effective_scale;

        let Some(font) = text_context.font_mut(font_id) else {
            return Err(ErrorKind::NoFontFound);
        };

        let font_face = font.face_ref_with_normalized_coords(normalized_coords);

        let mut color_glyphs = Vec::new();

        let glyphs_it = glyphs.into_iter();
        let non_color_glyphs = glyphs_it
            .filter(|glyph| {
                if font
                    .glyph(&font_face, glyph.glyph_id, normalized_coords)
                    .is_some_and(|glyph| glyph.path.is_none())
                {
                    color_glyphs.push(glyph.clone());

                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();

        let scaled = |g: &PositionedGlyph| PositionedGlyph {
            x: g.x * effective_scale,
            y: g.y * effective_scale,
            glyph_id: g.glyph_id,
        };

        let mut draw_commands = if need_direct_rendering {
            text::render_direct(
                self,
                font,
                non_color_glyphs.into_iter(),
                &paint.flavor,
                paint.shape_anti_alias,
                &stroke,
                paint.text.font_size,
                render_mode,
                normalized_coords,
            )?;
            GlyphDrawCommands::default()
        } else {
            self.glyph_atlas.clone().render_atlas(
                self,
                font_id,
                font,
                &font_face,
                non_color_glyphs.iter().map(scaled),
                effective_font_size,
                stroke.line_width,
                render_mode,
                normalized_coords,
            )?
        };

        if !color_glyphs.is_empty() {
            let color_commands = {
                let atlas = if need_direct_rendering {
                    self.ephemeral_glyph_atlas
                        .get_or_insert_with(|| Rc::new(GlyphAtlas::new(&self.text_context)))
                        .clone()
                } else {
                    self.glyph_atlas.clone()
                };

                if need_direct_rendering {
                    atlas.render_atlas(
                        self,
                        font_id,
                        font,
                        &font_face,
                        color_glyphs.into_iter(),
                        paint.text.font_size,
                        stroke.line_width,
                        render_mode,
                        normalized_coords,
                    )?
                } else {
                    atlas.render_atlas(
                        self,
                        font_id,
                        font,
                        &font_face,
                        color_glyphs.iter().map(scaled),
                        effective_font_size,
                        stroke.line_width,
                        render_mode,
                        normalized_coords,
                    )?
                }
            };

            draw_commands.alpha_glyphs.extend(color_commands.alpha_glyphs);
            draw_commands.color_glyphs.extend(color_commands.color_glyphs);
        }

        match rasterization {
            Rasterization::ScaledAtlas {
                scale,
                true_scale,
                translation: (tx, ty),
            } => {
                let residual = true_scale / scale;
                let saved = self.state().transform;
                self.state_mut().transform = Transform2D::new(residual, 0.0, 0.0, residual, tx.round(), ty.round());
                self.draw_glyph_commands(draw_commands, paint);
                self.state_mut().transform = saved;
            }
            Rasterization::Atlas => {
                let saved = self.state().transform;
                let mut snapped = saved;
                snapped.0[4] = snapped.0[4].round();
                snapped.0[5] = snapped.0[5].round();
                self.state_mut().transform = snapped;
                self.draw_glyph_commands(draw_commands, paint);
                self.state_mut().transform = saved;
            }
            _ => self.draw_glyph_commands(draw_commands, paint),
        }

        Ok(())
    }

    fn render_triangles(
        &mut self,
        verts: &[Vertex],
        transform: &Transform2D,
        paint_flavor: &PaintFlavor,
        glyph_texture: GlyphTexture,
    ) {
        let scissor = self.state().scissor;

        let params = Params::new(
            &self.images,
            transform,
            paint_flavor,
            &glyph_texture,
            &scissor,
            1.0,
            self.fringe_width,
            -1.0,
        );

        let mut cmd = Command::new(CommandType::Triangles { params });
        cmd.composite_operation = self.state().composite_operation;
        cmd.glyph_texture = glyph_texture;

        if let &PaintFlavor::Image { id, .. } = paint_flavor {
            cmd.image = Some(id);
        } else if let Some(paint::GradientColors::MultiStop { stops }) = paint_flavor.gradient_colors() {
            cmd.image = self
                .gradients
                .lookup_or_add(stops, &mut self.images, &mut self.renderer)
                .ok();
        }

        cmd.triangles_verts = Some((self.verts.len(), verts.len()));
        self.append_cmd(cmd);

        self.verts.extend_from_slice(verts);
    }

    fn state(&self) -> &State {
        self.state_stack.last().unwrap()
    }

    fn state_mut(&mut self) -> &mut State {
        self.state_stack.last_mut().unwrap()
    }
}

impl<T: Renderer> Drop for Canvas<T> {
    fn drop(&mut self) {
        self.images.clear(&mut self.renderer);
    }
}

#[derive(Clone, Debug)]
pub struct PositionedGlyph {
    pub x: f32,
    pub y: f32,
    pub glyph_id: u16,
}

pub use imgref;
pub use rgb;
