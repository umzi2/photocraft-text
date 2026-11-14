//! PSD text-layer reader: a `Txt2` text object plus its `TySh` block into the
//! [`LayerSpec`] model [`author`](super::author) writes — the reverse of
//! authoring.
//!
//! # Where each value comes from
//!
//! Photoshop composes from `Txt2` and keeps a second copy of most of it in the
//! layer's `TySh` `EngineData`. The two disagree where one model has no slot
//! (old-style figures, stylistic sets, hyphenate-capitalised words) and after
//! an edit one of them can be stale, so the reader has an explicit precedence:
//!
//! 1. **Text** — `TySh`'s `Txt `, else `EngineData`'s
//!    `EngineDict/Editor/Text`, else the `Txt2` object's text. The descriptor is
//!    the layer's declared text; a `Txt2` block that says something else is
//!    stale and is ignored as a whole (styles, paragraph, frame, antialias),
//!    exactly as the fork's importer refused it. U+000D is the paragraph break
//!    and the trailing break is dropped, so the result is [`LayerSpec::text`]
//!    as `author` takes it; run lengths then cover the text plus its
//!    terminator.
//! 2. **`Txt2` is used only while it is current**: the text object named by the
//!    descriptor's `TextIndex` must still hold the layer's text. A `Txt2` left
//!    behind by an edit elsewhere is ignored as a whole (styles, paragraph,
//!    frame, antialias), exactly as the fork's importer refused it.
//! 3. **Character style** — `Txt2` style run `i` (`/1 /1 [obj] /0 /6 /0 [i]`,
//!    properties under `/0 /0 /6`), with each value falling back to the
//!    `EngineData` style run covering that run's first UTF-16 unit
//!    (`crate::runs::from_engine_data`), then to the model's default. Kerning
//!    mode comes from `Txt2` `/6 /11` (`kerning_mode`), because `EngineData`
//!    writes metrics and optical alike as `AutoKerning true`; the manual
//!    amount comes from `/6 /10` (16.16 fixed point) and, failing that, from
//!    `EngineData`'s `Kerning`.
//! 4. **Paragraph** — `Txt2` paragraph run 0's sheet (`/0 /5 /0 [0] /0 /0 /5`;
//!    [`ParagraphSpec`] formats the whole layer, which is what `author` writes),
//!    else `EngineData`'s first paragraph run. Justification stays in
//!    Photoshop's own code space (0 left … 6 justify-last-left), so it needs no
//!    translation.
//! 5. **Frame** — `Txt2`'s object frame rectangle (`/99 /F` `/1`, or the frame
//!    list entry the object names) in text space, which is the space
//!    [`LayerSpec::frame`] uses; else `EngineData`'s box shape
//!    (`Rendered/Shapes/Children[0]` with `ShapeType 1` and `BoxBounds`). Point
//!    text has no frame.
//! 6. **Shape** — the `Txt2` object's frame curve (`/1 /0`, the cubic segment
//!    list, in text space through `/2 /2`) decoded by [`super::path`] into
//!    [`ShapeSpec`](super::ShapeSpec): the stored frame verbatim (so re-authoring
//!    an unchanged layer keeps its bytes) plus the path with the type's
//!    start/end/flip resolved. Box/area frames (`kind 1`) and type on a path
//!    (`kind 2`) have one; point text has no curve. [`LayerSpec::frame`] stays
//!    the composed rectangle.
//! 7. **Antialias** — the `Txt2` object code (`/10 /0`, with the missing
//!    dictionary meaning smooth), else the `TySh` `AntA` enum, else
//!    `EngineDict/AntiAlias`.
//! 8. **Orientation and transform** — `EngineData`'s
//!    `Rendered/Shapes/WritingDirection` (2 = vertical) and the `TySh` header
//!    transform; both are `None` when the file carries no such value.
//!    Photos in the wild can carry a `TySh` `Ornt` that disagrees with the
//!    engine's `WritingDirection` (an older writer left `Ornt` at `Hrzn` on
//!    vertical text, which makes Photoshop refuse the document with "Could not
//!    complete your request because of a program error"). The composer's
//!    `WritingDirection` is authoritative because it is what lays the glyphs
//!    out: when the two disagree the reader prefers it and warns, and `Ornt` is
//!    only used when the engine data has no writing direction.
//!
//! Run lengths are validated against the text: when the `Txt2` lengths do not
//! cover the text plus its terminator the reader falls back to the
//! `EngineData` runs, and either way the last run is adjusted so
//! [`author`](super::author)'s coverage check accepts the spec.

use photocraft_psd::PsdFile;
use photocraft_psd::descriptor::Value as DescValue;

use crate::engine_data::{self, Value as EdValue};
use crate::runs::{self, ParagraphRun, RunStyle};
use crate::txt2::{self, ParagraphProperty, RunProperty, Value as TxtValue};

use super::{Error, LayerSpec, ParagraphSpec, RunSpec};

/// Walks nested `EngineData` dictionaries by key.
fn ed_path<'a>(value: &'a EdValue, keys: &[&str]) -> Option<&'a EdValue> {
    let mut current = value;
    for key in keys {
        current = current.get(key)?;
    }
    Some(current)
}

/// Reads text layer `layer_index` of `file` into the authoring model.
///
/// `layer_index` is a layer index (as in [`PsdFile::layers`]), not the
/// `Txt2` object index. Errors when the layer is not a text layer or its
/// `TySh` cannot be parsed; a document without a `Txt2` block is read through
/// `EngineData` alone.
pub fn read_layer(file: &PsdFile, layer_index: usize) -> Result<LayerSpec, Error> {
    let tysh = file
        .text_layer_tysh(layer_index)
        .ok_or(Error::MissingTextLayer)??;
    let engine = match tysh.engine_data() {
        Some(raw) => Some(engine_data::parse(raw)?),
        None => None,
    };
    let object = text_index(&tysh);
    let text = choose_text(
        tysh.text().as_deref(),
        engine_text(engine.as_ref()).as_deref(),
        object
            .and_then(|object| {
                file.txt2()
                    .and_then(|block| txt2::object_text(block, object))
            })
            .as_deref(),
    );
    let txt2 = file.txt2().filter(|block| {
        object
            .and_then(|object| txt2::object_text(block, object))
            .is_some_and(|found| normalize(&found) == text)
    });
    let engine_runs = engine
        .as_ref()
        .map(runs::from_engine_data)
        .unwrap_or_default();
    let engine_paragraphs = engine
        .as_ref()
        .map(runs::paragraphs_from_engine_data)
        .unwrap_or_default();
    let units = text.encode_utf16().count().saturating_add(1);
    let runs = match (txt2, object) {
        (Some(block), Some(object)) => txt2_runs(block, object, units, &engine_runs),
        _ => Vec::new(),
    };
    let runs = if runs.is_empty() {
        engine_run_specs(&engine_runs, units)
    } else {
        runs
    };
    let mut paragraphs = match (txt2, object) {
        (Some(block), Some(object)) => paragraphs_from_txt2(block, object),
        _ => Vec::new(),
    };
    if paragraphs.is_empty() {
        paragraphs = paragraphs_from_engine(engine.as_ref(), &engine_paragraphs);
    }
    // `Txt2` and `EngineData` can disagree on a paragraph's auto-leading ratio
    // (layer 05: `Txt2` 1.5, engine 1.2). Photoshop's run-sheet `Leading` uses
    // the engine ratio, so the model takes it from there when it exists.
    for (i, engine_para) in engine_paragraphs.iter().enumerate() {
        if let Some(para) = paragraphs.get_mut(i) {
            if engine_para.auto_leading.is_finite() && engine_para.auto_leading > 0.0 {
                para.auto_leading = engine_para.auto_leading;
            }
        }
    }
    let paragraph = paragraphs
        .first()
        .cloned()
        .unwrap_or_else(|| paragraph_from_engine(&engine_paragraphs));
    let aa = txt2
        .and_then(|block| object.and_then(|object| txt2::object_antialias(block, object)))
        .or_else(|| tysh_antialias(&tysh))
        .or_else(|| {
            engine
                .as_ref()
                .and_then(|ed| ed_path(ed, &["EngineDict", "AntiAlias"]))
                .and_then(EdValue::as_i64)
        });
    let frame = match (txt2, object) {
        (Some(block), Some(object)) => {
            txt2::object_frame_geometry(block, object).map(|g| [g.left, g.top, g.right, g.bottom])
        }
        _ => None,
    }
    .or_else(|| engine_box_bounds(engine.as_ref()));
    // The frame's curve, when it has one: box/area frames and type on a path.
    // The composed rectangle in `frame` stays authoritative for box text.
    let shape = match (txt2, object) {
        (Some(block), Some(object)) => txt2::object_frame_path(block, object)
            .and_then(|frame| super::path::read_shape(super::path::FrameSource::Frame(&frame))),
        _ => None,
    };
    let engine_orientation = engine
        .as_ref()
        .and_then(|ed| {
            ed_path(
                ed,
                &["EngineDict", "Rendered", "Shapes", "WritingDirection"],
            )
        })
        .and_then(EdValue::as_i64)
        .map(|direction| i64::from(direction == 2));
    let ornt = tysh
        .text_descriptor()
        .get("Ornt")
        .is_some()
        .then(|| match tysh.orientation() {
            photocraft_psd::text_layer::Orientation::Horizontal => 0,
            photocraft_psd::text_layer::Orientation::Vertical => 1,
        });
    if let (Some(engine), Some(ornt)) = (engine_orientation, ornt)
        && engine != ornt
    {
        eprintln!(
            "photocraft-text: layer \"{}\" has TySh Ornt={} but EngineData WritingDirection={}; \
             using the engine data (Photoshop can refuse an inconsistent file)",
            tysh.text().unwrap_or_default(),
            if ornt == 1 { "Vrtc" } else { "Hrzn" },
            if engine == 1 { 2 } else { 0 }
        );
    }
    let orientation = engine_orientation.or(ornt);
    let fractional = engine
        .as_ref()
        .and_then(|ed| ed_path(ed, &["EngineDict", "UseFractionalGlyphWidths"]))
        .and_then(EdValue::as_bool);
    Ok(LayerSpec {
        text,
        runs,
        paragraph,
        paragraphs,
        aa,
        frame,
        shape,
        atlas: None,
        fractional,
        orientation,
        // The reader always normalises the stored text (the trailing U+000D
        // terminator is not paragraph text), so a spec it returns authors the
        // same bytes without needing the flag.
        preserve_trailing_break: false,
        transform: Some(tysh.transform),
    })
}

/// `EngineData`'s own copy of the layer text, normalised.
fn engine_text(engine: Option<&EdValue>) -> Option<String> {
    engine
        .and_then(|ed| ed_path(ed, &["EngineDict", "Editor", "Text"]))
        .and_then(EdValue::as_str)
        .map(normalize)
}

/// The layer text: `TySh`'s declared value, else `EngineData`'s copy, else the
/// `Txt2` object's (see the module docs).
fn choose_text(tysh: Option<&str>, engine: Option<&str>, txt2: Option<&str>) -> String {
    tysh.or(engine).or(txt2).map(normalize).unwrap_or_default()
}

/// Splits editor line endings into Photoshop's paragraph break and drops the
/// terminator.
pub(crate) fn normalize(text: &str) -> String {
    text.replace("\r\n", "\r")
        .replace('\n', "\r")
        .trim_end_matches('\r')
        .to_string()
}

/// `TextIndex` of the layer's `Txt2` text object, when the descriptor carries it.
fn text_index(tysh: &photocraft_psd::TySh) -> Option<usize> {
    match tysh.text_descriptor().get("TextIndex")? {
        DescValue::Integer(index) => usize::try_from(*index).ok(),
        DescValue::LargeInteger(index) => usize::try_from(*index).ok(),
        _ => None,
    }
}

/// The `TySh` `AntA` enum as Photoshop's antialias code.
///
/// `Anno`/`antiAliasNone` 0, `AnCr`/`antiAliasCrisp` 1,
/// `AnSt`/`antiAliasStrong` 2, `AnSm`/`antiAliasSmooth` 3,
/// `AnSh`/`antiAliasSharp` 4. The platform modes (`antiAliasPlatformGray`,
/// `antiAliasPlatformLCD`) have no code in this model and return `None`, so the
/// `EngineData` value decides.
fn tysh_antialias(tysh: &photocraft_psd::TySh) -> Option<i64> {
    let DescValue::Enumerated { value, .. } = tysh.text_descriptor().get("AntA")? else {
        return None;
    };
    match String::from_utf8_lossy(value.as_bytes()).as_ref() {
        "Anno" | "antiAliasNone" => Some(0),
        "AnCr" | "antiAliasCrisp" => Some(1),
        "AnSt" | "antiAliasStrong" => Some(2),
        "AnSm" | "antiAliasSmooth" => Some(3),
        "AnSh" | "antiAliasSharp" => Some(4),
        _ => None,
    }
}

/// Start offset (UTF-16 units) of every `EngineData` style run, plus the end.
fn starts(runs: &[RunStyle]) -> Vec<usize> {
    let mut out = Vec::with_capacity(runs.len() + 1);
    let mut at = 0usize;
    for run in runs {
        out.push(at);
        at = at.saturating_add(run.len);
    }
    out.push(at);
    out
}

/// The `EngineData` style run covering UTF-16 unit `at`.
fn engine_style_at<'a>(runs: &'a [RunStyle], starts: &[usize], at: usize) -> Option<&'a RunStyle> {
    let index = starts
        .partition_point(|start| *start <= at)
        .checked_sub(1)?;
    runs.get(index)
}

/// The style runs of the layer's current `Txt2` object as [`RunSpec`]s.
///
/// Returns an empty vector when the stored lengths do not cover `units`
/// (`text` plus its terminator) or are not positive, so the caller can fall
/// back to the `EngineData` runs.
fn txt2_runs(block: &[u8], object: usize, units: usize, engine_runs: &[RunStyle]) -> Vec<RunSpec> {
    let Ok(lengths) = txt2::run_lengths_at(block, object) else {
        return Vec::new();
    };
    if lengths.is_empty() || lengths.iter().any(|len| *len <= 0) {
        return Vec::new();
    }
    let covered: usize = lengths.iter().map(|len| *len as usize).sum();
    if covered != units {
        return Vec::new();
    }
    let starts = starts(engine_runs);
    let mut out = Vec::with_capacity(lengths.len());
    let mut at = 0usize;
    for (index, len) in lengths.iter().enumerate() {
        let len = *len as usize;
        let run = txt2_run(
            block,
            object,
            index,
            len,
            engine_style_at(engine_runs, &starts, at),
        );
        out.push(run);
        at = at.saturating_add(len);
    }
    out
}

/// One `Txt2` style run: the slot values, with the `EngineData` run at the same
/// offset filling absent slots.
fn txt2_run(
    block: &[u8],
    object: usize,
    index: usize,
    len: usize,
    engine: Option<&RunStyle>,
) -> RunSpec {
    let value = |property: RunProperty| txt2::run_property_at(block, object, index, property);
    let int = |property: RunProperty| match value(property) {
        Some(TxtValue::Int(v)) => Some(v),
        _ => None,
    };
    let real = |property: RunProperty| match value(property) {
        Some(TxtValue::Real(v)) => Some(v as f32).filter(|v| v.is_finite()),
        Some(TxtValue::Int(v)) => Some(v as f32),
        _ => None,
    };
    let flag = |property: RunProperty| match value(property) {
        Some(TxtValue::Bool(v)) => Some(v),
        _ => None,
    };
    let i16_of = |v: i64| i16::try_from(v).ok();
    let kerning_mode = int(RunProperty::KerningType);
    let auto_kerning = match kerning_mode {
        Some(0) => Some(false),
        Some(_) => Some(true),
        None => engine.and_then(|run| run.auto_kerning),
    };
    let kerning = match kerning_mode {
        Some(0) => txt2::run_manual_kerning_at(block, object, index)
            .and_then(|units| i16::try_from(units).ok())
            .or_else(|| engine.and_then(|run| run.kerning)),
        Some(_) => None,
        None => engine.and_then(|run| run.kerning),
    };
    // The engine is the authority here: PS's `Txt2` may carry a resolved
    // `Leading` (e.g. 18 pt at a 1.5 paragraph ratio -> 27) while its
    // `EngineData` still says `AutoLeading` with the 1.2 run height (21.6).
    let auto_leading =
        engine.and_then(|r| r.auto_leading).or_else(|| flag(RunProperty::AutoLeading));
    let leading = match auto_leading {
        Some(true) => None,
        _ => real(RunProperty::Leading).or_else(|| engine.and_then(|run| run.leading)),
    };
    let caps = int(RunProperty::Capitalization);
    let (all_caps, small_caps) = match caps {
        Some(1) => (false, true),
        Some(2) => (true, false),
        _ => (
            engine.is_some_and(|run| run.all_caps),
            engine.is_some_and(|run| run.small_caps),
        ),
    };
    RunSpec {
        len,
        font: int(RunProperty::FontIndex)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| txt2::font_name(block, index))
            .or_else(|| engine.and_then(|run| run.font.clone())),
        size: real(RunProperty::FontSize).or_else(|| engine.and_then(|run| run.size)),
        leading,
        auto_leading,
        tracking: int(RunProperty::Tracking)
            .and_then(i16_of)
            .or_else(|| engine.and_then(|run| run.tracking)),
        baseline_shift: real(RunProperty::BaselineShift)
            .or_else(|| engine.and_then(|run| run.baseline_shift)),
        horizontal_scale: real(RunProperty::HorizontalScale)
            .or_else(|| engine.and_then(|run| run.horizontal_scale)),
        vertical_scale: real(RunProperty::VerticalScale)
            .or_else(|| engine.and_then(|run| run.vertical_scale)),
        color: txt2::run_fill_color_at(block, object, index)
            .map(|[_, r, g, b]| color_rgb8(r, g, b))
            .or_else(|| engine.and_then(|run| run.color)),
        auto_kerning,
        kerning,
        kerning_mode,
        bold: flag(RunProperty::FauxBold)
            .unwrap_or_else(|| engine.is_some_and(|run| run.faux_bold)),
        italic: flag(RunProperty::FauxItalic)
            .unwrap_or_else(|| engine.is_some_and(|run| run.faux_italic)),
        underline: int(RunProperty::Underline)
            .map(|code| code != 0)
            .unwrap_or_else(|| engine.is_some_and(|run| run.underline)),
        strikethrough: int(RunProperty::Strikethrough)
            .map(|code| code != 0)
            .unwrap_or_else(|| engine.is_some_and(|run| run.strikethrough)),
        underline_type: int(RunProperty::Underline),
        strike_type: int(RunProperty::Strikethrough),
        all_caps,
        small_caps,
        old_style: int(RunProperty::OldStyle)
            .map(|level| level != 0)
            .unwrap_or_else(|| engine.is_some_and(|run| run.old_style)),
        ligatures: flag(RunProperty::Ligatures).or_else(|| engine.and_then(|run| run.ligatures)),
        alternate_ligatures: flag(RunProperty::AlternateLigatures)
            .or_else(|| engine.and_then(|run| run.alternate_ligatures)),
        no_break: flag(RunProperty::NoBreak).or_else(|| engine.and_then(|run| run.no_break)),
        language: int(RunProperty::Language).or_else(|| engine.and_then(|run| run.language)),
        // `EngineData` carries no stroke colour in the style-run reader, so the
        // paint is `Txt2`-only.
        baseline: int(RunProperty::FontBaseline).or_else(|| engine.and_then(|run| run.baseline)),
        rotate_roman: engine.and_then(|run| run.rotate_roman),
        features: txt2_features(block, object, index)
            .into_iter()
            .chain(engine.map(|run| run.features.clone()).unwrap_or_default())
            .collect(),
        stroke: txt2::run_stroke_color_at(block, object, index)
            .map(|[_, r, g, b]| color_rgb8(r, g, b)),
    }
}

/// An 8-bit colour from Photoshop's stored fractions.
fn color_rgb8(r: f32, g: f32, b: f32) -> [u8; 3] {
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [channel(r), channel(g), channel(b)]
}

/// The `EngineData` style runs as [`RunSpec`]s, last run adjusted so the lengths
/// cover `units`.
fn engine_run_specs(runs: &[RunStyle], units: usize) -> Vec<RunSpec> {
    if runs.is_empty() {
        return vec![RunSpec {
            len: units,
            ..Default::default()
        }];
    }
    let mut out: Vec<RunSpec> = runs
        .iter()
        .map(|run| RunSpec {
            len: run.len,
            font: run.font.clone(),
            size: run.size,
            leading: run.leading,
            auto_leading: run.auto_leading,
            tracking: run.tracking,
            baseline_shift: run.baseline_shift,
            horizontal_scale: run.horizontal_scale,
            vertical_scale: run.vertical_scale,
            color: run.color,
            auto_kerning: run.auto_kerning,
            kerning: run.kerning,
            kerning_mode: None,
            bold: run.faux_bold,
            italic: run.faux_italic,
            underline: run.underline,
            underline_type: None,
            strikethrough: run.strikethrough,
            strike_type: None,
            all_caps: run.all_caps,
            small_caps: run.small_caps,
            old_style: run.old_style,
            ligatures: run.ligatures,
            alternate_ligatures: run.alternate_ligatures,
            no_break: run.no_break,
            language: run.language,
            baseline: run.baseline,
            rotate_roman: run.rotate_roman,
            features: run.features.clone(),
            stroke: None,
        })
        .collect();
    fit_coverage(&mut out, units);
    out
}

/// Adjusts the last run so the lengths cover `units`, keeping every length
/// positive (an unrepresentable spec collapses to one run).
fn fit_coverage(runs: &mut Vec<RunSpec>, units: usize) {
    let covered: usize = runs.iter().map(|run| run.len).sum();
    if covered == units {
        return;
    }
    if let Some(last) = runs.last_mut().filter(|_| covered < units) {
        last.len = last.len.saturating_add(units - covered);
        return;
    }
    // Too wide: shrink from the end, dropping runs that would become empty.
    let mut excess = covered - units;
    while excess > 0 {
        match runs.last_mut() {
            Some(last) if last.len > excess => {
                last.len -= excess;
                return;
            }
            Some(last) => {
                excess -= last.len;
                runs.pop();
            }
            None => break,
        }
    }
    if runs.is_empty() {
        runs.push(RunSpec {
            len: units,
            ..Default::default()
        });
    }
}

/// The `Txt2` paragraph run `index` as a [`ParagraphSpec`].
fn paragraph_from_txt2(block: &[u8], object: usize, index: usize) -> ParagraphSpec {
    let value =
        |property: ParagraphProperty| txt2::paragraph_property_at(block, object, index, property);
    let int = |property: ParagraphProperty| match value(property) {
        Some(TxtValue::Int(v)) => Some(v),
        _ => None,
    };
    let real = |property: ParagraphProperty| match value(property) {
        Some(TxtValue::Real(v)) => Some(v as f32).filter(|v| v.is_finite()),
        Some(TxtValue::Int(v)) => Some(v as f32),
        _ => None,
    };
    let flag = |property: ParagraphProperty| match value(property) {
        Some(TxtValue::Bool(v)) => Some(v),
        _ => None,
    };
    let range =
        |property: ParagraphProperty| txt2::paragraph_range_at(block, object, index, property);
    ParagraphSpec {
        justification: int(ParagraphProperty::Justification).unwrap_or(0),
        every_line_composer: flag(ParagraphProperty::EveryLineComposer).unwrap_or(false),
        first_line_indent: real(ParagraphProperty::FirstLineIndent).unwrap_or(0.0),
        start_indent: real(ParagraphProperty::StartIndent).unwrap_or(0.0),
        end_indent: real(ParagraphProperty::EndIndent).unwrap_or(0.0),
        space_before: real(ParagraphProperty::SpaceBefore).unwrap_or(0.0),
        space_after: real(ParagraphProperty::SpaceAfter).unwrap_or(0.0),
        auto_leading: real(ParagraphProperty::AutoLeading).unwrap_or(1.2),
        word_spacing: range(ParagraphProperty::WordSpacing).unwrap_or([0.8, 1.0, 1.33]),
        letter_spacing: range(ParagraphProperty::LetterSpacing).unwrap_or([0.0, 0.0, 0.0]),
        glyph_scaling: range(ParagraphProperty::GlyphScaling).unwrap_or([1.0, 1.0, 1.0]),
        hanging_punctuation: flag(ParagraphProperty::HangingPunctuation).unwrap_or(false),
        hyphenate: flag(ParagraphProperty::Hyphenate).unwrap_or(false),
        hyphenate_words_longer_than: int(ParagraphProperty::HyphenateWordSize).unwrap_or(6),
        hyphenate_after_first: int(ParagraphProperty::HyphenateAfterFirst).unwrap_or(2),
        hyphenate_before_last: int(ParagraphProperty::HyphenateBeforeLast).unwrap_or(2),
        hyphenate_limit: int(ParagraphProperty::HyphenLimit).unwrap_or(0),
        hyphenate_zone: real(ParagraphProperty::HyphenationZone).unwrap_or(36.0),
        hyphenate_capital_words: flag(ParagraphProperty::HyphenateCapitalWords).unwrap_or(true),
        // Neither has a measured `Txt2` slot; they live in `EngineData`.
        burasagari: false,
        kinsoku_order: 0,
    }
}

/// Every `Txt2` paragraph run of `object` as a [`ParagraphSpec`], in order.
fn paragraphs_from_txt2(block: &[u8], object: usize) -> Vec<ParagraphSpec> {
    let count = txt2::paragraph_count_at(block, object).unwrap_or(0);
    (0..count)
        .map(|index| paragraph_from_txt2(block, object, index))
        .collect()
}

/// The `EngineData` paragraph run 0 as the layer's [`ParagraphSpec`].
fn engine_paragraph_spec(paragraph: &ParagraphRun) -> ParagraphSpec {
    ParagraphSpec {
        justification: justification_code(paragraph.justification),
        every_line_composer: paragraph.every_line_composer,
        first_line_indent: paragraph.first_line_indent,
        start_indent: paragraph.start_indent,
        end_indent: paragraph.end_indent,
        space_before: paragraph.space_before,
        space_after: paragraph.space_after,
        auto_leading: paragraph.auto_leading,
        word_spacing: paragraph.word_spacing,
        letter_spacing: paragraph.letter_spacing,
        glyph_scaling: paragraph.glyph_scaling,
        hanging_punctuation: paragraph.hanging,
        hyphenate: paragraph.hyphenate,
        hyphenate_words_longer_than: paragraph.words_longer_than,
        hyphenate_after_first: paragraph.pre_hyphen,
        hyphenate_before_last: paragraph.post_hyphen,
        hyphenate_limit: paragraph.consecutive_hyphens,
        hyphenate_zone: paragraph.hyphen_zone,
        // `EngineData` has no key for this one; `Txt2` `/5/15` is the only copy.
        hyphenate_capital_words: true,
        burasagari: false,
        kinsoku_order: 0,
    }
}

/// Every `EngineData` paragraph run as a [`ParagraphSpec`], with the two
/// `EngineData`-only keys (`Burasagari`, `KinsokuOrder`) read from the raw
/// paragraph sheets (the shared paragraph model does not carry them).
fn paragraphs_from_engine(engine: Option<&EdValue>, runs: &[ParagraphRun]) -> Vec<ParagraphSpec> {
    let items = engine
        .and_then(|ed| ed_path(ed, &["EngineDict", "ParagraphRun", "RunArray"]))
        .and_then(EdValue::as_array)
        .unwrap_or(&[]);
    runs.iter()
        .enumerate()
        .map(|(index, run)| {
            let props = items
                .get(index)
                .and_then(|item| item.get("ParagraphSheet"))
                .and_then(|sheet| sheet.get("Properties"));
            let mut spec = engine_paragraph_spec(run);
            spec.burasagari = props
                .and_then(|p| p.get("Burasagari"))
                .and_then(EdValue::as_bool)
                .unwrap_or(false);
            spec.kinsoku_order = props
                .and_then(|p| p.get("KinsokuOrder"))
                .and_then(EdValue::as_i64)
                .unwrap_or(0);
            spec
        })
        .collect()
}

/// OpenType features a `Txt2` character sheet turns on, beyond the modelled
/// `old_style`/`ligatures` flags: the seven remaining measured flags
/// (`/6/20` `calt`, `/6/23` `frac`, `/6/24` `ordn`, `/6/25` `swsh`, `/6/26`
/// `titl`, `/6/28` `salt`, `/6/29` `ornm`) and the Stylistic Sets bit mask
/// (`/6/85`: bit n selects `ss{n+1}`, n = 0…19).
fn txt2_features(block: &[u8], object: usize, index: usize) -> Vec<crate::runs::RunFeature> {
    const FLAGS: [(txt2::RunProperty, &str); 7] = [
        (txt2::RunProperty::ContextualLigatures, "calt"),
        (txt2::RunProperty::Fractions, "frac"),
        (txt2::RunProperty::Ordinals, "ordn"),
        (txt2::RunProperty::Swash, "swsh"),
        (txt2::RunProperty::Titling, "titl"),
        (txt2::RunProperty::StylisticAlternates, "salt"),
        (txt2::RunProperty::Ornaments, "ornm"),
    ];
    let flag = |property| txt2::run_property_at(block, object, index, property);
    let mut out = Vec::new();
    for (property, tag) in FLAGS {
        match flag(property).and_then(|v| v.as_bool()) {
            Some(true) => out.push(crate::runs::RunFeature {
                tag: tag.to_owned(),
                value: 1,
            }),
            Some(false) if tag == "calt" => out.push(crate::runs::RunFeature {
                tag: tag.to_owned(),
                value: 0,
            }),
            _ => {}
        }
    }
    if let Some(mask) = flag(txt2::RunProperty::StylisticSets)
        .and_then(|v| v.as_i64())
        .filter(|mask| (0..1 << 20).contains(mask))
    {
        for n in 0..20 {
            if mask & (1 << n) != 0 {
                out.push(crate::runs::RunFeature {
                    tag: format!("ss{:02}", n + 1),
                    value: 1,
                });
            }
        }
    }
    out
}

/// Photoshop's code for a layout justification.
///
/// Both `Txt2` `/5/0` and `EngineData` `Justification` use Photoshop's code
/// space, measured with the 7-alignment oracle (`out/verify/sync/justify_probe.py`):
/// `0` left, `1` right, `2` centre, `3` justify-last-left, `4` justify-last-right,
/// `5` justify-last-centre, `6` justify-all.
fn justification_code(justification: crate::area::Justification) -> i64 {
    use crate::area::Justification;
    match justification {
        Justification::Left => 0,
        Justification::Right => 1,
        Justification::Center => 2,
        Justification::JustifyLastLeft => 3,
        Justification::JustifyLastRight => 4,
        Justification::JustifyLastCenter => 5,
        Justification::JustifyAll => 6,
    }
}

/// The `EngineData` paragraph run 0 as the layer's [`ParagraphSpec`].
fn paragraph_from_engine(paragraphs: &[ParagraphRun]) -> ParagraphSpec {
    paragraphs
        .first()
        .map_or_else(ParagraphSpec::default, engine_paragraph_spec)
}

/// `EngineData`'s box frame (`Rendered/Shapes/Children[0]` with `ShapeType 1`).
///
/// Photoshop keeps the frame in `Txt2`; this is the fallback for a file whose
/// `Txt2` is missing or stale.
fn engine_box_bounds(engine: Option<&EdValue>) -> Option<[f32; 4]> {
    let shape = ed_path(engine?, &["EngineDict", "Rendered", "Shapes", "Children"])
        .and_then(EdValue::as_array)
        .and_then(<[EdValue]>::first)?;
    if shape.get("ShapeType").and_then(EdValue::as_i64) != Some(1) {
        return None;
    }
    let bounds =
        ed_path(shape, &["Cookie", "Photoshop", "BoxBounds"]).and_then(EdValue::as_array)?;
    let values: Vec<f32> = bounds.iter().filter_map(EdValue::as_f32).collect();
    match values.as_slice() {
        [left, top, right, bottom] if right > left && bottom > top => {
            Some([*left, *top, *right, *bottom])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{fit_coverage, normalize};
    use crate::psd::RunSpec;

    #[test]
    fn text_is_normalised_to_paragraph_breaks_without_the_terminator() {
        assert_eq!(normalize("A\r\nB\nC\r"), "A\rB\rC");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("\r\n\n"), "");
    }

    #[test]
    fn coverage_always_reaches_the_text_plus_terminator() {
        let mut runs = vec![RunSpec {
            len: 1,
            ..Default::default()
        }];
        fit_coverage(&mut runs, 5);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 5);
        assert_eq!(runs.len(), 1);

        let mut runs = vec![
            RunSpec {
                len: 3,
                ..Default::default()
            },
            RunSpec {
                len: 4,
                ..Default::default()
            },
        ];
        fit_coverage(&mut runs, 5);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 5);
        assert_eq!(runs[0].len, 3);
        assert_eq!(runs[1].len, 2);
    }
}
