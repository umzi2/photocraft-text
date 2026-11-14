//! Shared PSD authoring: write text, style runs, paragraph formatting, the
//! layer's renderer settings, and the pixels that go with them, into a template
//! PSD. Used by `examples/author_multi` and the live editor's PSD export.
//!
//! The copies must agree or Photoshop shows one thing and other readers
//! another:
//!
//! 1. `Txt2` text + paragraph run table (`txt2::replace_text_at`),
//! 2. `Txt2` style run table: lengths, per-run properties, the `/5 1`
//!    character-run marker, font indices, fill/stroke paints, and an emptied
//!    layout cache,
//! 3. `Txt2` paragraph properties and the layer's antialias code,
//! 4. `TySh` `EngineData`: the same split, fonts, paragraphs, antialias, text,
//! 5. the layer's prerendered pixels, re-rendered from that EngineData.
//!
//! [`author_layer`] does this for any text layer of a document: it finds the
//! layer's text object in the document `Txt2` block through the layer's `TySh`
//! `TextIndex`, writes that object, and leaves every other text layer's object
//! alone. An object that already agrees with the spec is kept byte for byte, so
//! re-saving a Photoshop file that was not edited in this crate does not
//! rewrite its text model.

use photocraft_psd::{PsdFile, TySh};

use crate::layer_render::FaceStore;
use crate::runs::{ParagraphRun, RunStyle};
use crate::txt2::{self, ParagraphProperty, Value as TxtValue};

use super::{Error, LayerSpec, ParagraphSpec, RunSpec, engine_data, pixels};

/// Photoshop's own scaffolding for a one-layer document (PS 20.0.2): the `/98`
/// version, the document resources under `/0` and the text-engine state under
/// `/1` (`/1 /0` document settings, `/1 /2` default character sheet, `/1 /3`
/// default paragraph sheet), with the object array, frame list and font list
/// left empty. A block with only a frame list and an object array is refused by
/// Photoshop at open ("General Photoshop error"), so a native block needs the
/// scaffolding too.
pub(super) const NATIVE_TXT2_SCAFFOLD: &[u8] = include_bytes!("native_txt2.ps");

/// Rewrites the first text layer using the caller's loaded fonts and atlases.
///
/// Run lengths cover the normalized text plus its trailing U+000D terminator.
/// Invalid run lengths and surrogate splits are rejected before mutation.
/// Errors leave the file unchanged; only the edited layer is staged.
pub fn author(file: &mut PsdFile, spec: &LayerSpec, store: &FaceStore) -> Result<(), Error> {
    let layer_index = file
        .layers()
        .iter()
        .position(|l| l.block(b"TySh").is_some())
        .ok_or(Error::MissingTextLayer)?;
    author_layer(file, layer_index, spec, store)
}

/// Rewrites text layer `layer_index` (an index into [`PsdFile::layers`]) using
/// the caller's loaded fonts and atlases.
///
/// The layer's `Txt2` object is the one its `TySh` `TextIndex` names; the other
/// objects of the block are kept. A layer without a `Txt2` object gets one
/// written (the document block itself is created from Photoshop's scaffolding
/// when the file has none). Errors leave the file unchanged.
pub fn author_layer(
    file: &mut PsdFile,
    layer_index: usize,
    spec: &LayerSpec,
    store: &FaceStore,
) -> Result<(), Error> {
    let text = spec_text(spec);
    validate_runs(&text, &spec.runs)?;

    let raw = file
        .layers()
        .get(layer_index)
        .and_then(|layer| layer.block(b"TySh"))
        .map(|block| block.data.clone())
        .ok_or(Error::MissingTextLayer)?;
    if raw.len() < 52 {
        // The fixed header (version, transform, text version) is missing; the
        // same error `with_engine_data` reports for a truncated block.
        return Err(Error::TruncatedTySh {
            needed: 52,
            len: raw.len(),
        });
    }
    let tysh = TySh::from_bytes(&raw)?;
    let object = object_index(&tysh);

    // 1. The `Txt2` block and this layer's object. Prepare the whole block
    // before committing anything: errors leave the file untouched.
    let previous = file.txt2().map(<[u8]>::to_vec);
    let mut block = previous
        .clone()
        .unwrap_or_else(|| NATIVE_TXT2_SCAFFOLD.to_vec());
    let agreed = previous
        .as_deref()
        .is_some_and(|prev| object_agrees(prev, object, &text, spec));
    if !agreed {
        block = write_object(block, object, &text, spec)?;
    }
    // The writing direction lives in the frame entry, not in the object's runs,
    // so it must follow `spec.orientation` even when the object itself is
    // unchanged (a re-author that only flips the orientation).
    if let Some(orientation) = spec.orientation {
        block = txt2::set_frame_vertical(&block, object, orientation == 1)?;
    }
    // Photoshop refuses to open a document whose paragraph sheet holds a
    // non-empty dictionary at `/27`/`/28` (measured: `app.open` throws and
    // never saves, while the file is otherwise valid). Clamp it to the slot's
    // `/nil` default before the block is stored.
    block = txt2::sanitise_dict_slots_at(&block, object)?;
    let changed = !agreed || previous.as_deref() != Some(block.as_slice());

    // 2. `TySh`: the text, the EngineData mirror, and the optional descriptor
    // fields the spec sets.
    let mut tysh = tysh;
    // The descriptor's `Txt ` is the text every `TySh`-only reader (and
    // Photoshop's own Text field) shows; without this the three copies disagree.
    tysh.set_text(&text);
    let styles = run_styles(spec);
    let paragraphs = paragraph_runs_for(&text, spec);
    let tysh_block = tysh_block(&tysh, &text, spec, styles, paragraphs)?;
    tysh = TySh::from_bytes(&tysh_block)?;
    if let Some(orientation) = spec.orientation {
        tysh.set_orientation(if orientation == 1 {
            photocraft_psd::text_layer::Orientation::Vertical
        } else {
            photocraft_psd::text_layer::Orientation::Horizontal
        });
    }
    if let Some(transform) = spec.transform {
        tysh.set_transform(transform);
    }

    // 3. The prerendered pixels, from the same EngineData and updated frame.
    let mut layer = file.layers()[layer_index].clone();
    let tysh_index = layer
        .blocks
        .iter()
        .position(|b| &b.key == b"TySh")
        .ok_or(Error::MissingTySh)?;
    layer.blocks[tysh_index].data = tysh.to_bytes();
    layer.blocks[tysh_index].padding = None;
    // Area text is laid out in its frame; point text is measured from the runs
    // (a point object's frame list entry is only the layer's stored rectangle,
    // not a text box, so it must not be used as one).
    let frame = if spec.frame.is_some() {
        txt2::object_frame_geometry(&block, object)
    } else {
        None
    };
    pixels::refresh_layer(
        &mut layer,
        &text,
        store,
        frame,
        file.header.depth,
        file.header.version,
    )?;

    if changed {
        file.set_txt2(Some(block));
    }
    file.layers_mut()[layer_index] = layer;
    Ok(())
}

/// The `Txt2` strike style code (`/6/15`): explicit `strike_type`, else the
/// boolean's box code.
fn strike_code(spec: &RunSpec) -> i64 {
    spec.strike_type
        .unwrap_or(if spec.strikethrough { 2 } else { 0 })
}

/// The `Txt2` underline style code (`/6/16`): explicit `underline_type`, else
/// the boolean's left code.
fn underline_code(spec: &RunSpec) -> i64 {
    spec.underline_type
        .unwrap_or(if spec.underline { 2 } else { 0 })
}

/// A rectangular area frame as Photoshop's frame list stores it: four cubic
/// segments (anchor, out handle, in handle, next anchor), handles on the
/// corners, frame kind `1` (box) and the `[-2, -2]` default segment range.
fn rect_frame_path(left: f32, top: f32, right: f32, bottom: f32) -> txt2::FramePath {
    let corners = [
        ((left, top), (right, top)),
        ((right, top), (right, bottom)),
        ((right, bottom), (left, bottom)),
        ((left, bottom), (left, top)),
    ];
    let mut points = Vec::with_capacity(32);
    for (anchor, next) in corners {
        points.extend_from_slice(&[
            anchor.0, anchor.1, anchor.0, anchor.1, next.0, next.1, next.0, next.1,
        ]);
    }
    txt2::FramePath {
        points: points.into_iter().map(f64::from).collect(),
        kind: 1,
        range: [-2.0, -2.0],
        // Photoshop's box frames carry no `/2 /11 /0`; keep the donor's flip
        // flag instead of adding one.
        flip: None,
        matrix: None,
        vertical: None,
    }
}

/// The text object a layer's `TySh` names (`TextIndex`), 0 when absent.
fn object_index(tysh: &TySh) -> usize {
    tysh.text_index()
        .and_then(|index| usize::try_from(index).ok())
        .unwrap_or(0)
}

/// Builds the layer's `TySh` block with `text` and an `EngineData` tree that
/// mirrors `spec`.
///
/// A block whose `EngineData` cannot be read (a writer left the payload empty or
/// malformed) gets a fresh tree from [`engine_data::build_engine_data`]; a real
/// Photoshop tree is edited in place so its unmodelled keys survive.
pub(super) fn tysh_block(
    tysh: &TySh,
    text: &str,
    spec: &LayerSpec,
    mut styles: Vec<RunStyle>,
    paragraphs: Vec<ParagraphRun>,
) -> Result<Vec<u8>, Error> {
    let block = tysh.to_bytes();
    match engine_data::with_engine_data(&block, |ed| {
        engine_data::apply_engine_data(ed, text, spec, &mut styles, &paragraphs);
    }) {
        Ok(edited) => Ok(edited),
        Err(_) => {
            let fresh = engine_data::build_engine_data(text, spec, &mut styles, &paragraphs);
            let bytes = crate::engine_data::write(&fresh);
            let mut fresh_tysh = tysh.clone();
            fresh_tysh.set_engine_data(&bytes);
            Ok(fresh_tysh.to_bytes())
        }
    }
}

/// Normalises a PSD paragraph break (U+000D) out of editor input (LF or CRLF).
pub(super) fn normalize(text: &str) -> String {
    text.replace("\r\n", "\r")
        .replace('\n', "\r")
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

/// The text a spec authors: [`normalize`], plus one U+000D per trailing hard
/// break [`normalize`] trimmed when [`LayerSpec::preserve_trailing_break`] is
/// set. The fork's editor contract keeps the empty final paragraph a caret can
/// sit on (`"abc\n"` -> `"abc\r"`), which the default trimming drops.
pub(super) fn spec_text(spec: &LayerSpec) -> String {
    if spec.preserve_trailing_break {
        let normalized = spec.text.replace("\r\n", "\r").replace('\n', "\r");
        let trimmed = normalized.trim_end_matches('\r');
        let mut out = trimmed.to_string();
        out.extend(std::iter::repeat_n('\r', normalized.len() - trimmed.len()));
        out
    } else {
        normalize(&spec.text)
    }
}

pub(super) fn validate_runs(text: &str, runs: &[RunSpec]) -> Result<(), Error> {
    if runs.is_empty() {
        return Err(Error::NoRuns);
    }
    let expected = text
        .encode_utf16()
        .count()
        .checked_add(1)
        .ok_or(Error::RunLengthOverflow)?;
    let mut covered = 0usize;
    for (index, run) in runs.iter().enumerate() {
        if run.len == 0 {
            return Err(Error::ZeroLengthRun { index });
        }
        i64::try_from(run.len).map_err(|_| Error::RunLengthOverflow)?;
        covered = covered
            .checked_add(run.len)
            .ok_or(Error::RunLengthOverflow)?;
    }
    if covered != expected {
        return Err(Error::RunCoverage { covered, expected });
    }

    let mut chars = text.chars().chain(std::iter::once('\r'));
    let mut offset = 0;
    let mut boundary = 0;
    for (index, run) in runs.iter().enumerate() {
        offset += run.len;
        for ch in chars.by_ref() {
            boundary += ch.len_utf16();
            if boundary >= offset {
                break;
            }
        }
        if boundary != offset {
            return Err(Error::SurrogateSplit { index, offset });
        }
    }
    Ok(())
}

/// One paragraph run per paragraph, each owning its break, carrying the text's
/// character count so the EngineData run table matches the text.
pub(super) fn paragraph_runs_for(text: &str, spec: &LayerSpec) -> Vec<ParagraphRun> {
    text.split('\r')
        .enumerate()
        .map(|(index, p)| paragraph_run(p, paragraph_spec(spec, index)))
        .collect()
}

/// The paragraph sheet for run `index`: [`LayerSpec::paragraphs`] wins over the
/// single [`LayerSpec::paragraph`], a shorter list repeats its last entry and
/// an empty list falls back to `paragraph` (the convention `read_layer` fills
/// and `apply_paragraph_formats` uses).
pub(super) fn paragraph_spec(spec: &LayerSpec, index: usize) -> &ParagraphSpec {
    if spec.paragraphs.is_empty() {
        return &spec.paragraph;
    }
    &spec.paragraphs[index.min(spec.paragraphs.len() - 1)]
}

/// One paragraph run for `paragraph` (which owns its U+000D break).
pub(super) fn paragraph_run(paragraph: &str, para: &ParagraphSpec) -> ParagraphRun {
    ParagraphRun {
        len: paragraph.encode_utf16().count() + 1,
        word_spacing: para.word_spacing,
        letter_spacing: para.letter_spacing,
        glyph_scaling: para.glyph_scaling,
        justification: crate::area::Justification::from_psd(para.justification as i32),
        every_line_composer: para.every_line_composer,
        first_line_indent: para.first_line_indent,
        start_indent: para.start_indent,
        end_indent: para.end_indent,
        space_before: para.space_before,
        space_after: para.space_after,
        auto_leading: para.auto_leading,
        hanging: para.hanging_punctuation,
        hyphenate: para.hyphenate,
        // Photoshop always writes the engine defaults for these slots
        // (`HyphenatedWordSize 6 / PreHyphen 2 / PostHyphen 2 /
        // ConsecutiveHyphens 8 / Zone 36`) whether or not hyphenation is on;
        // zeroing them on an unhyphenated paragraph makes the sheet differ
        // from Photoshop's.
        words_longer_than: para.hyphenate_words_longer_than,
        pre_hyphen: para.hyphenate_after_first,
        post_hyphen: para.hyphenate_before_last,
        consecutive_hyphens: para.hyphenate_limit,
        hyphen_zone: para.hyphenate_zone,
    }
}

/// The line height Photoshop writes beside `AutoLeading: true` for run `index`:
/// `size x` the auto-leading fraction of the paragraph the run starts in
/// (`/5/7`, `1.2` unless the paragraph overrides it). The writer-parity oracle
/// measured `22 pt x 1.5 = 33.0` for an `auto_leading: 1.5` paragraph and
/// `48 pt x 1.2 = 57.6` for the default.
fn auto_leading_pt(spec: &LayerSpec, index: usize) -> Option<f32> {
    let run = spec.runs.get(index)?;
    if run.leading.is_some() || run.auto_leading == Some(false) {
        return None;
    }
    let size = run.size?;
    // The paragraph a run starts in: cumulative UTF-16 run lengths against the
    // paragraph lengths (`paragraph_run` counts each paragraph's own break).
    let mut start = 0i64;
    for previous in &spec.runs[..index] {
        start += previous.len as i64;
    }
    let mut paragraph = 0usize;
    let mut end = 0i64;
    for (i, piece) in spec.text.split('\r').enumerate() {
        end += piece.encode_utf16().count() as i64 + 1;
        if start < end {
            paragraph = i;
            break;
        }
    }
    let ratio = paragraph_spec(spec, paragraph).auto_leading;
    Some((size * ratio * 1000.0).round() / 1000.0)
}

/// Style runs of the layer for the `EngineData` mirror. `stroke` has no
/// `EngineData` slot in this crate's `RunStyle`, so it reaches `Txt2` (and the
/// stored pixels) only.
pub(super) fn run_styles(spec: &LayerSpec) -> Vec<RunStyle> {
    spec.runs
        .iter()
        .enumerate()
        .map(|(index, r)| {
            // The mirror Photoshop keeps beside an auto-leading run: its line
            // height is `size x` the paragraph ratio, and the auto flag must
            // stay on (`leading: Some` alone would read back as explicit).
            let auto_pt = auto_leading_pt(spec, index);
            // An auto-leading run's sheet `Leading` follows the covering
            // paragraph's ratio (measured: 22 pt at 1.5 -> 33, 48 pt at 1.5 ->
            // 72), so a stored mirror value must not override the ratio the
            // spec resolves; only an explicit leading wins.
            let auto = r.auto_leading == Some(true)
                || (r.auto_leading.is_none() && r.leading.is_none());
            let leading = if auto { auto_pt.or(r.leading) } else { r.leading };
            RunStyle {
                leading,
                auto_leading: r.auto_leading.or(if auto { Some(true) } else { None }),
                len: r.len,
                font: r.font.clone(),
                size: r.size,
                tracking: r.tracking,
                baseline_shift: r.baseline_shift,
                horizontal_scale: r.horizontal_scale,
                vertical_scale: r.vertical_scale,
                color: r.color,
                auto_kerning: r.auto_kerning,
                kerning: r.kerning,
                faux_bold: r.bold,
                faux_italic: r.italic,
                underline: r.underline,
                underline_type: r.underline_type,
                strikethrough: r.strikethrough,
                strike_type: r.strike_type,
                all_caps: r.all_caps,
                small_caps: r.small_caps,
                old_style: r.old_style,
                ligatures: r.ligatures,
                alternate_ligatures: r.alternate_ligatures,
                no_break: r.no_break,
                language: r.language,
                baseline: r.baseline,
                rotate_roman: r.rotate_roman,
                features: r.features.clone(),
                ..RunStyle::default()
            }
        })
        .collect()
}

/// The Txt2 kerning mode of a run: `0` manual, `1` metrics, `2` optical.
fn kerning_mode(run: &RunSpec) -> Option<i64> {
    run.kerning_mode
        .or_else(|| run.auto_kerning.map(|auto| if auto { 1 } else { 0 }))
}

/// Writes one object of the document block.
///
/// An existing object keeps its shape — the view naming its frame, its
/// paragraph runs, the keys this writer does not model — with the text and the
/// run tables refitted; a missing one is created from the spec. Either way the
/// per-run and per-paragraph properties, the antialias code, the frame and the
/// composed-layout cache are written afterwards by the same code.
pub(super) fn write_object(
    mut block: Vec<u8>,
    object: usize,
    text: &str,
    spec: &LayerSpec,
) -> Result<Vec<u8>, Error> {
    let existing = txt2::object_text(&block, object).is_some();
    if existing {
        // A changed object loses the glyph pen positions (`/21`) that belonged
        // to the old text; the run tables and the cache are refitted below.
        block = txt2::replace_text_at(&block, object, text)?;
        block = txt2::drop_object_entry(&block, object, "21")?;
    } else {
        block = txt2::ensure_object(&block, object)?;
        block = txt2::insert_object(&block, object, &fresh_object(text, spec, object))?;
    }
    block = txt2::ensure_style_run_at(&block, object)?;
    block = txt2::set_run_layout_at(&block, object, &runs_len(&spec.runs))?;

    // Per-run character properties, the `/5 1` marker and the font indices.
    for (i, run) in spec.runs.iter().enumerate() {
        block = txt2::set_character_run_marker(&block, object, i)?;
        block = write_run(&block, object, i, run, auto_leading_pt(spec, i))?;
    }

    // Paragraph properties, per paragraph run. A hand-built/minimal block may
    // carry no paragraph table; nothing is patched then (the text still writes).
    let count = txt2::paragraph_count_at(&block, object).unwrap_or(0);
    for i in 0..count {
        let para = paragraph_spec(spec, i);
        block = txt2::set_paragraph_property_or_insert_at(
            &block,
            object,
            i,
            ParagraphProperty::Justification,
            &TxtValue::Int(justification(para.justification).code()),
        )?;
        for (prop, value) in [
            (ParagraphProperty::FirstLineIndent, para.first_line_indent),
            (ParagraphProperty::StartIndent, para.start_indent),
            (ParagraphProperty::EndIndent, para.end_indent),
            (ParagraphProperty::SpaceBefore, para.space_before),
            (ParagraphProperty::SpaceAfter, para.space_after),
            (ParagraphProperty::AutoLeading, para.auto_leading),
        ] {
            block = txt2::set_paragraph_property_or_insert_at(
                &block,
                object,
                i,
                prop,
                &TxtValue::Real(f64::from(value)),
            )?;
        }
        block = txt2::set_paragraph_property_or_insert_at(
            &block,
            object,
            i,
            ParagraphProperty::EveryLineComposer,
            &TxtValue::Bool(para.every_line_composer),
        )?;
        block = txt2::set_paragraph_property_or_insert_at(
            &block,
            object,
            i,
            ParagraphProperty::HangingPunctuation,
            &TxtValue::Bool(para.hanging_punctuation),
        )?;
        for (prop, values) in [
            (ParagraphProperty::WordSpacing, para.word_spacing),
            (ParagraphProperty::LetterSpacing, para.letter_spacing),
            (ParagraphProperty::GlyphScaling, para.glyph_scaling),
        ] {
            block = txt2::set_paragraph_range_at(&block, object, i, prop, values)?;
        }
        block = txt2::set_paragraph_hyphenation_at(
            &block,
            object,
            i,
            txt2::Hyphenation {
                hyphenate: para.hyphenate,
                words_longer_than: para.hyphenate_words_longer_than,
                after_first: para.hyphenate_after_first,
                before_last: para.hyphenate_before_last,
                limit: para.hyphenate_limit,
                zone_pt: para.hyphenate_zone,
                capital_words: para.hyphenate_capital_words,
            },
        )?;
    }

    if let Some(code) = spec.aa {
        block = txt2::set_antialias_at(&block, object, code)?;
    }
    // The composed rectangle lives in the object's `/99 /F` frame dictionary. A
    // fresh object the writer created has none (its frame is the frame-list
    // entry); there is nothing to patch then.
    if let Some([left, top, right, bottom]) = spec.frame
        && txt2::object_frame_geometry(&block, object).is_some()
    {
        block = txt2::set_frame_geometry_at(
            &block,
            object,
            txt2::FrameGeometry {
                left,
                top,
                right,
                bottom,
            },
        )?;
        // Photoshop composes area text from the **frame list** (`/0/8/0[i]`),
        // not from the object's `/99 /F` mirror: two files authored with frame
        // widths 200 and 400 were both recomposed at the template's 430 until
        // the frame-list points followed (`out/verify/sync/probes/fw_200/400`).
        // A rectangle is four cubic segments whose handles sit on the corners
        // (Photoshop's own box path).
        if spec.shape.is_none() && txt2::object_frame_index(&block, object).is_some() {
            block = txt2::set_object_frame_path(
                &block,
                object,
                &rect_frame_path(left, top, right, bottom),
            )?;
        }
    }
    // A non-rectangular frame lives in the document frame list the object's
    // view names; the setter leaves an equal path byte for byte. A stored
    // frame is re-emitted verbatim (an unedited layer keeps Photoshop's
    // bytes); a decoded-curve-only shape is baked by `to_frame_parts`.
    if let Some(shape) = &spec.shape
        && let Some(frame) = super::path::to_frame_parts(shape)
    {
        block = txt2::set_object_frame_path(&block, object, &frame)?;
    } else if spec.shape.is_none()
        && spec.frame.is_none()
        && txt2::object_frame_path(&block, object).is_some()
    {
        // Taken off its path: a point frame, or the layer reopens on the path.
        block = txt2::set_point_frame(&block, object)?;
    }
    txt2::clear_cache(&block, object).map_err(Error::from)
}

/// Writes one style run's character properties into object `object`.
fn write_run(
    block: &[u8],
    object: usize,
    run: usize,
    spec: &RunSpec,
    auto_leading_pt: Option<f32>,
) -> Result<Vec<u8>, Error> {
    let mut out = block.to_vec();
    if let Some(name) = &spec.font {
        // A hand-built/minimal block may carry no font table at all; the run then
        // keeps its (default) font index instead of losing the whole object.
        if let Ok((next, index)) = txt2::ensure_font(&out, name) {
            out = next;
            out = set_run_value(
                &out,
                object,
                run,
                txt2::RunProperty::FontIndex,
                TxtValue::Int(index as i64),
            )?;
        }
    }
    if let Some(v) = spec.size {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::FontSize,
            TxtValue::Real(f64::from(v)),
        )?;
    }
    if let Some(v) = spec.leading {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::AutoLeading,
            TxtValue::Bool(false),
        )?;
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::Leading,
            TxtValue::Real(f64::from(v)),
        )?;
    } else {
        // No explicit leading: the model means auto leading (a `None` pair),
        // and Photoshop writes `AutoLeading true` plus the computed line height
        // (`size x 1.2`; measured 48 pt -> 57.6, 22 pt -> 26.4 by the
        // writer-parity oracle). Leaving the template's stale `/6/5` is what
        // made a 22 pt run keep the 48 pt template's 57.6.
        let auto = spec.auto_leading.unwrap_or(true);
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::AutoLeading,
            TxtValue::Bool(auto),
        )?;
        if auto {
            let leading = auto_leading_pt
                .map(f64::from)
                .or_else(|| spec.size.map(|size| f64::from(size) * 1.2));
            if let Some(leading) = leading {
                out = set_run_value(
                    &out,
                    object,
                    run,
                    txt2::RunProperty::Leading,
                    TxtValue::Real((leading * 1000.0).round() / 1000.0),
                )?;
            }
        }
    }
    if let Some(v) = spec.tracking {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::Tracking,
            TxtValue::Int(i64::from(v)),
        )?;
    }
    if let Some(v) = spec.baseline {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::FontBaseline,
            TxtValue::Int(v),
        )?;
    }
    if let Some(v) = spec.baseline_shift {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::BaselineShift,
            TxtValue::Real(f64::from(v)),
        )?;
    }
    if let Some(v) = spec.horizontal_scale {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::HorizontalScale,
            TxtValue::Real(f64::from(v)),
        )?;
    }
    if let Some(v) = spec.vertical_scale {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::VerticalScale,
            TxtValue::Real(f64::from(v)),
        )?;
    }
    if let Some(mode) = kerning_mode(spec) {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::KerningType,
            TxtValue::Int(mode),
        )?;
    }
    if let Some(v) = spec.kerning {
        // Photoshop stores `/6/10` as 16.16 fixed point (kern / 65536), not in
        // 1/1000 em like `EngineData`'s `Kerning`; the model keeps 1/1000 em.
        out = set_run_scalar(
            &out,
            object,
            run,
            "10",
            TxtValue::Real(f64::from(v) / 65_536.0),
        )?;
    }
    // Character flags. Photoshop's own `Txt2` sheet carries these slots; the
    // values are the ones measured in `docs/txt2-map.md` (`/6/30` old-style is
    // level `2` for on, strike/underline keep their style codes).
    out = set_run_value(
        &out,
        object,
        run,
        txt2::RunProperty::FauxBold,
        TxtValue::Bool(spec.bold),
    )?;
    out = set_run_value(
        &out,
        object,
        run,
        txt2::RunProperty::FauxItalic,
        TxtValue::Bool(spec.italic),
    )?;
    out = set_run_value(
        &out,
        object,
        run,
        txt2::RunProperty::Strikethrough,
        TxtValue::Int(strike_code(spec)),
    )?;
    out = set_run_value(
        &out,
        object,
        run,
        txt2::RunProperty::Underline,
        TxtValue::Int(underline_code(spec)),
    )?;
    out = set_run_value(
        &out,
        object,
        run,
        txt2::RunProperty::Capitalization,
        TxtValue::Int(caps_code(spec)),
    )?;
    out = set_run_value(
        &out,
        object,
        run,
        txt2::RunProperty::OldStyle,
        TxtValue::Int(if spec.old_style { 2 } else { 0 }),
    )?;
    if let Some(on) = spec.ligatures {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::Ligatures,
            TxtValue::Bool(on),
        )?;
    }
    if let Some(on) = spec.alternate_ligatures {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::AlternateLigatures,
            TxtValue::Bool(on),
        )?;
    }
    if let Some(on) = spec.no_break {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::NoBreak,
            TxtValue::Bool(on),
        )?;
    }
    if let Some(code) = spec.language {
        out = set_run_value(
            &out,
            object,
            run,
            txt2::RunProperty::Language,
            TxtValue::Int(code),
        )?;
    }
    out = write_features(out, object, run, &spec.features)?;
    // Paints go through the shared style module too: it owns the
    // `<< /99 /SimplePaint … >>` container and its real formatting.
    if let Some(color) = spec.color {
        out = txt2::set_run_fill_color_at(&out, object, run, paint(color))?;
    }
    if let Some(color) = spec.stroke {
        out = txt2::set_run_stroke_color_at(&out, object, run, paint(color))?;
    }
    Ok(out)
}

/// The `Txt2` character sheet slot of an OpenType feature tag, for the seven
/// flags Photoshop stores individually (`calt`, `frac`, `ordn`, `swsh`, `titl`,
/// `salt`, `ornm`). Stylistic sets share the `/6/85` mask instead.
fn feature_property(tag: &str) -> Option<txt2::RunProperty> {
    match tag {
        "calt" => Some(txt2::RunProperty::ContextualLigatures),
        "frac" => Some(txt2::RunProperty::Fractions),
        "ordn" => Some(txt2::RunProperty::Ordinals),
        "swsh" => Some(txt2::RunProperty::Swash),
        "titl" => Some(txt2::RunProperty::Titling),
        "salt" => Some(txt2::RunProperty::StylisticAlternates),
        "ornm" => Some(txt2::RunProperty::Ornaments),
        _ => None,
    }
}

/// `ss01`…`ss20` as the bit `n - 1` of the Stylistic Sets mask.
fn stylistic_set_index(tag: &str) -> Option<u32> {
    let digits = tag.strip_prefix("ss")?;
    let set: u32 = digits.parse().ok()?;
    (1..=20).contains(&set).then_some(set - 1)
}

/// The Stylistic Sets mask a feature list selects (`/6/85`, bit n = `ss{n+1}`).
fn stylistic_set_mask(features: &[crate::runs::RunFeature]) -> Option<i64> {
    let mut mask = 0i64;
    let mut found = false;
    for feature in features {
        if let Some(index) = stylistic_set_index(&feature.tag) {
            found = true;
            if feature.value > 0 {
                mask |= 1 << index;
            }
        }
    }
    found.then_some(mask)
}

/// Writes the OpenType features the `Txt2` character sheet can hold.
fn write_features(
    mut out: Vec<u8>,
    object: usize,
    run: usize,
    features: &[crate::runs::RunFeature],
) -> Result<Vec<u8>, Error> {
    for feature in features {
        if let Some(property) = feature_property(&feature.tag) {
            out = set_run_value(
                &out,
                object,
                run,
                property,
                TxtValue::Bool(feature.value > 0),
            )?;
        }
    }
    // A rewritten sheet must not keep the previous text's alternates: Photoshop's
    // sheet carries both slots, and the spec's values (off / no sets) replace the
    // old ones (a stale `/28 true` made an edited layer keep the old `salt`).
    let salt = features.iter().any(|f| f.tag == "salt" && f.value > 0);
    out = set_run_value(
        &out,
        object,
        run,
        txt2::RunProperty::StylisticAlternates,
        TxtValue::Bool(salt),
    )?;
    out = set_run_value(
        &out,
        object,
        run,
        txt2::RunProperty::StylisticSets,
        TxtValue::Int(stylistic_set_mask(features).unwrap_or(0)),
    )?;
    Ok(out)
}

/// Whether the features the spec sets match the sheet's slots.
fn features_agree(
    block: &[u8],
    object: usize,
    run: usize,
    features: &[crate::runs::RunFeature],
) -> bool {
    for feature in features {
        if let Some(property) = feature_property(&feature.tag)
            && !flag_agrees(block, object, run, property.key(), feature.value > 0)
        {
            return false;
        }
    }
    let salt = features.iter().any(|f| f.tag == "salt" && f.value > 0);
    if !flag_agrees(
        block,
        object,
        run,
        txt2::RunProperty::StylisticAlternates.key(),
        salt,
    ) {
        return false;
    }
    run_integer(block, object, run, txt2::RunProperty::StylisticSets.key()).unwrap_or(0)
        == stylistic_set_mask(features).unwrap_or(0)
}

/// Writes a scalar into run `run`'s character sheet through the
/// object-aware style setter, inserting the slot when the template's sheet
/// omits it.
fn set_run_value(
    block: &[u8],
    object: usize,
    run: usize,
    property: txt2::RunProperty,
    value: TxtValue,
) -> Result<Vec<u8>, Error> {
    txt2::set_run_ensured_at(block, object, run, property, &value).map_err(Error::from)
}

/// Writes a scalar under a raw `Txt2` key (for slots without a
/// [`txt2::RunProperty`], currently only the 16.16 fixed manual kern `/6/10`).
fn set_run_scalar(
    block: &[u8],
    object: usize,
    run: usize,
    key: &str,
    value: TxtValue,
) -> Result<Vec<u8>, Error> {
    let mut path = txt2::run_path_for_at(object, run);
    for step in ["0", "0", "6"] {
        path.push(txt2::Step::Key(step));
    }
    path.push(txt2::Step::Key(key));
    txt2::ensure_value(block, &path, &value).map_err(Error::from)
}

/// A run colour as Photoshop's `SimplePaint` wants it: opaque, straight RGB
/// in 0…1 (the model stores 8-bit channels).
fn paint(color: [u8; 3]) -> [f32; 4] {
    [
        1.0,
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
    ]
}

/// The run lengths of a spec. Callers count the terminal U+000D in the last run
/// (it is part of the string Photoshop stores), so these are the lengths the
/// object carries; `set_run_layout_at` only adds it when no run covers it.
fn runs_len(spec: &[RunSpec]) -> Vec<i64> {
    spec.iter().map(|r| r.len as i64).collect()
}

/// Photoshop's `Txt2` justification code for the model's code.
fn justification(code: i64) -> txt2::Justification {
    txt2::Justification::from_code(code).unwrap_or(txt2::Justification::Left)
}

/// A text object for slot `object` of a block that has none: the model (text,
/// one style run, one paragraph run per paragraph), a view naming the layer's
/// frame and the `/2 [ ]` "not laid out" cache marker Photoshop recomposes.
fn fresh_object(text: &str, spec: &LayerSpec, object: usize) -> Vec<u8> {
    let units = text.encode_utf16().count() + 1;
    let mut para_runs = Vec::new();
    for paragraph in text.split('\r') {
        let len = paragraph.encode_utf16().count() + 1;
        para_runs.extend_from_slice(
            format!(" << /0 << /0 << /0 () /5 << >> /6 1 >> >> /1 {len} >>").as_bytes(),
        );
    }
    let style = format!("<< /0 << /0 << /0 () /5 1 /6 << /0 0 /11 1 >> >> >> /1 {units} >>");
    let aa = spec.aa.unwrap_or(3);
    let mut out = Vec::new();
    out.extend_from_slice(b"<< /0 << /0 (");
    out.extend_from_slice(&txt2::utf16(text));
    out.extend_from_slice(b") /5 << /0 [");
    out.extend_from_slice(&para_runs);
    out.extend_from_slice(b" ] >> /6 << /0 [ ");
    out.extend_from_slice(style.as_bytes());
    out.extend_from_slice(
        format!(" ] >> /10 << /0 {aa} /2 true >> >> /1 << /0 [ << /0 {object} >> ] /2 [ ] >> >>")
            .as_bytes(),
    );
    out
}

/// Whether the file's object `object` already carries what `spec` describes:
/// the same text, the same run split and every property the spec sets.
///
/// A property the spec leaves unset is not evidence of a mismatch, so an
/// untouched Photoshop object passes even though this writer models only part of
/// its sheet.
pub(super) fn object_agrees(block: &[u8], object: usize, text: &str, spec: &LayerSpec) -> bool {
    // `object_text` decodes the model's string without the trailing U+000D
    // terminator Photoshop stores after it.
    if txt2::object_text(block, object).as_deref() != Some(text) {
        return false;
    }
    // A hand-built object may carry an empty run table (the writer inserts one
    // when it rewrites); only a table with runs can disagree.
    match txt2::run_lengths_at(block, object) {
        Ok(lengths) if lengths.is_empty() => {}
        Ok(lengths) if lengths != runs_len(&spec.runs) => return false,
        Ok(_) => {}
        Err(_) => return false,
    }
    // A missing paragraph table cannot disagree (a minimal object may have none).
    if let Ok(count) = txt2::paragraph_count_at(block, object)
        && count != text.split('\r').count()
    {
        return false;
    }
    // An object that stores no antialias code cannot disagree with one.
    if let Some(code) = spec.aa
        && txt2::object_antialias(block, object).is_some_and(|stored| stored != code)
    {
        return false;
    }
    let stored_path = txt2::object_frame_path(block, object);
    if let Some(shape) = &spec.shape
        && let Some(frame) = super::path::to_frame_parts(shape)
        && stored_path != Some(frame)
    {
        return false;
    }
    // A layer taken off its path asks for no frame at all; its object must not
    // keep the old path frame, or it reopens on the path.
    if spec.shape.is_none() && spec.frame.is_none() && stored_path.is_some() {
        return false;
    }
    if let Some(frame) = spec.frame {
        let geometry = txt2::object_frame_geometry(block, object);
        let same = geometry.is_some_and(|g| {
            (g.left - frame[0]).abs() < 0.01
                && (g.top - frame[1]).abs() < 0.01
                && (g.right - frame[2]).abs() < 0.01
                && (g.bottom - frame[3]).abs() < 0.01
        });
        if !same {
            return false;
        }
    }
    // Only runs the object actually stores can disagree: a minimal block with an
    // empty (or shorter) run table keeps the defaults rather than forcing a rewrite.
    let stored_runs = txt2::run_count_at(block, object).unwrap_or(0);
    for (i, run) in spec.runs.iter().enumerate().take(stored_runs) {
        if !run_agrees(block, object, i, run) {
            return false;
        }
    }
    let count = txt2::paragraph_count_at(block, object).unwrap_or(0);
    for i in 0..count {
        let para = paragraph_spec(spec, i);
        let same = |prop, want: f32| {
            txt2::paragraph_property_at(block, object, i, prop)
                .and_then(|v| match v {
                    TxtValue::Real(v) => Some(v as f32),
                    TxtValue::Int(v) => Some(v as f32),
                    _ => None,
                })
                .is_none_or(|have| (have - want).abs() < 0.01)
        };
        if txt2::paragraph_justification_at(block, object, i)
            .map(|j| j.code())
            .is_some_and(|code| code != para.justification)
        {
            return false;
        }
        for (prop, want) in [
            (ParagraphProperty::FirstLineIndent, para.first_line_indent),
            (ParagraphProperty::StartIndent, para.start_indent),
            (ParagraphProperty::EndIndent, para.end_indent),
            (ParagraphProperty::SpaceBefore, para.space_before),
            (ParagraphProperty::SpaceAfter, para.space_after),
            (ParagraphProperty::AutoLeading, para.auto_leading),
        ] {
            if !same(prop, want) {
                return false;
            }
        }
        if txt2::paragraph_every_line_composer_at(block, object, i)
            .is_some_and(|on| on != para.every_line_composer)
            || txt2::paragraph_hanging_punctuation_at(block, object, i)
                .is_some_and(|on| on != para.hanging_punctuation)
        {
            return false;
        }
    }
    true
}

/// A scalar of run `run`'s character sheet as a float.
fn run_number(block: &[u8], object: usize, run: usize, key: &str) -> Option<f32> {
    match read_run_scalar(block, object, run, key)? {
        TxtValue::Real(v) => Some(v as f32),
        TxtValue::Int(v) => Some(v as f32),
        _ => None,
    }
}

/// A scalar of run `run`'s character sheet as a boolean (Photoshop stores some
/// flags as `true`/`false` and others as 0/2).
fn run_flag(block: &[u8], object: usize, run: usize, key: &str) -> Option<bool> {
    match read_run_scalar(block, object, run, key)? {
        TxtValue::Bool(b) => Some(b),
        TxtValue::Int(v) => Some(v != 0),
        _ => None,
    }
}

/// A scalar of run `run`'s character sheet as an integer.
fn run_integer(block: &[u8], object: usize, run: usize, key: &str) -> Option<i64> {
    match read_run_scalar(block, object, run, key)? {
        TxtValue::Int(v) => Some(v),
        TxtValue::Real(v) => Some(v as i64),
        _ => None,
    }
}

/// Whether a float property the spec sets matches the block (`None` from the
/// block means the template has no such slot, which is not a mismatch).
fn number_agrees(block: &[u8], object: usize, run: usize, key: &str, want: f32) -> bool {
    run_number(block, object, run, key).is_none_or(|have| (have - want).abs() < 0.01)
}

fn flag_agrees(block: &[u8], object: usize, run: usize, key: &str, want: bool) -> bool {
    run_flag(block, object, run, key).is_none_or(|have| have == want)
}

fn integer_agrees(block: &[u8], object: usize, run: usize, key: &str, want: i64) -> bool {
    run_integer(block, object, run, key).is_none_or(|have| have == want)
}

/// The caps slot Photoshop stores (`0` none, `1` small caps, `2` all caps).
fn caps_code(spec: &RunSpec) -> i64 {
    if spec.all_caps {
        2
    } else if spec.small_caps {
        1
    } else {
        0
    }
}

/// Whether style run `run` of object `object` agrees with `spec` on the font and
/// the numeric properties the spec sets.
fn run_metrics_agree(block: &[u8], object: usize, run: usize, spec: &RunSpec) -> bool {
    if let Some(name) = &spec.font {
        let index = run_integer(block, object, run, "0").and_then(|i| usize::try_from(i).ok());
        if index.and_then(|i| txt2::font_name(block, i)).as_deref() != Some(name.as_str()) {
            return false;
        }
    }
    if let Some(v) = spec.size
        && !number_agrees(block, object, run, "1", v)
    {
        return false;
    }
    if let Some(v) = spec.leading {
        if !(flag_agrees(block, object, run, "4", false)
            && number_agrees(block, object, run, "5", v))
        {
            return false;
        }
    } else if let Some(auto) = spec.auto_leading
        && !flag_agrees(block, object, run, "4", auto)
    {
        return false;
    }
    if let Some(v) = spec.tracking
        && !integer_agrees(block, object, run, "8", i64::from(v))
    {
        return false;
    }
    if let Some(v) = spec.baseline_shift
        && !number_agrees(block, object, run, "9", v)
    {
        return false;
    }
    if let Some(v) = spec.horizontal_scale
        && !number_agrees(block, object, run, "6", v)
    {
        return false;
    }
    if let Some(v) = spec.vertical_scale
        && !number_agrees(block, object, run, "7", v)
    {
        return false;
    }
    if let Some(mode) = kerning_mode(spec)
        && !integer_agrees(block, object, run, "11", mode)
    {
        return false;
    }
    if let Some(v) = spec.kerning
        && txt2::run_manual_kerning_at(block, object, run)
            .is_some_and(|have| i64::from(have) != i64::from(v))
    {
        return false;
    }
    true
}

/// Whether style run `run` of object `object` agrees with `spec` on the
/// character flags and optional features.
fn run_flags_agree(block: &[u8], object: usize, run: usize, spec: &RunSpec) -> bool {
    let flag = |key: &str, want: bool| flag_agrees(block, object, run, key, want);
    let integer = |key: &str, want: i64| integer_agrees(block, object, run, key, want);
    flag("2", spec.bold)
        && flag("3", spec.italic)
        && integer("15", strike_code(spec))
        && integer("16", underline_code(spec))
        && integer("12", caps_code(spec))
        && integer("30", if spec.old_style { 2 } else { 0 })
        && spec.ligatures.is_none_or(|on| flag("18", on))
        && spec.alternate_ligatures.is_none_or(|on| flag("19", on))
        && spec.no_break.is_none_or(|on| flag("52", on))
        && spec.language.is_none_or(|code| integer("38", code))
        && features_agree(block, object, run, &spec.features)
}

/// Whether style run `run` of object `object` agrees with `spec` on the
/// properties the spec sets.
fn run_agrees(block: &[u8], object: usize, run: usize, spec: &RunSpec) -> bool {
    run_metrics_agree(block, object, run, spec) && run_flags_agree(block, object, run, spec)
}

/// One scalar of run `run`'s character sheet in object `object`, parsed from the
/// block's tokens.
fn read_run_scalar(block: &[u8], object: usize, run: usize, key: &str) -> Option<TxtValue> {
    let mut path = txt2::run_path_for_at(object, run);
    for step in ["0", "0", "6"] {
        path.push(txt2::Step::Key(step));
    }
    path.push(txt2::Step::Key(key));
    let raw = txt2::read_path(block, &path).ok()??;
    parse_scalar(&raw)
}

/// Parses a CoolType scalar token (`true`, `false`, an integer or a real).
fn parse_scalar(raw: &[u8]) -> Option<TxtValue> {
    let text = std::str::from_utf8(raw).ok()?;
    match text {
        "true" => Some(TxtValue::Bool(true)),
        "false" => Some(TxtValue::Bool(false)),
        _ if text.starts_with('/') => {
            Some(TxtValue::Name(text.trim_start_matches('/').to_string()))
        }
        _ if text.contains('.') => text.parse().ok().map(TxtValue::Real),
        _ => text.parse().ok().map(TxtValue::Int),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use photocraft_psd::PsdFile;

    use super::{normalize, validate_runs};
    use crate::layer_render::FaceStore;
    use crate::psd::{
        PATH_FRAME, PathKnot, PathSpec, RunSpec, ShapeSpec, author_layer, read_layer,
    };
    use crate::txt2;

    fn assets() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
    }

    fn noto_store() -> FaceStore {
        let mut store = FaceStore::new();
        store
            .load(&assets().join("fonts"), "NotoSans-Regular")
            .expect("fixture font");
        store
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() <= 1e-4 && (a[1] - b[1]).abs() <= 1e-4
    }

    fn assert_same_shape(before: &ShapeSpec, after: &ShapeSpec, what: &str) {
        let (a, b) = (
            before.path.as_ref().expect("path before"),
            after.path.as_ref().expect("path after"),
        );
        assert_eq!(a.closed, b.closed, "{what}: closed");
        assert!(
            (a.start - b.start).abs() <= 1e-4,
            "{what}: start {} vs {}",
            a.start,
            b.start
        );
        assert_eq!(a.end.is_some(), b.end.is_some(), "{what}: end presence");
        if let (Some(x), Some(y)) = (a.end, b.end) {
            assert!((x - y).abs() <= 1e-4, "{what}: end {x} vs {y}");
        }
        assert_eq!(a.flip, b.flip, "{what}: flip");
        assert_eq!(a.matrix, b.matrix, "{what}: matrix");
        assert_eq!(a.kind, b.kind, "{what}: kind");
        assert_eq!(a.knots.len(), b.knots.len(), "{what}: knot count");
        for (i, (x, y)) in a.knots.iter().zip(&b.knots).enumerate() {
            assert!(
                close(x.anchor, y.anchor)
                    && close(x.in_ctrl, y.in_ctrl)
                    && close(x.out_ctrl, y.out_ctrl),
                "{what}: knot {i} moved: {x:?} vs {y:?}"
            );
            assert_eq!(x.smooth, y.smooth, "{what}: knot {i} smoothing");
        }
        let (f, g) = (
            before.frame.as_ref().expect("frame before"),
            after.frame.as_ref().expect("frame after"),
        );
        assert_eq!(f.kind, g.kind, "{what}: frame kind");
        assert_eq!(f.points, g.points, "{what}: frame points");
        assert_eq!(f.range, g.range, "{what}: frame range");
        assert_eq!(f.flip, g.flip, "{what}: frame flip");
        assert_eq!(f.matrix, g.matrix, "{what}: frame matrix");
    }

    /// Task-15 acceptance: the two type-in-area layers of the showcase files
    /// read into a shape, re-author unchanged, keep every object's frame path
    /// verbatim, and read back as the same shape.
    #[test]
    fn showcase_area_layers_round_trip_through_read_and_author() {
        let fonts = noto_store();
        let sources = [
            "/media/H/zed2/photocraft/out/psd_showcase/10_all_features.psd",
            "/media/H/zed2/photocraft/out/psdcheck/showcase_psresave.psd",
        ];
        let mut checked = 0;
        for source in sources {
            if !std::path::Path::new(source).exists() {
                eprintln!("skip {source}: showcase corpus not present");
                continue;
            }
            let bytes = std::fs::read(source).expect("showcase psd");
            let file = PsdFile::from_bytes(&bytes).expect("parse showcase psd");
            for name in ["04 area justify full hanging", "05 area indents spacing"] {
                let index = file
                    .layers()
                    .iter()
                    .position(|layer| layer.name() == name)
                    .unwrap_or_else(|| panic!("{source}: layer {name:?}"));
                let spec = read_layer(&file, index).expect("read layer");
                let shape = spec.shape.clone().expect("area frame has a shape");
                let stored = shape.frame.clone().expect("stored frame");
                let path = shape.path.clone().expect("decoded path");
                assert_eq!(stored.kind, 1, "{source} {name}: area frame kind");
                assert_eq!(stored.points.len(), 32, "{source} {name}: box path numbers");
                assert!(
                    path.closed && path.knots.len() == 4,
                    "{source} {name}: rectangle path"
                );
                assert_eq!(
                    path.kind.as_deref(),
                    Some("box"),
                    "{source} {name}: kind name"
                );

                let mut copy = PsdFile::from_bytes(&bytes).expect("parse copy");
                author_layer(&mut copy, index, &spec, &fonts)
                    .unwrap_or_else(|error| panic!("{source} {name}: author: {error:?}"));

                // Every object's frame path is untouched: the reader-author
                // round trip re-emits the stored frames verbatim, and the whole
                // `Txt2` block is byte for byte the file's.
                let before = file.txt2().expect("txt2");
                let after = copy.txt2().expect("txt2");
                assert_eq!(
                    before, after,
                    "{source} {name}: re-author changed the Txt2 block"
                );
                for object in 0..8 {
                    assert_eq!(
                        txt2::object_frame_path(before, object),
                        txt2::object_frame_path(after, object),
                        "{source} {name}: object {object} frame changed"
                    );
                }
                let reread = read_layer(&copy, index).expect("read back");
                assert_same_shape(
                    &shape,
                    &reread.shape.expect("read-back shape"),
                    &format!("{source} {name}"),
                );
                checked += 1;
            }
        }
        eprintln!("showcase area round trip: {checked} layers");
        if checked == 0 {
            eprintln!("skipped: showcase corpus not present");
        }
    }

    /// A decoded-curve-only shape is written as a path frame (`kind 2`, moved
    /// start in `/6`, flip baked) and reads back as the same path; authoring the
    /// read-back spec leaves the `Txt2` block untouched.
    #[test]
    fn a_decoded_path_shape_is_written_as_a_path_frame() {
        let fonts = noto_store();
        let source = "/media/H/zed2/photocraft/out/psd_showcase/10_all_features.psd";
        if !std::path::Path::new(source).exists() {
            eprintln!("skip {source}: showcase corpus not present");
            return;
        }
        let bytes = std::fs::read(source).expect("showcase psd");
        let file = PsdFile::from_bytes(&bytes).expect("parse showcase psd");
        let index = file
            .layers()
            .iter()
            .position(|layer| layer.name() == "04 area justify full hanging")
            .expect("area layer");
        let mut spec = read_layer(&file, index).expect("read layer");
        // An open wave in text space, its start moved into the curve.
        let knots = vec![
            PathKnot {
                anchor: [10.0, 100.0],
                in_ctrl: [10.0, 100.0],
                out_ctrl: [40.0, 40.0],
                smooth: Some(false),
            },
            PathKnot {
                anchor: [110.0, 70.0],
                in_ctrl: [80.0, 70.0],
                out_ctrl: [140.0, 70.0],
                smooth: Some(true),
            },
            PathKnot {
                anchor: [210.0, 40.0],
                in_ctrl: [180.0, 40.0],
                out_ctrl: [210.0, 40.0],
                smooth: Some(false),
            },
        ];
        spec.shape = Some(ShapeSpec {
            frame: None,
            path: Some(PathSpec {
                knots: knots.clone(),
                closed: false,
                start: 30.0,
                end: None,
                flip: false,
                matrix: None,
                kind: Some("path".into()),
            }),
        });
        let mut copy = PsdFile::from_bytes(&bytes).expect("parse copy");
        author_layer(&mut copy, index, &spec, &fonts).expect("author path frame");

        let reread = read_layer(&copy, index).expect("read back");
        let reread_shape = reread.shape.clone().expect("shape");
        let path = reread_shape.path.clone().expect("path");
        assert!(!path.closed, "{path:?}");
        assert_eq!(path.kind.as_deref(), Some("path"));
        assert_eq!(reread_shape.frame.expect("frame").kind, PATH_FRAME);
        assert!(!path.flip, "the writer emits unflipped type");
        assert!((path.start - 30.0).abs() < 1e-3, "{}", path.start);
        assert_eq!(path.end, None, "a default end stays -1");
        assert_eq!(path.knots.len(), knots.len());
        for (i, (a, b)) in path.knots.iter().zip(&knots).enumerate() {
            assert!(
                close(a.anchor, b.anchor)
                    && close(a.in_ctrl, b.in_ctrl)
                    && close(a.out_ctrl, b.out_ctrl),
                "knot {i} moved: {a:?} vs {b:?}"
            );
        }
        // Re-authoring the read-back spec is a no-op.
        let block = copy.txt2().expect("txt2").to_vec();
        let mut again =
            PsdFile::from_bytes(&copy.to_bytes().expect("serialize")).expect("parse twice");
        author_layer(&mut again, index, &reread, &fonts).expect("re-author");
        assert_eq!(again.txt2().expect("txt2"), block.as_slice());
    }

    #[test]
    fn normalized_utf16_boundaries_include_the_terminator() {
        let text = normalize("A😀\r\nB\n");
        assert_eq!(text, "A😀\rB");
        let runs = [1, 2, 1, 2].map(|len| RunSpec {
            len,
            ..Default::default()
        });
        validate_runs(&text, &runs).unwrap();
        validate_runs(
            "",
            &[RunSpec {
                len: 1,
                ..Default::default()
            }],
        )
        .unwrap();
    }
}
