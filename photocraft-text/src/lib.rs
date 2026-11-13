//! Photoshop text engine reimplementation.
//!
//! The PSD stores, per text layer, the Adobe engine's own output (EngineData).
//! This crate reconstructs the engine itself: sfnt font access, glyph advances,
//! kerning (metrics and optical), tracking, leading, justification and the
//! point/area text layout that produces those numbers.

pub mod area;
pub mod atlas;
pub mod composer;
pub mod engine_data;
pub mod error;
pub mod font;
pub mod hyphen;
pub mod layer_render;
pub mod layout;
pub mod optical;
pub mod outline;
pub mod psd;
pub mod raster;
pub mod runs;
pub mod txt2;
pub mod typography;
pub mod warp;

pub use error::{Error, Result};
pub use font::{Face, GlyphMetrics};
