//! Recorded from the pre-refactor implementation; these are compatibility,
//! not Photoshop-correctness, expectations. All fixture files are mandatory.
//! Source baseline: repository commit c900bdd, captured before source changes.
//!
//! **Re-record discipline (mandatory).** Every regeneration must take
//! `cp -a tests/assets /tmp/goldens_pre` first, then `REA_REGEN=1 cargo test
//! --test compatibility`, then `diff -rq /tmp/goldens_pre tests/assets`; report
//! only that diff (and keep the pre-copy path in the report). Without the
//! pre-copy an audit can only compare against the committed tree, which in a
//! shared checkout conflates concurrent work.
//!
//! **Optical feature render re-record (2026-11-14).** `goldens/features_optical*` were
//! re-recorded because the optical kerning model was calibrated to Photoshop
//! (`optical.rs`: `optical_pair_chars` scales the fitted model's raw closure by a measured
//! character class — punctuation 0.81, `F`+round 0.42, `W`/`V`+round 0.78, `A/V/W/X/K` pairs
//! 0.64, `T`/`Y` pairs 0.95 — with `OPTICAL_MAX` 0.19 em; `tests/optical_pairs.rs` pins twelve
//! Photoshop-measured pairs within 1 px at 48 pt). The audited diff against
//! `/tmp/goldens_pre_txt2` is exactly the four optical render files and nothing else:
//!
//! - `features_optical.pixels.bin` (16 B geometry): width 533 -> 517, height 49 and the
//!   offset (4.0, -37.228) unchanged.
//! - `features_optical.rgba.bin`: 104468 -> 101332 B = -784 px = the 16 columns the tighter
//!   kerning removes from the layer canvas.
//! - `features_optical.layout.bin`: same 1050 B; line width 539.596 -> 523.681 and every glyph
//!   `x`/`advance` delta is that pair's optical closure (`Ha` -0.997, `am`/`mb` -1.496,
//!   `AV` -1.799, `y.` -2.459, `st`/`iv`/`v `/` A`/`Ry`/`. ` 0), the sum matching the line width
//!   delta to 0.0001 px; gids, chars, colours, scales, faux, leading, baseline and run indices
//!   are identical.
//! - `features_optical_fixed_rect.rgba.bin`: same 64000 B; ink bbox (18, 9, 159, 46) and top
//!   row unchanged, 1567 changed pixels, every one of them a glyph-ink pixel (0 changed outside
//!   ink).
//!
//! No authored `.psd.bin`, no `models/*.engine.bin`, no input, and no other case's pixels/rgba
//! changed (83 files compared, 4 differ). The authored files still open silently in Photoshop
//! (152/210/249 ms for `point_plain`/`area_paragraphs`/`point_styles`) and so does the UI-saved
//! `optical_on` file (179 ms).
//!
//! **Vertical family re-record (2026-11-14).** `goldens/vertical.*` and
//! `point_styles.*` were re-recorded because the vertical flow changed
//! (`layer_render/vertical.rs`): the column step is the paragraph leading
//! (`size * auto_leading`, or an explicit `Leading`) instead of a flat em, the
//! first column's one-em box is anchored flush with the box's right edge, and
//! columns whose box leaves the frame are dropped. `vertical.rgba.bin`
//! 133304 -> 161704 B is expected under a layout change (the block spans more
//! differently placed columns) and the small `*.engine.bin` shrink (e.g.
//! 8963 -> 8911 B) is the neutral `Leading 0` form that auto leading now
//! writes. Pixels, models and rgba for *horizontal* cases are not part of this
//! change (their re-record came from the first-baseline cap rule and the
//! auto-leading form, both documented in out/verify/text-baseline-offset.md).
//!
//! **Vertical anchor and across clip (2026-11-14, §14 of the same report).**
//! `layer_render/vertical.rs` anchors a boxed vertical column's em cell
//! `size * vertical_b0_ratio(face)` inside the frame's right edge instead of
//! half an em (measured from Photoshop's own composed cache: 0.3729935 em
//! Myriad, 0.3634949 em Times, 0.4029998 em Noto Sans; see
//! `out/verify/text-baseline-offset.md` §14.1-14.4), and the across-axis
//! visibility rule is one-sided: a column is hidden only when its whole em cell
//! is left of the box, so the first cell's deliberate overhang past the right
//! edge stays. No golden moved — the recorded vertical case is point text
//! (`box_size = None`), whose first column stays on the `TySh` origin.
//!
//! The five authored-PSD baselines (`goldens/{point_plain,point_styles,
//! point_center,area_paragraphs,area_composer}.psd.bin`) were re-recorded when
//! `psd::author` started writing the layer text into the `TySh` `Txt `
//! descriptor (the pre-refactor writer left the template's stale text there, so
//! the baseline encoded an internally inconsistent `TySh`: new `Txt2`/
//! `EngineData`, old descriptor). The re-recorded diff is that text change plus
//! the character/paragraph properties the writer newly models; it is not a
//! render change:
//!
//! - `TySh` text shrinks by the character deltas below, `EngineData` is
//!   byte-identical, the layer channel bytes are identical, and no layer rect
//!   moves.
//! - `Txt2` is byte-identical for `point_plain`/`area_paragraphs`/
//!   `area_composer`; `point_styles` gains `/6/2,3,12,15,16` (bold, italic,
//!   all caps, strike, underline: −2 data bytes, absorbed by the 4-byte block
//!   padding) and `point_center` gains `/6/10` + `/6/11 0`: manual kerning in
//!   Photoshop's 16.16 fixed-point form (kern 80 → `0.00122`, +12 data bytes);
//!   the older `+7` note was an integer `80` that Photoshop would have read as
//!   5 242 880 milliem.)
//! - File deltas: `point_plain` −14 B = 7 UTF-16 units (`Txt2` unchanged);
//!   `area_paragraphs` −44 B = 22 units; `area_composer` −60 B = 30 units;
//!   `point_styles` −20 B = 10 units and `point_center` −28 B = 20 units plus
//!   the `Txt2` deltas above (TySh −40, Txt2 block +12).
//!
//! **EngineData parity re-record (2026-11-14, `txt2-writer`).** The five
//! `models/*.engine.bin` writer outputs moved by exactly the keys the parity
//! work changed, nothing else (audited with `examples/ed_file_diff.rs` against
//! `git show HEAD:./tests/assets/models/<name>.engine.bin`):
//!
//! | model | delta | leaves |
//! |---|---|---|
//! | `area_composer` | +1 B | 1 paragraph run: `HyphenatedWordSize 0→6`, `PreHyphen 0→2`, `PostHyphen 0→2`, `Zone 0→36` (+1 B for `36`); `FontSet[3].FontType 0→1` |
//! | `area_paragraphs` | +2 B | 2 paragraph runs of the same four hyphenation slots (+1 B each for `Zone 36`) |
//! | `point_center` | +2 B | 2 paragraph runs, same four slots (two `Zone` widenings) |
//! | `point_plain` | +1 B | 1 paragraph run, same four slots plus `ConsecutiveHyphens 0→8`, one `Zone` widening |
//! | `point_styles` | +2 B | 2 paragraph runs of the same five slots, two `Zone` widenings, `FontSet[3].FontType 0→1` |
//!
//! The causes are the measured Photoshop forms: (a) hyphenation detail slots are
//! written even when hyphenation is off (`6/2/2/8/36`, from PS's own
//! `10_all_features.psd`), (b) real faces are `FontType 1`, and (c) numeric
//! values use PS's shortest form (`36`, not `36.0`), which is where the +1/+2
//! bytes come from. `EngineData` structural equality is otherwise unchanged; no
//! render or channel bytes moved. See `out/verify/sync/enginedata-parity.md`.
//!
//! A later behaviour fix re-recorded the two area cases (and their
//! `tests/assets/models/*.engine.bin`): a frame change now also patches the
//! `TySh` `EngineData` `/Rendered/Shapes/Children[0]/Cookie/Photoshop/BoxBounds`
//! array, because Photoshop composes a box from that cookie and silently keeps
//! the old box otherwise (measured by the PS fit experiment: authored frame
//! widths 479/480/481 all re-composed as the template's 480). Only that one
//! array changes — `area_paragraphs` `[0.0 0.0 430.0 200.0]` →
//! `[20.0 30.0 270.0 230.0]`, `area_composer` `[0.0 0.0 430.0 200.0]` →
//! `[15.0 25.0 265.0 225.0]`, the spec frames — +2 bytes each: the `TySh`
//! block grows 35950 → 35952 (resp. 19274 → 19276), the layer pixels, channels,
//! `Txt2` and every other block stay byte-identical (the `.rgba.bin`/
//! `.pixels.bin` goldens still match). The point cases are untouched (they
//! write `PointBase`, which did not change).
//!
//! A composer fix then re-recorded the `area_composer` *render* goldens
//! (`area_composer.{layout,pixels,rgba}.bin`) and their `_spec` pair
//! (`area_composer_spec.{pixels,rgba}.bin`, moved identically): the render
//! geometry goes `250x73` (3 lines) -> `252x46` (2 lines). The case's paragraph
//! carries `GlyphSpacing [0.9, 1.0, 1.1]`; the paragraph-run resolver used to
//! look up a legacy `GlyphScaling` spelling, missed it and composed with
//! `[1, 1, 1]`, so the justified line had no glyph-compression room and wrapped
//! one line early. With the real key honoured it fits 2 lines. That is
//! Photoshop-correct, not a regression: authoring the case and letting Photoshop
//! recompose it gives 2 lines too (its cache holds 40 + 11 glyphs), and forcing
//! the three spacing keys back to `[1, 1, 1]` reproduces the old `250x73`
//! baseline exactly. `area_paragraphs` is untouched (its render goldens still
//! match). Full chain and the ruled-out causes: `. /out/verify/composition-grid.md`
//! section 16 (`photocraft` checkout root, `out/verify`).

//! `photocraft_psd` pads layer tagged blocks to an even length and global tag
//! blocks to 4 bytes, which is where the ±1/±2-byte block rounding comes from.

//! The four vertical baselines (`vertical.pixels.bin`, `vertical.rgba.bin`,
//! `vertical_fixed_rect.pixels.bin`, `vertical_fixed_rect.rgba.bin`) were
//! re-recorded when vertical hard breaks started a new column: the
//! `point_styles` text `"Red Bold\rSmall caps"` used to compose as one
//! 18-glyph column (38x877 at offset (-23,-19)) and now gives two columns,
//! the second one a column to the left (72x493 at offset (-57,-19)). The
//! fixed-rectangle render keeps its 160x100 crop (its `.pixels.bin` bytes are
//! unchanged) and only its pixel bytes moved. No other baseline moved for
//! that fix; the two area-PSD re-records above are the `BoxBounds` change.

//! `warp_arch.rgba.bin` was re-recorded when `warp::map_for` replaced the
//! 50 %-fitted arch constant with Photoshop's computed arch style math (ported
//! from the fork's `geom::warp::preset_grid`); only the pixel values inside the
//! same 120x60 rectangle changed (`warp_arch.pixels.bin` and `.rect.bin` are
//! byte-identical, so no geometry moved). `warp_arc.rgba.bin` is **unchanged**:
//! the arc style keeps its per-layer fitted grid, because the generic style math
//! measured `.8981` IoU on layer 12 against the fitted grid's `.9323`.

//! Five authored-PSD baselines were re-recorded when `EngineData::write` /
//! `fmt_number` (`src/engine_data.rs:313`) started printing the shortest `f32`
//! form: the writer used to widen `f32` style values to `f64` and print the full
//! binary expansion (`.800000011920929`, `1.2000000476837158`), and Photoshop's
//! CoolType parser aborts a document on those 17-18 character number tokens
//! ("program error"). The five old baselines were the bytes Photoshop rejected:
//!
//! | golden | before | after |
//! |---|---|---|
//! | `point_plain.psd.bin` | 128 228 | 128 152 |
//! | `point_styles.psd.bin` | 122 874 | 122 722 |
//! | `point_center.psd.bin` | 110 000 | 109 914 |
//! | `area_composer.psd.bin` | 144 906 | 144 830 |
//! | `area_paragraphs.psd.bin` | 162 856 | 162 740 |
//!
//! Every delta is the removed mantissa digits; no layout, pixels, rgba or model
//! baseline moved (verified file by file against a pre-regen copy). Acceptance
//! evidence, including the four export shapes opening in Photoshop 20.0.2, is in
//! `out/verify/psd-photoshop-open.md`.

use std::path::{Path, PathBuf};

use photocraft_psd::PsdFile;
use photocraft_text::area::{self, AreaLayout};
use photocraft_text::atlas::Atlas;
use photocraft_text::engine_data::{self, Value};
use photocraft_text::layer_render::{
    self, FaceStore, LayerPixels, LayerTextStyle, RectTarget, RenderOptions, RunFeatures,
};
use photocraft_text::layout::{Kerning, TextStyle};
use photocraft_text::psd::{LayerSpec, author};
use photocraft_text::runs::{self, FontSet, RunStyle};
use photocraft_text::warp::{FlatGeometry, Style, Warp};

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    template: String,
    spec: LayerSpec,
}

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

fn check_environment() {
    for (key, _) in std::env::vars_os() {
        assert!(
            !key.to_string_lossy().starts_with("PHOTOCRAFT_"),
            "baseline requires no PHOTOCRAFT_* overrides: {key:?}"
        );
    }
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
    let raw = std::fs::read(
        assets()
            .join("models")
            .join(format!("{}.engine.bin", case.name)),
    )
    .expect("checked-in EngineData input");
    engine_data::parse(&raw).expect("EngineData input")
}

fn golden(path: PathBuf, actual: &[u8]) {
    // `REA_REGEN=1` re-records any golden this suite compares, so a writer fix
    // that legitimately moves pixels has one uniform regen path (the two
    // `regen_*` tests cover the families that need a different construction).
    if std::env::var("REA_REGEN").as_deref() == Ok("1") {
        println!("re-recorded {} ({} bytes)", path.display(), actual.len());
        std::fs::write(&path, actual).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        return;
    }
    let expected = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("required baseline {}: {e}", path.display()));
    let difference = expected.iter().zip(actual).position(|(a, b)| a != b);
    assert!(
        difference.is_none() && expected.len() == actual.len(),
        "{}: first differing byte {:?}, expected {} bytes, got {}",
        path.display(),
        difference.or_else(
            || (expected.len() != actual.len()).then_some(expected.len().min(actual.len()))
        ),
        expected.len(),
        actual.len()
    );
}

fn check_bytes(name: &str, actual: &[u8]) {
    golden(assets().join("goldens").join(name), actual);
}

/// The 16-byte geometry a pixel golden stores: width, height and the offset as
/// little-endian u32 words.
fn pixel_geometry(pixels: &LayerPixels) -> Vec<u8> {
    let mut geometry = Vec::new();
    for word in [
        pixels.width,
        pixels.height,
        pixels.offset.0.to_bits(),
        pixels.offset.1.to_bits(),
    ] {
        geometry.extend_from_slice(&word.to_le_bytes());
    }
    geometry
}

fn check_pixels(name: &str, pixels: &LayerPixels) {
    assert_eq!(
        pixels.rgba.len(),
        pixels.width as usize * pixels.height as usize * 4
    );
    assert!(
        pixels.rgba.chunks_exact(4).any(|p| p[3] != 0),
        "{name}: no ink"
    );
    check_bytes(&format!("{name}.pixels.bin"), &pixel_geometry(pixels));
    check_bytes(&format!("{name}.rgba.bin"), &pixels.rgba);
}

fn engine_data_of(file: &PsdFile) -> Value {
    let tysh = file
        .layers()
        .iter()
        .find_map(|l| l.block(b"TySh"))
        .expect("TySh");
    let (desc, _) = photocraft_psd::descriptor::VersionedDescriptor::parse_prefix(&tysh.data[52..])
        .expect("TySh descriptor");
    let Some(photocraft_psd::descriptor::Value::RawData(raw)) = desc.descriptor.get("EngineData")
    else {
        panic!("TySh EngineData missing")
    };
    engine_data::parse(raw).expect("authored EngineData")
}

fn check_author(case: &Case) {
    let store = store();
    let bytes = std::fs::read(assets().join("inputs").join(&case.template)).expect("PSD input");
    let mut file = PsdFile::from_bytes(&bytes).expect("PSD input parse");
    author(&mut file, &case.spec, &store).expect("author original PSD");
    check_bytes(
        &format!("{}.psd.bin", case.name),
        &file.to_bytes().expect("serialize entire PSD"),
    );
    golden(
        assets()
            .join("models")
            .join(format!("{}.engine.bin", case.name)),
        &engine_data::write(&engine_data_of(&file)),
    );
}

fn style_of(runs: &[RunStyle]) -> TextStyle {
    let run = &runs[0];
    TextStyle {
        size: run.size.unwrap_or(12.0),
        tracking: run.tracking.unwrap_or(0),
        horizontal_scale: run.horizontal_scale.unwrap_or(1.0),
        baseline_shift: run.baseline_shift.unwrap_or(0.0),
        leading: if run.auto_leading == Some(true) {
            None
        } else {
            run.leading
        },
        ..Default::default()
    }
}

fn layout(case: &Case, features: &[RunFeatures]) -> AreaLayout {
    let store = store();
    let mut faces = FontSet::new(
        "NotoSans-Regular",
        store.face("NotoSans-Regular").expect("regular face"),
    );
    faces.insert(
        "NotoSans-Bold",
        store.face("NotoSans-Bold").expect("bold face"),
    );
    let ed = model(case);
    let mut runs = runs::from_engine_data(&ed);
    for (run, feature) in runs.iter_mut().zip(features) {
        run.old_style |= feature.old_style;
    }
    let mut style = style_of(&runs);
    if features.iter().any(|f| f.optical_kerning) {
        style.kerning = Kerning::Optical;
        for run in &mut runs {
            run.auto_kerning = None;
            run.kerning = None;
        }
    }
    let paras = layer_render::paragraph_styles_of(&runs::paragraphs_from_engine_data(&ed));
    let ascender = area::first_baseline_ratio(
        faces
            .get(runs[0].font.as_deref().expect("explicit run font"))
            .expect("primary face"),
    );
    let advances: Vec<f32> = features
        .iter()
        .flat_map(|f| f.advance_deltas.iter().copied())
        .collect();
    let size = box_size(case);
    if size.0 > 0.0 && size.1 > 0.0 {
        area::layout_area_runs_paragraphs_with_advances(
            &faces,
            &text(case),
            &runs,
            &style,
            &paras,
            size.0,
            size.1,
            (0.25, 1.5),
            ascender,
            &advances,
        )
    } else {
        area::layout_area_runs_paragraphs_point(
            &faces,
            &text(case),
            &runs,
            &style,
            &paras,
            1.0e7,
            1.0e7,
            (0.25, 1.5),
            ascender,
            &advances,
        )
    }
}

fn layout_bytes(layout: &AreaLayout) -> Vec<u8> {
    let mut bytes = Vec::new();
    let word = |bytes: &mut Vec<u8>, value: u32| bytes.extend_from_slice(&value.to_le_bytes());
    word(
        &mut bytes,
        layout.lines.len().try_into().expect("line count"),
    );
    for line in &layout.lines {
        word(&mut bytes, line.baseline.to_bits());
        word(&mut bytes, line.width.to_bits());
        word(
            &mut bytes,
            line.glyphs.len().try_into().expect("glyph count"),
        );
        assert_eq!(
            line.glyphs.len(),
            line.attrs.len(),
            "parallel glyph attributes"
        );
        for (glyph, attrs) in line.glyphs.iter().zip(&line.attrs) {
            for value in [
                u32::from(glyph.gid),
                glyph.ch as u32,
                glyph.x.to_bits(),
                glyph.y.to_bits(),
                glyph.advance.to_bits(),
                glyph.nominal_advance.to_bits(),
                attrs.run.try_into().expect("run index"),
            ] {
                word(&mut bytes, value);
            }
            bytes.extend_from_slice(&attrs.color);
            for value in [
                attrs.vertical_scale.to_bits(),
                attrs.x_scale.to_bits(),
                u32::from(attrs.faux_italic),
                attrs.leading.to_bits(),
            ] {
                word(&mut bytes, value);
            }
        }
    }
    bytes
}

/// Every pixel/geometry golden one authored case records, in check order.
fn case_render_goldens(case: &Case) -> Vec<(String, Vec<u8>)> {
    let store = store();
    let ed = model(case);
    let pixels = layer_render::render_from_engine_data(
        &store,
        &ed,
        &text(case),
        box_size(case),
        (0.25, 1.5),
    )
    .expect("EngineData render");
    assert!(
        pixels.rgba.chunks_exact(4).any(|p| p[3] != 0),
        "{}: no ink",
        case.name
    );
    let mut goldens = vec![
        (format!("{}.pixels.bin", case.name), pixel_geometry(&pixels)),
        (format!("{}.rgba.bin", case.name), pixels.rgba.clone()),
        (
            format!("{}.layout.bin", case.name),
            layout_bytes(&layout(case, &[])),
        ),
    ];
    let runs = runs::from_engine_data(&ed);
    let paragraphs = layer_render::paragraph_styles_of(&runs::paragraphs_from_engine_data(&ed));
    let spec = LayerTextStyle {
        style: style_of(&runs),
        paragraph: paragraphs[0],
        box_size: box_size(case),
        origin: (0.25, 1.5),
        color: [17, 33, 65],
    };
    let pixels = store
        .render_spec_with(&text(case), &runs, &spec, RenderOptions::default())
        .expect("direct specification render");
    assert!(
        pixels.rgba.chunks_exact(4).any(|p| p[3] != 0),
        "{}_spec: no ink",
        case.name
    );
    goldens.push((
        format!("{}_spec.pixels.bin", case.name),
        pixel_geometry(&pixels),
    ));
    goldens.push((format!("{}_spec.rgba.bin", case.name), pixels.rgba.clone()));
    goldens
}

fn check_case_render(case: &Case) {
    for (name, bytes) in case_render_goldens(case) {
        check_bytes(&name, &bytes);
    }
}

/// Re-records the authored PSD and `EngineData` model goldens (run explicitly,
/// see the header note): `REA_REGEN=1 cargo test -p photocraft-text --test
/// compatibility regen_author_goldens -- --ignored`.
#[test]
#[ignore = "writes the checked-in goldens; run explicitly when a writer fix changes them"]
fn regen_author_goldens() {
    assert_eq!(
        std::env::var("REA_REGEN").as_deref(),
        Ok("1"),
        "set REA_REGEN=1 to confirm re-recording the checked-in goldens"
    );
    for case in cases() {
        let store = store();
        let bytes = std::fs::read(assets().join("inputs").join(&case.template));
        let Ok(bytes) = bytes else { continue };
        let mut file = PsdFile::from_bytes(&bytes).expect("PSD input parse");
        author(&mut file, &case.spec, &store).expect("author original PSD");
        let psd = file.to_bytes().expect("serialize entire PSD");
        std::fs::write(
            assets()
                .join("goldens")
                .join(format!("{}.psd.bin", case.name)),
            &psd,
        )
        .expect("write psd golden");
        let model = engine_data::write(&engine_data_of(&file));
        std::fs::write(
            assets()
                .join("models")
                .join(format!("{}.engine.bin", case.name)),
            &model,
        )
        .expect("write model golden");
        println!(
            "re-recorded {} ({} B psd, {} B model)",
            case.name,
            psd.len(),
            model.len()
        );
    }
}

/// Re-records the render goldens of the named cases (run explicitly, see the
/// header note): `REA_REGEN=1 cargo test -p photocraft-text --test compatibility
/// regen_render_goldens -- --ignored`.
#[test]
#[ignore = "writes the checked-in goldens; run explicitly when a writer fix changes them"]
fn regen_render_goldens() {
    assert_eq!(
        std::env::var("REA_REGEN").as_deref(),
        Ok("1"),
        "set REA_REGEN=1 to confirm re-recording the checked-in goldens"
    );
    let cases: Vec<String> = std::env::var("REA_REGEN_CASES")
        .map(|list| list.split(',').map(str::to_owned).collect())
        .unwrap_or_else(|_| vec!["area_paragraphs".into(), "area_composer".into()]);
    for name in cases {
        for (golden, bytes) in case_render_goldens(&case(&name)) {
            std::fs::write(assets().join("goldens").join(&golden), &bytes)
                .unwrap_or_else(|e| panic!("write {golden}: {e}"));
            println!("re-recorded {golden} ({} bytes)", bytes.len());
        }
    }
}

fn target() -> RectTarget {
    RectTarget {
        origin: (-3, -5),
        size: (160, 100),
        anchor: (10.25, 40.5),
    }
}

fn check_fixed_and_vertical() {
    let case = case("point_styles");
    let ed = model(&case);
    let store = store();
    let pixels = layer_render::render_from_engine_data_in_rect(
        &store,
        &ed,
        &text(&case),
        &[],
        (0.0, 0.0),
        (0.25, 1.5),
        target(),
    )
    .expect("fixed rectangle render");
    check_pixels("fixed_rect", &pixels);
    let pixels =
        layer_render::render_vertical_from_engine_data(&store, &ed, &text(&case), (0.25, 1.5))
            .expect("vertical render");
    check_pixels("vertical", &pixels);
    let pixels = layer_render::render_vertical_from_engine_data_in_rect(
        &store,
        &ed,
        &text(&case),
        (0.25, 1.5),
        None,
        target(),
    )
    .expect("vertical fixed rectangle render");
    check_pixels("vertical_fixed_rect", &pixels);
}

fn check_features() {
    let case = case("point_plain");
    let ed = model(&case);
    let store = store();
    for (name, features) in [
        (
            "features_optical",
            vec![RunFeatures {
                optical_kerning: true,
                old_style: true,
                ..Default::default()
            }],
        ),
        (
            "features_advances",
            vec![RunFeatures {
                advance_deltas: vec![0.25, -0.125, 0.5, -0.75, 0.0625, 0.0, 0.375],
                ..Default::default()
            }],
        ),
    ] {
        let pixels = layer_render::render_from_engine_data_with_paragraphs_and_features(
            &store,
            &ed,
            &text(&case),
            &[],
            &features,
            (0.0, 0.0),
            (0.25, 1.5),
        )
        .expect("character features render");
        check_pixels(name, &pixels);
        check_bytes(
            &format!("{name}.layout.bin"),
            &layout_bytes(&layout(&case, &features)),
        );
        let pixels = layer_render::render_from_engine_data_in_rect_with_features(
            &store,
            &ed,
            &text(&case),
            &[],
            &features,
            (0.0, 0.0),
            (0.25, 1.5),
            target(),
        )
        .expect("fixed feature render");
        check_pixels(&format!("{name}_fixed_rect"), &pixels);
    }
}

fn check_warps() {
    let case = case("point_plain");
    let mut ed = model(&case);
    let mut run = runs::from_engine_data(&ed).remove(0);
    run.len = 4;
    run.size = Some(24.0);
    assert!(runs::apply_style_runs(&mut ed, &[run]));
    let store = store();
    for (name, style) in [("warp_arc", Style::Arc), ("warp_arch", Style::Arch)] {
        let out = layer_render::render_from_engine_data_warped(
            &store,
            &ed,
            "Warp",
            &[],
            &[],
            (0.0, 0.0),
            &Warp {
                style,
                value: 50.0,
                ..Default::default()
            },
            FlatGeometry {
                bounds: (10.0, 20.0, 110.0, 44.0),
                baseline: 40.0,
            },
            RectTarget {
                origin: (0, 0),
                size: (120, 60),
                anchor: (10.0, 40.0),
            },
        )
        .expect("warped render");
        check_pixels(name, &out.pixels);
        let mut rect = Vec::new();
        for value in [out.rect.0 as u32, out.rect.1 as u32, out.rect.2, out.rect.3] {
            rect.extend_from_slice(&value.to_le_bytes());
        }
        check_bytes(&format!("{name}.rect.bin"), &rect);
    }
}

fn atlas(phase: f32, varying: bool) -> Atlas {
    let mut atlas = Atlas::load(&assets().join(if varying { "atlas/vary" } else { "atlas" }))
        .expect("checked-in atlas");
    atlas.phase = phase;
    atlas
}

fn atlas_spec() -> (Vec<RunStyle>, LayerTextStyle) {
    let runs = vec![RunStyle {
        len: 22,
        font: Some("NotoSans-Regular".into()),
        size: Some(48.0),
        tracking: Some(23),
        color: Some([35, 90, 180]),
        ..Default::default()
    }];
    let spec = LayerTextStyle {
        style: TextStyle {
            size: 48.0,
            ..Default::default()
        },
        origin: (0.25, 0.5),
        ..Default::default()
    };
    (runs, spec)
}

fn check_atlases() {
    let (runs, spec) = atlas_spec();
    for (name, atlases) in [
        ("atlas_single", vec![atlas(0.0, false)]),
        (
            "atlas_invariant",
            vec![
                atlas(0.5, false),
                atlas(0.0, false),
                atlas(0.25, false),
                atlas(0.75, false),
            ],
        ),
        (
            "atlas_invariant_no_zero",
            vec![atlas(0.25, false), atlas(0.75, false)],
        ),
        (
            "atlas_phases",
            vec![
                atlas(0.5, false),
                atlas(0.25, true),
                atlas(0.0, false),
                atlas(0.75, true),
            ],
        ),
        (
            "atlas_missing_phase",
            vec![atlas(0.0, false), atlas(0.5, true)],
        ),
        (
            "atlas_duplicate_phase",
            vec![atlas(0.25, true), atlas(0.25, false), atlas(0.0, false)],
        ),
    ] {
        let mut store = store();
        store.set_atlases(atlases);
        let pixels = store
            .render_spec_with(
                "Hamburgefonstiv AVTRy.",
                &runs,
                &spec,
                RenderOptions {
                    use_atlas: true,
                    ..Default::default()
                },
            )
            .expect("atlas selection render");
        check_pixels(name, &pixels);
    }
}

#[test]
fn authored_psd_bytes() {
    check_environment();
    for case in &cases() {
        check_author(case);
    }
}

#[test]
fn point_render_and_geometry() {
    check_environment();
    for name in ["point_plain", "point_styles", "point_center"] {
        check_case_render(&case(name));
    }
}

#[test]
fn area_render_and_geometry() {
    check_environment();
    for name in ["area_paragraphs", "area_composer"] {
        check_case_render(&case(name));
    }
}

#[test]
fn fixed_and_vertical_bytes() {
    check_environment();
    check_fixed_and_vertical();
}

#[test]
fn character_features_bytes() {
    check_environment();
    check_features();
}

#[test]
fn warped_pixels_bytes() {
    check_environment();
    check_warps();
}

#[test]
fn atlas_selection_bytes() {
    check_environment();
    check_atlases();
}
