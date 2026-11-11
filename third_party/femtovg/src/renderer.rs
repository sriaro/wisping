
use imgref::ImgVec;
use rgb::RGBA8;

use crate::{
    geometry::Position, paint::GlyphTexture, Color, CompositeOperationState, ErrorKind, FillRule, ImageFilter, ImageId,
    ImageInfo, ImageSource, ImageStore,
};

mod opengl;
pub use opengl::OpenGl;

mod params;
pub(crate) use params::Params;

#[derive(Copy, Clone, Default, Debug)]
pub struct Drawable {
    pub(crate) fill_verts: Option<(usize, usize)>,
    pub(crate) stroke_verts: Option<(usize, usize)>,
}

#[derive(Debug)]
pub enum CommandType {
    SetRenderTarget(RenderTarget),
    ClearRect {
        color: Color,
    },
    ConvexFill {
        params: Params,
    },
    ConcaveFill {
        stencil_params: Params,
        fill_params: Params,
    },
    Stroke {
        params: Params,
    },
    StencilStroke {
        params1: Params,
        params2: Params,
    },
    Triangles {
        params: Params,
    },
    RenderFilteredImage {
        target_image: ImageId,
        filter: ImageFilter,
    },
}

#[derive(Debug)]
pub struct Command {
    pub(crate) cmd_type: CommandType,
    pub(crate) drawables: Vec<Drawable>,
    pub(crate) triangles_verts: Option<(usize, usize)>,
    pub(crate) image: Option<ImageId>,
    pub(crate) glyph_texture: GlyphTexture,
    pub(crate) fill_rule: FillRule,
    pub(crate) composite_operation: CompositeOperationState,
}

impl Command {
    pub fn new(flavor: CommandType) -> Self {
        Self {
            cmd_type: flavor,
            drawables: Vec::new(),
            triangles_verts: None,
            image: None,
            glyph_texture: GlyphTexture::default(),
            fill_rule: FillRule::default(),
            composite_operation: CompositeOperationState::default(),
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum RenderTarget {
    Screen,
    Image(ImageId),
}

pub trait Renderer {
    type Image;

    type NativeTexture;

    type RenderOutput;

    type CommandBuffer;

    fn set_size(&mut self, width: u32, height: u32, dpi: f32);

    fn render(
        &mut self,
        output: impl Into<Self::RenderOutput>,
        images: &mut ImageStore<Self::Image>,
        verts: &[Vertex],
        commands: Vec<Command>,
    ) -> Self::CommandBuffer;

    fn alloc_image(&mut self, info: ImageInfo) -> Result<Self::Image, ErrorKind>;

    fn create_image_from_native_texture(
        &mut self,
        native_texture: Self::NativeTexture,
        info: ImageInfo,
    ) -> Result<Self::Image, ErrorKind>;

    fn update_image(&mut self, image: &mut Self::Image, data: ImageSource, x: usize, y: usize)
        -> Result<(), ErrorKind>;

    fn delete_image(&mut self, image: Self::Image, image_id: ImageId);

    fn screenshot(&mut self) -> Result<ImgVec<RGBA8>, ErrorKind>;
}

use bytemuck::{Pod, Zeroable};

#[derive(Copy, Clone, Debug, PartialEq, PartialOrd, Default, Pod, Zeroable)]
#[repr(C)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
}

impl Vertex {
    pub(crate) fn pos(position: Position, u: f32, v: f32) -> Self {
        let Position { x, y } = position;
        Self { x, y, u, v }
    }

    pub fn new(x: f32, y: f32, u: f32, v: f32) -> Self {
        Self { x, y, u, v }
    }

    pub fn set(&mut self, x: f32, y: f32, u: f32, v: f32) {
        *self = Self { x, y, u, v };
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Default)]
pub enum ShaderType {
    #[default]
    FillGradient,
    FillImage,
    Stencil,
    FillImageGradient,
    FilterImage,
    FillColor,
    TextureCopyUnclipped,
    FillGradientConic,
    FillImageGradientConic,
}

impl ShaderType {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::FillGradient => 0,
            Self::FillImage => 1,
            Self::Stencil => 2,
            Self::FillImageGradient => 3,
            Self::FilterImage => 4,
            Self::FillColor => 5,
            Self::TextureCopyUnclipped => 6,
            Self::FillGradientConic => 7,
            Self::FillImageGradientConic => 8,
        }
    }

    pub fn to_f32(self) -> f32 {
        self.to_u8() as f32
    }
}
