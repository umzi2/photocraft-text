//! The legacy `Txt2` text block.
//!
//! Photoshop takes a text layer's contents from this block, not from
//! `TySh/EngineData`: patching `TySh` alone leaves the layer showing its old
//! text, while patching `Txt2` alone changes it. The block is CoolType's
//! PostScript-token format — dictionaries `<< /k v >>`, arrays `[ … ]`, UTF-16BE
//! strings with a `\xfe\xff` BOM terminated by U+000D — and it stores the text
//! together with run lengths, per-glyph advances and the text bounds.
//!
//! Same-length text edits preserve the surrounding bytes. Length-changing edits
//! rebuild the paragraph/style run tables and clear the composed-layout cache
//! so Photoshop recomposes the text model instead of trusting stale geometry.

mod antialias;
mod cache;
mod frame;
mod paragraph;
mod path;
mod runs;
mod style;
mod syntax;
mod text;

pub use antialias::{antialias_dict_path, antialias_path, set_antialias};
pub use cache::{
    clear_cache, invalidate_changed_composed_layouts, invalidate_composed_layout,
    invalidate_composed_layouts,
};
pub use frame::{FrameGeometry, frame_geometry, set_frame_geometry};
pub use paragraph::{
    Hyphenation, Justification, ParagraphProperty, paragraph_auto_leading, paragraph_count,
    paragraph_end_indent, paragraph_every_line_composer, paragraph_first_line_indent,
    paragraph_hanging_punctuation, paragraph_hyphenation, paragraph_justification,
    paragraph_property, paragraph_props_path, paragraph_range, paragraph_start_indent,
    set_paragraph_auto_leading, set_paragraph_end_indent, set_paragraph_every_line_composer,
    set_paragraph_first_line_indent, set_paragraph_hanging_punctuation, set_paragraph_hyphenation,
    set_paragraph_justification, set_paragraph_justification_code, set_paragraph_property,
    set_paragraph_property_or_insert, set_paragraph_range, set_paragraph_start_indent,
};
pub use path::{
    Step, Value, ensure_container, ensure_value, read_path, replace_container, replace_value,
    value_range,
};
pub use runs::{
    duplicate_run, run_count, run_lengths, run_path_for, set_run_layout, set_run_lengths,
};
pub use style::{
    Capitalization, KerningType, Language, OLD_STYLE_ON, RunProperty, Strikethrough, Underline,
    ensure_font, font_count, run_alternate_ligatures, run_capitalization, run_faux_bold,
    run_faux_italic, run_fill_color, run_kerning_type, run_language, run_language_code,
    run_ligatures, run_no_break, run_old_style, run_property, run_strikethrough, run_stroke_color,
    run_underline, set_run_alternate_ligatures, set_run_capitalization,
    set_run_capitalization_code, set_run_faux_bold, set_run_faux_italic, set_run_fill_color,
    set_run_font_index, set_run_font_size, set_run_kerning_type, set_run_kerning_type_code,
    set_run_language, set_run_language_code, set_run_ligatures, set_run_no_break,
    set_run_old_style, set_run_property, set_run_strikethrough, set_run_strikethrough_code,
    set_run_stroke_color, set_run_underline, set_run_underline_code,
};
pub use syntax::{Kind, Token, dict_entries, skip_value, strings, tokens, untokenize, utf16};
pub use text::{paragraph_run_lengths, read_text, replace_text, text_span};
