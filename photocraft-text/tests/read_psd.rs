//! The upstream PSD text reader: `Txt2` + `TySh` -> `LayerSpec`.
//!
//! Measured values come from the fixtures themselves (read out of the block by
//! hand and quoted at the assertion), so a change in the reader that invents a
//! default fails here rather than passing silently.

use std::path::PathBuf;

use photocraft_text::layer_render::FaceStore;
use photocraft_text::psd::{LayerSpec, ParagraphSpec, RunSpec, author, read_layer};

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

fn store() -> FaceStore {
    let mut store = FaceStore::new();
    for name in ["NotoSans-Regular", "NotoSans-Bold"] {
        store
            .load(&assets().join("fonts"), name)
            .expect("fixture font");
    }
    store
}

fn template(name: &str) -> photocraft_psd::PsdFile {
    let bytes = std::fs::read(assets().join("inputs").join(name)).expect("fixture psd");
    photocraft_psd::PsdFile::from_bytes(&bytes).expect("parse psd")
}

/// The single text layer of a fixture.
fn text_layer(file: &photocraft_psd::PsdFile) -> usize {
    file.text_layers()
        .map(|(index, _)| index)
        .next()
        .expect("text layer")
}

#[test]
fn plain_latin_fixture_reads_its_measured_values() {
    // Measured in `tests/assets/inputs/plain_latin.bin`'s `Txt2` object 0:
    // text `Hamburgefonstiv AVATAR To Ty.` + U+000D; style run `/1 = 30`
    // (29 characters plus the terminator); `/1 = 48.0` size; `/6/11 = 1`
    // (metrics); fill `SimplePaint [1 0 0 0]` (alpha then RGB); `/10/0 = 4`
    // (sharp); paragraph `/5/0 = 0` (left), `/7 = 1.2` auto leading, all
    // indents and spacings zero. Point text carries no frame rectangle.
    let file = template("plain_latin.bin");
    let spec = read_layer(&file, text_layer(&file)).expect("read");
    assert_eq!(spec.text, "Hamburgefonstiv AVATAR To Ty.");
    assert_eq!(spec.runs.len(), 1, "{:?}", spec.runs);
    let run = &spec.runs[0];
    assert_eq!(run.len, 30);
    assert_eq!(run.size, Some(48.0));
    assert_eq!(run.font.as_deref(), Some("NotoSans-Regular"));
    assert_eq!(run.color, Some([0, 0, 0]));
    assert_eq!(run.tracking, Some(0));
    assert_eq!(run.kerning_mode, Some(1));
    assert_eq!(run.auto_kerning, Some(true));
    assert!(run.auto_leading == Some(true));
    assert_eq!(spec.aa, Some(4));
    assert_eq!(spec.paragraph.justification, 0);
    assert_eq!(spec.paragraph.auto_leading, 1.2);
    assert_eq!(spec.paragraph.word_spacing, [0.8, 1.0, 1.33]);
    assert_eq!(spec.paragraph.first_line_indent, 0.0);
    assert!(spec.frame.is_none());
    assert_eq!(spec.orientation, Some(0));
    assert!(spec.transform.is_some());
}

#[test]
fn box_left_fixture_reads_its_frame_rectangle() {
    // `tests/assets/inputs/box_left.bin`: object 0's composed-layout frame names
    // `/99 /F` `/1 [ 0 0 430 200 ]`; the text object's style run is `/1 = 81`
    // at size 48, and the layer antialias dict is `/10 << /0 4 /2 true >>`.
    let file = template("box_left.bin");
    let spec = read_layer(&file, text_layer(&file)).expect("read");
    assert_eq!(
        spec.text,
        "The quick brown fox jumps over the lazy dog while the wide sphinx judges my vow."
    );
    assert_eq!(spec.runs.len(), 1);
    assert_eq!(spec.runs[0].len, 81);
    assert_eq!(spec.runs[0].size, Some(48.0));
    assert_eq!(spec.aa, Some(4));
    assert_eq!(spec.frame, Some([0.0, 0.0, 430.0, 200.0]));
}

#[test]
fn an_authored_spec_reads_back_unchanged() {
    let mut file = template("plain_latin.bin");
    let spec = LayerSpec {
        text: "Hi\nall".into(),
        runs: vec![
            RunSpec {
                len: 2,
                font: Some("NotoSans-Bold".into()),
                size: Some(72.0),
                color: Some([255, 0, 0]),
                tracking: Some(125),
                horizontal_scale: Some(1.25),
                bold: true,
                underline: true,
                all_caps: true,
                kerning_mode: Some(0),
                auto_kerning: Some(false),
                kerning: Some(80),
                ..Default::default()
            },
            RunSpec {
                len: 5,
                size: Some(32.0),
                baseline_shift: Some(-4.5),
                kerning_mode: Some(2),
                ..Default::default()
            },
        ],
        paragraph: ParagraphSpec {
            justification: 5,
            every_line_composer: true,
            first_line_indent: 12.0,
            start_indent: 18.0,
            end_indent: 6.0,
            space_before: 4.0,
            space_after: 8.0,
            auto_leading: 1.5,
            word_spacing: [0.9, 1.0, 1.1],
            letter_spacing: [0.01, 0.02, 0.03],
            glyph_scaling: [0.95, 1.0, 1.05],
            hanging_punctuation: true,
            hyphenate: true,
            hyphenate_words_longer_than: 8,
            hyphenate_after_first: 3,
            hyphenate_before_last: 4,
            hyphenate_limit: 2,
            hyphenate_zone: 18.0,
            hyphenate_capital_words: false,
            burasagari: true,
            kinsoku_order: 2,
        },
        paragraphs: Vec::new(),
        aa: Some(1),
        frame: None,
        shape: None,
        atlas: None,
        fractional: None,
        orientation: None,
        transform: None,
        preserve_trailing_break: false,
    };
    author(&mut file, &spec, &store()).expect("author");
    // `author` does not refresh the descriptor's `Txt ` yet (reported); set it so
    // the file says the same thing in all three copies, as a fixed writer will.
    file.set_text_layer_text(text_layer(&file), "Hi\rall")
        .expect("text");
    let read = read_layer(&file, text_layer(&file)).expect("read");
    assert_eq!(read.text, "Hi\rall");
    assert_eq!(read.runs.len(), 2, "{:?}", read.runs);
    assert_eq!(read.runs[0].len, 2);
    assert_eq!(read.runs[1].len, 5);
    assert_eq!(read.runs[0].size, Some(72.0));
    assert_eq!(read.runs[1].size, Some(32.0));
    assert_eq!(read.runs[0].color, Some([255, 0, 0]));
    assert_eq!(read.runs[0].tracking, Some(125));
    assert_eq!(read.runs[0].horizontal_scale, Some(1.25));
    assert_eq!(read.runs[0].kerning_mode, Some(0));
    assert_eq!(read.runs[0].auto_kerning, Some(false));
    assert_eq!(read.runs[1].baseline_shift, Some(-4.5));
    assert_eq!(read.runs[1].kerning_mode, Some(2));
    // `author` writes the character flags into `Txt2` now, and `Txt2` wins.
    assert!(read.runs[0].bold && read.runs[0].underline && read.runs[0].all_caps);
    assert!(!read.runs[0].italic);
    assert_eq!(read.aa, Some(1));
    assert_eq!(read.paragraph.justification, 5);
    assert!(read.paragraph.every_line_composer);
    assert_eq!(read.paragraph.first_line_indent, 12.0);
    assert_eq!(read.paragraph.start_indent, 18.0);
    assert_eq!(read.paragraph.end_indent, 6.0);
    assert_eq!(read.paragraph.space_before, 4.0);
    assert_eq!(read.paragraph.space_after, 8.0);
    assert_eq!(read.paragraph.auto_leading, 1.5);
    assert_eq!(read.paragraph.word_spacing, [0.9, 1.0, 1.1]);
    assert_eq!(read.paragraph.glyph_scaling, [0.95, 1.0, 1.05]);
}

#[test]
fn a_photoshop_shaped_file_with_two_style_runs_reads_two_run_specs() {
    // The authored block is the same shape Photoshop writes (one style run per
    // `Txt2` `/6/0` entry, lengths in UTF-16 units over text + terminator), so
    // this is the "file with two style runs" case: two run dicts, lengths 3 and
    // 4 ("one" + CR and " two", i.e. 4 + 4 units — see below).
    let mut file = template("plain_latin.bin");
    let spec = LayerSpec {
        text: "one two".into(),
        runs: vec![
            RunSpec {
                len: 4,
                size: Some(64.0),
                ..Default::default()
            },
            RunSpec {
                len: 4,
                size: Some(24.0),
                ..Default::default()
            },
        ],
        paragraph: ParagraphSpec::default(),
        paragraphs: Vec::new(),
        aa: None,
        frame: None,
        shape: None,
        atlas: None,
        fractional: None,
        orientation: None,
        transform: None,
        preserve_trailing_break: false,
    };
    author(&mut file, &spec, &store()).expect("author");
    file.set_text_layer_text(text_layer(&file), "one two")
        .expect("text");
    let read = read_layer(&file, text_layer(&file)).expect("read");
    assert_eq!(read.text, "one two");
    let lengths: Vec<usize> = read.runs.iter().map(|run| run.len).collect();
    assert_eq!(lengths, vec![4, 4], "{:?}", read.runs);
    assert_eq!(read.runs[0].size, Some(64.0));
    assert_eq!(read.runs[1].size, Some(24.0));
    assert_eq!(
        lengths.iter().sum::<usize>(),
        "one two".encode_utf16().count() + 1
    );
}

#[test]
fn a_box_layer_keeps_its_frame_through_authoring() {
    let mut file = template("box_left.bin");
    let spec = LayerSpec {
        text: "Boxed".into(),
        runs: vec![RunSpec {
            len: 6,
            size: Some(40.0),
            ..Default::default()
        }],
        paragraph: ParagraphSpec::default(),
        paragraphs: Vec::new(),
        aa: None,
        frame: Some([10.0, 20.0, 210.0, 120.0]),
        shape: None,
        atlas: None,
        fractional: None,
        orientation: None,
        transform: None,
        preserve_trailing_break: false,
    };
    author(&mut file, &spec, &store()).expect("author");
    file.set_text_layer_text(text_layer(&file), "Boxed")
        .expect("text");
    let read = read_layer(&file, text_layer(&file)).expect("read");
    assert_eq!(read.text, "Boxed");
    assert_eq!(read.frame, Some([10.0, 20.0, 210.0, 120.0]));
}

#[test]
fn a_document_without_txt2_reads_the_engine_data_copy() {
    // Removing the block is a legal PSD (Photoshop writes documents without one
    // when no text was ever composed); the reader must fall back to `TySh` and
    // the `EngineData` run table, with the same text, size and frame.
    let mut file = template("box_left.bin");
    file.set_txt2(None);
    let spec = read_layer(&file, text_layer(&file)).expect("read");
    assert_eq!(
        spec.text,
        "The quick brown fox jumps over the lazy dog while the wide sphinx judges my vow."
    );
    assert!(!spec.runs.is_empty());
    assert_eq!(spec.runs[0].size, Some(48.0));
    let covered: usize = spec.runs.iter().map(|run| run.len).sum();
    assert_eq!(covered, spec.text.encode_utf16().count() + 1);
    // The box shape survives in `EngineData` (`ShapeType 1` + `BoxBounds`).
    assert_eq!(spec.frame, Some([0.0, 0.0, 430.0, 200.0]));
}

#[test]
fn stale_txt2_for_another_text_is_ignored() {
    // A `TySh` text edit that did not go through `Txt2` leaves the block holding
    // the old text. Photoshop composes from the stale block, so the reader must
    // not mix the two models: it takes the text from `TySh` and falls back to
    // `EngineData` for the runs.
    let mut file = template("plain_latin.bin");
    let index = text_layer(&file);
    file.set_text_layer_text(index, "Edited text, much longer than before.")
        .expect("set text");
    let spec = read_layer(&file, index).expect("read");
    assert_eq!(spec.text, "Edited text, much longer than before.");
    let covered: usize = spec.runs.iter().map(|run| run.len).sum();
    assert_eq!(covered, spec.text.encode_utf16().count() + 1);
}

#[test]
fn multi_paragraph_text_reads_with_paragraph_breaks() {
    let mut file = template("plain_latin.bin");
    let spec = LayerSpec {
        text: "First\rSecond".into(),
        runs: vec![RunSpec {
            len: 13,
            size: Some(36.0),
            ..Default::default()
        }],
        paragraph: ParagraphSpec {
            justification: 2,
            ..Default::default()
        },
        paragraphs: Vec::new(),
        aa: None,
        frame: None,
        shape: None,
        atlas: None,
        fractional: None,
        orientation: None,
        transform: None,
        preserve_trailing_break: false,
    };
    author(&mut file, &spec, &store()).expect("author");
    file.set_text_layer_text(text_layer(&file), "First\rSecond")
        .expect("text");
    let read = read_layer(&file, text_layer(&file)).expect("read");
    assert_eq!(read.text, "First\rSecond");
    assert_eq!(read.runs.iter().map(|run| run.len).sum::<usize>(), 13);
    assert_eq!(read.paragraph.justification, 2);
}

#[test]
fn character_flags_come_from_engine_data_without_txt2() {
    // Without a `Txt2` block the only character-style copy is `EngineData`, so
    // the flags the author wrote there are what the reader returns.
    let mut file = template("plain_latin.bin");
    let spec = LayerSpec {
        text: "Flag".into(),
        runs: vec![RunSpec {
            len: 5,
            size: Some(30.0),
            bold: true,
            italic: true,
            underline: true,
            strikethrough: true,
            all_caps: true,
            small_caps: false,
            ..Default::default()
        }],
        paragraph: ParagraphSpec::default(),
        paragraphs: Vec::new(),
        aa: None,
        frame: None,
        shape: None,
        atlas: None,
        fractional: None,
        orientation: None,
        transform: None,
        preserve_trailing_break: false,
    };
    author(&mut file, &spec, &store()).expect("author");
    file.set_text_layer_text(text_layer(&file), "Flag")
        .expect("text");
    file.set_txt2(None);
    let read = read_layer(&file, text_layer(&file)).expect("read");
    assert_eq!(read.text, "Flag");
    let run = &read.runs[0];
    assert!(
        run.bold && run.italic && run.underline && run.strikethrough && run.all_caps,
        "{run:?}"
    );
}

/// `Txt2`-only character slots (no `EngineData` copy) read back through the
/// `Txt2` path: old-style figures, language, ligatures, no-break and the
/// strike/underline/caps codes.
#[test]
fn txt2_only_character_slots_read_back() {
    use photocraft_text::txt2::{
        self, Capitalization, Language, OLD_STYLE_ON, Strikethrough, Underline,
    };

    let mut file = template("plain_latin.bin");
    let mut block = file.txt2().expect("Txt2").to_vec();
    block = txt2::set_run_old_style(&block, 0, OLD_STYLE_ON).expect("old style");
    block = txt2::set_run_language(&block, 0, Language::German).expect("language");
    block = txt2::set_run_ligatures(&block, 0, false).expect("ligatures");
    block = txt2::set_run_alternate_ligatures(&block, 0, true).expect("discretionary");
    block = txt2::set_run_no_break(&block, 0, true).expect("no break");
    block = txt2::set_run_capitalization(&block, 0, Capitalization::SmallCaps).expect("caps");
    block = txt2::set_run_underline(&block, 0, Underline::Left).expect("underline");
    block = txt2::set_run_strikethrough(&block, 0, Strikethrough::StrikeBox).expect("strike");
    block = txt2::set_run_faux_bold(&block, 0, true).expect("bold");
    file.set_txt2(Some(block));

    let read = read_layer(&file, text_layer(&file)).expect("read");
    assert_eq!(read.text, "Hamburgefonstiv AVATAR To Ty.");
    let run = &read.runs[0];
    assert!(run.old_style, "{run:?}");
    assert_eq!(run.language, Some(42));
    assert_eq!(run.ligatures, Some(false));
    assert_eq!(run.alternate_ligatures, Some(true));
    assert_eq!(run.no_break, Some(true));
    assert!(run.small_caps && !run.all_caps);
    assert!(run.underline && run.strikethrough && run.bold);
    assert!(!run.italic);
}

/// `Txt2`-only paragraph slots (hanging punctuation, hyphenation, the
/// hyphenate-capitalised flag, which has no `EngineData` key at all).
#[test]
fn txt2_only_paragraph_slots_read_back() {
    use photocraft_text::txt2::{self, Hyphenation};

    let mut file = template("box_left.bin");
    let mut block = file.txt2().expect("Txt2").to_vec();
    block = txt2::set_paragraph_hanging_punctuation(&block, 0, true).expect("hanging");
    block = txt2::set_paragraph_hyphenation(
        &block,
        0,
        Hyphenation {
            hyphenate: true,
            words_longer_than: 8,
            after_first: 3,
            before_last: 4,
            limit: 2,
            zone_pt: 18.0,
            capital_words: false,
        },
    )
    .expect("hyphenation");
    file.set_txt2(Some(block));

    let read = read_layer(&file, text_layer(&file)).expect("read");
    let paragraph = &read.paragraph;
    assert!(paragraph.hanging_punctuation, "{paragraph:?}");
    assert!(paragraph.hyphenate);
    assert_eq!(paragraph.hyphenate_words_longer_than, 8);
    assert_eq!(paragraph.hyphenate_after_first, 3);
    assert_eq!(paragraph.hyphenate_before_last, 4);
    assert_eq!(paragraph.hyphenate_limit, 2);
    assert_eq!(paragraph.hyphenate_zone, 18.0);
    assert!(!paragraph.hyphenate_capital_words);
}
