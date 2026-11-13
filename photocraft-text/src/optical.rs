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

/// Target ink gap the model pulls a pair towards, as a fraction of the em.
pub const OPTICAL_TARGET_GAP: f32 = 0.0425;
/// Share of the distance to [`OPTICAL_TARGET_GAP`] the model closes.
pub const OPTICAL_GAP_FACTOR: f32 = 0.475;
/// Largest correction the model may apply to one pair, as a fraction of the em.
pub const OPTICAL_CLAMP: f32 = 0.0275;
/// Rows sampled across the band the two glyphs share.
const BANDS: usize = 48;
/// Line segments each outline is flattened into before measuring; curves are
/// sampled this many times.
const FLATTEN_STEPS: usize = 12;

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
    let target = OPTICAL_TARGET_GAP * upm;
    let clamp = OPTICAL_CLAMP * upm;
    (OPTICAL_GAP_FACTOR * (target - gap)).clamp(-clamp, clamp) / upm * size
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
            out.push(Seg { x0: w[0].0 + dx, y0: w[0].1, x1: w[1].0 + dx, y1: w[1].1 });
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
