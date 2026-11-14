//! The public layout API (`layer_render::layout_from_engine_data*`).
//!
//! The point of the API is that it is *the renderer's* layout, so these tests
//! pin it against the pixels: every caret stop must sit on the drawn glyphs of
//! its line, the stops must cover the text, and a hit test must round-trip
//! through the caret position. The fixtures are the checked-in recorded
//! `EngineData` models the render goldens use (`tests/assets/models`), so the
//! layout and the goldens describe the same text.

use std::path::{Path, PathBuf};

use photocraft_text::engine_data::{self, Value};
use photocraft_text::layer_render::{self, FaceStore, RectTarget, TextLayout};
use photocraft_text::psd::LayerSpec;

#[derive(Clone, serde::Deserialize)]
struct Case {
    name: String,
    template: String,
    spec: LayerSpec,
}

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!("assets/cases.json")).expect("compatibility input cases")
}

fn case(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|c| c.name == name)
        .expect("named case")
}

fn store() -> FaceStore {
    let mut store = FaceStore::new();
    for name in ["NotoSans-Regular", "NotoSans-Bold"] {
        store
            .load(&assets().join("fonts"), name)
            .expect("checked-in font");
    }
    store
}

fn text(case: &Case) -> String {
    case.spec
        .text
        .replace("\r\n", "\r")
        .replace('\n', "\r")
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

fn box_size(case: &Case) -> (f32, f32) {
    case.spec
        .frame
        .map(|[l, t, r, b]| (r - l, b - t))
        .unwrap_or((0.0, 0.0))
}

fn model(case: &Case) -> Value {
    let _ = &case.template;
    let raw = std::fs::read(
        assets()
            .join("models")
            .join(format!("{}.engine.bin", case.name)),
    )
    .expect("checked-in EngineData input");
    engine_data::parse(&raw).expect("EngineData input")
}

fn layout(case: &Case) -> TextLayout {
    layout_box(case, box_size(case))
}

fn layout_box(case: &Case, size: (f32, f32)) -> TextLayout {
    let ed = model(case);
    let store = store();
    let text = text(case);
    layer_render::layout_from_engine_data(&store, &ed, &text, size, (0.0, 0.0))
        .unwrap_or_else(|e| panic!("{}: layout: {e}", case.name))
}

fn horizontal_cases() -> Vec<Case> {
    [
        "point_plain",
        "point_styles",
        "point_center",
        "area_paragraphs",
        "area_composer",
    ]
    .map(case)
    .to_vec()
}

#[test]
fn every_caret_stop_sits_on_the_glyphs_the_renderer_drew() {
    for case in horizontal_cases() {
        let layout = layout(&case);
        assert!(!layout.lines.is_empty(), "{}: no lines", case.name);
        for (index, line) in layout.lines.iter().enumerate() {
            let glyphs: Vec<_> = layout
                .glyphs
                .iter()
                .filter(|g| {
                    line.range.contains(&g.range.start) || g.range.start == line.range.start
                })
                .collect();
            if glyphs.is_empty() {
                // An empty paragraph's caret row: its stops are the paragraph
                // separator bytes, drawn nowhere; only the x must match.
                for cluster in layout.clusters.iter().filter(|c| c.line == index) {
                    assert!(
                        (cluster.x - line.x0).abs() < 0.001,
                        "{}: empty paragraph stop {} at {} vs caret x {}",
                        case.name,
                        cluster.range.start,
                        cluster.x,
                        line.x0
                    );
                }
                continue;
            }
            let mut left = f32::INFINITY;
            let mut right = f32::NEG_INFINITY;
            let mut pen_end = f32::NEG_INFINITY;
            for glyph in &glyphs {
                left = left.min(glyph.x);
                pen_end = pen_end.max(glyph.x + glyph.advance);
                let drawn = layout
                    .text
                    .get(glyph.range.clone())
                    .is_none_or(|slice| slice.chars().any(|c| !c.is_whitespace()));
                if drawn {
                    right = right.max(glyph.x + glyph.advance);
                }
            }
            assert!(
                (line.x0 - left).abs() < 0.001 && (line.x1 - right).abs() < 0.001,
                "{}: line {} box {}..{} vs glyphs {}..{}",
                case.name,
                index,
                line.x0,
                line.x1,
                left,
                right
            );
            assert!(
                pen_end >= line.x1 - 0.001,
                "{}: the line's pen ends before its ink",
                case.name
            );
            for cluster in layout.clusters.iter().filter(|c| c.line == index) {
                assert!(
                    cluster.x >= line.x0 - 0.001 && cluster.x <= line.x1 + 0.001,
                    "{}: caret at {} is outside line {} ({}..{})",
                    case.name,
                    cluster.range.start,
                    index,
                    line.x0,
                    line.x1
                );
                assert!(
                    cluster.advance >= 0.0,
                    "{}: negative advance at {}",
                    case.name,
                    cluster.range.start
                );
                // A stop with glyphs must coincide with one of their pens.
                let on_glyph = glyphs.iter().any(|g| {
                    (g.x - cluster.x).abs() < 0.01 || (g.x + g.advance - cluster.x).abs() < 0.01
                });
                assert!(
                    on_glyph || cluster.advance == 0.0,
                    "{}: caret at {} ({:?}) is off the glyphs of line {}",
                    case.name,
                    cluster.range.start,
                    cluster.range,
                    index
                );
            }
        }
    }
}

#[test]
fn caret_stops_cover_every_drawn_character_in_order() {
    for case in horizontal_cases() {
        let text = text(&case);
        let layout = layout(&case);
        let mut cursor = 0usize;
        for (i, cluster) in layout.clusters.iter().enumerate() {
            assert!(
                cluster.range.start >= cursor,
                "{}: stop {i} starts before the previous one",
                case.name
            );
            let skipped = &text[cursor..cluster.range.start];
            assert!(
                skipped
                    .chars()
                    .all(|c| matches!(c, '\r' | '\n' | '\u{2029}')),
                "{}: stop {i} skips text {skipped:?}",
                case.name
            );
            assert!(
                cluster.range.end > cluster.range.start,
                "{}: empty stop at {}",
                case.name,
                cluster.range.start
            );
            assert!(
                text.is_char_boundary(cluster.range.start)
                    && text.is_char_boundary(cluster.range.end),
                "{}: stop {i} is not on character boundaries",
                case.name
            );
            cursor = cluster.range.end;
        }
        let tail = &text[cursor..];
        assert!(
            tail.chars().all(|c| matches!(c, '\r' | '\n' | '\u{2029}')),
            "{}: stops do not reach the text end: {tail:?}",
            case.name
        );
        // Caret lines follow the text order.
        let mut line = 0usize;
        for cluster in &layout.clusters {
            assert!(cluster.line >= line, "{}: stops go back a line", case.name);
            line = cluster.line;
        }
    }
}

#[test]
fn separators_are_line_breaks_not_caret_stops() {
    for case in horizontal_cases() {
        let text = text(&case);
        let layout = layout(&case);
        for cluster in &layout.clusters {
            let slice = &text[cluster.range.clone()];
            assert!(
                !slice.chars().all(|c| matches!(c, '\r' | '\n' | '\u{2029}')),
                "{}: separator got a caret stop",
                case.name
            );
        }
        // A line's break byte maps to that line's end, not to the next line.
        for line in &layout.lines {
            let current = layout.line_of(line.range.start).unwrap_or(0);
            let next = layout.line_step(line.range.start, true, line.x0 + 30.0);
            if current + 1 < layout.lines.len() {
                assert!(
                    next >= layout.lines[current].range.end
                        && next <= layout.lines[current + 1].range.end,
                    "{}: forward step from line {current} gave {next}, outside {}..={}",
                    case.name,
                    layout.lines[current].range.end,
                    layout.lines[current + 1].range.end
                );
            } else {
                assert_eq!(
                    next,
                    text.len(),
                    "{}: forward step from the last line",
                    case.name
                );
            }
            let back = layout.line_step(line.range.start, false, line.x0 + 30.0);
            if current == 0 {
                assert_eq!(back, 0, "{}: back step from line 0", case.name);
            } else {
                assert!(
                    back <= layout.lines[current].range.start
                        && back >= layout.lines[current - 1].range.start,
                    "{}: back step from line {current} gave {back}, outside {}..={}",
                    case.name,
                    layout.lines[current - 1].range.start,
                    layout.lines[current].range.start
                );
            }
        }
    }
}

/// D6: word boundaries are alphanumeric runs; punctuation and spaces are not
/// part of a word.
#[test]
fn word_boundaries_use_alphanumeric_runs() {
    for case in horizontal_cases() {
        let layout = layout(&case);
        let text = text(&case);
        let mut at = 0usize;
        while at < text.len() {
            let next = layout.word_boundary(at, true);
            assert!(
                next > at,
                "{}: forward word boundary did not advance at {at}",
                case.name
            );
            assert!(
                text.is_char_boundary(next),
                "{}: word boundary off a char",
                case.name
            );
            let before = text[..next].chars().next_back();
            assert!(
                before.is_none_or(char::is_alphanumeric) || next == text.len(),
                "{}: word boundary {next} ends after {before:?} without reaching the text end",
                case.name
            );
            let after = text[next..].chars().next();
            assert!(
                after.is_none_or(|c| !c.is_alphanumeric()),
                "{}: word boundary {next} starts inside a word at {after:?}",
                case.name
            );
            at = next;
        }
        // Backwards walks the same stops in reverse.
        let mut back = text.len();
        let mut guard = 0usize;
        while back > 0 {
            let prev = layout.word_boundary(back, false);
            assert!(
                prev < back,
                "{}: backward word boundary did not move at {back}",
                case.name
            );
            back = prev;
            guard += 1;
            assert!(
                guard < text.len() + 2,
                "{}: backward word walk did not terminate",
                case.name
            );
        }
    }
}

/// D8: a glyph-less layer still has a caret line, so clicks land somewhere and
/// `bounds` is not `None`.
#[test]
fn an_empty_layer_keeps_one_caret_line() {
    let case = case("point_plain");
    let ed = model(&case);
    let store = store();
    let layout = layer_render::layout_from_engine_data(&store, &ed, "", (0.0, 0.0), (0.0, 0.0))
        .expect("empty layout");
    assert_eq!(
        layout.lines.len(),
        1,
        "empty text must place one caret line"
    );
    assert_eq!(layout.lines[0].range, 0..0);
    assert!(layout.clusters.is_empty());
    let bounds = layout.bounds().expect("an empty layer is still hittable");
    assert!(bounds.height() > 0.0, "the caret line needs a height");
    assert_eq!(layout.hit_test(0.0, 0.0), 0);
    let caret = layout.caret(0);
    assert_eq!(caret.line, 0);
    assert!(caret.y1 > caret.y0, "the empty caret line has no height");
    assert!(!layout.overflows());
}

#[test]
fn hit_test_round_trips_through_every_caret_position() {
    for case in horizontal_cases() {
        let layout = layout(&case);
        for cluster in &layout.clusters {
            let caret = layout.caret(cluster.range.start);
            let expect = layout.line_of(cluster.range.start).expect("line");
            assert_eq!(
                caret.line, expect,
                "{}: caret line for byte {}",
                case.name, cluster.range.start
            );
            let middle = (caret.y0 + caret.y1) * 0.5;
            // Just right of the stop selects it; the exact pen must not hop
            // over the stop on rounding.
            if cluster.advance > 0.0 {
                let x = caret.x0;
                let (hit, hit_line) = layout.hit_test_line(x + 0.05, middle);
                assert_eq!(
                    hit,
                    cluster.range.start,
                    "{}: hit at ({}, {}) climbed the line (line {hit_line})",
                    case.name,
                    x + 0.05,
                    middle
                );
            }
            if cluster.range.start > 0 {
                let before = layout.hit_test(caret.x0 - 0.05, middle);
                assert!(
                    before <= cluster.range.start,
                    "{}: hit just left of {} gave {}",
                    case.name,
                    cluster.range.start,
                    before
                );
            }
        }
        // Past the last stop of the last line the hit is the text end.
        let last = layout.lines.last().expect("line");
        let (end, line) = layout.hit_test_line(last.end + 100.0, last.baseline);
        assert_eq!(end, layout.text_len(), "{}: hit past the end", case.name);
        assert_eq!(
            line,
            layout.lines.len() - 1,
            "{}: hit past the end line",
            case.name
        );
    }
}

#[test]
fn the_text_end_caret_is_past_the_last_glyph() {
    for case in horizontal_cases() {
        let layout = layout(&case);
        let last = layout.lines.last().expect("line");
        let end = layout.text_len();
        let caret = layout.caret(end);
        assert_eq!(
            caret.line,
            layout.lines.len() - 1,
            "{}: end caret line",
            case.name
        );
        let last_cluster = layout.clusters.last().expect("cluster");
        assert!(
            caret.x0 >= last_cluster.x + last_cluster.advance - 0.01,
            "{}: end caret {} is before the last glyph's advance end {}",
            case.name,
            caret.x0,
            last_cluster.x + last_cluster.advance
        );
        assert!(
            caret.x0 >= last.x1 - 0.01,
            "{}: end caret {} is before the line's ink end {}",
            case.name,
            caret.x0,
            last.x1
        );
        // The end caret matches a hit test past the last glyph.
        let hit = layout.hit_test(last.end + 100.0, last.baseline);
        assert_eq!(hit, end, "{}: hit past the end", case.name);
    }
}

/// The paragraph's first line starts after `start_indent + first_line_indent`
/// and every following line after `start_indent` alone (left-aligned runs, so
/// the origin is the indent and not an alignment offset).
#[test]
fn paragraph_indents_move_the_first_line_and_every_line() {
    for name in ["area_paragraphs", "area_composer"] {
        let case = case(name);
        let ed = model(&case);
        let paragraphs = photocraft_text::runs::paragraphs_from_engine_data(&ed);
        let mut left = paragraphs.clone();
        for paragraph in &mut left {
            paragraph.justification = photocraft_text::area::Justification::Left;
        }
        let store = store();
        let text = text(&case);
        let size = box_size(&case);
        let layout = layer_render::layout_from_engine_data_with_paragraphs(
            &store,
            &ed,
            &text,
            &left,
            size,
            (0.0, 0.0),
        )
        .unwrap_or_else(|e| panic!("{name}: layout: {e}"));
        assert!(!layout.lines.is_empty(), "{name}: no lines");
        for line in &layout.lines {
            let p = left
                .get(line.paragraph)
                .or_else(|| left.last())
                .expect("paragraph run");
            let one = p.start_indent;
            let other = p.start_indent + p.first_line_indent;
            assert!(
                (line.x0 - one).abs() < 0.51 || (line.x0 - other).abs() < 0.51,
                "{name}: line {} starts at {} outside its indents {one}/{other}",
                line.paragraph,
                line.x0
            );
            // Only the first line of a paragraph takes the first-line indent.
            if let Some(previous) = layout
                .lines
                .iter()
                .find(|l| l.paragraph == line.paragraph && l.range.end <= line.range.start)
            {
                let _ = previous;
                assert!(
                    (line.x0 - one).abs() < 0.51,
                    "{name}: a later line of paragraph {} took the first-line indent",
                    line.paragraph
                );
            }
        }
        // The first line of the first paragraph takes both indents.
        let first = left.first().expect("paragraph");
        assert!(
            (layout.lines[0].x0 - (first.start_indent + first.first_line_indent)).abs() < 0.51,
            "{name}: first line starts at {} not {}",
            layout.lines[0].x0,
            first.start_indent + first.first_line_indent
        );
    }
}

/// A justified paragraph keeps its indents (a justified line starts at the
/// start indent) and its ink reaches the measure, not the box edge.
#[test]
fn a_composer_paragraph_keeps_its_indents_and_justification() {
    let case = case("area_composer");
    let layout = layout(&case);
    let (width, _) = box_size(&case);
    let spec = &case.spec.paragraphs;
    let style = spec.last().unwrap_or(&case.spec.paragraph);
    for line in &layout.lines {
        assert!(
            line.x0 >= style.start_indent - 0.51,
            "line starts left of the start indent: {}",
            line.x0
        );
        let limit = width - style.end_indent + 8.0;
        assert!(
            line.x1 <= limit,
            "line ink {} is past the measure {}",
            line.x1,
            limit
        );
    }
    // At least one non-last line was justified to the measure.
    let measure = width - style.end_indent;
    assert!(
        layout
            .lines
            .iter()
            .take(layout.lines.len().saturating_sub(1))
            .any(|line| (line.x1 - measure).abs() < 1.0),
        "no justified line reaches the measure {measure}"
    );
}

/// Point text is placed against its first baseline: the first line's baseline is
/// 0 and the caret spans the ascent above and the descent below it.
#[test]
fn point_text_carets_start_at_the_first_baseline() {
    for name in ["point_plain", "point_styles"] {
        let case = case(name);
        let layout = layout(&case);
        let line = &layout.lines[0];
        assert!(
            line.baseline.abs() < 0.01,
            "{name}: first baseline at {}",
            line.baseline
        );
        let caret = layout.caret(0);
        assert!(
            caret.y0 < line.baseline && caret.y1 > line.baseline,
            "{name}: caret box"
        );
        let _ = &layout.lines;
        assert!(
            (caret.y0 - (line.baseline - line.ascent)).abs() < 0.01,
            "{name}: caret top is not the ascent"
        );
        assert!(
            (caret.y1 - (line.baseline + line.descent)).abs() < 0.01,
            "{name}: caret bottom is not the descent"
        );
    }
}

/// A short box drops lines and reports overflow; a tall one does not.
#[test]
fn a_short_box_reports_overflow_and_a_tall_one_does_not() {
    let case = case("area_paragraphs");
    let tall = layout_box(&case, (480.0, 400.0));
    assert!(!tall.overflows(), "the tall box drops nothing");
    let short = layout_box(&case, (480.0, 30.0));
    assert!(
        short.lines.len() < tall.lines.len(),
        "the short box kept every line"
    );
    assert!(
        short.overflows(),
        "the short box drops text without reporting it"
    );
}

/// The in-rect entry points report document positions: the anchor moves the
/// caret by the canvas/box offset.
#[test]
fn the_in_rect_layout_reports_document_positions() {
    let case = case("area_paragraphs");
    let ed = model(&case);
    let store = store();
    let text = text(&case);
    let size = box_size(&case);
    let target = RectTarget {
        origin: (100, 50),
        size: (size.0 as u32, size.1 as u32),
        anchor: (10.0, 20.0),
    };
    let paragraphs = photocraft_text::runs::paragraphs_from_engine_data(&ed);
    let layout = layer_render::layout_from_engine_data_in_rect(
        &store,
        &ed,
        &text,
        &paragraphs,
        size,
        (0.0, 0.0),
        target,
    )
    .expect("in-rect layout");
    let plain = layout_box(&case, size);
    assert_eq!(layout.anchor, (110.0, 70.0));
    let a = plain.caret(0);
    let b = layout.caret(0);
    // Layout space is unchanged; the anchor is the document position a caller
    // adds to it (`document = anchor + (x, y)`).
    assert!((b.x0 - a.x0).abs() < 0.01, "the anchor moved the layout x");
    assert!((b.y1 - a.y1).abs() < 0.01, "the anchor moved the layout y");
    assert_eq!(layout.anchor, (110.0, 70.0));
    let bounds = layout.bounds().expect("bounds");
    let plain_bounds = plain.bounds().expect("bounds");
    assert!((layout.anchor.0 + bounds.x0 - (110.0 + plain_bounds.x0)).abs() < 0.01);
    assert!((layout.anchor.1 + bounds.y0 - (70.0 + plain_bounds.y0)).abs() < 0.01);
    // The line origin maps onto the target's document position.
    assert!((layout.anchor.0 + layout.lines[0].x0 - (110.0 + plain.lines[0].x0)).abs() < 0.01);
}

#[test]
fn vertical_caret_stops_run_down_the_column() {
    let case = case("point_plain");
    let ed = model(&case);
    let store = store();
    let text = text(&case);
    let layout = layer_render::layout_vertical_from_engine_data(&store, &ed, &text, (0.0, 0.0))
        .expect("vertical layout");
    assert!(
        layout.vertical,
        "the vertical entry point returned a horizontal layout"
    );
    assert!(!layout.clusters.is_empty(), "no vertical clusters");
    assert!(!layout.lines.is_empty(), "no vertical line");
    let mut last = f32::NEG_INFINITY;
    for cluster in &layout.clusters {
        assert!(
            cluster.x >= last - 0.01,
            "vertical caret stops do not advance down the column: {} after {last}",
            cluster.x
        );
        last = cluster.x;
        let caret = layout.caret(cluster.range.start);
        assert!((caret.y0 - cluster.x).abs() < 0.01, "vertical caret y");
        assert!(
            (caret.y1 - caret.y0).abs() < 0.01,
            "vertical caret is not a bar"
        );
        assert!(caret.x1 > caret.x0, "vertical caret has no width");
    }
}

/// Vertical type: a hard break starts a new column, columns advance leftwards
/// (Photoshop's right-to-left column order), and the caret/hit test/line step
/// work per column.
#[test]
fn vertical_hard_breaks_start_a_column_to_the_left() {
    let case = case("point_styles");
    let ed = model(&case);
    let store = store();
    let text = "ABC\rDEF";
    let layout = layer_render::layout_vertical_from_engine_data(&store, &ed, text, (0.0, 0.0))
        .expect("vertical layout");
    assert!(layout.vertical);
    assert_eq!(
        layout.lines.len(),
        2,
        "a hard break must start a second column"
    );
    let [first, second] = [&layout.lines[0], &layout.lines[1]];
    assert!(
        second.x0 < first.x0 - 1.0,
        "the second column is not to the left in layout space: {} vs {}",
        second.x0,
        first.x0
    );
    assert_eq!(&text[first.range.clone()], "ABC");
    assert_eq!(&text[second.range.clone()], "DEF");
    // The caret at the break byte belongs to the column it ends; the text end
    // belongs to the last column.
    assert_eq!(layout.caret(3).line, 0);
    assert_eq!(layout.caret(text.len()).line, 1);
    // A point inside the second column resolves to it.
    let across = (second.x0 + second.x1) * 0.5;
    let (hit, line) = layout.hit_test_line(across, first.end * 0.5);
    assert_eq!(line, 1, "hit test did not pick the second column");
    assert!(
        hit >= 4,
        "hit test returned a byte before the second column: {hit}"
    );
    // Column steps.
    let next = layout.line_step(0, true, 0.0);
    assert_eq!(
        layout.line_of(next),
        Some(1),
        "line_step did not move to the next column"
    );
    assert_eq!(layout.line_step(text.len(), true, 0.0), text.len());
    assert_eq!(layout.line_step(0, false, 0.0), 0);
    // Both columns span the bounds.
    let bounds = layout.bounds().expect("bounds");
    assert!(
        bounds.width() > 10.0,
        "bounds ignore the second column: {bounds:?}"
    );
    assert!(bounds.y1 > 0.0);
}

/// Vertical type wraps a column when the box height runs out.
#[test]
fn vertical_soft_wraps_when_the_column_height_runs_out() {
    let case = case("point_styles");
    let ed = model(&case);
    let store = store();
    let text = "ABCDEFGH";
    let size = case.spec.runs.first().and_then(|r| r.size).unwrap_or(18.0);
    let tall = layer_render::layout_vertical_from_engine_data(&store, &ed, text, (0.0, 0.0))
        .expect("vertical layout");
    assert_eq!(
        tall.lines.len(),
        1,
        "without a box the text stays one column"
    );
    let short = layer_render::layout_vertical_from_engine_data_in_box(
        &store,
        &ed,
        text,
        (0.0, 0.0),
        (size * 3.0, size * 3.0),
    )
    .expect("vertical box layout");
    // Three em of height hold three glyphs per column, so the eight glyphs
    // wrap into three columns (3 + 3 + 2). Every one of them is composed: a
    // vertical box has no width clip (a Photoshop box-width sweep down to 4 pt
    // keeps all five columns, `out/verify/txt2work/sweep_result.json`), and the
    // column anchored at the box's right edge deliberately overhangs it.
    assert_eq!(short.lines.len(), 3, "three wrapped columns");
    assert_eq!(&text[short.lines[2].range.clone()], "GH");
    assert!(
        short.lines[1].x0 < short.lines[0].x0,
        "wrapped columns do not advance left"
    );
    assert_eq!(&text[short.lines[0].range.clone()], "ABC");
}

/// A vertical paragraph box anchors its first column at the box's **right**
/// edge and advances leftwards, the way Photoshop composes vertical type.
///
/// Regression for the user-reported export (`user_vertical_bad.psd`): the
/// layout used to put column 0 at the box's left edge, so a 453.69-wide box with
/// five 30 px columns drew them at x = -120..0 — the whole block one box width
/// outside the frame the user sees.
#[test]
fn a_vertical_box_anchors_its_columns_at_the_right_edge() {
    let case = case("point_styles");
    let ed = model(&case);
    let store = store();
    let text = "ABCDEFGH";
    let size = case.spec.runs.first().and_then(|r| r.size).unwrap_or(18.0);
    let (width, height) = (size * 8.0, size * 4.0);
    let layout = layer_render::layout_vertical_from_engine_data_in_box(
        &store,
        &ed,
        text,
        (0.0, 0.0),
        (width, height),
    )
    .expect("vertical box layout");
    assert_eq!(
        layout.lines.len(),
        2,
        "four em of height hold four glyphs per column"
    );
    // (the 1.2-em step still fits both columns inside an eight-em box)
    // A column is one em wide but the column *step* is the leading (Photoshop
    // advances columns by the line height, measured on `hotfix_u12.psd`: 36 px
    // at 30 pt with `AutoLeading 1.2`, 150 px at 5.0, 3 px at 0.1); the first
    // column's em cell is centred `size * vertical_b0_ratio` inside the box's
    // right edge — Photoshop's composed cache gives 17.90369 pt at 48 pt
    // Myriad and 19.34399 pt at 48 pt Noto Sans (`box_v`/`h_notosans`), a font
    // constant, not half an em.
    let pitch = size * case.spec.paragraph.auto_leading;
    let centre = |line: &photocraft_text::layer_render::TextLine| (line.x0 + line.x1) * 0.5;
    let face = store.face("NotoSans-Regular").expect("checked-in font");
    let b0 = size * layer_render::vertical_b0_ratio(&face);
    let first = &layout.lines[0];
    assert!(
        (centre(first) - (width - b0)).abs() < 0.5,
        "first column centre {:.2}, expected {:.2} (width {width}, b0 {b0})",
        centre(first),
        width - b0
    );
    assert!(
        (b0 - 0.4029998 * size).abs() < 1e-4,
        "Noto Sans anchor {b0} is not the measured 0.4029998 em"
    );
    // Every following column steps exactly one leading left and stays inside the box.
    for (index, line) in layout.lines.iter().enumerate() {
        let expected = centre(first) - index as f32 * pitch;
        assert!(
            (centre(line) - expected).abs() < 0.51,
            "column {index} centre {:.2}, expected {expected:.2}",
            centre(line)
        );
        assert!(
            centre(line) >= size * 0.5 - 0.5,
            "column {index} left the box: centre {:.2}",
            centre(line)
        );
    }
    // Point text has no box width: column 0 stays on the TySh origin.
    let point = layer_render::layout_vertical_from_engine_data(&store, &ed, text, (0.0, 0.0))
        .expect("vertical point layout");
    assert!(
        point.lines[0].x1 > -size && point.lines[0].x0 < 0.0,
        "point text column moved off its origin: {:.2}..{:.2}",
        point.lines[0].x0,
        point.lines[0].x1
    );
}

/// A vertical layer must never lay out or render empty, whatever the paragraph
/// leading: the user's `u12` layers (auto-leading 0.1 and 5.0) once lost their
/// text entirely — the layer cached a blank raster after an edit.
///
/// The extreme ratios are built through the writer (`build_engine_data`) so the
/// test exercises the same `ParagraphRun` the editor produces, and the check
/// covers both paths: a fresh box render and the stored-rectangle rerender the
/// editor uses when a layer already has pixels.
#[test]
fn extreme_vertical_leading_never_lays_out_empty() {
    let store = store();
    for ratio in [0.1f32, 5.0] {
        let case = case("point_styles");
        let mut spec = case.spec.clone();
        spec.orientation = Some(1);
        spec.frame = Some([0.0, 0.0, 135.0, 179.0]);
        spec.paragraphs.clear();
        spec.paragraph.auto_leading = ratio;
        for run in &mut spec.runs {
            run.leading = None;
        }
        let ed = photocraft_text::psd::build_engine_data(&spec);
        let text = spec.text.replace('\n', "\r");
        let box_size = (135.0f32, 179.0f32);
        let layout = layer_render::layout_vertical_from_engine_data_in_box(
            &store,
            &ed,
            &text,
            (0.0, 0.0),
            box_size,
        )
        .unwrap_or_else(|e| panic!("ratio {ratio}: layout refused: {e:?}"));
        assert!(
            !layout.lines.is_empty(),
            "ratio {ratio}: vertical layout produced no columns"
        );
        let bounds = layout
            .bounds()
            .unwrap_or_else(|| panic!("ratio {ratio}: no bounds"));
        assert!(
            bounds.width() > 0.0 && bounds.height() > 0.0,
            "ratio {ratio}: empty bounds {bounds:?}"
        );
        let pixels = layer_render::render_vertical_from_engine_data_in_box(
            &store,
            &ed,
            &text,
            (0.0, 0.0),
            box_size,
        )
        .unwrap_or_else(|e| panic!("ratio {ratio}: render refused: {e:?}"));
        assert!(
            pixels.rgba.chunks(4).any(|px| px[3] != 0),
            "ratio {ratio}: fresh render has no ink"
        );
        // The stored-rectangle rerender the editor uses on an edited layer: the
        // layer's stored rectangle is the box itself, and the anchor is the box
        // origin, which is the layout's own origin here.
        let target = RectTarget {
            origin: (0, 0),
            size: (135, 179),
            anchor: (0.0, 0.0),
        };
        let again = layer_render::render_vertical_from_engine_data_in_rect(
            &store,
            &ed,
            &text,
            (0.0, 0.0),
            Some(box_size),
            target,
        )
        .unwrap_or_else(|e| panic!("ratio {ratio}: rerender refused: {e:?}"));
        assert!(
            again.rgba.chunks(4).any(|px| px[3] != 0),
            "ratio {ratio}: stored-rect rerender is blank (the text would disappear)"
        );
    }
}

/// A vertical area box anchors its first column's em cell `b0` inside the
/// right edge, with `b0 = size * ratio` a **font** constant — Photoshop's own
/// composed cache gives 17.90369 pt at 48 pt Myriad and 8.95184 pt at 24 pt
/// (0.3729935 em, identical at 24/48/72), and 19.34399 pt at 48 pt Noto Sans
/// (0.4029998 em). Anchoring it half an em inside the edge put every column
/// `(0.5 - ratio) * size` too far right in the app-vs-Photoshop comparison.
#[test]
fn vertical_box_anchors_the_first_column_b0_inside_the_right_edge() {
    let case = case("point_plain"); // first run: NotoSans-Regular 48 pt
    let ed = model(&case);
    let store = store();
    let layout = layer_render::layout_vertical_from_engine_data_in_box(
        &store,
        &ed,
        "H",
        (0.0, 0.0),
        (600.0, 400.0),
    )
    .expect("vertical box layout");
    assert!(layout.vertical);
    let line = &layout.lines[0];
    let centre = (line.x0 + line.x1) * 0.5;
    // 600 - 0.4029998 * 48 = 580.656.
    assert!(
        (centre - 580.656).abs() < 0.02,
        "first column centre {centre}, want 580.656 (x0 {} x1 {})",
        line.x0,
        line.x1
    );
    // Not the old half-em anchor (576.0), which was off by 4.656 pt here.
    assert!((centre - 576.0).abs() > 4.0);
}

/// Along the column, a vertical box starts each column's em cell at the
/// paragraph's justification offset inside the frame — `Left`/top at the frame
/// top, `Center` at `(height - column extent) / 2` — and our pen sits
/// `first_off` inside that cell. The cache numbers this pins (Photoshop's own
/// composed cache, `out/verify/text-baseline-offset.md` §15): `Left` 24/48/72 pt
/// Myriad boxes give column start 0.0; the `Center` u12 layers give 10.0/55.0
/// (30 pt, frame 200, columns 180/90), 14.5/74.5/44.5 (frame 179, columns
/// 150/30/90) and 77.316 (frame 334.63, column 180).
#[test]
fn vertical_box_centres_each_column_along_the_frame_for_center_justification() {
    let case = case("point_center"); // paragraphs are `justification 2` (Center)
    let ed = model(&case);
    let store = store();
    let text = "AVATAR\rTy"; // 6 + 2 glyphs at 24 pt -> 144 pt and 48 pt columns
    let layout = layer_render::layout_vertical_from_engine_data_in_box(
        &store,
        &ed,
        text,
        (0.0, 0.0),
        (200.0, 200.0),
    )
    .expect("vertical box layout");
    assert_eq!(layout.lines.len(), 2, "two paragraphs, two columns");
    // NotoSans-Regular cap 714/1000 at 24 pt -> first_off = (24 + 17.136)/2.
    let first_off = (24.0 + 0.714 * 24.0) * 0.5;
    let first_column = layout.caret(0).y0;
    let second_column = layout.caret(7).y0; // after "AVATAR\r"
    assert!(
        (first_column - (first_off + 28.0)).abs() < 0.02,
        "column 0 along start {first_column}, want {} (frame 200, extent 144)",
        first_off + 28.0
    );
    assert!(
        (second_column - (first_off + 76.0)).abs() < 0.02,
        "column 1 along start {second_column}, want {} (frame 200, extent 48)",
        first_off + 76.0
    );
    // The two columns differ by exactly the difference of their centring offsets.
    assert!((second_column - first_column - 48.0).abs() < 0.02);
}

/// Photoshop stops *composing* a boxed vertical line at the first column whose
/// em cell lies entirely left of the frame; it never skips one and keeps going,
/// and point text has no limit at all. Measured from the composed caches with
/// zero residual (`out/verify/text-baseline-offset.md` §15.9): the u12 30 pt
/// layers (frame 135 pt) keep 4 of 6 columns at a 36 pt pitch — the 5th cell
/// `[-34.905, -4.905]` stops the line — and 1 of 3 at a 150 pt pitch, while
/// their 144 pt frame / pitch-3 sibling keeps all 6 and `box_v` (454 pt frame,
/// 57.6 pt pitch) keeps all 5, including the first cell that overhangs the
/// right edge. The 8-column discriminator carries a 435 pt-wide frame, not the
/// 135 pt box it was authored with (the writer stored the box's absolute
/// right/bottom: `Txt2` frame x 0..435, y 0..379), so all eight of its 36
/// pt-pitch cells intersect that frame and the stop never triggers.
#[test]
fn a_vertical_box_stops_composing_at_the_first_column_outside_the_frame() {
    let case = case("point_styles"); // 48 pt Noto, auto leading 1.2 -> 57.6 pt pitch
    let ed = model(&case);
    let store = store();
    let text = "ABCD\rEFGH\rIJKL"; // three paragraphs, one column each
    let size = case.spec.runs.first().and_then(|r| r.size).unwrap_or(18.0);
    let height = size * 4.0; // four glyphs per column: no soft wrap
    let face = store.face("NotoSans-Regular").expect("checked-in font");
    let b0 = size * layer_render::vertical_b0_ratio(&face); // Noto 48 pt: 0.4029998 * 48
    let first_centre = |width: f32| width - b0;
    // 60 pt frame: centres 40.656 / -16.944 / -74.544 -> the third cell's right
    // edge is -50.5 pt, so the line stops after two columns.
    let narrow = layer_render::layout_vertical_from_engine_data_in_box(
        &store,
        &ed,
        text,
        (0.0, 0.0),
        (60.0, height),
    )
    .expect("vertical box layout");
    assert_eq!(
        narrow.lines.len(),
        2,
        "the third column is outside the frame"
    );
    let centre = |line: &photocraft_text::layer_render::TextLine| (line.x0 + line.x1) * 0.5;
    assert!((centre(&narrow.lines[0]) - first_centre(60.0)).abs() < 0.01);
    assert!((centre(&narrow.lines[1]) - (first_centre(60.0) - 57.6)).abs() < 0.01);
    assert!(
        centre(&narrow.lines[1]) < 0.0,
        "the kept column straddles the edge"
    );
    // The same text in a wide frame keeps all three columns.
    let wide = layer_render::layout_vertical_from_engine_data_in_box(
        &store,
        &ed,
        text,
        (0.0, 0.0),
        (400.0, height),
    )
    .expect("vertical box layout");
    assert_eq!(wide.lines.len(), 3, "a wide frame keeps every column");
    // Point text has no frame and therefore no limit.
    let point = layer_render::layout_vertical_from_engine_data(&store, &ed, text, (0.0, 0.0))
        .expect("vertical point layout");
    assert_eq!(point.lines.len(), 3, "point text keeps every column");
}
