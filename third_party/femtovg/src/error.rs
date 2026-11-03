use std::{
    error::Error,
    ffi::NulError,
    fmt::{self, Display, Formatter},
    io,
};

#[derive(Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    UnknownError,
    GeneralError(String),
    IoError(io::Error),
    FontParseError,
    NoFontFound,
    FontInfoExtractionError,
    FontSizeTooLargeForAtlas,
    ShaderCompileError(String),
    ShaderLinkError(String),
    RenderTargetError(String),
    ImageIdNotFound,
    ImageUpdateOutOfBounds,
    ImageUpdateWithDifferentFormat,
    InvalidImageData,
    UnsupportedImageFormat,
    UnsupportedOperation,
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "canvas error")
    }
}

impl From<io::Error> for ErrorKind {
    fn from(error: io::Error) -> Self {
        Self::IoError(error)
    }
}

impl From<NulError> for ErrorKind {
    fn from(error: NulError) -> Self {
        Self::GeneralError(error.to_string())
    }
}

impl Error for ErrorKind {}
