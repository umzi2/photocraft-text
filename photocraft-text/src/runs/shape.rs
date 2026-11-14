use crate::engine_data::Value;
use crate::font::gsub::{self, Gsub, Span};
use crate::layout::{GlyphPos, Kerning, TextStyle};
use crate::typography::faux_bold_extra_advance;

use super::model::{FontSet, GlyphAttrs, RunFeature, RunStyle, SMALL_CAPS_SCALE, ShapedGlyph};

/// Pen positions for a multi-run string. Same semantics as
/// [`crate::layout::shape`] (advance = pen delta to the next glyph), across
/// runs. Kerning does not cross run boundaries.
///
/// Pipeline: caps mapping per character, then `liga` (and `dlig` when enabled)
/// over adjacent glyphs sharing font, feature flags and small-caps state. A
/// ligature may span runs with identical font and features; it takes the first
/// component's run and attributes. Then advances.
///
/// Panics if the run lengths do not cover the text in UTF-16 units, or a run
/// names a font missing from `faces`: both are caller errors.
pub fn shape_runs_attrs(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
) -> Vec<(GlyphPos, GlyphAttrs)> {
    let text_units = text.encode_utf16().count();
    let run_units: usize = runs.iter().map(|r| r.len).sum();
    assert_eq!(
        run_units, text_units,
        "run lengths cover {run_units} UTF-16 units, text has {text_units}"
    );

    let items = collect_items(faces, text, runs);
    let spans = substitute(faces, &items);
    layout_glyphs(faces, runs, style, &items, &spans)
}

pub fn shape_runs(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
) -> Vec<GlyphPos> {
    shape_runs_attrs(faces, text, runs, style)
        .into_iter()
        .map(|(g, _)| g)
        .collect()
}

/// Like [`shape_runs`], but keeps the run index, colour and vertical scale.
pub fn shape_runs_detailed(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
) -> Vec<ShapedGlyph> {
    shape_runs_attrs(faces, text, runs, style)
        .into_iter()
        .map(|(glyph, a)| ShapedGlyph {
            glyph,
            run: a.run,
            vertical_scale: a.vertical_scale,
            color: runs[a.run].color,
        })
        .collect()
}

/// One glyph before substitution.
struct Item<'a> {
    gid: u16,
    /// Character after caps mapping.
    ch: char,
    run: usize,
    small: bool,
    /// Font name key into the `FontSet`.
    face: &'a str,
    /// Byte range of the source character in the layer text.
    source: (usize, usize),
    ligatures: bool,
    dligatures: bool,
    /// OpenType features this character's run requests (tag + amount).
    features: &'a [RunFeature],
}

impl Item<'_> {
    fn shares_shaping(&self, other: &Item<'_>) -> bool {
        self.face == other.face
            && self.ligatures == other.ligatures
            && self.dligatures == other.dligatures
            && self.features == other.features
            && self.small == other.small
    }
}

fn collect_items<'a>(faces: &'a FontSet<'_>, text: &str, runs: &'a [RunStyle]) -> Vec<Item<'a>> {
    let mut items = Vec::with_capacity(text.len());
    for (run_index, byte, ch) in char_runs(text, runs) {
        if ch == '\r' || ch == '\n' {
            continue;
        }
        let source = (byte, byte + ch.len_utf8());
        let run = &runs[run_index];
        let name = face_name(faces, run);
        let face = faces.get(name).expect("face_name validated the font");
        let (ligatures, dligatures) = ligature_flags(run);
        let mapped: Vec<(char, bool)> = if run.all_caps {
            ch.to_uppercase().map(|u| (u, false)).collect()
        } else {
            vec![(ch, false)]
        };
        for (c, all_cap) in mapped {
            let Some(mut gid) = face.glyph_index(c) else {
                continue;
            };
            let mut small = false;
            if run.small_caps && !all_cap {
                // Photoshop lays small caps out with the face's real `smcp`
                // design, not a scaled capital: the advances measured from a
                // 26 pt capture sheet's repeated-glyph lines are the `*.sc`
                // advances (`m.sc` 736/1000 em -> 19.14 pt vs 19.10 measured,
                // `a.sc` 528 -> 13.73 vs 13.75, ...), while `capital * 0.8`
                // drifts by up to 0.55 pt per glyph. The *bitmaps* also come
                // from the face's small caps design; a `small_caps` atlas
                // capture (see `Atlas::small_caps`) supplies Photoshop's own
                // raster for them. Faces without `smcp` fall back to the
                // synthetic scaled capital.
                if let Some(sub) = substitute_feature(face, c, *b"smcp", gid) {
                    gid = sub;
                } else if c.is_lowercase()
                    && let Some(upper) = c.to_uppercase().next().and_then(|u| face.glyph_index(u))
                {
                    gid = upper;
                    small = true;
                }
            }
            if run.old_style && c.is_numeric() {
                // Photoshop's old-style figures are proportional: `pnum`
                // (`zero` -> `zero.lf`) then `onum` (`zero.lf` -> `zero.osf`).
                // `onum` alone selects the *tabular* `.tosf` set (547/1000 em
                // for every digit), which lays the figures out up to 3.8 pt
                // per digit away from Photoshop (measured: `1` 10.40 pt =
                // `one.osf` 399/1000 em, `0` 15.25 = `zero.osf` 587/1000).
                let step = substitute_feature(face, c, *b"pnum", gid).unwrap_or(gid);
                if let Some(subst) = substitute_feature(face, c, *b"onum", step) {
                    gid = subst;
                } else {
                    gid = step;
                }
            }
            if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "12") {
                eprintln!(
                    "item: ch={:?} gid={:05X} small={} old={}",
                    c, gid, small, run.old_style
                );
            }
            items.push(Item {
                gid,
                ch: c,
                run: run_index,
                small,
                face: name,
                ligatures,
                dligatures,
                features: &run.features,
                source,
            });
        }
    }
    items
}

/// `(Ligatures, DLigatures)` from the run's sheet. Defaults match Photoshop's
/// `default_sheet` ([INFERENCE] for runs without a sheet).
fn ligature_flags(run: &RunStyle) -> (bool, bool) {
    let flag = |key: &str, default: bool| {
        run.sheet
            .as_ref()
            .and_then(|s| s.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(default)
    };
    (flag("Ligatures", true), flag("DLigatures", false))
}

/// Font name key of a run, asserting it exists in the set.
fn face_name<'a>(faces: &'a FontSet<'_>, run: &'a RunStyle) -> &'a str {
    match &run.font {
        Some(name) => {
            assert!(
                faces.get(name).is_some(),
                "run font {name:?} is not in the FontSet"
            );
            name
        }
        None => faces.default_name(),
    }
}

/// One-glyph feature substitution (`smcp`, `c2sc`, `onum`): the replacement
/// glyph of the first lookup that changes `gid`, or `None` when the face does
/// not implement the feature for this glyph.
fn substitute_feature(
    face: &crate::font::Face<'_>,
    ch: char,
    tag: [u8; 4],
    gid: u16,
) -> Option<u16> {
    let data = face.table(gsub::TAG_GSUB)?;
    let table = Gsub::parse(data).ok()?;
    let mut span = Span {
        gid,
        start: 0,
        len: 1,
    };
    let mut changed = false;
    for lookup in table.lookups(&script_tags(ch), &[tag]) {
        let out = table.apply(lookup, &[span]);
        if let Some(first) = out.first()
            && first.gid != span.gid
        {
            span = *first;
            changed = true;
        }
    }
    changed.then_some(span.gid)
}

/// Script tags tried in order for a group: Latin letters use `latn`, others
/// `DFLT`. The other tag is the fallback.
fn script_tags(ch: char) -> [[u8; 4]; 2] {
    const LATN: [u8; 4] = *b"latn";
    const DFLT: [u8; 4] = *b"DFLT";
    if ch.is_ascii_alphabetic() || ('\u{00C0}'..='\u{024F}').contains(&ch) {
        [LATN, DFLT]
    } else {
        [DFLT, LATN]
    }
}

/// Ligature substitution over the whole item list, one shaping group at a
/// time (adjacent items sharing font, flags and small-caps state).
fn substitute(faces: &FontSet<'_>, items: &[Item<'_>]) -> Vec<Span> {
    let mut out = Vec::with_capacity(items.len());
    let mut start = 0;
    while start < items.len() {
        let head = &items[start];
        let len = items[start..]
            .iter()
            .take_while(|it| it.shares_shaping(head))
            .count();
        out.extend(substitute_group(faces, &items[start..start + len], start));
        start += len;
    }
    out
}

fn substitute_group(faces: &FontSet<'_>, group: &[Item<'_>], base: usize) -> Vec<Span> {
    let mut spans: Vec<Span> = group
        .iter()
        .enumerate()
        .map(|(i, it)| Span {
            gid: it.gid,
            start: base + i,
            len: 1,
        })
        .collect();
    let head = &group[0];
    let mut features: Vec<[u8; 4]> = Vec::with_capacity(2 + head.features.len());
    if head.ligatures {
        features.push(*b"liga");
    }
    if head.dligatures {
        features.push(*b"dlig");
    }
    // Run features (`Txt2` flags and stylistic sets). An amount of 0 is off
    // and is not applied; amounts above 1 select an alternate, which GSUB
    // LookupType 3 does not implement, so any positive amount means "on".
    for feature in head.features {
        // `liga`/`dlig` are the flags above; `onum` goes through the old-style
        // figures path (which applies `pnum` first, Photoshop's proportional
        // figures); small caps use their dedicated substitution.
        const HANDLED: [&str; 5] = ["liga", "dlig", "onum", "smcp", "c2sc"];
        if feature.value == 0
            || HANDLED.contains(&feature.tag.as_str())
            || features.iter().any(|t| t == feature.tag.as_bytes())
        {
            continue;
        }
        if let Ok(tag) = <[u8; 4]>::try_from(feature.tag.as_bytes()) {
            features.push(tag);
        }
    }
    if features.is_empty() {
        return spans;
    }
    let Some(table) = faces
        .get(head.face)
        .and_then(|f| f.table(gsub::TAG_GSUB))
        .and_then(|d| Gsub::parse(d).ok())
    else {
        return spans;
    };
    for lookup in table.lookups(&script_tags(head.ch), &features) {
        spans = table.apply(lookup, &spans);
    }
    spans
}

/// Advances, kerning, tracking, faux bold and attribute assembly.
fn layout_glyphs(
    faces: &FontSet<'_>,
    runs: &[RunStyle],
    style: &TextStyle,
    items: &[Item<'_>],
    spans: &[Span],
) -> Vec<(GlyphPos, GlyphAttrs)> {
    let mut out: Vec<(GlyphPos, GlyphAttrs)> = Vec::with_capacity(spans.len());
    let mut pen = 0f32;
    let mut prev: Option<(usize, u16, char)> = None;
    for span in spans {
        let item = &items[span.start];
        let run = &runs[item.run];
        let face = faces.get(item.face).expect("face_name validated the font");
        let rs = resolve(run, style);
        let upm = face.units_per_em() as f32;
        let script_scale = match run.baseline {
            Some(1) | Some(2) => SCRIPT_SIZE,
            _ => 1.0,
        };
        let size_scale = if item.small { SMALL_CAPS_SCALE } else { 1.0 } * script_scale;
        let glyph_scale = rs.size / upm * size_scale;
        let hscale = rs.horizontal_scale;
        let kern = match prev {
            Some((run_prev, prev_gid, prev_ch)) if run_prev == item.run => match rs.kerning {
                Kerning::Metrics => face.kern_pair(prev_gid, span.gid) as f32 * glyph_scale,
                Kerning::Manual(v) => v as f32 / 1000.0 * rs.size,
                Kerning::Optical => crate::optical::optical_pair_chars(face, prev_gid, span.gid, rs.size, prev_ch, item.ch),
            },
            _ => 0.0,
        };
        pen += kern * hscale;
        let tracking = rs.tracking as f32 / 1000.0 * rs.size;
        let extra = if run.faux_bold {
            faux_bold_extra_advance(rs.size)
        } else {
            0.0
        };
        // Tracking is a horizontal step, so it follows the horizontal scale
        // (measured on `10_all_features.psd` layer `03 tracking baseline
        // scales`: 22 pt, hscale 1.5, tracking 300 gives a 304 px ink width;
        // unscaled predicts 268).
        let advance =
            face.advance(span.gid) as f32 * glyph_scale * hscale + tracking * hscale + extra;
        let glyph = GlyphPos {
            gid: span.gid,
            ch: item.ch,
            x: pen,
            y: rs.baseline_shift,
            advance,
            nominal_advance: advance,
        };
        let attrs = GlyphAttrs {
            run: item.run,
            color: run.color.unwrap_or([0, 0, 0]),
            // Vertical scale stretches the glyph in y only: it must not compose
            // with the horizontal scale, or a 50% horizontal squeeze would also
            // halve the glyph height.
            vertical_scale: run.vertical_scale.unwrap_or(1.0) * size_scale,
            x_scale: size_scale,
            faux_italic: run.faux_italic,
            leading: rs.leading_pt(),
            rotate: matches!(run.rotate_roman, Some(true)) && crate::typography::is_roman(item.ch),
            // The span is the ligature's components; they are adjacent in the
            // text, so the first start and the last end form the range.
            source: (
                items[span.start].source.0,
                items[span.start + span.len - 1].source.1,
            ),
        };
        out.push((glyph, attrs));
        pen += advance;
        prev = Some((item.run, span.gid, item.ch));
    }
    // advance = pen delta to the next glyph (includes the next glyph's kern).
    for i in 0..out.len().saturating_sub(1) {
        let next_x = out[i + 1].0.x;
        out[i].0.advance = next_x - out[i].0.x;
    }
    out
}

/// `(char after caps mapping, run index)` for every source character.
fn char_runs(text: &str, runs: &[RunStyle]) -> Vec<(usize, usize, char)> {
    let mut out = Vec::with_capacity(text.len());
    let mut index = 0usize;
    let mut remaining = runs.first().map_or(0, |r| r.len);
    for (byte, ch) in text.char_indices() {
        while remaining == 0 && index + 1 < runs.len() {
            index += 1;
            remaining = runs[index].len;
        }
        remaining = remaining.saturating_sub(ch.len_utf16());
        out.push((index, byte, ch));
    }
    out
}

/// Photoshop's superscript/subscript size and position, from the Type
/// preferences it stores in a PSD's text resources (`SuperscriptSize` 0.583,
/// `SuperscriptPosition` 0.333 and their `Subscript` twins): a script run is
/// drawn at 58.3 % of its size and shifted by 33.3 % of it.
pub const SCRIPT_SIZE: f32 = 0.583;
pub const SCRIPT_POSITION: f32 = 0.333;

/// Effective per-run [`TextStyle`] (baseline shift in points, positive up).
fn resolve(run: &RunStyle, base: &TextStyle) -> TextStyle {
    let mut size = run.size.unwrap_or(base.size);
    let mut baseline_shift = run.baseline_shift.unwrap_or(base.baseline_shift);
    match run.baseline {
        Some(1) => {
            baseline_shift += SCRIPT_POSITION * size;
            size *= SCRIPT_SIZE;
        }
        Some(2) => {
            baseline_shift -= SCRIPT_POSITION * size;
            size *= SCRIPT_SIZE;
        }
        _ => {}
    }
    TextStyle {
        size,
        tracking: run.tracking.unwrap_or(base.tracking),
        kerning: kerning_for(run, base.kerning),
        horizontal_scale: run.horizontal_scale.unwrap_or(base.horizontal_scale),
        baseline_shift,
        // Auto leading (explicitly, or implied by the absence of a leading)
        // steps by `size x paragraph fraction`; a manual leading overrides it.
        leading: match run.auto_leading_flag() {
            Some(true) => None,
            _ => run.leading.or(base.leading),
        },
        auto_leading: base.auto_leading,
    }
}

fn kerning_for(run: &RunStyle, base: Kerning) -> Kerning {
    match (run.auto_kerning, run.kerning) {
        (Some(true), _) => Kerning::Metrics,
        (Some(false), manual) => Kerning::Manual(manual.unwrap_or(0)),
        (None, Some(manual)) => Kerning::Manual(manual),
        (None, None) => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer_render::FaceStore;

    fn assets() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("assets")
            .join("fonts")
    }

    fn store() -> FaceStore {
        let mut store = FaceStore::new();
        store.load(&assets(), "NotoSans-Regular").expect("font");
        store
    }

    fn set(store: &FaceStore) -> crate::runs::FontSet<'_> {
        crate::runs::FontSet::new(
            "NotoSans-Regular",
            store.face("NotoSans-Regular").expect("face"),
        )
    }

    fn run(len: usize) -> RunStyle {
        RunStyle {
            len,
            font: Some("NotoSans-Regular".into()),
            size: Some(48.0),
            ..Default::default()
        }
    }

    /// A requested OpenType feature reaches GSUB: NotoSans' `zero` substitutes
    /// the digit, so the shaped glyph differs from the plain one.
    #[test]
    fn run_features_reach_shaping() {
        let store = store();
        let set = set(&store);
        let base = TextStyle {
            size: 48.0,
            ..Default::default()
        };
        let plain = run(1);
        let mut featured = plain.clone();
        featured.features = vec![RunFeature {
            tag: "zero".into(),
            value: 1,
        }];
        let plain_glyphs = shape_runs(&set, "0", &[plain], &base);
        let featured_glyphs = shape_runs(&set, "0", &[featured], &base);
        assert_eq!(plain_glyphs.len(), 1);
        assert_eq!(featured_glyphs.len(), 1);
        assert_ne!(
            plain_glyphs[0].gid, featured_glyphs[0].gid,
            "the `zero` feature did not reach GSUB"
        );
    }

    /// A feature amount of 0 is off (Photoshop writes an explicit `calt 0`).
    #[test]
    fn a_zero_amount_does_not_apply_the_feature() {
        let store = store();
        let set = set(&store);
        let base = TextStyle {
            size: 48.0,
            ..Default::default()
        };
        let plain = run(1);
        let mut off = plain.clone();
        off.features = vec![RunFeature {
            tag: "zero".into(),
            value: 0,
        }];
        assert_eq!(
            shape_runs(&set, "0", &[plain], &base)[0].gid,
            shape_runs(&set, "0", &[off], &base)[0].gid
        );
    }

    /// EngineData root with one style run over `text`.
    fn engine_data(text: &str, size: f64, extra: &[(&str, Value)]) -> Value {
        let mut sheet = vec![
            ("Font".to_string(), Value::Int(0)),
            ("FontSize".to_string(), Value::Float(size)),
        ];
        for (key, value) in extra {
            sheet.push(((*key).to_string(), value.clone()));
        }
        Value::dict([
            (
                "EngineDict",
                Value::dict([(
                    "StyleRun",
                    Value::dict([
                        (
                            "RunLengthArray",
                            Value::Array(vec![Value::Int(text.encode_utf16().count() as i64)]),
                        ),
                        (
                            "RunArray",
                            Value::Array(vec![Value::dict([(
                                "StyleSheet",
                                Value::dict([("StyleSheetData", Value::Dict(sheet))]),
                            )])]),
                        ),
                    ]),
                )]),
            ),
            (
                "ResourceDict",
                Value::dict([(
                    "FontSet",
                    Value::Array(vec![Value::dict([(
                        "Name",
                        Value::Str("NotoSans-Regular".into()),
                    )])]),
                )]),
            ),
        ])
    }

    /// A requested feature reaches the pixels: `Fractions` substitutes the
    /// glyphs of `1/2` and the rendered ink differs.
    #[test]
    fn a_feature_changes_the_rendered_glyphs() {
        use crate::layer_render::render_from_engine_data;
        let store = store();
        let plain = engine_data("1/2", 48.0, &[]);
        let fractions = engine_data("1/2", 48.0, &[("Fractions", Value::Bool(true))]);
        let a = render_from_engine_data(&store, &plain, "1/2", (200.0, 100.0), (10.0, 10.0))
            .expect("render");
        let b = render_from_engine_data(&store, &fractions, "1/2", (200.0, 100.0), (10.0, 10.0))
            .expect("render");
        assert_ne!(
            a.rgba, b.rgba,
            "Fractions did not change the rendered glyphs"
        );
    }

    /// `FontBaseline 1`/`2` shrink the run to 58.3 % of its size and shift it
    /// up/down by 33.3 % of it, in the pixels as in the metrics.
    #[test]
    fn script_baselines_change_the_rendered_ink() {
        use crate::layer_render::render_from_engine_data;
        let store = store();
        let render = |baseline: i64| {
            let ed = engine_data("H", 60.0, &[("FontBaseline", Value::Int(baseline))]);
            render_from_engine_data(&store, &ed, "H", (200.0, 200.0), (20.0, 20.0)).expect("render")
        };
        let plain = render(0);
        let sup = render(1);
        let sub = render(2);
        let ratio = sup.height as f32 / plain.height.max(1) as f32;
        assert!((ratio - SCRIPT_SIZE).abs() < 0.08, "height ratio {ratio}");
        // The canvas is ink-cropped, so the offset plus the ink height is the
        // baseline: superscript's is higher (smaller y) by 33.3 % of its size,
        // subscript's lower by the same amount.
        let baseline = |p: &crate::layer_render::LayerPixels| p.offset.1 + p.height as f32;
        assert!(
            baseline(&plain) - baseline(&sup) > 5.0,
            "superscript should sit higher: sup {} vs plain {}",
            baseline(&sup),
            baseline(&plain)
        );
        assert!(
            baseline(&sub) - baseline(&plain) > 5.0,
            "subscript should sit lower: sub {} vs plain {}",
            baseline(&sub),
            baseline(&plain)
        );
    }

    /// Vertical roman text: `BaselineDirection 2` turns the glyphs 90°
    /// clockwise in the column instead of standing them upright.
    #[test]
    fn rotate_roman_turns_the_vertical_glyphs() {
        use crate::layer_render::render_vertical_from_engine_data;
        let store = store();
        let upright = engine_data("AB", 60.0, &[("BaselineDirection", Value::Int(1))]);
        let rotated = engine_data("AB", 60.0, &[("BaselineDirection", Value::Int(2))]);
        let a = render_vertical_from_engine_data(&store, &upright, "AB", (20.0, 20.0))
            .expect("upright");
        let b = render_vertical_from_engine_data(&store, &rotated, "AB", (20.0, 20.0))
            .expect("rotated");
        assert!(!a.rgba.is_empty() && !b.rgba.is_empty());
        // A roman glyph is taller than wide; turned 90° it is wider than tall,
        // so the laid-out column gets wider and shorter.
        assert_ne!(a.rgba, b.rgba, "rotate_roman did not turn the glyphs");
        assert!(
            b.width > a.width && b.height < a.height,
            "rotated column should be wider and shorter: upright {}x{}, rotated {}x{}",
            a.width,
            a.height,
            b.width,
            b.height
        );
    }

    /// Photoshop's superscript/subscript: 58.3 % of the size, raised or lowered
    /// by 33.3 % of it (`SuperscriptSize`/`SuperscriptPosition` text resources).
    #[test]
    fn script_baselines_shrink_and_shift_the_run() {
        let base = TextStyle {
            size: 60.0,
            ..Default::default()
        };
        let normal = RunStyle {
            len: 1,
            size: Some(60.0),
            ..Default::default()
        };
        let mut superscript = normal.clone();
        superscript.baseline = Some(1);
        let mut subscript = normal.clone();
        subscript.baseline = Some(2);
        let plain = resolve(&normal, &base);
        let sup = resolve(&superscript, &base);
        let sub = resolve(&subscript, &base);
        assert!((sup.size - 60.0 * SCRIPT_SIZE).abs() < 1e-4);
        assert!((sub.size - 60.0 * SCRIPT_SIZE).abs() < 1e-4);
        assert!((sup.baseline_shift - 60.0 * SCRIPT_POSITION).abs() < 1e-4);
        assert!((sub.baseline_shift + 60.0 * SCRIPT_POSITION).abs() < 1e-4);
        assert_eq!(plain.size, 60.0);
        assert_eq!(plain.baseline_shift, 0.0);
    }
}
