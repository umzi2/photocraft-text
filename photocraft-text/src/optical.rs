//! Optical kerning.
//!
//! Photoshop's "optical" kerning (`AutoKernType.OPTICAL`) ignores the font's
//! `kern`/`GPOS` pairs and spaces glyphs by their shape. Adobe's algorithm is
//! undocumented, but its result is not: every glyph run in the composed-layout
//! cache a PSD's `Txt2` block carries holds a `\3` spacing entry for the pair
//! it starts with. Those entries scale linearly with the point size and are a
//! function of the outlines alone.
//!
//! This module measures the tightest horizontal ink gap between the two facing
//! edges over the height the glyphs share, and closes part of it:
//! [`OPTICAL_GAP_FACTOR`] of the distance to a [`OPTICAL_TARGET_GAP`] em gap,
//! clamped to [`OPTICAL_CLAMP`] em. The three constants were fitted by least
//! squares to Photoshop's own entries for five typefaces -- Times New Roman,
//! Arial, Georgia, Impact and Noto Sans, all 676 lower-case pairs of each, plus
//! both upper-case alphabets of Times and Noto -- 4,713 pairs in all. The fit
//! is within 0.6 px (mean) and 1.5 px (95th percentile) of Photoshop for
//! lower-case text at 72 pt -- 0.42/1.39 px for Times, 0.52/1.44 for Noto --
//! where Photoshop moves a pair by up to 4 px.
//! `tests/optical.rs` pins it to measured entries.
//!
//! The measurement reads only the outlines and metrics, so it runs on any font.
//! Pairs whose inks do not share a horizontal band (spaces, accents) get no
//! kerning.

use crate::font::Face;
use crate::outline::glyph_contours;

/// Fallback target ink gap, as a fraction of the em, for a face outside
/// [`OPTICAL_TARGETS`].
pub const OPTICAL_TARGET_GAP: f32 = 0.0425;

/// Photoshop's optical target ink gap at 24 pt, per font, keyed by the same
/// five-field metric signature `layer_render::vertical_b0_ratio` uses.
///
/// Measured on Photoshop's own recomposition of 730 pairs x 11 fonts
/// (`out/verify/sync/optical-targets.md`, `out/verify/sync/spacing/targets.json`):
/// the target is a *per-font* constant (0.0616 em for Impact to 0.1166 for
/// Courier New), which is why a single value cannot serve every face — with the
/// old universal 0.0425 em, `ll` in Noto Sans came out at -4.04 pt where
/// Photoshop composes -2.15 pt.
///
/// The remaining error of this table is pair-specific, not font-level: at 24 pt
/// over the same corpus the model leaves sd 0.21 px / max 0.94 px, and the worst
/// classes are punctuation (`,A`, `.A`) and heavy diagonals (`AV`, `TA`). The
/// table is a measured stopgap for Photoshop's undocumented optical kerning, not
/// its algorithm; see `out/verify/sync/spacing-spec.md` §6.1 for the open RE.
pub const OPTICAL_TARGETS: &[(OpticalSignature, f32)] = &[
    // Impact
    ((2048, 2066, -432, None, None), 0.0616404),
    // Trebuchet MS
    ((2048, 1923, -455, None, None), 0.0638117),
    // Times New Roman
    ((2048, 1825, -443, None, None), 0.0671740),
    // Georgia
    ((2048, 1878, -449, None, None), 0.0716090),
    // Myriad Pro
    ((1000, 750, -250, Some(674), Some(484)), 0.0731000),
    // Arial
    ((2048, 1854, -434, None, None), 0.0799569),
    // Arial Black
    ((2048, 2254, -634, None, None), 0.0849948),
    // Comic Sans MS
    ((2048, 2257, -597, None, None), 0.0991394),
    // Verdana
    ((2048, 2059, -430, None, None), 0.1033544),
    // Noto Sans
    ((1000, 1069, -293, Some(714), Some(536)), 0.1150145),
    // Courier New
    ((2048, 1705, -615, None, None), 0.1165914),
];

/// `(unitsPerEm, hhea.ascender, hhea.descender, OS/2 cap height, OS/2 x-height)`.
pub type OpticalSignature = (u16, i16, i16, Option<i16>, Option<i16>);

/// Photoshop's optical target gap scales with the point size: anchors measured on
/// Myriad Pro over 20 pairs at 8/12/18/24/36/48/72 pt, normalised to 24 pt.
///
/// The curve is Photoshop's own: small sizes keep more air (1.75x the 24 pt gap at
/// 8 pt) and display sizes tighten (0.46x at 72 pt), i.e. the optical value is not
/// an em constant (`out/verify/sync/spacing-spec.md` §2.2).
pub const OPTICAL_SIZE_FACTOR: &[(f32, f32)] = &[
    (8.0, 1.7514),
    (12.0, 1.1436),
    (18.0, 1.0709),
    (24.0, 1.0000),
    (36.0, 0.8622),
    (48.0, 0.7258),
    (72.0, 0.4597),
];
/// Share of the distance to [`OPTICAL_TARGET_GAP`] the model closes.
///
/// Per-font least squares against Photoshop's own recomposition of 730 pairs x 10 fonts at
/// 24 pt put the closure at 0.41-0.50; the shared value 0.44 is the fit
/// (`out/verify/sync/spacing-spec.md` §2, `out/verify/sync/spacing/`).
pub const OPTICAL_GAP_FACTOR: f32 = 0.44;
/// Largest correction the model may apply to one pair, as a fraction of the em.
///
/// Photoshop's optical values reach -0.18 em on punctuation/diagonal classes (`.V` in Courier
/// New is -4.0632 pt at 24 pt) and about -0.19 em on the heaviest diagonals, so a 0.0275 em
/// cap (fitted to lower-case text) clipped them. Raising it is only safe together with the
/// per-font target above: with the universal 0.0425 em target the clamp was the only thing
/// holding lower-case pairs in place (`ll` in Noto Sans overshot to -4.04 pt against
/// Photoshop's -2.15); with [`optical_target_gap`] the same pair lands at -3.71 pt.
pub const OPTICAL_CLAMP: f32 = 0.25;
/// Rows sampled across the band the two glyphs share.
const BANDS: usize = 48;
/// Line segments each outline is flattened into before measuring; curves are
/// sampled this many times.
const FLATTEN_STEPS: usize = 12;

/// The five-field metric signature of a face, the key of [`OPTICAL_TARGETS`].
pub fn optical_signature(face: &Face<'_>) -> OpticalSignature {
    (
        face.units_per_em(),
        face.hhea.ascender,
        face.hhea.descender,
        face.os2.and_then(|o2| o2.s_cap_height),
        face.os2.and_then(|o2| o2.s_x_height),
    )
}

/// The measured optical target gap of `size` for an unlisted face.
pub fn optical_target_gap(face: &Face<'_>, size: f32) -> f32 {
    let base = OPTICAL_TARGETS
        .iter()
        .find(|(known, _)| *known == optical_signature(face))
        .map(|(_, gap)| *gap)
        .unwrap_or(OPTICAL_TARGET_GAP);
    base * optical_size_factor(size)
}

/// Photoshop's optical size factor at `size`: piecewise-linear over
/// [`OPTICAL_SIZE_FACTOR`], clamped to the outer anchors.
pub fn optical_size_factor(size: f32) -> f32 {
    let table = OPTICAL_SIZE_FACTOR;
    if !(size.is_finite() && size > 0.0) {
        return 1.0;
    }
    let (first_size, first) = table[0];
    if size <= first_size {
        return first;
    }
    for window in table.windows(2) {
        let ((s0, f0), (s1, f1)) = (window[0], window[1]);
        if size <= s1 {
            let t = (size - s0) / (s1 - s0);
            return f0 + (f1 - f0) * t;
        }
    }
    table[table.len() - 1].1
}

/// Optical kerning adjustment for `a` followed by `b`, in points (negative
/// tightens).
///
/// Returns `0.0` when a glyph has no outline (spaces, controls) or the two inks
/// do not share a horizontal band: there is nothing to measure against.
pub fn optical_pair(face: &Face<'_>, a: u16, b: u16, size: f32) -> f32 {
    let upm = face.units_per_em() as f32;
    let Some(gap) = facing_gap(face, a, b) else {
        return 0.0;
    };
    let target = optical_target_gap(face, size) * upm;
    let clamp = OPTICAL_CLAMP * upm;
    let em = (OPTICAL_GAP_FACTOR * (target - gap)).clamp(-clamp, clamp) / upm;
    // Photoshop's cache stores the value as an exact multiple of 1e-4 em (724/724 measured
    // pairs), so the engine's own output is quantised the same way before it is scaled to
    // points.
    em_quantum(em) * size
}

/// Round an em fraction to Photoshop's 1e-4 em optical quantum.
pub fn em_quantum(v: f32) -> f32 {
    (v * 1.0e4).round() / 1.0e4
}

/// Largest correction a class-scaled pair may take, as a fraction of the em.
///
/// The fitted [`OPTICAL_CLAMP`] (0.0275 em) suits the lower-case pairs the model was fitted on,
/// whose Photoshop corrections are small. Photoshop's optical kerning moves upper-case diagonals
/// and punctuation much further: measured against Photoshop's own recomposition of Myriad Pro at
/// 48 pt, `AV` is -3 px (-0.0625 em), `AT` -5 px (-0.104 em), `P.` -7 px (-0.146 em) while
/// `Wa`/`Ve` are -2 px (-0.042 em). This cap leaves room for those classes.
pub const OPTICAL_MAX: f32 = 0.19;

/// Photoshop's optical class factor for a pair of characters, applied to the fitted model's raw
/// closure (before its lower-case clamp). Measured against Photoshop's own recomposition of
/// Myriad Pro at 48 pt (`tests/optical_pairs.rs` pins the values): the raw closure is close to
/// Photoshop for `T`/`Y` pairs and punctuation, but too strong for `A`/`V` capitals and for `F`
/// before a round lower-case glyph, and slightly too strong for `W`/`V` before one.
pub fn optical_class_factor(a: char, b: char) -> f32 {
    let punctuation = |c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '-' | '–' | '—' | '…');
    let diagonal = |c: char| matches!(c, 'A' | 'V' | 'W' | 'X' | 'K');
    let round = |c: char| matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y' | 'c' | 'd' | 'g' | 'q');
    if punctuation(b) {
        0.81
    } else if a == 'F' && round(b) {
        0.42
    } else if matches!(a, 'W' | 'V') && round(b) {
        0.78
    } else if diagonal(a) && diagonal(b) {
        0.64
    } else if matches!(a, 'T' | 'Y') || matches!(b, 'T' | 'Y') {
        0.95
    } else {
        1.0
    }
}

/// [`optical_pair`] with Photoshop's class calibration: the fitted model's raw closure, scaled by
/// [`optical_class_factor`] and capped at [`OPTICAL_MAX`] em. `a`/`b` are the pair's glyph ids and
/// `ca`/`cb` the characters they came from.
pub fn optical_pair_chars(face: &Face<'_>, a: u16, b: u16, size: f32, ca: char, cb: char) -> f32 {
    let upm = face.units_per_em() as f32;
    let Some(gap) = facing_gap(face, a, b) else {
        return 0.0;
    };
    let target = optical_target_gap(face, size) * upm;
    let closure = em_quantum(OPTICAL_GAP_FACTOR * (target - gap) / upm) * size;
    (closure * optical_class_factor(ca, cb)).clamp(-OPTICAL_MAX * size, 0.0)
}

/// Tightest horizontal ink gap between `a`'s right edge and `b`'s left edge
/// over the rows the two glyphs share, in font units.
fn facing_gap(face: &Face<'_>, a: u16, b: u16) -> Option<f32> {
    let ca = glyph_contours(face, a).ok()?;
    let cb = glyph_contours(face, b).ok()?;
    let advance = face.advance(a) as f32;
    let left = segments(&ca, 0.0);
    let right = segments(&cb, advance);
    let (_, ay0, _, ay1) = contours_box(&ca)?;
    let (_, by0, _, by1) = contours_box(&cb)?;
    let (y0, y1) = (ay0.max(by0), ay1.min(by1));
    if y1 <= y0 || left.is_empty() || right.is_empty() {
        return None;
    }
    let band_h = (y1 - y0) / BANDS as f32;
    let mut gap = f32::INFINITY;
    for i in 0..BANDS {
        let y = y0 + (i as f32 + 0.5) * band_h;
        let (Some((_, ar)), Some((bl, _))) = (row_extent(&left, y), row_extent(&right, y)) else {
            continue;
        };
        gap = gap.min(bl - ar);
    }
    gap.is_finite().then_some(gap)
}

/// Horizontal extent of a set of flattened edges at height `y`.
fn row_extent(segs: &[Seg], y: f32) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for s in segs {
        let (x0, y0) = (s.x0, s.y0);
        let (x1, y1) = (s.x1, s.y1);
        if (y0 - y).signum() == (y1 - y).signum() && y0 != y && y1 != y {
            continue;
        }
        if y1 == y0 {
            if (y - y0).abs() < 1e-4 {
                lo = lo.min(x0.min(x1));
                hi = hi.max(x0.max(x1));
            }
            continue;
        }
        let t = (y - y0) / (y1 - y0);
        if (0.0..=1.0).contains(&t) {
            let x = x0 + (x1 - x0) * t;
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    (hi >= lo).then_some((lo, hi))
}

/// One flattened outline edge.
#[derive(Clone, Copy, Debug)]
struct Seg {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// Flattens a glyph's contours into edges shifted right by `dx`.
fn segments(contours: &crate::outline::Contours, dx: f32) -> Vec<Seg> {
    let mut out = Vec::new();
    for contour in contours {
        let pts = flatten(contour, FLATTEN_STEPS);
        for w in pts.windows(2) {
            out.push(Seg {
                x0: w[0].0 + dx,
                y0: w[0].1,
                x1: w[1].0 + dx,
                y1: w[1].1,
            });
        }
    }
    out
}

/// Bounding box of a glyph's ink contours.
fn contours_box(contours: &crate::outline::Contours) -> Option<(f32, f32, f32, f32)> {
    let mut x0 = f32::INFINITY;
    let mut y0 = f32::INFINITY;
    let mut x1 = f32::NEG_INFINITY;
    let mut y1 = f32::NEG_INFINITY;
    for c in contours {
        for p in c {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
    }
    (x0.is_finite() && x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// Flattens one contour into a closed polyline in font units.
///
/// Quadratic outlineless contours (TrueType control points, implied on-curve
/// midpoints) and straight CFF polylines both come out as point sequences.
fn flatten(contour: &[crate::outline::Point], steps: usize) -> Vec<(f32, f32)> {
    let n = contour.len();
    let mut out = Vec::new();
    if n == 0 {
        return out;
    }
    // Start at an on-curve point; TrueType contours may start on a control.
    let Some(first_on) = contour.iter().position(|p| p.on) else {
        return out;
    };
    let mut cur = contour[first_on];
    // The first point must start the polyline, or the edge from it to the next
    // point is missing and the outline appears open.
    out.push((cur.x, cur.y));
    let mut ctrl: Option<crate::outline::Point> = None;
    for k in 1..=n {
        let p = contour[(first_on + k) % n];
        if p.on {
            if let Some(c) = ctrl.take() {
                for s in 1..=steps {
                    let t = s as f32 / steps as f32;
                    let mt = 1.0 - t;
                    out.push((
                        mt * mt * cur.x + 2.0 * mt * t * c.x + t * t * p.x,
                        mt * mt * cur.y + 2.0 * mt * t * c.y + t * t * p.y,
                    ));
                }
            } else {
                out.push((p.x, p.y));
            }
            cur = p;
        } else if ctrl.is_none() {
            ctrl = Some(p);
        } else {
            // Two controls in a row: the implied on-curve point is their midpoint.
            let c = ctrl.take().unwrap_or(p);
            let mid = crate::outline::Point {
                x: (c.x + p.x) * 0.5,
                y: (c.y + p.y) * 0.5,
                on: true,
            };
            for s in 1..=steps {
                let t = s as f32 / steps as f32;
                let mt = 1.0 - t;
                out.push((
                    mt * mt * cur.x + 2.0 * mt * t * c.x + t * t * mid.x,
                    mt * mt * cur.y + 2.0 * mt * t * c.y + t * t * mid.y,
                ));
            }
            cur = mid;
            ctrl = Some(p);
        }
    }
    out
}
