//! Render inputs and pixel surfaces.

use crate::area::ParagraphStyle;
use crate::layout::TextStyle;

/// A rendered text layer: straight (non-premultiplied) RGBA, row-major.
#[derive(Clone, Debug)]
pub struct LayerPixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Top-left corner of the canvas relative to the text box's top-left.
    ///
    /// Photoshop sizes a type layer to the ink, not to the box, so glyphs that
    /// rise above the box top (accents, `Й`) or hang below it still land in the
    /// layer. The caller adds this offset to the box origin to get the layer
    /// rectangle.
    pub offset: (f32, f32),
}

/// Parameters of one type layer, in the same units Photoshop stores.
#[derive(Clone, Debug, Default)]
pub struct LayerTextStyle {
    pub style: TextStyle,
    pub paragraph: ParagraphStyle,
    /// Width and height of the text box in points.
    pub box_size: (f32, f32),
    /// Top-left corner of the text box inside the layer rect, in points.
    pub origin: (f32, f32),
    /// Fill colour, straight RGB.
    pub color: [u8; 3],
}

/// Options for [`super::FaceStore::render_spec_with`].
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderOptions {
    /// Antialias mode to rasterise with; `None` keeps [`crate::raster::AntiAlias::Sharp`].
    pub aa: Option<crate::raster::AntiAlias>,
    /// Use the attached glyph atlas for the glyphs it covers.
    pub use_atlas: bool,
}

/// A warped type layer's pixels: the RGBA surface is exactly the layer's
/// stored document rectangle, so the caller can paint it at `rect` without
/// touching strides or origins.
#[derive(Clone, Debug)]
pub struct WarpedPixels {
    pub pixels: LayerPixels,
    /// Document rectangle the surface covers: `(x, y, width, height)` in layer
    /// pixels. `pixels.width/height` equal the rectangle's size.
    pub rect: (i32, i32, u32, u32),
}

/// Integer document rectangle a layer is rasterised into.
///
/// Photoshop stores each type layer as a raster cropped from the document at
/// `layer.rect`; painting straight into that rectangle keeps the glyph phase
/// relative to the document pixel grid, which an ink-cropped surface would lose.
#[derive(Clone, Copy, Debug)]
pub struct RectTarget {
    /// Top-left of the canvas in document pixels.
    pub origin: (i32, i32),
    /// Canvas size in pixels.
    pub size: (u32, u32),
    /// Document-space position of the layout origin handed to the layout
    /// functions: the `TySh` translation for point text, the text box origin for
    /// area text.
    pub anchor: (f32, f32),
}

/// Character features that live only in `Txt2`'s character sheet and therefore
/// cannot be read from `EngineData`. Indexed by the layer's style run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunFeatures {
    /// `Txt2` `/6/30` old-style figures (Photoshop's "on" value is 2).
    pub old_style: bool,
    /// Photoshop's optical kerning (`AutoKernType.OPTICAL`).
    ///
    /// The PSD does not record which kerning mode was used — `EngineData`
    /// carries `AutoKerning: True, Kerning: 0` for a metrics layer and for the
    /// layer authored with optical kerning alike — so a caller that knows the
    /// setting (its `Txt2`/layer model) asks for it per layer. When set, the
    /// engine spaces every pair by the shape-based [`crate::optical::optical_pair`]
    /// model instead of the font's pair kerning.
    pub optical_kerning: bool,
    /// Per-glyph advance corrections in points, one per shaped glyph in text
    /// order (spaces included, dictionary hyphens excluded; positive widens).
    ///
    /// This is where Txt2's composed cache belongs: the every-line composer's
    /// per-run tracking deltas (`/3` inside the cache run records) and optical
    /// kerning (the difference between the cached glyph origin and the metric
    /// advance of the same glyph). When non-empty the layout uses these
    /// advances instead of its uniform every-line nudge, so line breaking sees
    /// the same widths Photoshop did.
    pub advance_deltas: Vec<f32>,
    /// Per-space advance corrections in points, one per **word space** of the
    /// layer text in text order — the every-line composer's word-spacing sheet
    /// in `Txt2`'s composed cache (`composed_layout(..).deltas()`).
    ///
    /// Unlike [`Self::advance_deltas`] these are indexed by space, not by
    /// glyph, because Photoshop balances a line by moving its spaces: layer
    /// `05 area indents spacing` records eight `-.0468` corrections on line 1
    /// and seven `+.0468` on lines 4/5. When non-empty the layout applies them
    /// to the space glyphs in order and the uniform every-line nudge is
    /// disabled (the cache carries the real correction). The sheet describes the
    /// whole text, so only the first populated entry in a feature list is used.
    pub word_spacing_deltas: Vec<f32>,
}

impl RunFeatures {
    /// The composed-cache half of a layer's features, read from its `Txt2`
    /// block: Photoshop's every-line composer word-spacing corrections, in
    /// text order (see [`Self::word_spacing_deltas`]).
    ///
    /// The caller adds the character-sheet flags it knows (`old_style`,
    /// `optical_kerning`, `advance_deltas`) on top:
    ///
    /// ```ignore
    /// let features = RunFeatures { optical_kerning, ..RunFeatures::from_composed_cache(txt2, object)? };
    /// ```
    pub fn from_composed_cache(block: &[u8], object: usize) -> Result<Self, crate::error::Error> {
        let layout = crate::txt2::composed_layout(block, object)?;
        Ok(Self {
            word_spacing_deltas: layout.deltas(),
            ..Self::default()
        })
    }
}
