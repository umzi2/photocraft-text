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
    AutoLeadingRow {
        font: "NotoSans-Regular",
        size: 12.0,
        leading: Some(14.4),
        source: "corpus/size_12",
    },
    AutoLeadingRow {
        font: "NotoSans-Regular",
        size: 24.0,
        leading: Some(28.8),
        source: "corpus/size_24",
    },
    AutoLeadingRow {
        font: "NotoSans-Regular",
        size: 48.0,
        leading: Some(57.6),
        source: "corpus/box_left",
    },
    AutoLeadingRow {
        font: "NotoSans-Regular",
        size: 96.0,
        leading: Some(115.2),
        source: "corpus/boxprobe_96",
    },
    AutoLeadingRow {
        font: "NotoSerif-Regular",
        size: 48.0,
        leading: Some(57.6),
        source: "corpus/serif",
    },
    AutoLeadingRow {
        font: "ArialMT",
        size: 48.0,
        leading: Some(57.6),
        source: "corpus/boxprobe_arial",
    },
    AutoLeadingRow {
        font: "v_CCWildButNotCrazy",
        size: 0.0,
        leading: None,
        source: "no auto layer in corpus",
    },
    AutoLeadingRow {
        font: "CCMeanwhile",
        size: 0.0,
        leading: None,
        source: "UNMEASURED",
    },
    AutoLeadingRow {
        font: "CCGrimNGritty",
        size: 0.0,
        leading: None,
        source: "UNMEASURED",
    },
    AutoLeadingRow {
        font: "CCSpellcaster",
        size: 0.0,
        leading: None,
        source: "UNMEASURED",
    },
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
pub fn apply_paragraph_spacing(
    first_baseline: f32,
    paragraphs: &[ParagraphBlock],
) -> Vec<PlacedLine> {
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

/// Whether `ch` is roman text for vertical composition: Latin letters,
/// digits and punctuation. Photoshop rotates those 90° clockwise down a
/// vertical column when the run's `BaselineDirection` is 2; CJK text always
/// stands upright.
pub fn is_roman(ch: char) -> bool {
    (ch.is_ascii() && !ch.is_ascii_control()) || ('\u{00C0}'..='\u{024F}').contains(&ch)
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

/// Faux bold strength divisor. Photoshop's faux bold adds a smaller advance
/// than FreeType's `FT_GlyphSlot_Embolden` (`size / 24`): layer `01
/// capitalisation underline faux bold` (Noto Sans Bold 30 pt) has a run width of
/// 573.47 pt, which is 25.9 pt more than the plain metrics run (547.6 pt) for
/// 32 glyphs, i.e. `size / 37` per glyph. With `size / 24` the glyphs drift
/// 14 pt past the stored rectangle. [MEASURED on 10_all_features.psd]
pub const FAUX_BOLD_DIVISOR: f32 = 37.0;

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

// ---------------------------------------------------------------------------
// Measured underline / strikeout bands (Photoshop 20.0.2, CC 2019)
// ---------------------------------------------------------------------------
//
// Photoshop rasterises rules at a 4x subpixel grid, so a stored row's coverage
// is a multiple of 1/4 (alpha 128 = .50, 192 = .75). Measured on
// `10_all_features.psd` against the device baseline row (NotoSans 24/26/30 pt,
// Crisp AA):
//
// | layer | size | kind  | stored rows                 | band            |
// |-------|------|-------|-----------------------------|-----------------|
// | 01    | 30   | under | 28: .50, 29: 1.00           | 28.50 .. 30.00  |
// | 07    | 24   | under | 23: .75, 24: .50            | 23.25 .. 24.50  |
// | 02    | 26   | strike| 6: .50, 7: .75              | 6.50 .. 7.75    |
//
// Baselines in those canvases are rows 22 (01), 18 (07) and 20 (02), which the
// glyph cells confirm. Fitting the three bands in device space (distance from
// the baseline to the band's TOP edge, positive down) gives:
//
//   underline top = quantise_quarter(0.2 * size + 0.5)
//   strike top    = -quantise_quarter(0.5 * size + 0.5)
//   thickness     = quantise_quarter(0.05 * size), at least 1 px
//
// which is 30 pt -> +6.5 / 1.5, 24 pt -> +5.25 / 1.25, 26 pt -> -13.5 / 1.25:
// exactly the stored bands above. Photoshop does not use the `post` or `OS/2`
// decoration metrics of these fonts (NotoSans has -100/50 for both), and the
// half-pixel term is part of the fit: the rule baseline sits half a pixel
// below the integer row the glyph cells use.

/// Round to the 1/4 px grid Photoshop's rule rasteriser works on.
fn quantise_quarter(value: f32) -> f32 {
    (value * 4.0).round() / 4.0
}

/// Underline band: `offset` is the distance from the baseline to the band's
/// top edge, positive downward in device space (the convention
/// [`crate::raster::Canvas::underline`] takes); `thickness` is the band height.
pub fn underline_band(size: f32) -> Decoration {
    Decoration {
        offset: quantise_quarter(0.2 * size + 0.5),
        thickness: decoration_thickness(size),
    }
}

/// Strikeout band: same conventions as [`underline_band`], with a negative
/// offset because the band sits above the baseline.
pub fn strikeout_band(size: f32) -> Decoration {
    Decoration {
        offset: -quantise_quarter(0.5 * size + 0.5),
        thickness: decoration_thickness(size),
    }
}

/// Measured rule thickness: a twentieth of the size, on the 1/4 px grid and at
/// least one pixel.
pub fn decoration_thickness(size: f32) -> f32 {
    quantise_quarter(0.05 * size).max(1.0)
}

// ---------------------------------------------------------------------------
// Measured hanging punctuation (Photoshop 20.0.2, CC 2019)
// ---------------------------------------------------------------------------

/// Where a character may hang and how much of its advance it keeps in the
/// line's flow: `0.0` means the glyph hangs entirely in the margin (the next
/// glyph starts on the margin), `0.5` means half of the advance stays in the
/// flow. The amounts are measured, not inferred from side bearings: see
/// `docs/hanging-punctuation.md`, artifacts `out/probe5/chars.pdf` (Noto Sans
/// 36 pt, box left x = 20) and `out/probe5/hang3..6.pdf`.
///
/// Left-hang glyphs measured in `chars.pdf`; the shift of the glyph's pen from
/// the margin was: `“` 10.40, `”` 10.40, `‘` 3.78, `’` 3.78, `«` 18.32,
/// `»` 18.32, `-` 11.59, `,` 9.65, `.` 9.65, `;` 4.82, `:` 4.82, `*` 9.92,
/// `…` 14.24, `–` 9.00, `—` 18.00. For the first group the shift equals the
/// shaped advance (including kerning to the following glyph: `“A` advances
/// 10.40 although `“` alone is 12.92), i.e. they keep 0.0 in the flow; `;`,
/// `:`, `*`, `…`, `–`, `—` are shifted by half their advance, i.e. 0.5.
/// `(`, `[`, `{`, `!`, `?`, `•` did NOT hang (no table entry).
pub const LEFT_HANG: &[(char, f32)] = &[
    ('\u{201C}', 0.0), // “
    ('\u{201D}', 0.0), // ”
    ('\u{2018}', 0.0), // ‘
    ('\u{2019}', 0.0), // ’
    ('\u{00AB}', 0.0), // «
    ('\u{00BB}', 0.0), // »
    ('"', 0.0),
    ('\'', 0.0),
    ('-', 0.0),
    (',', 0.0),
    ('.', 0.0),
    (';', 0.5),
    (':', 0.5),
    ('*', 0.5),
    ('\u{2026}', 0.5), // …
    ('\u{2013}', 0.5), // –
    ('\u{2014}', 0.5), // —
];

/// Right-hang characters: a line may end with them, they contribute nothing to
/// the line-fit width and are drawn after the fitted content, overflowing the
/// right margin by their advance. Measured in `out/probe5/hang4..6.pdf`
/// (`AAAAA BBBBB,` fits a 242 pt box with hanging, needs 251 pt without; the
/// comma is drawn at pen 261.02 for a 242 pt box at x = 20, its ink ends at
/// 267.7). `;`, `:`, `!`, `?`, `)`, `]`, `}`, `–`, `—`, `…`, `%`, `/` are NOT
/// in the set (each pushed the preceding word to the next line at box 242).
pub const RIGHT_HANG: &[(char, f32)] = &[
    (',', 0.0),
    ('.', 0.0),
    ('\u{201D}', 0.0), // ”
    ('\u{2019}', 0.0), // ’
    ('\u{00BB}', 0.0), // »
    ('"', 0.0),
    ('\'', 0.0),
    ('-', 0.0),
];

/// CJK full-width left-hang characters.
///
/// **Not measured on a native CJK face.** `out/probe5/chars.pdf` does contain
/// 『 and （ (the probe text used them; Photoshop rendered them through a
/// fallback because NotoSans-Regular has no CJK): their pen shifts 17.2805 for
/// an advance of 35.281, i.e. exactly half the advance stays in the flow. But
/// the dedicated CJK probe through `AdobeHeitiStd-Regular` showed no hanging at
/// all (`docs/STATUS.md`, "Hanging Punctuation is Roman-only in this build"),
/// so whether burasagari applies to a native CJK face is open. The set is
/// [INFERENCE] from JIS X 4051 and uses the half-em fraction the fallback
/// measurements show.
#[allow(dead_code)]
pub const CJK_LEFT_HANG: &[(char, f32)] = &[
    ('\u{300C}', 0.5), // 「
    ('\u{300E}', 0.5), // 『
    ('\u{FF08}', 0.5), // （
    ('\u{3010}', 0.5), // 【
    ('\u{3008}', 0.5), // 〈
    ('\u{300A}', 0.5), // 《
    ('\u{3014}', 0.5), // 〔
    ('\u{FF3B}', 0.5), // ［
    ('\u{FF5B}', 0.5), // ｛
    ('\u{201C}', 0.5), // “ (CJK text often uses the full-width quote)
    ('\u{2018}', 0.5), // ‘
];

/// CJK full-width right-hang characters. [INFERENCE]: no Photoshop artefact
/// shows a CJK line-final glyph hanging (the CJK hanging probe showed no effect
/// at all), the model is JIS X 4051 "burasage" with the same half-em fraction.
#[allow(dead_code)]
pub const CJK_RIGHT_HANG: &[(char, f32)] = &[
    ('\u{3002}', 0.5), // 。
    ('\u{3001}', 0.5), // 、
    ('\u{FF0C}', 0.5), // ，
    ('\u{FF0E}', 0.5), // ．
    ('\u{FF1A}', 0.5), // ：
    ('\u{FF1B}', 0.5), // ；
    ('\u{FF01}', 0.5), // ！
    ('\u{FF1F}', 0.5), // ？
    ('\u{300D}', 0.5), // 」
    ('\u{300F}', 0.5), // 』
    ('\u{FF09}', 0.5), // ）
    ('\u{3011}', 0.5), // 】
    ('\u{3009}', 0.5), // 〉
    ('\u{300B}', 0.5), // 》
    ('\u{3015}', 0.5), // 〕
    ('\u{FF3D}', 0.5), // ］
    ('\u{FF5D}', 0.5), // ｝
    ('\u{2015}', 0.5), // ―
];

fn lookup(table: &'static [(char, f32)], ch: char) -> Option<f32> {
    table.iter().find(|(c, _)| *c == ch).map(|(_, f)| *f)
}

/// Fraction of a left-hanging glyph's advance that stays in the line flow, when
/// the glyph is one of the measured/inferred left-hang characters. `None` for
/// characters that do not hang on the left.
pub fn left_hang_fit(ch: char) -> Option<f32> {
    // CJK punctuation does not hang on a native CJK face: `10_all_features.psd`
    // layer `10 cjk heiti` starts `「` exactly at the box edge and its line ends
    // at the box edge (PDF glyph x = 620.0 and 1170.0), whereas the half-em CJK
    // tables would shift the whole line 11 px left.  The `CJK_*_HANG` tables are
    // kept for the fallback-font probe only; they are not consulted here.
    lookup(LEFT_HANG, ch)
}

/// Fraction of a right-hanging glyph's advance that stays in the line-fit
/// width. `None` for characters that do not hang on the right.
pub fn right_hang_fit(ch: char) -> Option<f32> {
    lookup(RIGHT_HANG, ch)
}

/// Whether `ch` may hang into the left margin at the start of a line.
pub fn is_left_hang(ch: char) -> bool {
    left_hang_fit(ch).is_some()
}

/// Whether `ch` may hang into the right margin at the end of a line.
pub fn is_right_hang(ch: char) -> bool {
    right_hang_fit(ch).is_some()
}

/// The overhang a left-hanging glyph adds to the line start (the distance its
/// pen sits left of the margin). The full advance, minus the part kept in the
/// flow.
pub fn left_hang_shift(ch: char, advance: f32) -> f32 {
    left_hang_fit(ch).map_or(0.0, |fit| advance * (1.0 - fit))
}

/// Kinsoku line-breaking tables. Latin closing punctuation must not start a
/// line, opening punctuation must not end one; the CJK sets are the standard
/// JIS X 4051 sets. Photoshop's paragraph sheet stores `KinsokuOrder` (0 in
/// every corpus layer) but the mapping of that code to the rule variants is
/// unmeasured; these are the conservative sets. [INFERENCE] for CJK and for the
/// Latin punctuation that the hanging probe did not measure.
pub mod kinsoku {
    /// Characters that may not start a line (closing punctuation). `,` `.`
    /// `”` `’` `»` `"` `'` `-` are in the hanging set instead, so a line may
    /// start with them when hanging is enabled — the breaker checks the
    /// hanging tables first.
    pub const NO_LINE_START: &str = ",.;:!?)]}\u{00BB}\u{203A}\u{201D}\u{2019}\u{3001}\u{3002}\u{FF0C}\u{FF0E}\u{FF1A}\u{FF1B}\u{FF01}\u{FF1F}\u{FF09}\u{300D}\u{FF3D}\u{FF5D}\u{300F}\u{3011}\u{3009}\u{300B}\u{3015}\u{30FB}\u{2026}\u{2025}";

    /// Characters that may not end a line (opening punctuation).
    pub const NO_LINE_END: &str = "([{\u{00AB}\u{2039}\u{201C}\u{2018}\u{300C}\u{300E}\u{FF08}\u{3010}\u{3008}\u{300A}\u{3014}\u{FF3B}\u{FF5B}";

    /// Whether `ch` may start a line.
    pub fn may_start_line(ch: char) -> bool {
        !NO_LINE_START.contains(ch)
    }

    /// Whether `ch` may end a line at a soft break.
    pub fn may_end_line(ch: char) -> bool {
        !NO_LINE_END.contains(ch)
    }
}

/// Every-line composer tracking nudge as a fraction of the em. Measured by
/// comparing `out/composer_every.pdf` with `out/composer_single.pdf` (+0.2496 pt
/// per inter-letter gap at 24 pt) and the two layout caches
/// (`out/composer_every.psd` `/3 .24959` at 24 pt, `out/c_every.psd`
/// `/3 .208` at 20 pt).
pub const EVERY_LINE_TRACK: f32 = 0.0104;

/// Tracking added between two letters of a word by the every-line composer.
pub fn every_line_nudge(size: f32) -> f32 {
    size * EVERY_LINE_TRACK
}
