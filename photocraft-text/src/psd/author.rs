//! Shared PSD authoring: write text, style runs, paragraph formatting, the
//! layer's renderer settings, and the pixels that go with them, into a template
//! PSD. Used by `examples/author_multi` and the live editor's PSD export.
//!
//! The copies must agree or Photoshop shows one thing and other readers
//! another:
//!
//! 1. `Txt2` text + paragraph run table (`txt2::replace_text`),
//! 2. `Txt2` style run table: lengths, per-run properties, font indices, fill
//!    paints, and an emptied layout cache,
//! 3. `Txt2` paragraph properties and the layer's antialias code,
//! 4. `TySh` `EngineData`: the same split, fonts, paragraphs, antialias, text,
//! 5. the layer's prerendered pixels, re-rendered from that EngineData.

use photocraft_psd::PsdFile;

use crate::layer_render::FaceStore;
use crate::raster::AntiAlias;
use crate::runs::{self, ParagraphRun, RunStyle};
use crate::txt2::{self, ParagraphProperty, RunProperty, Value as TxtValue};

use super::{Error, LayerSpec, ParagraphSpec, RunSpec, engine_data, pixels};

/// Rewrites the first text layer using the caller's loaded fonts and atlases.
///
/// Run lengths cover the normalized text plus its trailing U+000D terminator.
/// Invalid run lengths and surrogate splits are rejected before mutation.
/// Errors leave the file unchanged; only the edited layer is staged.
pub fn author(file: &mut PsdFile, spec: &LayerSpec, store: &FaceStore) -> Result<(), Error> {
    let text = normalize(&spec.text);
    validate_runs(&text, &spec.runs)?;

    // 1. Text and paragraph runs. Prepare both text blocks before committing.
    let txt2_index = file
        .global_blocks
        .iter()
        .position(|b| &b.key == b"Txt2")
        .ok_or(Error::MissingTxt2)?;
    let mut data = txt2::replace_text(&file.global_blocks[txt2_index].data, &text)?;

    // 2. The style run table Photoshop reads; cache emptied so it recomposes.
    data = txt2::set_run_layout(&data, &runs_len(&spec.runs))?;
    let mut font_index = Vec::new();
    for run in &spec.runs {
        font_index.push(match &run.font {
            Some(name) => {
                let (next, index) = txt2::ensure_font(&data, name)?;
                data = next;
                Some(index)
            }
            None => None,
        });
    }
    for (i, run) in spec.runs.iter().enumerate() {
        for (prop, value) in style_of(run) {
            data = txt2::set_run_property(&data, i, prop, &value)?;
        }
        if let Some(index) = font_index[i] {
            data = txt2::set_run_font_index(&data, i, index)?;
        }
        if let Some([r, g, b]) = run.color {
            data = txt2::set_run_fill_color(
                &data,
                i,
                [
                    1.0,
                    f32::from(r) / 255.0,
                    f32::from(g) / 255.0,
                    f32::from(b) / 255.0,
                ],
            )?;
        }
    }

    // 3. Paragraph properties, per paragraph run, and the layer antialias.
    let para = &spec.paragraph;
    let count = txt2::paragraph_count(&data)?;
    for i in 0..count {
        data = txt2::set_paragraph_property(
            &data,
            i,
            ParagraphProperty::Justification,
            &TxtValue::Int(para.justification),
        )?;
        for (prop, value) in [
            (ParagraphProperty::FirstLineIndent, para.first_line_indent),
            (ParagraphProperty::StartIndent, para.start_indent),
            (ParagraphProperty::EndIndent, para.end_indent),
            (ParagraphProperty::SpaceBefore, para.space_before),
            (ParagraphProperty::SpaceAfter, para.space_after),
            (ParagraphProperty::AutoLeading, para.auto_leading),
        ] {
            data = txt2::set_paragraph_property(&data, i, prop, &TxtValue::Real(f64::from(value)))?;
        }
        data = txt2::set_paragraph_property(
            &data,
            i,
            ParagraphProperty::EveryLineComposer,
            &TxtValue::Bool(para.every_line_composer),
        )?;
        for (prop, values) in [
            (ParagraphProperty::WordSpacing, para.word_spacing),
            (ParagraphProperty::LetterSpacing, para.letter_spacing),
            (ParagraphProperty::GlyphScaling, para.glyph_scaling),
        ] {
            data = txt2::set_paragraph_range(&data, i, prop, values)?;
        }
    }
    if let Some(code) = spec.aa {
        data = txt2::set_antialias(&data, code)?;
    }
    if let Some([left, top, right, bottom]) = spec.frame {
        data = txt2::set_frame_geometry(
            &data,
            txt2::FrameGeometry {
                left,
                top,
                right,
                bottom,
            },
        )?;
    }
    data = txt2::clear_cache(&data, 0)?;

    // 4. TySh EngineData: fonts, runs, paragraphs, antialias, text.
    let mut styles = run_styles(&spec.runs);
    let paragraphs = paragraph_runs(&text, para);
    let layer_index = file
        .layers()
        .iter()
        .position(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .ok_or(Error::MissingTextLayer)?;
    let tysh_index = file.layers()[layer_index]
        .blocks
        .iter()
        .position(|b| &b.key == b"TySh")
        .ok_or(Error::MissingTySh)?;
    let tysh_data =
        engine_data::with_engine_data(&file.layers()[layer_index].blocks[tysh_index].data, |ed| {
            for (i, run) in spec.runs.iter().enumerate() {
                if let Some(name) = &run.font {
                    styles[i].font_index = runs::ensure_engine_font(ed, name).map(|v| v as i64);
                }
            }
            runs::apply_style_runs(ed, &styles);
            runs::apply_paragraph_runs(ed, &paragraphs);
            if let Some(code) = spec.aa.and_then(AntiAlias::from_code) {
                runs::set_antialias(ed, code);
            }
            if let Some(on) = spec.fractional {
                runs::set_fractional_glyph_widths(ed, on);
            }
            if let Some(crate::engine_data::Value::Dict(entries)) =
                ed.get_mut("EngineDict").and_then(|d| d.get_mut("Editor"))
            {
                let text = crate::engine_data::Value::Str(format!("{text}\r"));
                match entries.iter_mut().find(|(k, _)| k == "Text") {
                    Some((_, value)) => *value = text,
                    None => entries.push(("Text".to_string(), text)),
                }
            }
        })?;
    let mut layer = file.layers()[layer_index].clone();
    layer.blocks[tysh_index].data = tysh_data;
    layer.blocks[tysh_index].padding = None;

    // 5. The prerendered pixels, from the same EngineData and updated frame.
    pixels::refresh_layer(
        &mut layer,
        &text,
        store,
        txt2::frame_geometry(&data),
        file.header.depth,
        file.header.version,
    )?;
    file.global_blocks[txt2_index].data = data;
    file.global_blocks[txt2_index].padding = None;
    file.layers_mut()[layer_index] = layer;
    Ok(())
}

/// Normalises a PSD paragraph break (U+000D) out of editor input (LF or CRLF).
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\r")
        .replace('\n', "\r")
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

fn validate_runs(text: &str, runs: &[RunSpec]) -> Result<(), Error> {
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
fn paragraph_runs(text: &str, para: &ParagraphSpec) -> Vec<ParagraphRun> {
    text.split('\r')
        .map(|p| ParagraphRun {
            len: p.encode_utf16().count() + 1,
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
            ..ParagraphRun::default()
        })
        .collect()
}

fn runs_len(spec: &[RunSpec]) -> Vec<i64> {
    spec.iter().map(|r| r.len as i64).collect()
}

fn run_styles(spec: &[RunSpec]) -> Vec<RunStyle> {
    spec.iter()
        .map(|r| RunStyle {
            len: r.len,
            font: r.font.clone(),
            size: r.size,
            leading: r.leading,
            auto_leading: r.auto_leading,
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
            strikethrough: r.strikethrough,
            all_caps: r.all_caps,
            small_caps: r.small_caps,
            ..RunStyle::default()
        })
        .collect()
}

fn style_of(run: &RunSpec) -> Vec<(RunProperty, TxtValue)> {
    let mut props = Vec::new();
    if let Some(v) = run.size {
        props.push((RunProperty::FontSize, TxtValue::Real(f64::from(v))));
    }
    if let Some(v) = run.leading {
        props.push((RunProperty::AutoLeading, TxtValue::Bool(false)));
        props.push((RunProperty::Leading, TxtValue::Real(f64::from(v))));
    } else if let Some(auto) = run.auto_leading {
        props.push((RunProperty::AutoLeading, TxtValue::Bool(auto)));
    }
    if let Some(v) = run.tracking {
        props.push((RunProperty::Tracking, TxtValue::Int(i64::from(v))));
    }
    if let Some(v) = run.baseline_shift {
        props.push((RunProperty::BaselineShift, TxtValue::Real(f64::from(v))));
    }
    if let Some(v) = run.horizontal_scale {
        props.push((RunProperty::HorizontalScale, TxtValue::Real(f64::from(v))));
    }
    if let Some(v) = run.vertical_scale {
        props.push((RunProperty::VerticalScale, TxtValue::Real(f64::from(v))));
    }
    props
}

#[cfg(test)]
mod tests {
    use super::{normalize, validate_runs};
    use crate::psd::RunSpec;

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
