//! `Txt2` slot authoring: every measured character and paragraph property is
//! written into the block, written into a PSD and read back.
//!
//! The slot numbers are the ones measured in the round-2 probes
//! (`out/probe2/*.psd` diffs against `out/probe2/base.psd`) and in Photoshop's
//! own re-saves of authored files (`out/verify/*_res.psd`). A property that is
//! not listed there must not appear here; the module refuses unknown codes
//! rather than writing them, which is checked at the bottom of this file.

use photocraft_psd::PsdFile;
use photocraft_text::txt2::{
    self, Capitalization, Hyphenation, Justification, KerningType, Language, ParagraphProperty,
    RunProperty, Step, Strikethrough, Underline, Value, OLD_STYLE_ON,
};

fn corpus_bytes(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
    std::fs::read(dir.join(format!("{name}.psd"))).expect("corpus psd")
}

fn corpus_block(name: &str) -> Vec<u8> {
    let bytes = corpus_bytes(name);
    let file = PsdFile::from_bytes(&bytes).expect("parse psd");
    file.global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .expect("Txt2 block")
        .data
        .clone()
}

/// Value at a path, as text (the raw token Photoshop stores).
fn raw_at(block: &[u8], path: &[Step<'_>]) -> String {
    let (s, e) = txt2::value_range(block, path).expect("path");
    String::from_utf8_lossy(&block[s..e]).into_owned()
}

fn run_sheet_path(run: usize, key: &'static str) -> Vec<Step<'static>> {
    let mut p = txt2::run_path_for(run);
    for step in ["0", "0", "6", key] {
        p.push(Step::Key(step));
    }
    p
}

fn paragraph_sheet_path(i: usize, key: &'static str) -> Vec<Step<'static>> {
    let mut p = txt2::paragraph_props_path(i);
    p.push(Step::Key(key));
    p
}

/// Splits the single corpus run in two so a property can be checked for
/// bleeding into its neighbour.
fn two_runs() -> Vec<u8> {
    let block = corpus_block("plain_latin");
    assert_eq!(txt2::run_count(&block).unwrap(), 1);
    txt2::set_run_layout(&block, &[10, 20]).expect("split")
}

#[test]
fn character_flags_round_trip() {
    let split = two_runs();
    let mut out = split.clone();
    // Run 0 gets every measured character flag; run 1 must stay untouched.
    out = txt2::set_run_faux_bold(&out, 0, true).unwrap();
    out = txt2::set_run_faux_italic(&out, 0, true).unwrap();
    out = txt2::set_run_kerning_type(&out, 0, KerningType::Optical).unwrap();
    out = txt2::set_run_capitalization(&out, 0, Capitalization::AllCaps).unwrap();
    out = txt2::set_run_underline(&out, 0, Underline::Left).unwrap();
    out = txt2::set_run_strikethrough(&out, 0, Strikethrough::StrikeBox).unwrap();
    out = txt2::set_run_ligatures(&out, 0, false).unwrap();
    out = txt2::set_run_alternate_ligatures(&out, 0, true).unwrap();
    out = txt2::set_run_old_style(&out, 0, OLD_STYLE_ON).unwrap();
    out = txt2::set_run_language(&out, 0, Language::German).unwrap();
    out = txt2::set_run_no_break(&out, 0, true).unwrap();
    out = txt2::set_run_fill_color(&out, 0, [1.0, 1.0, 0.0, 0.0]).unwrap();
    out = txt2::set_run_stroke_color(&out, 0, [1.0, 0.0, 0.0, 1.0]).unwrap();

    // Readers see every value back.
    assert_eq!(txt2::run_faux_bold(&out, 0), Some(true));
    assert_eq!(txt2::run_faux_italic(&out, 0), Some(true));
    assert_eq!(txt2::run_kerning_type(&out, 0), Some(KerningType::Optical));
    assert_eq!(txt2::run_capitalization(&out, 0), Some(Capitalization::AllCaps));
    assert_eq!(txt2::run_underline(&out, 0), Some(Underline::Left));
    assert_eq!(txt2::run_strikethrough(&out, 0), Some(Strikethrough::StrikeBox));
    assert_eq!(txt2::run_ligatures(&out, 0), Some(false));
    assert_eq!(txt2::run_alternate_ligatures(&out, 0), Some(true));
    assert_eq!(txt2::run_old_style(&out, 0), Some(OLD_STYLE_ON));
    assert_eq!(txt2::run_language(&out, 0), Some(Language::German));
    assert_eq!(txt2::run_language_code(&out, 0), Some(42));
    assert_eq!(txt2::run_no_break(&out, 0), Some(true));
    assert_eq!(txt2::run_fill_color(&out, 0), Some([1.0, 1.0, 0.0, 0.0]));
    assert_eq!(txt2::run_stroke_color(&out, 0), Some([1.0, 0.0, 0.0, 1.0]));

    // The generic reader agrees with the typed ones.
    assert_eq!(
        txt2::run_property(&out, 0, RunProperty::Capitalization),
        Some(Value::Int(2))
    );
    assert_eq!(
        txt2::run_property(&out, 0, RunProperty::KerningType),
        Some(Value::Int(2))
    );

    // Run 1 keeps its own values.
    assert_eq!(txt2::run_faux_bold(&out, 1), Some(false));
    assert_eq!(txt2::run_kerning_type(&out, 1), Some(KerningType::Metrics));
    assert_eq!(txt2::run_capitalization(&out, 1), Some(Capitalization::Normal));
    assert_eq!(txt2::run_underline(&out, 1), Some(Underline::Off));
    assert_eq!(txt2::run_strikethrough(&out, 1), Some(Strikethrough::Off));
    assert_eq!(txt2::run_ligatures(&out, 1), Some(true));
    assert_eq!(txt2::run_old_style(&out, 1), Some(0));
    assert_eq!(txt2::run_language(&out, 1), Some(Language::Default));
    assert_eq!(txt2::run_no_break(&out, 1), Some(false));
    assert_eq!(txt2::run_stroke_color(&out, 1), Some([1.0, 0.0, 0.0, 0.0]));

    // Re-tokenising reproduces the bytes exactly: the writer spliced spans and
    // did not reformat Photoshop's own spacing.
    assert_eq!(txt2::untokenize(&out, &txt2::tokens(&out).unwrap()), out);
    assert_eq!(txt2::run_lengths(&out).unwrap(), vec![10, 20]);
    assert_eq!(txt2::read_text(&out), txt2::read_text(&split));
}

#[test]
fn character_slots_are_the_measured_keys() {
    let block = corpus_block("plain_latin");
    let out = txt2::set_run_capitalization(&block, 0, Capitalization::SmallCaps).unwrap();
    // `/6/12` is the key Photoshop re-saved for all caps; small caps is 1.
    assert_eq!(raw_at(&out, &run_sheet_path(0, "12")), "1");
    assert_eq!(raw_at(&out, &run_sheet_path(0, "11")), "1", "metrics is the default");
    let out = txt2::set_run_kerning_type(&out, 0, KerningType::Manual).unwrap();
    assert_eq!(raw_at(&out, &run_sheet_path(0, "11")), "0");
    let out = txt2::set_run_underline(&out, 0, Underline::Right).unwrap();
    assert_eq!(raw_at(&out, &run_sheet_path(0, "16")), "1");
    let out = txt2::set_run_strikethrough(&out, 0, Strikethrough::StrikeBox).unwrap();
    assert_eq!(raw_at(&out, &run_sheet_path(0, "15")), "2");
    let out = txt2::set_run_language(&out, 0, Language::EnglishUk).unwrap();
    assert_eq!(raw_at(&out, &run_sheet_path(0, "38")), "14");
    let out = txt2::set_run_old_style(&out, 0, OLD_STYLE_ON).unwrap();
    assert_eq!(raw_at(&out, &run_sheet_path(0, "30")), "2");
    let out = txt2::set_run_faux_bold(&out, 0, true).unwrap();
    assert_eq!(raw_at(&out, &run_sheet_path(0, "2")), "true");
    let out = txt2::set_run_faux_italic(&out, 0, true).unwrap();
    assert_eq!(raw_at(&out, &run_sheet_path(0, "3")), "true");
    // Every write above must leave the block parseable.
    txt2::tokens(&out).unwrap();
}

#[test]
fn paragraph_properties_round_trip() {
    let block = corpus_block("plain_latin");
    let count = txt2::paragraph_count(&block).expect("paragraphs");
    assert!(count >= 1);
    let hyphenation = Hyphenation {
        hyphenate: true,
        words_longer_than: 7,
        after_first: 3,
        before_last: 4,
        limit: 5,
        zone_pt: 18.0,
        capital_words: true,
    };
    let mut out = block.clone();
    for i in 0..count {
        out = txt2::set_paragraph_justification(&out, i, Justification::JustifyLastLeft).unwrap();
        out = txt2::set_paragraph_first_line_indent(&out, i, 24.0).unwrap();
        out = txt2::set_paragraph_start_indent(&out, i, 18.0).unwrap();
        out = txt2::set_paragraph_end_indent(&out, i, 6.5).unwrap();
        out = txt2::set_paragraph_auto_leading(&out, i, 1.5).unwrap();
        out = txt2::set_paragraph_hanging_punctuation(&out, i, true).unwrap();
        out = txt2::set_paragraph_every_line_composer(&out, i, true).unwrap();
        out = txt2::set_paragraph_hyphenation(&out, i, hyphenation).unwrap();
        out = txt2::set_paragraph_range(&out, i, ParagraphProperty::WordSpacing, [0.9, 1.0, 1.2])
            .unwrap();
        out = txt2::set_paragraph_range(&out, i, ParagraphProperty::LetterSpacing, [-2.0, 1.0, 4.0])
            .unwrap();
        out = txt2::set_paragraph_range(&out, i, ParagraphProperty::GlyphScaling, [0.9, 1.0, 1.1])
            .unwrap();
    }

    for i in 0..count {
        assert_eq!(txt2::paragraph_justification(&out, i), Some(Justification::JustifyLastLeft));
        assert_eq!(txt2::paragraph_first_line_indent(&out, i), Some(24.0));
        assert_eq!(txt2::paragraph_start_indent(&out, i), Some(18.0));
        assert_eq!(txt2::paragraph_end_indent(&out, i), Some(6.5));
        assert_eq!(txt2::paragraph_auto_leading(&out, i), Some(1.5));
        assert_eq!(txt2::paragraph_hanging_punctuation(&out, i), Some(true));
        assert_eq!(txt2::paragraph_every_line_composer(&out, i), Some(true));
        assert_eq!(txt2::paragraph_hyphenation(&out, i), Some(hyphenation));
        assert_eq!(
            txt2::paragraph_range(&out, i, ParagraphProperty::WordSpacing),
            Some([0.9, 1.0, 1.2])
        );
        assert_eq!(
            txt2::paragraph_range(&out, i, ParagraphProperty::LetterSpacing),
            Some([-2.0, 1.0, 4.0])
        );
        assert_eq!(
            txt2::paragraph_range(&out, i, ParagraphProperty::GlyphScaling),
            Some([0.9, 1.0, 1.1])
        );
        // The zone is stored in points, the same number `EngineData` keeps.
        assert_eq!(
            txt2::paragraph_property(&out, i, ParagraphProperty::HyphenationZone),
            Some(Value::Real(18.0))
        );
        assert_eq!(
            txt2::paragraph_property(&out, i, ParagraphProperty::Hyphenate),
            Some(Value::Bool(true))
        );
        assert_eq!(
            txt2::paragraph_property(&out, i, ParagraphProperty::HyphenateWordSize),
            Some(Value::Int(7))
        );
        assert_eq!(
            txt2::paragraph_property(&out, i, ParagraphProperty::HyphenateAfterFirst),
            Some(Value::Int(3))
        );
        assert_eq!(
            txt2::paragraph_property(&out, i, ParagraphProperty::HyphenateBeforeLast),
            Some(Value::Int(4))
        );
        assert_eq!(
            txt2::paragraph_property(&out, i, ParagraphProperty::HyphenLimit),
            Some(Value::Int(5))
        );
        assert_eq!(
            txt2::paragraph_property(&out, i, ParagraphProperty::HyphenateCapitalWords),
            Some(Value::Bool(true))
        );
        // And the raw keys are the measured ones.
        assert_eq!(raw_at(&out, &paragraph_sheet_path(i, "0")), "6");
        assert_eq!(raw_at(&out, &paragraph_sheet_path(i, "1")), "24.0");
        assert_eq!(raw_at(&out, &paragraph_sheet_path(i, "2")), "18.0");
        assert_eq!(raw_at(&out, &paragraph_sheet_path(i, "3")), "6.5");
        assert_eq!(raw_at(&out, &paragraph_sheet_path(i, "7")), "1.5");
        assert_eq!(raw_at(&out, &paragraph_sheet_path(i, "9")), "true");
        assert_eq!(raw_at(&out, &paragraph_sheet_path(i, "14")), "18.0");
        assert_eq!(raw_at(&out, &paragraph_sheet_path(i, "21")), "true");
        assert_eq!(raw_at(&out, &paragraph_sheet_path(i, "29")), "true");
    }
    assert_eq!(txt2::paragraph_run_lengths(&out).unwrap(), txt2::paragraph_run_lengths(&block).unwrap());
    assert_eq!(txt2::untokenize(&out, &txt2::tokens(&out).unwrap()), out);
}

#[test]
fn author_write_reread_psd() {
    // Author every property into a real PSD, write it, parse it back and read
    // the same values out of the file on disk.
    let mut file = PsdFile::from_bytes(&corpus_bytes("plain_latin")).expect("psd");
    let block = file.global_blocks.iter_mut().find(|b| &b.key == b"Txt2").expect("Txt2");
    let mut data = txt2::set_run_layout(&block.data, &[10, 20]).unwrap();
    data = txt2::set_run_faux_bold(&data, 0, true).unwrap();
    data = txt2::set_run_capitalization(&data, 0, Capitalization::AllCaps).unwrap();
    data = txt2::set_run_underline(&data, 0, Underline::Left).unwrap();
    data = txt2::set_run_strikethrough(&data, 0, Strikethrough::StrikeBox).unwrap();
    data = txt2::set_run_kerning_type(&data, 0, KerningType::Optical).unwrap();
    data = txt2::set_run_language(&data, 0, Language::EnglishUk).unwrap();
    data = txt2::set_run_old_style(&data, 0, OLD_STYLE_ON).unwrap();
    data = txt2::set_run_ligatures(&data, 0, false).unwrap();
    data = txt2::set_run_alternate_ligatures(&data, 0, true).unwrap();
    data = txt2::set_run_no_break(&data, 0, true).unwrap();
    for i in 0..txt2::paragraph_count(&data).unwrap() {
        data = txt2::set_paragraph_justification(&data, i, Justification::Center).unwrap();
        data = txt2::set_paragraph_auto_leading(&data, i, 1.5).unwrap();
        data = txt2::set_paragraph_hanging_punctuation(&data, i, true).unwrap();
        data = txt2::set_paragraph_hyphenation(
            &data,
            i,
            Hyphenation { hyphenate: true, zone_pt: 12.0, ..Hyphenation::default() },
        )
        .unwrap();
    }
    block.data = data;
    block.padding = None;
    let written = file.to_bytes().expect("serialise psd");

    // Disk round-trip, exactly as the Lead's Photoshop run will do it.
    let path = std::env::temp_dir().join(format!("photocraft_txt2_slots_{}.psd", std::process::id()));
    std::fs::write(&path, &written).expect("write psd");
    let read_back = std::fs::read(&path).expect("read psd");
    let _ = std::fs::remove_file(&path);
    let file = PsdFile::from_bytes(&read_back).expect("re-parse psd");
    let block = file
        .global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .expect("Txt2 after round-trip")
        .data
        .clone();

    assert_eq!(txt2::read_text(&block).as_deref(), Some("Hamburgefonstiv AVATAR To Ty."));
    assert_eq!(txt2::run_lengths(&block).unwrap(), vec![10, 20]);
    assert_eq!(txt2::run_faux_bold(&block, 0), Some(true));
    assert_eq!(txt2::run_capitalization(&block, 0), Some(Capitalization::AllCaps));
    assert_eq!(txt2::run_underline(&block, 0), Some(Underline::Left));
    assert_eq!(txt2::run_strikethrough(&block, 0), Some(Strikethrough::StrikeBox));
    assert_eq!(txt2::run_kerning_type(&block, 0), Some(KerningType::Optical));
    assert_eq!(txt2::run_language(&block, 0), Some(Language::EnglishUk));
    assert_eq!(txt2::run_old_style(&block, 0), Some(OLD_STYLE_ON));
    assert_eq!(txt2::run_ligatures(&block, 0), Some(false));
    assert_eq!(txt2::run_alternate_ligatures(&block, 0), Some(true));
    assert_eq!(txt2::run_no_break(&block, 0), Some(true));
    for i in 0..txt2::paragraph_count(&block).unwrap() {
        assert_eq!(txt2::paragraph_justification(&block, i), Some(Justification::Center));
        assert_eq!(txt2::paragraph_auto_leading(&block, i), Some(1.5));
        assert_eq!(txt2::paragraph_hanging_punctuation(&block, i), Some(true));
        assert_eq!(txt2::paragraph_hyphenation(&block, i).unwrap().zone_pt, 12.0);
    }
}

#[test]
fn unknown_values_are_rejected() {
    let block = corpus_block("plain_latin");

    // Closed enums do not decode unknown codes.
    assert_eq!(KerningType::from_code(3), None);
    assert_eq!(Capitalization::from_code(3), None);
    assert_eq!(Underline::from_code(3), None);
    assert_eq!(Strikethrough::from_code(1), None);
    assert_eq!(Justification::from_code(7), None);

    // ... and the fallible writers refuse them instead of storing a value
    // Photoshop does not define. Every failure must leave the block untouched.
    let attempts: Vec<Box<dyn Fn(&[u8]) -> Result<Vec<u8>, photocraft_text::error::Error>>> = vec![
        Box::new(|b| txt2::set_run_kerning_type_code(b, 0, 9)),
        Box::new(|b| txt2::set_run_capitalization_code(b, 0, 3)),
        Box::new(|b| txt2::set_run_underline_code(b, 0, 3)),
        Box::new(|b| txt2::set_run_strikethrough_code(b, 0, 1)),
        Box::new(|b| txt2::set_run_old_style(b, 0, -1)),
        Box::new(|b| txt2::set_run_language_code(b, 0, -1)),
        Box::new(|b| txt2::set_paragraph_justification_code(b, 0, 7)),
        Box::new(|b| txt2::set_paragraph_auto_leading(b, 0, 0.0)),
        Box::new(|b| txt2::set_paragraph_auto_leading(b, 0, f32::NAN)),
        Box::new(|b| {
            txt2::set_paragraph_hyphenation(b, 0, Hyphenation { limit: -1, ..Hyphenation::default() })
        }),
    ];
    for attempt in attempts {
        assert!(attempt(&block).is_err(), "an unknown value was accepted");
    }

    // Slot names this module does not know are not addressable either: `/6/10`
    // is the manual-kerning amount, whose fixed-point encoding is still
    // unmeasured, so it deliberately has no property.
    assert_eq!(RunProperty::from_key("10"), None);
    assert_eq!(ParagraphProperty::from_key("6"), None, "unmeasured space keys stay unknown");
    assert_eq!(RunProperty::from_key("12"), Some(RunProperty::Capitalization));
    assert_eq!(ParagraphProperty::from_key("21"), Some(ParagraphProperty::HangingPunctuation));
}

fn antialias_dict_path(key: &'static str) -> Vec<Step<'static>> {
    let mut p = txt2::antialias_dict_path();
    p.push(Step::Key(key));
    p
}

#[test]
fn hyphenation_detail_keys_follow_the_flag() {
    let block = corpus_block("plain_latin");
    // Off: only `/9` is written; Photoshop leaves `/10` … `/15` absent, and the
    // reader falls back to the shared default sheet's values.
    let off = txt2::set_paragraph_hyphenation(
        &block,
        0,
        Hyphenation { hyphenate: false, ..Hyphenation::default() },
    )
    .unwrap();
    assert_eq!(raw_at(&off, &paragraph_sheet_path(0, "9")), "false");
    assert_eq!(
        txt2::paragraph_property(&off, 0, ParagraphProperty::HyphenateWordSize),
        None,
        "hyphenation off must not materialise the detail keys"
    );
    let read = txt2::paragraph_hyphenation(&off, 0).unwrap();
    assert!(!read.hyphenate);
    assert_eq!(read.words_longer_than, Hyphenation::default().words_longer_than);
    assert_eq!(read.zone_pt, Hyphenation::default().zone_pt);

    // On: turning the flag on materialises `/10` … `/15`, as Photoshop does
    // (out/probe5/pE.psd `hyphenation`).
    let on = txt2::set_paragraph_hyphenation(
        &off,
        0,
        Hyphenation { hyphenate: true, words_longer_than: 7, ..Hyphenation::default() },
    )
    .unwrap();
    assert_eq!(raw_at(&on, &paragraph_sheet_path(0, "9")), "true");
    assert_eq!(raw_at(&on, &paragraph_sheet_path(0, "10")), "7");
    assert_eq!(raw_at(&on, &paragraph_sheet_path(0, "13")), "0");
    assert_eq!(raw_at(&on, &paragraph_sheet_path(0, "14")), "36.0");
    assert_eq!(raw_at(&on, &paragraph_sheet_path(0, "15")), "true");
    assert_eq!(txt2::paragraph_hyphenation(&on, 0).unwrap().words_longer_than, 7);
}

#[test]
fn antialias_smooth_omits_the_dictionary() {
    let block = corpus_block("plain_latin");
    assert_eq!(raw_at(&block, &txt2::antialias_path()), "4", "corpus is sharp");
    assert_eq!(raw_at(&block, &antialias_dict_path("2")), "true");

    // Every reachable non-smooth code writes `/10/0` and keeps the `/2` flag.
    let sharp = txt2::set_antialias(&block, 1).unwrap();
    assert_eq!(raw_at(&sharp, &txt2::antialias_path()), "1");
    assert_eq!(raw_at(&sharp, &antialias_dict_path("2")), "true");
    let none = txt2::set_antialias(&sharp, 0).unwrap();
    assert_eq!(raw_at(&none, &txt2::antialias_path()), "0");

    // Smooth is the absent key, as Photoshop stores it.
    let smooth = txt2::set_antialias(&none, 3).unwrap();
    assert!(txt2::value_range(&smooth, &txt2::antialias_path()).is_err());
    assert!(txt2::value_range(&smooth, &txt2::antialias_dict_path()).is_err());
    txt2::tokens(&smooth).unwrap();
    assert_eq!(txt2::read_text(&smooth), txt2::read_text(&block));

    // Authoring a non-smooth code onto a smooth template recreates the dict in
    // Photoshop's shape `/10 << /0 <code> /2 true >>`.
    let again = txt2::set_antialias(&smooth, 4).unwrap();
    assert_eq!(raw_at(&again, &txt2::antialias_path()), "4");
    assert_eq!(raw_at(&again, &antialias_dict_path("2")), "true");
    assert_eq!(txt2::read_text(&again), txt2::read_text(&block));
}

#[test]
fn language_id_spaces_translate() {
    use photocraft_text::runs::{engine_language, to_engine_data, txt2_language, RunStyle};
    assert_eq!(engine_language(0), Some(0));
    assert_eq!(engine_language(14), Some(14));
    assert_eq!(engine_language(42), Some(4), "German is 42 in Txt2 but 4 in EngineData");
    assert_eq!(txt2_language(4), Some(42));
    assert_eq!(txt2_language(14), Some(14));
    assert_eq!(engine_language(7), None, "unmeasured ids are not invented");

    // The EngineData mirror writes the translated id.
    let value = to_engine_data(&[RunStyle { len: 1, language: Some(42), ..RunStyle::default() }]);
    let sheet = value
        .get("RunArray")
        .and_then(|r| r.as_array())
        .and_then(|r| r.first())
        .and_then(|r| r.get("StyleSheet"))
        .and_then(|s| s.get("StyleSheetData"))
        .expect("sheet");
    assert_eq!(sheet.get("Language").and_then(|v| v.as_i64()), Some(4));
}
