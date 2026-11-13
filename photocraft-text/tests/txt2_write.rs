//! Path-addressed `Txt2` edits, checked against the corpus blocks.

use photocraft_psd::PsdFile;
use photocraft_text::txt2::{self, Step, Value};

fn corpus_block(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
    let bytes = std::fs::read(dir.join(format!("{name}.psd"))).expect("corpus psd");
    let file = PsdFile::from_bytes(&bytes).expect("parse psd");
    file.global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .expect("Txt2 block")
        .data
        .clone()
}

fn font_size_path(run: usize) -> Vec<Step<'static>> {
    vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(0),
        Step::Key("0"),
        Step::Key("6"),
        Step::Key("0"),
        Step::Index(run),
        Step::Key("0"),
        Step::Key("0"),
        Step::Key("6"),
        Step::Key("1"),
    ]
}

/// Asserts that `new` differs from `old` only inside `old[start..end]` and that
/// the replacement is `expected`.
fn assert_single_span(old: &[u8], new: &[u8], start: usize, end: usize, expected: &str) {
    assert_eq!(&old[..start], &new[..start], "prefix changed");
    let delta = expected.len() as isize - (end - start) as isize;
    assert_eq!(
        &new[start..start + expected.len()],
        expected.as_bytes(),
        "span text"
    );
    assert_eq!(
        &old[end..],
        &new[(end as isize + delta) as usize..],
        "suffix changed"
    );
}

#[test]
fn every_corpus_run_table_matches_text() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("corpus dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("psd") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read psd");
        let Ok(file) = PsdFile::from_bytes(&bytes) else {
            continue;
        };
        for block in file.global_blocks.iter().filter(|b| &b.key == b"Txt2") {
            let data = &block.data;
            let lengths =
                txt2::run_lengths(data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(!lengths.is_empty(), "{}: no runs", path.display());
            // Writing the table back unchanged must succeed and be byte-identical.
            let same = txt2::set_run_lengths(data, &lengths)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(&same, data, "{}", path.display());
            checked += 1;
        }
    }
    assert!(checked > 0, "no Txt2 blocks found in corpus");
}

#[test]
fn plain_latin_run_table_and_font_size_path() {
    let block = corpus_block("plain_latin");
    assert_eq!(txt2::run_count(&block).unwrap(), 1);
    assert_eq!(txt2::run_lengths(&block).unwrap(), vec![30]);

    let (s, e) = txt2::value_range(&block, &font_size_path(0)).unwrap();
    assert_eq!(&block[s..e], b"48.0");
}

#[test]
fn font_size_change_touches_only_its_span() {
    let block = corpus_block("plain_latin");
    let (s, e) = txt2::value_range(&block, &font_size_path(0)).unwrap();
    let out = txt2::set_run_font_size(&block, 0, 96.0).unwrap();
    assert_single_span(&block, &out, s, e, "96.0");
    // Re-tokenising the result must still succeed.
    txt2::tokens(&out).unwrap();
}

#[test]
fn fractional_font_size_uses_trimmed_real_format() {
    let block = corpus_block("plain_latin");
    let (s, e) = txt2::value_range(&block, &font_size_path(0)).unwrap();
    let out = txt2::set_run_font_size(&block, 0, 57.6).unwrap();
    assert_single_span(&block, &out, s, e, "57.6");
}

#[test]
fn length_mismatch_is_refused() {
    let block = corpus_block("plain_latin");
    assert!(txt2::set_run_lengths(&block, &[29]).is_err());
    assert!(txt2::set_run_lengths(&block, &[15, 15]).is_err());
}

#[test]
fn duplicate_run_keeps_text_covered() {
    let block = corpus_block("plain_latin");
    let out = txt2::duplicate_run(&block, 0).unwrap();
    assert_eq!(txt2::run_count(&out).unwrap(), 2);
    assert_eq!(txt2::run_lengths(&out).unwrap(), vec![29, 1]);
    // Text itself must be untouched.
    assert_eq!(txt2::read_text(&out), txt2::read_text(&block));
    txt2::tokens(&out).unwrap();
}

#[test]
fn duplicate_run_takes_units_from_the_longest_run() {
    let block = corpus_block("plain_latin");
    let once = txt2::duplicate_run(&block, 0).unwrap();
    // The donor is the longest run, so the single 30-unit run keeps the text.
    assert_eq!(txt2::run_lengths(&once).unwrap(), vec![29, 1]);
    let twice = txt2::duplicate_run(&once, 1).unwrap();
    assert_eq!(txt2::run_count(&twice).unwrap(), 3);
    assert_eq!(txt2::run_lengths(&twice).unwrap().iter().sum::<i64>(), 30);
    assert_eq!(txt2::read_text(&twice), txt2::read_text(&block));
}

#[test]
fn replace_value_refuses_containers() {
    let block = corpus_block("plain_latin");
    let path = vec![Step::Key("1"), Step::Key("1")];
    assert!(txt2::replace_value(&block, &path, &Value::Int(1)).is_err());
}

#[test]
fn lead_text_keeps_internal_breaks_and_replaces_whole() {
    let block = corpus_block("lead_0");
    let expected = "line one\rline two\rline three";
    assert_eq!(txt2::read_text(&block).as_deref(), Some(expected));

    let (start, end) = txt2::text_span(&block).unwrap();
    // Span ends right after the final CR, which sits directly before ')'.
    assert_eq!(&block[end - 2..end], &[0x00, 0x0d]);
    assert_eq!(block[end], b')');
    assert_eq!(&block[start..start + 2], &[0xfe, 0xff]);

    let same = txt2::replace_text(&block, expected).unwrap();
    assert_eq!(
        same, block,
        "replace_text with identical text must be a no-op"
    );
}

#[test]
fn frame_geometry_reads_box_for_left_and_center() {
    use photocraft_text::txt2::FrameGeometry;
    let want = FrameGeometry {
        left: 0.0,
        top: 0.0,
        right: 430.0,
        bottom: 200.0,
    };
    assert_eq!(txt2::frame_geometry(&corpus_block("box_left")), Some(want));
    assert_eq!(
        txt2::frame_geometry(&corpus_block("box_center")),
        Some(want)
    );
}

#[test]
fn frame_geometry_none_for_point_text() {
    assert_eq!(txt2::frame_geometry(&corpus_block("plain_latin")), None);
}

#[test]
fn set_frame_geometry_round_trips_and_keeps_text() {
    use photocraft_text::txt2::FrameGeometry;
    let block = corpus_block("box_left");
    let g = FrameGeometry {
        left: 10.0,
        top: 20.5,
        right: 440.0,
        bottom: 220.25,
    };
    let out = txt2::set_frame_geometry(&block, g).unwrap();
    assert_eq!(txt2::frame_geometry(&out), Some(g));
    assert_eq!(txt2::read_text(&out), txt2::read_text(&block));
    txt2::tokens(&out).unwrap();
}

#[test]
fn txt2_run_properties_are_writable() {
    use photocraft_text::txt2::{self, RunProperty, Value};

    let bytes = std::fs::read("../corpus/plain_latin.psd").expect("corpus");
    let file = photocraft_psd::PsdFile::from_bytes(&bytes).expect("psd");
    let block = file.global_block(b"Txt2").expect("Txt2").data.clone();
    assert_eq!(txt2::run_count(&block).unwrap(), 1);

    // Split the single run into three, then style each one differently.
    let split = txt2::set_run_layout(&block, &[10, 10, 10]).expect("split");
    assert_eq!(txt2::run_lengths(&split).unwrap().len(), 3);
    assert_eq!(txt2::run_lengths(&split).unwrap().iter().sum::<i64>(), 30);

    let mut out = split.clone();
    out = txt2::set_run_property(&out, 0, RunProperty::FontSize, &Value::Real(96.0)).unwrap();
    out = txt2::set_run_property(&out, 1, RunProperty::Tracking, &Value::Int(250)).unwrap();
    out = txt2::set_run_property(&out, 2, RunProperty::BaselineShift, &Value::Real(12.0)).unwrap();
    out = txt2::set_run_property(&out, 2, RunProperty::HorizontalScale, &Value::Real(1.5)).unwrap();
    out = txt2::set_run_property(&out, 1, RunProperty::VerticalScale, &Value::Real(2.0)).unwrap();

    // Re-read: every property must land in its own run and leave the rest alone.
    let run = |b: &[u8], i: usize| -> String {
        let (s, e) = txt2::value_range(b, &txt2::run_path_for(i)).unwrap();
        String::from_utf8_lossy(&b[s..e]).into_owned()
    };
    assert!(
        run(&out, 0).contains("/1 96.0"),
        "size 96 not written: {}",
        run(&out, 0)
    );
    assert!(run(&out, 1).contains("/8 250"), "tracking 250 not written");
    assert!(
        run(&out, 1).contains("/7 2.0"),
        "vertical scale 2.0 not written"
    );
    assert!(
        run(&out, 2).contains("/9 12.0"),
        "baseline shift not written"
    );
    assert!(
        run(&out, 2).contains("/6 1.5"),
        "horizontal scale not written"
    );
    assert!(run(&out, 0).contains("/8 0"), "run 0 tracking changed");
}

#[test]
fn clear_cache_empties_only_the_view_layout() {
    let bytes = std::fs::read("../corpus/plain_latin.psd").expect("corpus");
    let file = photocraft_psd::PsdFile::from_bytes(&bytes).expect("psd");
    let block = file.global_block(b"Txt2").expect("Txt2").data.clone();

    let out = txt2::clear_cache(&block, 0).expect("clear");
    txt2::tokens(&out).expect("block still parses");
    assert_eq!(
        txt2::read_text(&out),
        txt2::read_text(&block),
        "text untouched"
    );
    assert_eq!(
        txt2::run_lengths(&out).unwrap(),
        txt2::run_lengths(&block).unwrap()
    );

    // The view keeps its `/2` key (Photoshop 2025 crashes without one) and the
    // model keeps the text; only the composed-layout array is emptied.
    let view = txt2::value_range(
        &out,
        &[
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(0),
            Step::Key("1"),
        ],
    )
    .expect("view");
    let view = String::from_utf8_lossy(&out[view.0..view.1]).into_owned();
    assert!(view.contains("/2 [ ]"), "cache not emptied: {view}");
    assert!(
        view.contains("/0 [ << /0 0 >> ]"),
        "frame index lost: {view}"
    );
    assert!(!view.contains("/99 /PC"), "cache content survived");
}

#[test]
fn replace_text_changes_length_and_empties_the_cache() {
    let block = corpus_block("plain_latin");
    let out = txt2::replace_text(&block, "Hello").expect("write short text");
    assert_eq!(txt2::read_text(&out).as_deref(), Some("Hello"));
    // One style run and one paragraph run, each covering the text and its
    // U+000D terminator.
    assert_eq!(txt2::run_lengths(&out).unwrap(), vec![6]);
    let view = txt2::value_range(
        &out,
        &[
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(0),
            Step::Key("1"),
        ],
    )
    .expect("view");
    assert!(String::from_utf8_lossy(&out[view.0..view.1]).contains("/2 [ ]"));
}

#[test]
fn replace_text_splits_paragraphs_at_breaks() {
    let block = corpus_block("plain_latin");
    let out = txt2::replace_text(&block, "two\rparagraphs").unwrap();
    assert_eq!(txt2::read_text(&out).as_deref(), Some("two\rparagraphs"));
    // Style runs cover the whole text; paragraph runs own one break each.
    assert_eq!(txt2::run_lengths(&out).unwrap(), vec![15]);
    assert_eq!(txt2::paragraph_run_lengths(&out).unwrap(), vec![4, 11]);
    // Same text is a no-op even with internal breaks.
    let lead = corpus_block("lead_0");
    let text = txt2::read_text(&lead).unwrap();
    assert_eq!(txt2::replace_text(&lead, &text).unwrap(), lead);
}

#[test]
fn run_fill_colour_round_trips() {
    let block = corpus_block("plain_latin");
    let split = txt2::set_run_layout(&block, &[10, 20]).unwrap();
    let red = [1.0, 1.0, 0.0, 0.0];
    let blue = [1.0, 0.0, 0.0, 1.0];
    let mut out = txt2::set_run_fill_color(&split, 0, red).unwrap();
    out = txt2::set_run_fill_color(&out, 1, blue).unwrap();
    assert_eq!(txt2::run_fill_color(&out, 0), Some(red));
    assert_eq!(txt2::run_fill_color(&out, 1), Some(blue));
    assert_eq!(txt2::run_lengths(&out).unwrap(), vec![10, 20]);
    txt2::tokens(&out).unwrap();
}

#[test]
fn fonts_can_be_appended_and_selected() {
    let block = corpus_block("plain_latin");
    let before = txt2::font_count(&block).unwrap();
    let (appended, idx) = txt2::ensure_font(&block, "NotoSans-Bold").unwrap();
    let (again, once) = txt2::ensure_font(&appended, "NotoSans-Bold").unwrap();
    assert_eq!(idx, once, "ensure_font must be idempotent");
    assert_eq!(again, appended, "second call must not append");
    let out = txt2::set_run_font_index(&appended, 0, idx).unwrap();
    assert_eq!(txt2::font_count(&out).unwrap(), before + 1);
    let split = txt2::set_run_layout(&out, &[10, 20]).unwrap();
    assert_eq!(txt2::font_count(&split).unwrap(), before + 1);
    txt2::tokens(&split).unwrap();
}

#[test]
fn fonts_survive_run_layout() {
    let block = corpus_block("plain_latin");
    let split = txt2::set_run_layout(&block, &[14, 16]).unwrap();
    eprintln!("block len {} split len {}", block.len(), split.len());
    let before = txt2::font_count(&split).unwrap();
    let (out, idx) = txt2::ensure_font(&split, "NotoSans-Regular").unwrap();
    assert_eq!(before, 4);
    assert_eq!(idx, 0, "the layer font must be found, not appended");
    assert_eq!(txt2::font_count(&out).unwrap(), before, "nothing appended");
}

#[test]
fn run_table_bytes_location() {
    let block = corpus_block("plain_latin");
    let (s, e) = txt2::value_range(&block, &txt2::run_path_for(0)).unwrap();
    eprintln!(
        "run0 at {s}..{e}: {:?}",
        String::from_utf8_lossy(&block[s.saturating_sub(40)..e.min(block.len())])
    );
    eprintln!(
        "font raw at {:?}",
        block.windows(17).position(|w| w == b"/99 /CoolTypeFont")
    );
}

/// Parentheses (and backslashes) inside the layer text must be stored with
/// CoolType's byte-wise backslash escapes: Photoshop itself writes
/// `00 5c 28` for "(" (verified in `out/demo/10_all_features.psd`, text
/// "English (UK)").  An unescaped parenthesis truncates the string token and
/// Photoshop 20.0.2 refuses to open the file (`out/demo/03` was such a file).
#[test]
fn parentheses_and_backslashes_round_trip_in_text() {
    let block = corpus_block("plain_latin");
    let text = "A (B) C \\ D\rSecond (line) \\ end";
    let out = txt2::replace_text(&block, text).unwrap();
    assert_eq!(txt2::read_text(&out).as_deref(), Some(text));
    // The raw token must escape '(' as `00 5c 28`, exactly like Photoshop.
    assert!(out.windows(3).any(|w| w == b"\x00\x5c\x28"));
    assert!(out.windows(3).any(|w| w == b"\x00\x5c\x29"));
    assert!(out.windows(3).any(|w| w == b"\x00\x5c\x5c"));
    // Run tables still agree with the escaped text length.
    let units = text.encode_utf16().count() + 1;
    let split = txt2::set_run_layout(&out, &[units as i64]).unwrap();
    assert_eq!(txt2::run_lengths(&split).unwrap(), vec![units as i64]);
    txt2::tokens(&split).unwrap();
}
