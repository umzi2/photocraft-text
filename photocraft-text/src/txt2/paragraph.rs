use crate::error::Error;

use super::path::{
    Step, Value, ensure_container, ensure_value, format_real, replace_value, scalar_at, value_span,
};
use super::syntax::tokens;
use super::text::{paragraph_run_lengths, table_path};

/// Paragraph-level properties Photoshop keeps per paragraph run in `Txt2`.
///
/// Keys are the ones Photoshop writes in the paragraph entry's property dict
/// (`/1/1/[0]/0/5/0/<run>/0/0/5`); each was measured by varying one control in
/// Photoshop and diffing two documents (`examples/txt2_diff`), or by authoring
/// the slot and letting Photoshop re-lay-out and re-save the layer
/// (`out/verify/*.psd`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParagraphProperty {
    /// `/0`: `0` left, `1` right, `2` centre, `6` justify all
    /// (`out/probe2/just_right.psd`, `just_center.psd`, `out/verify/just_center.psd`).
    Justification,
    /// `/1`, points: first-line indent. Measured by authoring only: the DOM
    /// setters silently fail, `out/exp/area_indent.psd` proves the slot
    /// (pen x = layer 20 + start 18 + first 24).
    FirstLineIndent,
    /// `/2`, points: left (start) indent.
    StartIndent,
    /// `/3`, points: right (end) indent.
    EndIndent,
    /// `/4`, points: space before the paragraph. **Unconfirmed**: not measured
    /// differentially yet, kept for the existing writer's callers.
    SpaceBefore,
    /// `/5`, points: space after the paragraph. **Unconfirmed**, like `/4`.
    SpaceAfter,
    /// `/7`, fraction of the size: the auto-leading ratio (`1.2` default,
    /// `1.5` measured in `out/exp/auto15.psd`).
    AutoLeading,
    /// `/9`, boolean: hyphenate (`AutoHyphenate` in `EngineData`).
    Hyphenate,
    /// `/10`: hyphenate words longer than this many characters.
    HyphenateWordSize,
    /// `/11`: minimum characters before the hyphen.
    HyphenateAfterFirst,
    /// `/12`: minimum characters after the hyphen.
    HyphenateBeforeLast,
    /// `/13`: maximum consecutive hyphens.
    HyphenLimit,
    /// `/14`: hyphenation zone in points. A default paragraph sheet may
    /// materialise `36.0` (Photoshop's own default), but a measured value is
    /// stored 1:1: DOM 18 pt -> `/5/14 = 18.0` (`out/probe5/pE.psd`).
    HyphenationZone,
    /// `/15`: hyphenate capitalised words. No `EngineData` key is known for
    /// this one, so it reaches `Txt2` only.
    HyphenateCapitalWords,
    /// `/17`, `[min desired max]` word spacing as fractions of the space width.
    WordSpacing,
    /// `/18`, `[min desired max]`: letter spacing, fractions of the em.
    LetterSpacing,
    /// `/19`, `[min desired max]`: glyph scaling (`GlyphSpacing` in
    /// `EngineData`; `out/probe2/glyph_scaling.psd` measured `[.9 1.0 1.1]`).
    GlyphScaling,
    /// `/21`, boolean: hanging punctuation (`Hanging` in `EngineData`;
    /// `out/verify/hang.psd`).
    HangingPunctuation,
    /// `/29`, boolean: every-line composer (`out/probe2/composer_every.psd`
    /// could not measure it — the ExtendScript enum is invalid on CC 2019 — so
    /// the key comes from Photoshop's own paragraph dict).
    EveryLineComposer,
}

impl ParagraphProperty {
    /// The dict key Photoshop stores this property under.
    pub fn key(self) -> &'static str {
        match self {
            ParagraphProperty::Justification => "0",
            ParagraphProperty::FirstLineIndent => "1",
            ParagraphProperty::StartIndent => "2",
            ParagraphProperty::EndIndent => "3",
            ParagraphProperty::SpaceBefore => "4",
            ParagraphProperty::SpaceAfter => "5",
            ParagraphProperty::AutoLeading => "7",
            ParagraphProperty::Hyphenate => "9",
            ParagraphProperty::HyphenateWordSize => "10",
            ParagraphProperty::HyphenateAfterFirst => "11",
            ParagraphProperty::HyphenateBeforeLast => "12",
            ParagraphProperty::HyphenLimit => "13",
            ParagraphProperty::HyphenationZone => "14",
            ParagraphProperty::HyphenateCapitalWords => "15",
            ParagraphProperty::WordSpacing => "17",
            ParagraphProperty::LetterSpacing => "18",
            ParagraphProperty::GlyphScaling => "19",
            ParagraphProperty::HangingPunctuation => "21",
            ParagraphProperty::EveryLineComposer => "29",
        }
    }

    /// The property stored under `key`, or `None` for a key this module does
    /// not know. Unknown slots are refused rather than written.
    pub fn from_key(key: &str) -> Option<Self> {
        [
            Self::Justification,
            Self::FirstLineIndent,
            Self::StartIndent,
            Self::EndIndent,
            Self::SpaceBefore,
            Self::SpaceAfter,
            Self::AutoLeading,
            Self::Hyphenate,
            Self::HyphenateWordSize,
            Self::HyphenateAfterFirst,
            Self::HyphenateBeforeLast,
            Self::HyphenLimit,
            Self::HyphenationZone,
            Self::HyphenateCapitalWords,
            Self::WordSpacing,
            Self::LetterSpacing,
            Self::GlyphScaling,
            Self::HangingPunctuation,
            Self::EveryLineComposer,
        ]
        .into_iter()
        .find(|p| p.key() == key)
    }
}

/// Photoshop's paragraph justification code, as stored in `Txt2` `/5/0` and in
/// `EngineData` `Justification`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justification {
    /// `0`.
    Left,
    /// `1`.
    Right,
    /// `2`.
    Center,
    /// `3`.
    JustifyLastCenter,
    /// `4`.
    JustifyLastRight,
    /// `5`: justify every line, the last one included.
    JustifyAll,
    /// `6`: justify all but the last line (left).
    JustifyLastLeft,
}

impl Justification {
    pub fn code(self) -> i64 {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Center => 2,
            Self::JustifyLastCenter => 3,
            Self::JustifyLastRight => 4,
            Self::JustifyAll => 5,
            Self::JustifyLastLeft => 6,
        }
    }

    /// Decodes a stored code; `None` for a value Photoshop does not define.
    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Left,
            1 => Self::Right,
            2 => Self::Center,
            3 => Self::JustifyLastCenter,
            4 => Self::JustifyLastRight,
            5 => Self::JustifyAll,
            6 => Self::JustifyLastLeft,
            _ => return None,
        })
    }
}

/// Hyphenation settings of one paragraph (`Txt2` `/5/9` … `/5/15`).
///
/// Every field is the value **as stored in `Txt2`**. One conversion is worth
/// knowing: Photoshop's DOM `hyphenateWordsLongerThan` is stored as that value
/// **+ 1** (`out/probe5/pE.psd`, `hyph_params`: DOM 6 → `/5/10 = 7`), while
/// `hyphenationZone` is stored in points unchanged (`DOM 18` → `/5/14 = 18.0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hyphenation {
    /// `/9`: hyphenate at all.
    pub hyphenate: bool,
    /// `/10`: only words longer than this many characters — Photoshop's DOM
    /// value plus one.
    pub words_longer_than: i64,
    /// `/11`: characters required before the hyphen.
    pub after_first: i64,
    /// `/12`: characters required after the hyphen.
    pub before_last: i64,
    /// `/13`: maximum consecutive hyphens.
    pub limit: i64,
    /// `/14`, points: hyphenation zone.
    pub zone_pt: f32,
    /// `/15`: hyphenate capitalised words.
    pub capital_words: bool,
}

impl Default for Hyphenation {
    /// The values Photoshop's shared default paragraph sheet `/1/3` carries, so
    /// this is what it materialises when hyphenation is switched on without
    /// touching the parameters (`out/probe5/pE.psd`, `hyphenation`:
    /// `6 / 2 / 2 / 0 / 36.0 / true`). Note that `EngineData`'s default for
    /// `ConsecutiveHyphens` is `8` where `Txt2` stores `0` — the two models'
    /// default sheets disagree there, as `docs/txt2-map.md` records.
    fn default() -> Self {
        Self {
            hyphenate: false,
            words_longer_than: 6,
            after_first: 2,
            before_last: 2,
            limit: 0,
            zone_pt: 36.0,
            capital_words: true,
        }
    }
}

/// Path to paragraph run `i`'s property dictionary.
pub fn paragraph_props_path(i: usize) -> Vec<Step<'static>> {
    let mut p = table_path(false);
    p.push(Step::Index(i));
    p.push(Step::Key("0"));
    p.push(Step::Key("0"));
    p.push(Step::Key("5"));
    p
}

/// Number of paragraph runs in the layer text.
pub fn paragraph_count(block: &[u8]) -> Result<usize, Error> {
    Ok(paragraph_run_lengths(block)?.len())
}

/// Writes one scalar paragraph property of paragraph run `i`.
///
/// Fails when the template does not carry the slot; use
/// [`set_paragraph_property_or_insert`] to add it.
pub fn set_paragraph_property(
    block: &[u8],
    i: usize,
    property: ParagraphProperty,
    value: &Value,
) -> Result<Vec<u8>, Error> {
    let mut path = paragraph_props_path(i);
    path.push(Step::Key(property.key()));
    replace_value(block, &path, value)
}

/// Writes one scalar paragraph property, inserting it when the template omits
/// the slot (Photoshop leaves `/5/10` … `/5/15` out while hyphenation is off).
pub fn set_paragraph_property_or_insert(
    block: &[u8],
    i: usize,
    property: ParagraphProperty,
    value: &Value,
) -> Result<Vec<u8>, Error> {
    let mut path = paragraph_props_path(i);
    path.push(Step::Key(property.key()));
    ensure_value(block, &path, value)
}

/// Writes a `[min desired max]` triple (word/letter spacing, glyph scaling).
pub fn set_paragraph_range(
    block: &[u8],
    i: usize,
    property: ParagraphProperty,
    values: [f32; 3],
) -> Result<Vec<u8>, Error> {
    let mut path = paragraph_props_path(i);
    path.push(Step::Key(property.key()));
    let value = format!(
        "[ {} {} {} ]",
        format_real(f64::from(values[0])),
        format_real(f64::from(values[1])),
        format_real(f64::from(values[2]))
    );
    ensure_container(block, &path, value.as_bytes())
}

/// Reads one scalar paragraph property of paragraph run `i`.
///
/// `None` when the slot, the paragraph or the block is absent; the
/// `[min desired max]` triples are read with [`paragraph_range`].
pub fn paragraph_property(block: &[u8], i: usize, property: ParagraphProperty) -> Option<Value> {
    let mut path = paragraph_props_path(i);
    path.push(Step::Key(property.key()));
    scalar_at(block, &path)
}

/// Reads a `[min desired max]` triple, or `None` when the slot is absent.
pub fn paragraph_range(block: &[u8], i: usize, property: ParagraphProperty) -> Option<[f32; 3]> {
    let mut path = paragraph_props_path(i);
    path.push(Step::Key(property.key()));
    let (start, end) = value_span(&tokens(block).ok()?, &path)?;
    let text = std::str::from_utf8(&block[start..end]).ok()?;
    let body = text.strip_prefix('[')?.split(']').next()?;
    let nums: Vec<f32> = body
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    match nums.as_slice() {
        [a, b, c] => Some([*a, *b, *c]),
        _ => None,
    }
}

/// Justification of paragraph run `i` (`/5/0`).
pub fn set_paragraph_justification(
    block: &[u8],
    i: usize,
    justification: Justification,
) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::Justification,
        &Value::Int(justification.code()),
    )
}

/// Same, from a stored code. Rejects codes Photoshop does not define.
pub fn set_paragraph_justification_code(
    block: &[u8],
    i: usize,
    code: i64,
) -> Result<Vec<u8>, Error> {
    let j = Justification::from_code(code)
        .ok_or_else(|| Error::Txt2(format!("not a justification: {code} (0..6)")))?;
    set_paragraph_justification(block, i, j)
}

pub fn paragraph_justification(block: &[u8], i: usize) -> Option<Justification> {
    match paragraph_property(block, i, ParagraphProperty::Justification)? {
        Value::Int(code) => Justification::from_code(code),
        _ => None,
    }
}

/// First-line indent of paragraph run `i`, points (`/5/1`).
pub fn set_paragraph_first_line_indent(
    block: &[u8],
    i: usize,
    points: f32,
) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::FirstLineIndent,
        &Value::Real(f64::from(points)),
    )
}

pub fn paragraph_first_line_indent(block: &[u8], i: usize) -> Option<f32> {
    paragraph_number(block, i, ParagraphProperty::FirstLineIndent)
}

/// Left (start) indent of paragraph run `i`, points (`/5/2`).
pub fn set_paragraph_start_indent(block: &[u8], i: usize, points: f32) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::StartIndent,
        &Value::Real(f64::from(points)),
    )
}

pub fn paragraph_start_indent(block: &[u8], i: usize) -> Option<f32> {
    paragraph_number(block, i, ParagraphProperty::StartIndent)
}

/// Right (end) indent of paragraph run `i`, points (`/5/3`).
pub fn set_paragraph_end_indent(block: &[u8], i: usize, points: f32) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::EndIndent,
        &Value::Real(f64::from(points)),
    )
}

pub fn paragraph_end_indent(block: &[u8], i: usize) -> Option<f32> {
    paragraph_number(block, i, ParagraphProperty::EndIndent)
}

/// Auto-leading fraction of paragraph run `i` (`/5/7`, `1.2` = 120%).
pub fn set_paragraph_auto_leading(block: &[u8], i: usize, fraction: f32) -> Result<Vec<u8>, Error> {
    if !(fraction.is_finite() && fraction > 0.0) {
        return Err(Error::Txt2(format!(
            "not an auto-leading fraction: {fraction}"
        )));
    }
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::AutoLeading,
        &Value::Real(f64::from(fraction)),
    )
}

pub fn paragraph_auto_leading(block: &[u8], i: usize) -> Option<f32> {
    paragraph_number(block, i, ParagraphProperty::AutoLeading)
}

/// Hanging punctuation of paragraph run `i` (`/5/21`).
pub fn set_paragraph_hanging_punctuation(
    block: &[u8],
    i: usize,
    on: bool,
) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::HangingPunctuation,
        &Value::Bool(on),
    )
}

pub fn paragraph_hanging_punctuation(block: &[u8], i: usize) -> Option<bool> {
    match paragraph_property(block, i, ParagraphProperty::HangingPunctuation)? {
        Value::Bool(b) => Some(b),
        _ => None,
    }
}

/// Every-line composer of paragraph run `i` (`/5/29`).
pub fn set_paragraph_every_line_composer(
    block: &[u8],
    i: usize,
    on: bool,
) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::EveryLineComposer,
        &Value::Bool(on),
    )
}

pub fn paragraph_every_line_composer(block: &[u8], i: usize) -> Option<bool> {
    match paragraph_property(block, i, ParagraphProperty::EveryLineComposer)? {
        Value::Bool(b) => Some(b),
        _ => None,
    }
}

/// Writes the hyphenation slots of paragraph run `i` (`/5/9` … `/5/15`).
///
/// Photoshop only stores the detail keys while hyphenation is on — `/10` …
/// `/15` are absent in a plain run sheet — so they are inserted only when
/// `hyphenate` is true; switching hyphenation off writes `/9 false` and leaves
/// any existing detail keys alone. The zone is stored in points, the same
/// number `EngineData` `Zone` keeps (`out/probe5/pE.psd`: DOM 18 → `/5/14
/// 18.0`, `Zone 18.0`).
pub fn set_paragraph_hyphenation(
    block: &[u8],
    i: usize,
    hyphenation: Hyphenation,
) -> Result<Vec<u8>, Error> {
    if !(hyphenation.zone_pt.is_finite() && hyphenation.zone_pt >= 0.0) {
        return Err(Error::Txt2(format!(
            "not a hyphenation zone: {}",
            hyphenation.zone_pt
        )));
    }
    for (prop, value) in [
        (
            ParagraphProperty::HyphenateWordSize,
            hyphenation.words_longer_than,
        ),
        (
            ParagraphProperty::HyphenateAfterFirst,
            hyphenation.after_first,
        ),
        (
            ParagraphProperty::HyphenateBeforeLast,
            hyphenation.before_last,
        ),
        (ParagraphProperty::HyphenLimit, hyphenation.limit),
    ] {
        if value < 0 {
            return Err(Error::Txt2(format!(
                "negative hyphenation value {value} for {prop:?}"
            )));
        }
    }
    let mut out = block.to_vec();
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::Hyphenate,
        &Value::Bool(hyphenation.hyphenate),
    )?;
    if !hyphenation.hyphenate {
        return Ok(out);
    }
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenateWordSize,
        &Value::Int(hyphenation.words_longer_than),
    )?;
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenateAfterFirst,
        &Value::Int(hyphenation.after_first),
    )?;
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenateBeforeLast,
        &Value::Int(hyphenation.before_last),
    )?;
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenLimit,
        &Value::Int(hyphenation.limit),
    )?;
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenationZone,
        &Value::Real(f64::from(hyphenation.zone_pt)),
    )?;
    set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenateCapitalWords,
        &Value::Bool(hyphenation.capital_words),
    )
}

/// Hyphenation slots of paragraph run `i`.
///
/// A run sheet only carries `/10` … `/15` while hyphenation is on; absent
/// slots read back as the values Photoshop's shared default sheet `/1/3`
/// holds and materialises when hyphenation is switched on
/// (`out/probe5/pE.psd`). `None` when the paragraph run itself does not exist.
pub fn paragraph_hyphenation(block: &[u8], i: usize) -> Option<Hyphenation> {
    if paragraph_count(block).ok()? <= i {
        return None;
    }
    let default = Hyphenation::default();
    let int = |prop| match paragraph_property(block, i, prop) {
        Some(Value::Int(v)) => Some(v),
        _ => None,
    };
    let boolean = |prop, fallback| {
        paragraph_property(block, i, prop)
            .and_then(|v| v.as_bool())
            .unwrap_or(fallback)
    };
    Some(Hyphenation {
        hyphenate: boolean(ParagraphProperty::Hyphenate, default.hyphenate),
        words_longer_than: int(ParagraphProperty::HyphenateWordSize)
            .unwrap_or(default.words_longer_than),
        after_first: int(ParagraphProperty::HyphenateAfterFirst).unwrap_or(default.after_first),
        before_last: int(ParagraphProperty::HyphenateBeforeLast).unwrap_or(default.before_last),
        limit: int(ParagraphProperty::HyphenLimit).unwrap_or(default.limit),
        zone_pt: paragraph_number(block, i, ParagraphProperty::HyphenationZone)
            .unwrap_or(default.zone_pt),
        capital_words: boolean(
            ParagraphProperty::HyphenateCapitalWords,
            default.capital_words,
        ),
    })
}

fn paragraph_number(block: &[u8], i: usize, property: ParagraphProperty) -> Option<f32> {
    match paragraph_property(block, i, property)? {
        Value::Int(v) => Some(v as f32),
        Value::Real(v) => Some(v as f32),
        _ => None,
    }
}
