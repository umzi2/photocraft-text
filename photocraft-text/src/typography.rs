//! Paragraph-level typography rules that Photoshop applies on top of layout:
//! auto leading, hanging punctuation (kinsoku), paragraph spacing and indents,
//! standard-ligature policy, faux bold/italic and underline/strikeout geometry.
//!
//! Every constant carries its source. "Measured" means reproduced from a corpus
//! PSD/PDF pair (see `AUTO_LEADING_TABLE`). "[INFERENCE]" means no Photoshop
//! measurement exists in the corpus and the value comes from the named
//! external reference; those values must not be treated as Photoshop-verified.

use crate::font::Face;
use crate::layout::GlyphPos;

/// Auto leading ratio: EngineData stores `AutoLeading = 1.2` and the computed
/// `Leading` equals `1.2 x size` in every measured layer (see
/// `AUTO_LEADING_TABLE`). Measured.
pub const AUTO_LEADING: f32 = 1.2;

/// One auto-leading observation. `leading` is the Photoshop-computed leading in
/// points (EngineData `Leading` of an auto layer, cross-checked against PDF
/// baseline deltas where a multi-line layer exists). `None` = no corpus layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoLeadingRow {
    pub font: &'static str,
    pub size: f32,
    pub leading: Option<f32>,
    pub source: &'static str,
}

/// Auto leading per font. Source files are under `corpus/` and `out/`.
///
/// | font                     | size | leading | source                                      |
/// |--------------------------|------|---------|---------------------------------------------|
/// | NotoSans-Regular         | 12   | 14.4    | corpus/size_12 EngineData (measured)        |
/// | NotoSans-Regular         | 24   | 28.8    | corpus/size_24 EngineData (measured)        |
/// | NotoSans-Regular         | 48   | 57.6    | corpus/box_left EngineData + PDF baselines  |
/// | NotoSans-Regular         | 96   | 115.2   | corpus/boxprobe_96 EngineData + PDF         |
/// | NotoSerif-Regular        | 48   | 57.6    | corpus/serif EngineData (measured)          |
/// | ArialMT                  | 48   | 57.6    | corpus/boxprobe_arial EngineData (measured) |
/// | v_CCWildButNotCrazy      | -    | None    | corpus layers are all explicit leading      |
/// | CCMeanwhile              | -    | None    | not in corpus; UNMEASURED                   |
/// | CCGrimNGritty            | -    | None    | not in corpus; UNMEASURED                   |
/// | CCSpellcaster            | -    | None    | not in corpus; UNMEASURED                   |
pub const AUTO_LEADING_TABLE: &[AutoLeadingRow] = &[
    AutoLeadingRow { font: "NotoSans-Regular", size: 12.0, leading: Some(14.4), source: "corpus/size_12" },
    AutoLeadingRow { font: "NotoSans-Regular", size: 24.0, leading: Some(28.8), source: "corpus/size_24" },
    AutoLeadingRow { font: "NotoSans-Regular", size: 48.0, leading: Some(57.6), source: "corpus/box_left" },
    AutoLeadingRow { font: "NotoSans-Regular", size: 96.0, leading: Some(115.2), source: "corpus/boxprobe_96" },
    AutoLeadingRow { font: "NotoSerif-Regular", size: 48.0, leading: Some(57.6), source: "corpus/serif" },
    AutoLeadingRow { font: "ArialMT", size: 48.0, leading: Some(57.6), source: "corpus/boxprobe_arial" },
    AutoLeadingRow { font: "v_CCWildButNotCrazy", size: 0.0, leading: None, source: "no auto layer in corpus" },
    AutoLeadingRow { font: "CCMeanwhile", size: 0.0, leading: None, source: "UNMEASURED" },
    AutoLeadingRow { font: "CCGrimNGritty", size: 0.0, leading: None, source: "UNMEASURED" },
    AutoLeadingRow { font: "CCSpellcaster", size: 0.0, leading: None, source: "UNMEASURED" },
];

/// Auto leading for `face` at `size` points.
///
/// The ratio is font-independent in every measured row. The three CC fonts
/// are unmeasured; applying 1.2 to them is [INFERENCE].
pub fn auto_leading(_face: &Face<'_>, size: f32) -> f32 {
    size * AUTO_LEADING
}

/// Kinsoku / hanging-punctuation rules.
#[derive(Clone, Debug, PartialEq)]
pub struct Kinsoku {
    /// Whether hanging punctuation is on. Default off: the Hanging flag is
    /// false in every corpus paragraph (measured, EngineData).
    pub enabled: bool,
    /// Characters that may hang past the left edge. [INFERENCE] Roman set.
    pub hang_open: &'static str,
    /// Characters that may hang past the right edge (spec set). [INFERENCE]
    pub hang_close: &'static str,
    /// Maximum overhang as a multiple of the character's advance. Spec: "up to
    /// its advance", so 1.0. [INFERENCE]
    pub max_overhang: f32,
}

impl Default for Kinsoku {
    fn default() -> Self {
        Self {
            enabled: false,
            hang_open: "([{\u{201C}\u{2018}\u{00AB}\u{2039}",
            hang_close: ".,;:!?)]}",
            max_overhang: 1.0,
        }
    }
}

/// Left shift of a line's start so an opening punctuation glyph hangs into the
/// margin. Returns `0.0` when hanging is off, the line is empty, the first glyph
/// is not opening punctuation, or the line is wider than `box_width` (only lines
/// that fit the box are hung).
pub fn hanging_adjust(line: &[GlyphPos], box_width: f32, rules: &Kinsoku) -> f32 {
    if !rules.enabled {
        return 0.0;
    }
    let Some(first) = line.first() else {
        return 0.0;
    };
    let width: f32 = line.iter().map(|g| g.advance).sum();
    if width > box_width || !rules.hang_open.contains(first.ch) {
        return 0.0;
    }
    -(first.advance * rules.max_overhang)
}

/// Whether a trailing closing-punctuation glyph may exceed the box edge instead
/// of wrapping. `content_width` is the line width before the glyph; the glyph
/// hangs only if the content fits and the overhang is at most
/// `advance * max_overhang`.
pub fn can_hang_trailing(
    content_width: f32,
    ch: char,
    advance: f32,
    box_width: f32,
    rules: &Kinsoku,
) -> bool {
    if !rules.enabled || !rules.hang_close.contains(ch) || content_width > box_width {
        return false;
    }
    let overhang = content_width + advance - box_width;
    overhang <= advance * rules.max_overhang
}

/// Paragraph block as laid out in an area box. Units: points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParagraphBlock {
    pub space_before: f32,
    pub space_after: f32,
    pub first_line_indent: f32,
    pub start_indent: f32,
    pub line_count: usize,
    pub leading: f32,
}

/// Baseline and x offset of one placed line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedLine {
    pub baseline: f32,
    pub x_offset: f32,
}

/// Places paragraph lines from `first_baseline`.
///
/// Within a paragraph each line steps down by that paragraph's leading. A new
/// paragraph's first line steps by `leading + space_after(prev) + space_before(next)`.
/// The first paragraph's `space_before` is applied: [INFERENCE], no corpus layer
/// has a nonzero SpaceBefore (all 0.0, measured), so this is unverified against
/// PS. The first line of a paragraph is offset by `start_indent +
/// first_line_indent`; later lines by `start_indent`. Empty paragraphs are skipped.
pub fn apply_paragraph_spacing(first_baseline: f32, paragraphs: &[ParagraphBlock]) -> Vec<PlacedLine> {
    let mut out = Vec::new();
    let mut baseline = first_baseline;
    let mut prev_after: Option<f32> = None;
    for para in paragraphs.iter().filter(|p| p.line_count > 0) {
        match prev_after {
            None => baseline += para.space_before,
            Some(after) => baseline += para.leading + after + para.space_before,
        }
        prev_after = Some(para.space_after);
        for i in 0..para.line_count {
            if i > 0 {
                baseline += para.leading;
            }
            let x_offset = if i == 0 {
                para.start_indent + para.first_line_indent
            } else {
                para.start_indent
            };
            out.push(PlacedLine { baseline, x_offset });
        }
    }
    out
}

/// Searches a GSUB table for `tag` in its FeatureList. Returns `false` for
/// malformed or truncated tables.
pub fn gsub_has_feature(gsub: &[u8], tag: [u8; 4]) -> bool {
    let rd16 = |o: usize| -> Option<usize> {
        Some(u16::from_be_bytes([*gsub.get(o)?, *gsub.get(o + 1)?]) as usize)
    };
    let Some(feature_list) = rd16(6) else {
        return false;
    };
    let Some(count) = rd16(feature_list) else {
        return false;
    };
    (0..count).any(|i| {
        let rec = feature_list + 2 + i * 6;
        gsub.get(rec..rec + 4) == Some(&tag[..])
    })
}

/// Whether standard ligatures apply to a run: the layer flag must be on and the
/// face must have a GSUB `liga` feature. A font without GSUB gets no
/// substitution (measured for CCMeanwhile, which has no GSUB table).
///
/// AutoKerning does not gate ligatures: it only selects GPOS pair kerning
/// (`Face::kern_pair`), and liga is a GSUB feature. [INFERENCE] from the OpenType
/// table split; no corpus run toggles ligatures.
pub fn standard_ligatures_active(face: &Face<'_>, layer_ligatures: bool) -> bool {
    layer_ligatures
        && face
            .table(*b"GSUB")
            .is_some_and(|g| gsub_has_feature(g, *b"liga"))
}

/// Byte spans of standard ligature sequences in `text` (longest match first:
/// ffi, ffl, ff, fi, fl). Empty when `active` is false.
pub fn ligature_spans(text: &str, active: bool) -> Vec<(usize, usize)> {
    if !active {
        return Vec::new();
    }
    const SEQS: [&str; 5] = ["ffi", "ffl", "ff", "fi", "fl"];
    let mut spans = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        match SEQS.iter().find(|s| rest.starts_with(**s)) {
            Some(s) => {
                spans.push((i, i + s.len()));
                i += s.len();
            }
            None => i += rest.chars().next().map_or(1, char::len_utf8),
        }
    }
    spans
}

/// Faux italic skew: FreeType `FT_GlyphSlot_Oblique` slant 0x0366A / 65536
/// (about 12 degrees). [INFERENCE] Photoshop's value is not measured (no
/// FauxItalic run in the corpus; `out/compare/ps_*.psd` are plain).
pub const FAUX_ITALIC_SKEW: f32 = 0x0366A as f32 / 65536.0;

/// Shears a point about the baseline: `x' = x + y * FAUX_ITALIC_SKEW`.
pub fn faux_italic_point(x: f32, y: f32) -> (f32, f32) {
    (x + y * FAUX_ITALIC_SKEW, y)
}

/// Faux bold strength divisor. Strength = `size / 24`, FreeType
/// `FT_GlyphSlot_Embolden` (ftsynth.c). [INFERENCE] Photoshop not measured.
pub const FAUX_BOLD_DIVISOR: f32 = 24.0;

/// Faux bold: extra advance added per glyph, in points.
pub fn faux_bold_extra_advance(size: f32) -> f32 {
    size / FAUX_BOLD_DIVISOR
}

/// Faux bold: stroke offset outward per side, in points (half the strength).
pub fn faux_bold_stroke_offset(size: f32) -> f32 {
    faux_bold_extra_advance(size) * 0.5
}

/// Line decoration geometry: `offset` is the distance from the baseline
/// (positive up), `thickness` in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decoration {
    pub offset: f32,
    pub thickness: f32,
}

/// Underline from the `post` table: `underlinePosition` (i16 at byte 8) and
/// `underlineThickness` (i16 at byte 10), in font units (OpenType spec). The
/// stored position is negative below the baseline; this returns it positive up.
/// [INFERENCE] Photoshop not measured.
pub fn underline_geometry(post: &[u8], size: f32, units_per_em: u16) -> Option<Decoration> {
    let pos = i16::from_be_bytes([*post.get(8)?, *post.get(9)?]) as f32;
    let thick = i16::from_be_bytes([*post.get(10)?, *post.get(11)?]) as f32;
    let k = size / units_per_em as f32;
    Some(Decoration {
        offset: pos * k,
        thickness: thick * k,
    })
}

/// Strikeout from the `OS/2` table: `yStrikeoutPosition` (i16 at byte 28) and
/// `yStrikeoutSize` (i16 at byte 26), font units (OpenType spec). [INFERENCE]
/// Photoshop not measured.
pub fn strikeout_geometry(os2: &[u8], size: f32, units_per_em: u16) -> Option<Decoration> {
    let thick = i16::from_be_bytes([*os2.get(26)?, *os2.get(27)?]) as f32;
    let pos = i16::from_be_bytes([*os2.get(28)?, *os2.get(29)?]) as f32;
    let k = size / units_per_em as f32;
    Some(Decoration {
        offset: pos * k,
        thickness: thick * k,
    })
}
