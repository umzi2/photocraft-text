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

pub use antialias::{
    antialias_dict_path, antialias_dict_path_at, antialias_path, antialias_path_at,
    object_antialias, set_antialias, set_antialias_at,
};
pub use cache::{
    ComposedLayout, ComposedLine, ComposedRun, Orientation, clear_cache, composed_layout,
    composed_layout_oriented, composed_layout_vertical, invalidate_changed_composed_layouts,
    invalidate_composed_layout, invalidate_composed_layouts,
};
pub use frame::{
    FrameGeometry, FramePath, ensure_frame_list, frame_geometry, frame_list_entry, frame_path,
    object_first_baseline, object_frame_geometry, object_frame_index, object_frame_path,
    set_frame_geometry,
    set_frame_geometry_at, set_frame_vertical, set_object_frame_path, set_point_frame,
};
pub use paragraph::{
    Hyphenation,
    Justification,
    ParagraphProperty,
    paragraph_auto_leading,
    // object-index forms (task-7 writer; the reader uses the getters)
    paragraph_auto_leading_at,
    paragraph_count,
    paragraph_count_at,
    paragraph_end_indent,
    paragraph_end_indent_at,
    paragraph_every_line_composer,
    paragraph_every_line_composer_at,
    paragraph_first_line_indent,
    paragraph_first_line_indent_at,
    paragraph_hanging_punctuation,
    paragraph_hanging_punctuation_at,
    paragraph_hyphenation,
    paragraph_hyphenation_at,
    paragraph_justification,
    paragraph_justification_at,
    paragraph_property,
    paragraph_property_at,
    paragraph_props_path,
    paragraph_props_path_at,
    paragraph_range,
    paragraph_range_at,
    paragraph_space_after_at,
    paragraph_space_before_at,
    paragraph_start_indent,
    paragraph_start_indent_at,
    set_paragraph_auto_leading,
    set_paragraph_auto_leading_at,
    set_paragraph_end_indent,
    set_paragraph_end_indent_at,
    set_paragraph_every_line_composer,
    set_paragraph_every_line_composer_at,
    set_paragraph_first_line_indent,
    set_paragraph_first_line_indent_at,
    set_paragraph_hanging_punctuation,
    set_paragraph_hanging_punctuation_at,
    set_paragraph_hyphenation,
    set_paragraph_hyphenation_at,
    set_paragraph_justification,
    set_paragraph_justification_at,
    set_paragraph_justification_code,
    set_paragraph_justification_code_at,
    set_paragraph_property,
    set_paragraph_property_at,
    set_paragraph_property_or_insert,
    dict_slot_violations_at, sanitise_dict_slots_at,
    set_paragraph_property_or_insert_at,
    set_paragraph_range,
    set_paragraph_range_at,
    set_paragraph_space_after_at,
    set_paragraph_space_before_at,
    set_paragraph_start_indent,
    set_paragraph_start_indent_at,
};
pub use path::{
    Step, Value, ensure_container, ensure_value, read_path, replace_container, replace_value,
    value_range,
};
pub use runs::{
    duplicate_run, duplicate_run_at, ensure_style_run_at, run_count, run_count_at, run_lengths,
    run_lengths_at, run_path_for, run_path_for_at, set_character_run_marker, set_run_layout,
    set_run_layout_at, set_run_lengths, set_run_lengths_at,
};
pub use style::{
    Capitalization, KerningType, Language, OLD_STYLE_ON, RunProperty, Strikethrough, Underline,
    antialias_code, ensure_font, font_count, font_name, run_alternate_ligatures,
    run_capitalization, run_faux_bold, run_faux_italic, run_fill_color, run_fill_color_at,
    run_kerning_type, run_language, run_language_code, run_ligatures, run_manual_kerning,
    run_manual_kerning_at, run_no_break, run_old_style, run_property, run_property_at,
    run_strikethrough, run_stroke_color, run_stroke_color_at, run_underline,
    set_run_alternate_ligatures, set_run_capitalization, set_run_capitalization_code,
    set_run_ensured_at, set_run_faux_bold, set_run_faux_italic, set_run_fill_color,
    set_run_fill_color_at, set_run_font_index, set_run_font_size, set_run_kerning_type,
    set_run_kerning_type_code, set_run_language, set_run_language_code, set_run_ligatures,
    set_run_no_break, set_run_old_style, set_run_property, set_run_property_at,
    set_run_strikethrough, set_run_strikethrough_code, set_run_stroke_color,
    set_run_stroke_color_at, set_run_underline, set_run_underline_code,
};
pub use syntax::{Kind, Token, dict_entries, skip_value, strings, tokens, untokenize, utf16};
pub use text::{
    drop_object_entry, ensure_object, insert_object, object_text, paragraph_run_lengths,
    paragraph_run_lengths_at, read_text, replace_text, replace_text_at, table_path_at, text_span,
    text_span_at,
};
