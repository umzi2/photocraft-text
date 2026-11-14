//! Multi-run (per-character) styling.
//!
//! A text layer is a sequence of style runs: font, size, colour, tracking,
//! kerning, baseline shift, horizontal/vertical scale and faux styles can change
//! mid-string. This module owns the run model and the shaping that consumes it;
//! `layout` and `area` stay single-run and are reused per run.
//!
//! Run lengths are UTF-16 code units, as in EngineData's `RunLengthArray`.

mod engine;
mod model;
mod shape;

pub use engine::{
    antialias, apply_paragraph_formats, apply_paragraph_runs, apply_style_runs, engine_language,
    ensure_engine_font, from_engine_data, from_engine_data_with_text,
    paragraph_formats_from_engine_data, paragraphs_from_engine_data, set_antialias,
    set_fractional_glyph_widths, to_engine_data, txt2_language,
};
pub use model::{
    FontSet, GlyphAttrs, ParagraphFormat, ParagraphRun, RunFeature, RunStyle, SMALL_CAPS_SCALE,
    ShapedGlyph,
};
pub use shape::{shape_runs, shape_runs_attrs, shape_runs_detailed};
