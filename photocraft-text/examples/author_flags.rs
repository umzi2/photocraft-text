//! Authors a text layer with the measured character/paragraph flags and prints
//! every `Txt2` path it changed.
//!
//! `cargo run --example author_flags -- base.psd spec.json out.psd`
//!
//! The spec is JSON; every field is optional except the run lengths when the
//! text changes:
//!
//! ```json
//! {
//!   "text": "The quick brown fox jumps over the lazy dog",
//!   "runs": [
//!     {"len": 20, "size": 48, "capitalization": "all", "underline": "left"},
//!     {"len": 24, "size": 48, "strike": true, "kerning": "optical",
//!      "language": 14, "ligatures": false, "alternate_ligatures": true,
//!      "old_style": true, "no_break": true, "faux_bold": true}
//!   ],
//!   "paragraph": {
//!     "justification": "full", "auto_leading": 1.5,
//!     "first_line_indent": 24, "start_indent": 18, "end_indent": 0,
//!     "hanging": true, "hyphenate": true, "hyphenation_zone": 18
//!   },
//!   "aa": 4
//! }
//! ```
//!
//! The writes go through `Txt2` (what Photoshop reads) and are mirrored into
//! `TySh` `EngineData`, then the layer's stored pixels are re-rendered from that
//! same model. Slots whose encoding is still unmeasured are not accepted:
//! manual kerning amounts, space before/after.

#[path = "common/author.rs"]
mod common_author;

use std::collections::BTreeMap;

use common_author::{author, LayerSpec, ParagraphSpec, RunSpec as AuthorRun};
use photocraft_psd::PsdFile;
use photocraft_text::runs::{self, ParagraphFormat, ParagraphRun, RunStyle};
use photocraft_text::txt2::{
    self, Capitalization, Hyphenation, Justification, KerningType, ParagraphProperty, RunProperty,
    Strikethrough, Underline, OLD_STYLE_ON,
};

#[path = "common/pixels.rs"]
mod pixels;

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct Spec {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    runs: Vec<Run>,
    #[serde(default)]
    paragraph: Paragraph,
    /// `EngineDict/AntiAlias`: 0 none, 1 crisp, 2 strong, 3 smooth, 4 sharp.
    #[serde(default)]
    aa: Option<i64>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct Run {
    /// UTF-16 units including the terminator on the last run.
    len: usize,
    #[serde(default)]
    font: Option<String>,
    #[serde(default)]
    size: Option<f32>,
    #[serde(default)]
    leading: Option<f32>,
    #[serde(default)]
    auto_leading: Option<bool>,
    #[serde(default)]
    tracking: Option<i16>,
    #[serde(default)]
    baseline_shift: Option<f32>,
    #[serde(default)]
    horizontal_scale: Option<f32>,
    #[serde(default)]
    vertical_scale: Option<f32>,
    #[serde(default)]
    color: Option<[u8; 3]>,
    /// `"metrics"` (default), `"manual"` or `"optical"` (`Txt2` `/6/11`).
    #[serde(default)]
    kerning: Option<String>,
    /// `"normal"`, `"small"` or `"all"` (`Txt2` `/6/12`).
    #[serde(default)]
    capitalization: Option<String>,
    /// `"off"`, `"left"` or `"right"` (`Txt2` `/6/16`).
    #[serde(default)]
    underline: Option<String>,
    #[serde(default)]
    strike: Option<bool>,
    #[serde(default)]
    ligatures: Option<bool>,
    #[serde(default)]
    alternate_ligatures: Option<bool>,
    #[serde(default)]
    old_style: Option<bool>,
    /// Photoshop language code (0 default, 14 English UK, 42 German).
    #[serde(default)]
    language: Option<i64>,
    #[serde(default)]
    no_break: Option<bool>,
    #[serde(default)]
    faux_bold: Option<bool>,
    #[serde(default)]
    faux_italic: Option<bool>,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default)]
struct Paragraph {
    /// `"left"`, `"right"`, `"center"`, `"full"`, plus the justify-last codes.
    justification: String,
    auto_leading: f32,
    first_line_indent: f32,
    start_indent: f32,
    end_indent: f32,
    /// `Txt2` `/5/4`, points. The DOM setters are silent no-ops; the slot is
    /// written here directly (round-trip through Photoshop not measured yet).
    space_before: f32,
    /// `Txt2` `/5/5`, points, same caveat as `space_before`.
    space_after: f32,
    hanging: bool,
    every_line_composer: bool,
    word_spacing: [f32; 3],
    letter_spacing: [f32; 3],
    glyph_scaling: [f32; 3],
    hyphenate: bool,
    /// DOM `hyphenateWordsLongerThan`; `Txt2` stores it **+ 1**.
    hyphenate_words_longer_than: i64,
    /// DOM `hyphenateAfterFirst` (`Txt2` `/5/11`).
    hyphenate_after_first: i64,
    /// DOM `hyphenateBeforeLast` (`Txt2` `/5/12`).
    hyphenate_before_last: i64,
    /// DOM `hyphenLimit` (`Txt2` `/5/13`).
    hyphen_limit: i64,
    /// DOM `hyphenationZone`, points; `Txt2` `/5/14` and `EngineData.Zone`
    /// store the same number.
    hyphenation_zone: f32,
    /// DOM `hyphenateCapitalWords` (`Txt2` `/5/15`, no EngineData key).
    hyphenate_capital_words: bool,
}

impl Default for Paragraph {
    fn default() -> Self {
        let hyphenation = Hyphenation::default();
        Self {
            justification: "left".into(),
            auto_leading: 1.2,
            first_line_indent: 0.0,
            start_indent: 0.0,
            end_indent: 0.0,
            space_before: 0.0,
            space_after: 0.0,
            hanging: false,
            every_line_composer: false,
            word_spacing: [0.8, 1.0, 1.33],
            letter_spacing: [0.0, 0.0, 0.0],
            glyph_scaling: [1.0, 1.0, 1.0],
            hyphenate: hyphenation.hyphenate,
            hyphenate_words_longer_than: hyphenation.words_longer_than,
            hyphenate_after_first: hyphenation.after_first,
            hyphenate_before_last: hyphenation.before_last,
            hyphen_limit: hyphenation.limit,
            hyphenation_zone: hyphenation.zone_pt,
            hyphenate_capital_words: hyphenation.capital_words,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        return Err("usage: author_flags <base.psd> <spec.json> <out.psd>".into());
    }
    let spec: Spec = serde_json::from_str(&std::fs::read_to_string(&args[2])?)?;
    let mut file = PsdFile::from_bytes(&std::fs::read(&args[1])?)?;

    // The text comes from the spec, or is kept from the layer's own block.
    let text = match &spec.text {
        Some(t) => t.clone(),
        None => txt2::read_text(
            &file.global_block(b"Txt2").ok_or("no Txt2 block")?.data,
        )
        .ok_or("no layer text")?,
    };
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    let units = normalized.encode_utf16().count() + 1;
    let runs = if spec.runs.is_empty() {
        vec![Run { len: units, ..Run::default() }]
    } else {
        spec.runs.clone()
    };
    let covered: usize = runs.iter().map(|r| r.len).sum();
    if covered != units {
        return Err(format!("runs cover {covered} units, text + terminator is {units}").into());
    }

    // Base authoring: text, run table, basic paragraph formatting, EngineData,
    // pixels. The measured flags are applied on top of it below.
    author(
        &mut file,
        &LayerSpec {
            text: normalized.clone(),
            runs: runs.iter().map(author_run).collect(),
            paragraph: author_paragraph(&spec.paragraph),
            aa: spec.aa,
            frame: None,
            atlas: None,
            fractional: None,
        },
    )?;

    // 1. `Txt2`: every measured run slot, then the paragraph blocks.
    let before = file.global_block(b"Txt2").ok_or("no Txt2")?.data.clone();
    if std::env::var("PHOTOCRAFT_TRACE").is_ok() {
        let t = txt2::read_text(&before).unwrap_or_default();
        eprintln!("after author: {:?}", &t[t.len().saturating_sub(40)..]);
    }
    let mut data = before.clone();
    for (i, run) in runs.iter().enumerate() {
        if let Some(on) = run.faux_bold {
            data = txt2::set_run_faux_bold(&data, i, on)?;
        }
        if let Some(on) = run.faux_italic {
            data = txt2::set_run_faux_italic(&data, i, on)?;
        }
        if let Some(mode) = parse_kerning(run.kerning.as_deref())? {
            data = txt2::set_run_kerning_type(&data, i, mode)?;
        }
        if let Some(caps) = parse_capitalization(run.capitalization.as_deref())? {
            data = txt2::set_run_capitalization(&data, i, caps)?;
        }
        if let Some(kind) = parse_underline(run.underline.as_deref())? {
            data = txt2::set_run_underline(&data, i, kind)?;
        }
        if let Some(on) = run.strike {
            let kind = if on { Strikethrough::StrikeBox } else { Strikethrough::Off };
            data = txt2::set_run_strikethrough(&data, i, kind)?;
        }
        if let Some(on) = run.ligatures {
            data = txt2::set_run_ligatures(&data, i, on)?;
        }
        if let Some(on) = run.alternate_ligatures {
            data = txt2::set_run_alternate_ligatures(&data, i, on)?;
        }
        if let Some(on) = run.old_style {
            data = txt2::set_run_old_style(&data, i, if on { OLD_STYLE_ON } else { 0 })?;
        }
        if let Some(code) = run.language {
            data = txt2::set_run_language_code(&data, i, code)?;
        }
        if let Some(on) = run.no_break {
            data = txt2::set_run_no_break(&data, i, on)?;
        }
    }
    if std::env::var("PHOTOCRAFT_TRACE").is_ok() {
        let t = txt2::read_text(&data).unwrap_or_default();
        eprintln!("after run slots: {:?}", &t[t.len().saturating_sub(40)..]);
    }
    let para = &spec.paragraph;
    let justification = parse_justification(&para.justification)?;
    let hyphenation = Hyphenation {
        hyphenate: para.hyphenate,
        // Photoshop stores the DOM's "words longer than N" as N + 1
        // (out/probe5/pE.psd hyph_params: DOM 6 -> /5/10 = 7).
        words_longer_than: para.hyphenate_words_longer_than + 1,
        after_first: para.hyphenate_after_first,
        before_last: para.hyphenate_before_last,
        limit: para.hyphen_limit,
        zone_pt: para.hyphenation_zone,
        capital_words: para.hyphenate_capital_words,
    };
    for i in 0..txt2::paragraph_count(&data)? {
        data = txt2::set_paragraph_justification(&data, i, justification)?;
        data = txt2::set_paragraph_first_line_indent(&data, i, para.first_line_indent)?;
        data = txt2::set_paragraph_start_indent(&data, i, para.start_indent)?;
        data = txt2::set_paragraph_end_indent(&data, i, para.end_indent)?;
        if para.space_before != 0.0 {
            data = txt2::set_paragraph_property_or_insert(
                &data,
                i,
                ParagraphProperty::SpaceBefore,
                &photocraft_text::txt2::Value::Real(f64::from(para.space_before)),
            )?;
        }
        if para.space_after != 0.0 {
            data = txt2::set_paragraph_property_or_insert(
                &data,
                i,
                ParagraphProperty::SpaceAfter,
                &photocraft_text::txt2::Value::Real(f64::from(para.space_after)),
            )?;
        }
        data = txt2::set_paragraph_auto_leading(&data, i, para.auto_leading)?;
        data = txt2::set_paragraph_hanging_punctuation(&data, i, para.hanging)?;
        data = txt2::set_paragraph_every_line_composer(&data, i, para.every_line_composer)?;
        data = txt2::set_paragraph_hyphenation(&data, i, hyphenation)?;
        data = txt2::set_paragraph_range(&data, i, ParagraphProperty::WordSpacing, para.word_spacing)?;
        data = txt2::set_paragraph_range(&data, i, ParagraphProperty::LetterSpacing, para.letter_spacing)?;
        data = txt2::set_paragraph_range(&data, i, ParagraphProperty::GlyphScaling, para.glyph_scaling)?;
    }
    if std::env::var("PHOTOCRAFT_TRACE").is_ok() {
        let t = txt2::read_text(&data).unwrap_or_default();
        eprintln!("after paragraphs: {:?}", &t[t.len().saturating_sub(40)..]);
    }
    if let Some(code) = spec.aa {
        data = txt2::set_antialias(&data, code)?;
    }
    report(&args[1], &before, &data);
    {
        let block = file
            .global_blocks
            .iter_mut()
            .find(|b| &b.key == b"Txt2")
            .ok_or("no Txt2")?;
        block.data = data;
        block.padding = None;
    }

    // 2. The same flags in `TySh` `EngineData`.
    let styles = engine_styles(&runs);
    let paragraphs = engine_paragraphs(&normalized, para, justification);
    let format = ParagraphFormat {
        hyphenate: para.hyphenate,
        // Stored like the `Txt2` slot: DOM value + 1.
        words_longer_than: para.hyphenate_words_longer_than + 1,
        after_first: para.hyphenate_after_first,
        before_last: para.hyphenate_before_last,
        limit: para.hyphen_limit,
        // `EngineData` `Zone` and `Txt2` `/5/14` both store the point value
        // (out/probe5/pE.psd: DOM 18 -> 18.0 in both models).
        zone: para.hyphenation_zone,
        hanging_punctuation: para.hanging,
    };
    let layer = file
        .layers_mut()
        .iter_mut()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .ok_or("no text layer")?;
    let tysh = layer
        .blocks
        .iter_mut()
        .find(|b| &b.key == b"TySh")
        .ok_or("no TySh block")?;
    let formats = vec![format; paragraphs.len()];
    tysh.data = pixels::with_engine_data(&tysh.data, |ed| {
        let mut styles = styles.clone();
        for (i, run) in runs.iter().enumerate() {
            if let Some(name) = &run.font {
                styles[i].font_index = runs::ensure_engine_font(ed, name).map(|v| v as i64);
            }
        }
        runs::apply_style_runs(ed, &styles);
        runs::apply_paragraph_runs(ed, &paragraphs);
        runs::apply_paragraph_formats(ed, &formats);
        if let Some(code) = spec.aa.and_then(photocraft_text::raster::AntiAlias::from_code) {
            runs::set_antialias(ed, code);
        }
    })?;
    tysh.padding = None;

    // 3. The stored pixels, from the model just written.
    pixels::refresh_text_layer(&mut file, &normalized)?;
    let bytes = file.to_bytes()?;
    if std::env::var("PHOTOCRAFT_TRACE").is_ok() {
        let t2 = bytes.windows(4).position(|w| w == b"Txt2").unwrap_or(0);
        let blk = &bytes[t2..];
        let t = txt2::read_text(blk).unwrap_or_default();
        eprintln!("serialized: {:?}", &t[t.len().saturating_sub(40)..]);
        let anchor = b"/1 [ << /0 << /0 (";
        let a = bytes.windows(anchor.len()).position(|w| w == anchor).unwrap_or(0);
        let hex: String = bytes[a + anchor.len() + 300..a + anchor.len() + 340]
            .iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
        eprintln!("in-memory after '(': {hex}");
    }
    std::fs::write(&args[3], bytes)?;
    println!("wrote {}", &args[3]);
    Ok(())
}

fn author_run(run: &Run) -> AuthorRun {
    let caps = parse_capitalization(run.capitalization.as_deref()).ok().flatten();
    AuthorRun {
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
        auto_kerning: parse_kerning(run.kerning.as_deref())
            .ok()
            .flatten()
            .map(KerningType::engine_auto_kerning),
        kerning: None,
        bold: run.faux_bold.unwrap_or(false),
        italic: run.faux_italic.unwrap_or(false),
        underline: parse_underline(run.underline.as_deref())
            .ok()
            .flatten()
            .is_some_and(Underline::engine_underline),
        strikethrough: run.strike.unwrap_or(false),
        all_caps: caps == Some(Capitalization::AllCaps),
        small_caps: caps == Some(Capitalization::SmallCaps),
    }
}

fn author_paragraph(para: &Paragraph) -> ParagraphSpec {
    ParagraphSpec {
        justification: parse_justification(&para.justification).unwrap_or(Justification::Left).code(),
        every_line_composer: para.every_line_composer,
        first_line_indent: para.first_line_indent,
        start_indent: para.start_indent,
        end_indent: para.end_indent,
        space_before: 0.0,
        space_after: 0.0,
        auto_leading: para.auto_leading,
        word_spacing: para.word_spacing,
        letter_spacing: para.letter_spacing,
        glyph_scaling: para.glyph_scaling,
    }
}

fn engine_styles(run_specs: &[Run]) -> Vec<RunStyle> {
    run_specs
        .iter()
        .map(|run| RunStyle {
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
            auto_kerning: parse_kerning(run.kerning.as_deref())
                .ok()
                .flatten()
                .map(KerningType::engine_auto_kerning),
            kerning: None,
            faux_bold: run.faux_bold.unwrap_or(false),
            faux_italic: run.faux_italic.unwrap_or(false),
            underline: parse_underline(run.underline.as_deref())
                .ok()
                .flatten()
                .is_some_and(Underline::engine_underline),
            strikethrough: run.strike.unwrap_or(false),
            all_caps: parse_capitalization(run.capitalization.as_deref()).ok().flatten()
                == Some(Capitalization::AllCaps),
            small_caps: parse_capitalization(run.capitalization.as_deref()).ok().flatten()
                == Some(Capitalization::SmallCaps),
            ligatures: run.ligatures,
            alternate_ligatures: run.alternate_ligatures,
            language: run.language,
            no_break: run.no_break,
            ..RunStyle::default()
        })
        .collect()
}

fn engine_paragraphs(text: &str, para: &Paragraph, justification: Justification) -> Vec<ParagraphRun> {
    text.split('\r')
        .map(|p| ParagraphRun {
            len: p.encode_utf16().count() + 1,
            word_spacing: para.word_spacing,
            letter_spacing: para.letter_spacing,
            glyph_scaling: para.glyph_scaling,
            justification: photocraft_text::area::Justification::from_psd(justification.code() as i32),
            every_line_composer: para.every_line_composer,
            first_line_indent: para.first_line_indent,
            start_indent: para.start_indent,
            end_indent: para.end_indent,
            space_before: 0.0,
            space_after: 0.0,
            auto_leading: para.auto_leading,
        })
        .collect()
}

fn parse_justification(value: &str) -> Result<Justification, Box<dyn std::error::Error>> {
    Ok(match value.to_ascii_lowercase().as_str() {
        "" | "left" => Justification::Left,
        "right" => Justification::Right,
        "center" | "centre" => Justification::Center,
        "justify_last_center" | "fl_center" => Justification::JustifyLastCenter,
        "justify_last_right" | "fl_right" => Justification::JustifyLastRight,
        "justify_all" | "all" => Justification::JustifyAll,
        "full" | "justify" | "fl_left" => Justification::JustifyLastLeft,
        other => return Err(format!("unknown justification {other:?}").into()),
    })
}

fn parse_kerning(value: Option<&str>) -> Result<Option<KerningType>, Box<dyn std::error::Error>> {
    Ok(match value.map(str::to_ascii_lowercase).as_deref() {
        None => None,
        Some("metrics") => Some(KerningType::Metrics),
        Some("manual") => Some(KerningType::Manual),
        Some("optical") => Some(KerningType::Optical),
        Some(other) => return Err(format!("unknown kerning {other:?}").into()),
    })
}

fn parse_capitalization(
    value: Option<&str>,
) -> Result<Option<Capitalization>, Box<dyn std::error::Error>> {
    Ok(match value.map(str::to_ascii_lowercase).as_deref() {
        None => None,
        Some("normal") | Some("none") => Some(Capitalization::Normal),
        Some("small") | Some("small_caps") => Some(Capitalization::SmallCaps),
        Some("all") | Some("all_caps") => Some(Capitalization::AllCaps),
        Some(other) => return Err(format!("unknown capitalization {other:?}").into()),
    })
}

fn parse_underline(value: Option<&str>) -> Result<Option<Underline>, Box<dyn std::error::Error>> {
    Ok(match value.map(str::to_ascii_lowercase).as_deref() {
        None => None,
        Some("off") | Some("none") => Some(Underline::Off),
        Some("left") => Some(Underline::Left),
        Some("right") => Some(Underline::Right),
        Some(other) => return Err(format!("unknown underline {other:?}").into()),
    })
}

/// Prints the `Txt2` leaf paths whose value changed between `before` and
/// `after`, plus the slot names the writer owns.
fn report(base: &str, before: &[u8], after: &[u8]) {
    let old = leaves(before);
    let new = leaves(after);
    println!("Txt2 changes in {base}:");
    let mut printed = 0;
    for (path, value) in &new {
        match old.get(path) {
            Some(previous) if previous == value => {}
            Some(previous) => {
                println!("  {path}: {previous} -> {value}");
                printed += 1;
            }
            None => {
                println!("  {path}: (absent) -> {value}");
                printed += 1;
            }
        }
    }
    if printed == 0 {
        println!("  (none)");
    }
    // The EngineData mirror keys the same flags land in.
    let mut mirrored: BTreeMap<&str, &str> = BTreeMap::new();
    for (slot, engine) in [
        (RunProperty::FauxBold, "FauxBold"),
        (RunProperty::FauxItalic, "FauxItalic"),
        (RunProperty::KerningType, "AutoKerning"),
        (RunProperty::Capitalization, "FontCaps"),
        (RunProperty::Underline, "Underline"),
        (RunProperty::Strikethrough, "Strikethrough"),
        (RunProperty::Ligatures, "Ligatures"),
        (RunProperty::AlternateLigatures, "DLigatures"),
        (RunProperty::Language, "Language"),
        (RunProperty::NoBreak, "NoBreak"),
    ] {
        mirrored.insert(engine, slot.key());
    }
    for (engine, slot) in mirrored {
        println!("  EngineData /{engine} mirrors Txt2 /6/{slot}");
    }
    println!("  EngineData /Language mirrors Txt2 /6/38 through runs::engine_language (German 42 -> 4)");
    println!("  EngineData AutoHyphenate/HyphenatedWordSize/PreHyphen/PostHyphen/ConsecutiveHyphens/Zone/Hanging mirror Txt2 /5/9..15,21");
}

/// Leaf `path -> value` map of a `Txt2` block, so two blocks can be diffed.
fn leaves(block: &[u8]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(toks) = txt2::tokens(block) else { return out };
    let mut i = 0;
    while i + 1 < toks.len() {
        if let txt2::Kind::Name(key) = &toks[i].kind {
            walk(block, &toks, i + 1, &format!("/{key}"), &mut out);
            match txt2::skip_value(&toks, i + 1) {
                Some(next) => i = next,
                None => break,
            }
        } else {
            i += 1;
        }
    }
    out
}

fn walk(
    block: &[u8],
    toks: &[txt2::Token],
    index: usize,
    path: &str,
    out: &mut BTreeMap<String, String>,
) {
    let Some(token) = toks.get(index) else { return };
    match &token.kind {
        txt2::Kind::DictOpen => {
            if let Some(entries) = txt2::dict_entries(toks, index) {
                for (key, value) in entries {
                    walk(block, toks, value, &format!("{path}/{key}"), out);
                }
            }
        }
        txt2::Kind::ArrayOpen => {
            let mut j = index + 1;
            let mut n = 0usize;
            while let Some(token) = toks.get(j) {
                if matches!(token.kind, txt2::Kind::ArrayClose) {
                    break;
                }
                walk(block, toks, j, &format!("{path}/{n}"), out);
                match txt2::skip_value(toks, j) {
                    Some(next) => j = next,
                    None => break,
                }
                n += 1;
            }
        }
        _ => {
            out.insert(
                path.to_string(),
                String::from_utf8_lossy(&block[token.start..token.end]).into_owned(),
            );
        }
    }
}
