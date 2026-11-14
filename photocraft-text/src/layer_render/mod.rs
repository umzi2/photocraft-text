//! Rendering a text layer's pixels.
//!
//! A Photoshop type layer stores both the editable text (`TySh`/`Txt2`) and a
//! rasterised copy of the glyphs in the layer's pixel channels. Apps without a
//! text engine show that copy, and Photoshop uses it until it re-renders the
//! layer. When we rewrite the text we must rewrite the pixels too, otherwise the
//! file shows the old words in every viewer that does not re-lay-out text.

mod fonts;
mod horizontal;
pub mod layout;
mod model;
mod paint;
mod vertical;
mod warped;

pub use fonts::{FaceStore, first_baseline_offset_of};
pub use horizontal::{
    layout_from_engine_data, layout_from_engine_data_in_rect,
    layout_from_engine_data_in_rect_with_features, layout_from_engine_data_with_paragraphs,
    layout_from_engine_data_with_paragraphs_and_features, paragraph_styles_of,
    render_from_engine_data, render_from_engine_data_in_rect,
    render_from_engine_data_in_rect_with_features, render_from_engine_data_with_paragraphs,
    render_from_engine_data_with_paragraphs_and_features, render_text_layer,
    render_text_layer_with_atlas, render_text_layer_with_atlases,
};
pub use layout::{Caret, CaretCluster, PlacedGlyph, Rect, TextLayout, TextLine};
pub use model::{
    LayerPixels, LayerTextStyle, RectTarget, RenderOptions, RunFeatures, WarpedPixels,
};
pub use vertical::{
    layout_vertical_from_engine_data, layout_vertical_from_engine_data_in_box,
    layout_vertical_from_engine_data_in_rect, render_vertical_from_engine_data,
    render_vertical_from_engine_data_in_box, render_vertical_from_engine_data_in_rect,
    vertical_b0_ratio,
};
pub use warped::render_from_engine_data_warped;
