//! PSD text-layer authoring with caller-owned fonts and glyph atlases.
//!
//! [`author`] edits the first text layer; [`author_layer`] edits any text layer
//! by index and writes that layer's own `Txt2` object (the one its `TySh`
//! `TextIndex` names), so a multi-text-layer document no longer has to have its
//! text model rewritten from object 0. Callers load every font and optional
//! atlas into a [`crate::layer_render::FaceStore`].
//!
//! ```no_run
//! use std::path::Path;
//! use photocraft_psd::PsdFile;
//! use photocraft_text::layer_render::FaceStore;
//! use photocraft_text::psd::{LayerSpec, RunSpec, author};
//!
//! # fn rewrite(input: &[u8], font_dir: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
//! let mut store = FaceStore::new();
//! store.load(font_dir, "NotoSans-Regular")?;
//! let mut file = PsdFile::from_bytes(input)?;
//! let spec = LayerSpec {
//!     text: "Hi".into(),
//!     runs: vec![RunSpec {
//!         len: "Hi".encode_utf16().count() + 1, // Includes the U+000D terminator.
//!         font: Some("NotoSans-Regular".into()),
//!         ..Default::default()
//!     }],
//!     paragraph: Default::default(),
//!     paragraphs: Vec::new(),
//!     aa: None,
//!     frame: None,
//!     shape: None,
//!     orientation: None,
//!     transform: None,
//!     atlas: None,
//!     fractional: None,
//!     preserve_trailing_break: false,
//! };
//! author(&mut file, &spec, &store)?;
//! let bytes = file.to_bytes()?;
//! # Ok(bytes)
//! # }
//! ```

mod author;
mod build;
mod engine_data;
mod error;
mod model;
mod path;
mod pixels;
mod read;

pub use author::{author, author_layer};
pub use build::{
    apply_engine_data, build_engine_data, build_txt2, build_tysh, build_tysh_from,
    refresh_pixels,
};
pub use engine_data::{set_frame, with_engine_data};
pub use error::Error;
pub use model::{FrameSpec, LayerSpec, ParagraphSpec, PathKnot, PathSpec, RunSpec, ShapeSpec};
pub use path::{FrameSource, PATH_FRAME, frame_kind, read_shape, to_frame_parts};
pub use pixels::refresh_text_layer;
pub use read::read_layer;
