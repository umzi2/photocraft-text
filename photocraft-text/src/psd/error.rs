//! Errors from PSD text-layer authoring.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("no runs")]
    NoRuns,

    #[error("run {index} has zero length")]
    ZeroLengthRun { index: usize },

    #[error("run lengths overflow the supported range")]
    RunLengthOverflow,

    #[error("runs cover {covered} units, text + terminator is {expected}")]
    RunCoverage { covered: usize, expected: usize },

    #[error("run {index} ends inside a UTF-16 surrogate pair at unit {offset}")]
    SurrogateSplit { index: usize, offset: usize },

    #[error("no Txt2 block")]
    MissingTxt2,

    #[error("no text layer")]
    MissingTextLayer,

    #[error("no TySh block")]
    MissingTySh,

    #[error("no EngineData")]
    MissingEngineData,

    #[error("truncated TySh: needed {needed} bytes, length {len}")]
    TruncatedTySh { needed: usize, len: usize },

    #[error("PSD: {0}")]
    Psd(#[from] photocraft_psd::PsdError),

    #[error("text: {0}")]
    Text(#[from] crate::Error),
}
