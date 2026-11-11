use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

#[derive(Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    UnknownError,
    FontParseError,
    NoFontFound,
    FontSizeTooLargeForAtlas,
    ShaderCompileError(String),
    ShaderLinkError(String),
    RenderTargetError(String),
    ImageIdNotFound,
    ImageUpdateOutOfBounds,
    ImageUpdateWithDifferentFormat,
    InvalidImageData,
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "canvas error")
    }
}

impl Error for ErrorKind {}
