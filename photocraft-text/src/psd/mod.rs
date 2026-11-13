//! PSD text-layer authoring with caller-owned fonts and glyph atlases.
//!
//! Edits the first text layer and the document's global `Txt2` block, not
//! arbitrary layers in a multi-text-layer document. Callers load every font
//! and optional atlas into a [`crate::layer_render::FaceStore`].
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
//!     aa: None,
//!     frame: None,
//!     atlas: None,
//!     fractional: None,
//! };
//! author(&mut file, &spec, &store)?;
//! let bytes = file.to_bytes()?;
//! # Ok(bytes)
//! # }
//! ```

mod author;
mod engine_data;
mod error;
mod model;
mod pixels;

pub use author::author;
pub use engine_data::with_engine_data;
pub use error::Error;
pub use model::{LayerSpec, ParagraphSpec, RunSpec};
pub use pixels::refresh_text_layer;
