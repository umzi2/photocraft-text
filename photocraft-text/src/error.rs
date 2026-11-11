//! Errors for the text engine.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("read past end of data: needed {needed}, length {len}")]
    OutOfBounds { needed: usize, len: usize },

    #[error("not a supported font: {0}")]
    BadFont(&'static str),

    #[error("missing font table: {0}")]
    MissingTable(&'static str),

    #[error("unsupported {what} format: {value}")]
    Unsupported { what: &'static str, value: i64 },

    #[error("malformed EngineData: {0}")]
    EngineData(&'static str),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("Txt2: {0}")]
    Txt2(String),

    #[error("atlas: {0}")]
    Atlas(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
