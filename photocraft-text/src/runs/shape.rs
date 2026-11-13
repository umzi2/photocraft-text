use crate::engine_data::Value;
use crate::font::gsub::{self, Gsub, Span};
use crate::layout::{GlyphPos, Kerning, TextStyle};
use crate::typography::faux_bold_extra_advance;

use super::model::{FontSet, GlyphAttrs, RunStyle, SMALL_CAPS_SCALE, ShapedGlyph};

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
    ligatures: bool,
    dligatures: bool,
}

impl Item<'_> {
    fn shares_shaping(&self, other: &Item<'_>) -> bool {
        self.face == other.face
            && self.ligatures == other.ligatures
            && self.dligatures == other.dligatures
            && self.small == other.small
    }
}

fn collect_items<'a>(faces: &'a FontSet<'_>, text: &str, runs: &'a [RunStyle]) -> Vec<Item<'a>> {
    let mut items = Vec::with_capacity(text.len());
    for (run_index, ch) in char_runs(text, runs) {
        if ch == '\r' || ch == '\n' {
            continue;
        }
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
                } else if c.is_lowercase() {
                    if let Some(upper) = c.to_uppercase().next().and_then(|u| face.glyph_index(u)) {
                        gid = upper;
                        small = true;
                    }
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
        if let Some(first) = out.first() {
            if first.gid != span.gid {
                span = *first;
                changed = true;
            }
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
    let mut features: Vec<[u8; 4]> = Vec::with_capacity(2);
    if head.ligatures {
        features.push(*b"liga");
    }
    if head.dligatures {
        features.push(*b"dlig");
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
    let mut prev: Option<(usize, u16)> = None;
    for span in spans {
        let item = &items[span.start];
        let run = &runs[item.run];
        let face = faces.get(item.face).expect("face_name validated the font");
        let rs = resolve(run, style);
        let upm = face.units_per_em() as f32;
        let size_scale = if item.small { SMALL_CAPS_SCALE } else { 1.0 };
        let glyph_scale = rs.size / upm * size_scale;
        let hscale = rs.horizontal_scale;
        let kern = match prev {
            Some((run_prev, prev_gid)) if run_prev == item.run => match rs.kerning {
                Kerning::Metrics => face.kern_pair(prev_gid, span.gid) as f32 * glyph_scale,
                Kerning::Manual(v) => v as f32 / 1000.0 * rs.size,
                Kerning::Optical => crate::optical::optical_pair(face, prev_gid, span.gid, rs.size),
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
        };
        out.push((glyph, attrs));
        pen += advance;
        prev = Some((item.run, span.gid));
    }
    // advance = pen delta to the next glyph (includes the next glyph's kern).
    for i in 0..out.len().saturating_sub(1) {
        let next_x = out[i + 1].0.x;
        out[i].0.advance = next_x - out[i].0.x;
    }
    out
}

/// `(char after caps mapping, run index)` for every source character.
fn char_runs(text: &str, runs: &[RunStyle]) -> Vec<(usize, char)> {
    let mut out = Vec::with_capacity(text.len());
    let mut index = 0usize;
    let mut remaining = runs.first().map_or(0, |r| r.len);
    for ch in text.chars() {
        while remaining == 0 && index + 1 < runs.len() {
            index += 1;
            remaining = runs[index].len;
        }
        remaining = remaining.saturating_sub(ch.len_utf16());
        out.push((index, ch));
    }
    out
}

/// Effective per-run [`TextStyle`] (baseline shift in points, positive up).
fn resolve(run: &RunStyle, base: &TextStyle) -> TextStyle {
    TextStyle {
        size: run.size.unwrap_or(base.size),
        tracking: run.tracking.unwrap_or(base.tracking),
        kerning: kerning_for(run, base.kerning),
        horizontal_scale: run.horizontal_scale.unwrap_or(base.horizontal_scale),
        baseline_shift: run.baseline_shift.unwrap_or(base.baseline_shift),
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
