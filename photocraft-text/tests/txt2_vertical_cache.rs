//! Reads a **vertical** composed cache the way Photoshop wrote it.
//!
//! Fixture: `tests/assets/inputs/box_v_txt2.bin` is the global `Txt2` block of
//! `recomp_box_v_0.psd` — a 454×642 pt area box, MyriadPro-Regular 48 pt,
//! vertical (`Ornt = Vrtc`), five columns of `фывфыв` / `фывф`, recomposed by
//! Photoshop CC 2019 itself (open, nudge tracking by ±0.001 to force a
//! recompose, save). Extracted with `tools/txt2_map.py read_txt2`; the block
//! starts at the last `8BIMTxt2` resource of the PSD and is 13596 bytes.
//!
//! The values pinned here are Photoshop's own composition, not our layout's:
//! the first column's baseline is 17.90369 pt inside the frame's right edge and
//! the columns step by the paragraph line height (57.6 pt = 48 pt × 1.2).

use std::path::PathBuf;

use photocraft_text::txt2::{composed_layout, composed_layout_vertical, Orientation};

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

fn block() -> Vec<u8> {
    let path = assets().join("inputs/box_v_txt2.bin");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn vertical_lines_report_the_line_origin_and_transposed_runs() {
    let block = block();
    let layout = composed_layout_vertical(&block, 0).expect("vertical cache");
    assert_eq!(layout.orientation, Orientation::Vertical);
    // Five columns of "фывфыв" / "фывфывфыв" / "фывфывфыв" / "фыв" /
    // "фывфыв": six, eight, eight, three and six glyphs plus the paragraph
    // separator glyph 1 that Photoshop keeps at the end of every column.
    assert_eq!(layout.glyph_count(), 36);
    assert_eq!(layout.lines.len(), 5);
    let columns: Vec<Vec<u16>> = layout.lines.iter().map(|l| l.gids().collect()).collect();
    assert_eq!(
        columns.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![7, 9, 9, 4, 7]
    );
    assert!(columns.iter().all(|g| g.iter().all(|gid| *gid != 0)));

    let first = &layout.lines[0];
    // `/0` = [0.0, 17.90369]: along start 0, across inset 17.90369.
    assert_eq!(first.origin, (0.0, 17.90369));
    assert!((first.baseline - 17.90369).abs() < 1e-4);
    // Vertical `/15` / `/14` are the column's half width, not font metrics.
    assert!(
        (first.ascent - 24.0).abs() < 1e-5,
        "ascent {}",
        first.ascent
    );
    assert!(
        (first.descent - 24.0).abs() < 1e-5,
        "descent {}",
        first.descent
    );
    assert_eq!(first.runs.len(), 1);
    let run = &first.runs[0];
    // `/8` = [412.09631, 0.0, 460.09631, 288.0] is x = across, y = along; the
    // reader transposes it to [along, across] like every other field.
    assert_eq!(run.origin, (0.0, 412.09631));
    // Relative frame again: the run is six ems along the column, one em across.
    assert_eq!(run.bounds, [0.0, -24.0, 288.0, 24.0]);
    assert_eq!(run.gids, vec![682, 689, 664, 682, 689, 664, 1]);
    // The first column's em cell spans 412.09631 .. 460.09631 across, i.e. it
    // is flush with the 454 pt frame's right edge plus Photoshop's overhang:
    // 454 - 17.90369 = 436.09631 is its centre.
    assert!((run.origin.1 + 24.0 - 436.09631).abs() < 1e-4);

    // Columns step by the paragraph line height (48 pt * 1.2), not by the em.
    let baselines: Vec<f32> = layout.lines.iter().map(|l| l.baseline).collect();
    assert_eq!(
        baselines,
        vec![17.90369, 75.50369, 133.1037, 190.7037, 248.30371]
    );
    for pair in baselines.windows(2) {
        assert!(
            (pair[1] - pair[0] - 57.6).abs() < 1e-3,
            "column pitch {}",
            pair[1] - pair[0]
        );
    }
}

#[test]
fn the_horizontal_default_still_reads_the_absolute_box() {
    // `composed_layout` keeps its old, horizontal field mapping — the mixed
    // pair a vertical cache cannot express: the run's absolute `/8` x (across,
    // 412.09631) lands in `origin.x` next to the `/10` baseline (also across,
    // 17.90369) in `origin.y`. Byte-identical for horizontal callers, and the
    // reason a vertical caller must ask for `composed_layout_vertical`.
    let block = block();
    let horizontal = composed_layout(&block, 0).expect("cache");
    assert_eq!(horizontal.orientation, Orientation::Horizontal);
    assert_eq!(horizontal.lines[0].origin, (412.09631, 17.90369));
    assert_eq!(horizontal.lines[0].runs[0].origin, (412.09631, 0.0));
    // Everything orientation-independent is the same in both readings.
    let vertical = composed_layout_vertical(&block, 0).expect("cache");
    assert_eq!(horizontal.glyph_count(), vertical.glyph_count());
    assert_eq!(horizontal.deltas(), vertical.deltas());
    assert_eq!(
        horizontal
            .lines
            .iter()
            .map(|l| l.baseline)
            .collect::<Vec<_>>(),
        vertical
            .lines
            .iter()
            .map(|l| l.baseline)
            .collect::<Vec<_>>()
    );
}
