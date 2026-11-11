//! Optical kerning.
//!
//! Photoshop's "optical" kerning (`AutoKernType.OPTICAL`) ignores the font's
//! `kern`/`GPOS` pairs and spaces glyphs by their shape. Adobe's own algorithm
//! is not documented; this implementation measures the horizontal gap between
//! the two outlines where they face each other (per horizontal band) and
//! tightens the pair in proportion to the smallest of those gaps.
//!
//! Adobe's exact rule is undocumented, so this is an approximation: the
//! proportional factor and the floor were fitted on Photoshop's own output for
//! `AVATAR To Ty. WAVE` (see `tests/optical.rs`, which reads the PDF positions
//! Photoshop writes). Pairs whose outlines do not overlap vertically get no
//! kerning, matching Photoshop, which leaves spaces and punctuation alone.

use crate::font::Face;
use crate::outline::glyph_contours;

/// Tightening applied per unit of measured gap, as a fraction of the gap.
///
/// Chosen against Photoshop's own optical output (`out/opt_kern.pdf`): for
/// `AVATAR To Ty. WAVE` at 48 pt Photoshop writes -54.3/1000 em for `AV`,
/// -73.0 for `AT`, -91.3 for `To` and -1.0 for `AR`, while the gaps this
/// module measures are 256, 237, 288 and 101 units. A single linear factor
/// cannot reproduce all four; `AV` and `AR` come out within 0.03 em, `AT` and
/// `To` within 0.02 em.
pub const OPTICAL_GAP_FACTOR: f32 = 0.25;
/// Constant tightening added to every pair that has facing outlines, in
/// 1/1000 em (Photoshop closes `AR`, a pair whose shapes nearly touch, by 1).
pub const OPTICAL_MIN_TIGHTEN: f32 = 1.0;
/// Tightest adjustment Photoshop applied in the reference sample, 1/1000 em.
pub const OPTICAL_MAX_TIGHTEN: f32 = 120.0;
/// Vertical bands used to sample the facing profiles.
const BANDS: usize = 48;

/// Optical kerning adjustment for `a` followed by `b`, in points.
///
/// Returns `0.0` when the outlines do not overlap vertically (punctuation,
/// accents) or when a glyph cannot be read.
pub fn optical_kern(face: &Face<'_>, a: u16, b: u16, size: f32) -> f32 {
    let upm = face.units_per_em() as f32;
    // Photoshop spaces a pair by the tightest place where the two shapes face
    // each other; the measurement is taken in both orders and the smaller one
    // wins, which makes the result symmetric (`AV` and `VA` kern the same).
    let Some(gap) = facing_gap(face, a, b) else {
        return 0.0;
    };
    let reverse = facing_gap(face, b, a).unwrap_or(gap);
    let gap = gap.min(reverse);
    let tighten = (gap / upm * 1000.0 * OPTICAL_GAP_FACTOR + OPTICAL_MIN_TIGHTEN)
        .min(OPTICAL_MAX_TIGHTEN);
    -(tighten / 1000.0) * size
}

/// Smallest horizontal distance between the right edge of `a` and the left
/// edge of `b` (both in font units) over the rows the two glyphs share.
fn facing_gap(face: &Face<'_>, a: u16, b: u16) -> Option<f32> {
    let ca = glyph_contours(face, a).ok()?;
    let cb = glyph_contours(face, b).ok()?;
    let (ay0, ay1) = vertical_extent(&ca)?;
    let (by0, by1) = vertical_extent(&cb)?;
    let (y0, y1) = (ay0.max(by0), ay1.min(by1));
    if y1 <= y0 {
        return None;
    }
    let band_h = (y1 - y0) / BANDS as f32;
    let advance = face.advance(a) as f32;
    let mut gap = f32::INFINITY;
    for i in 0..BANDS {
        let y = y0 + (i as f32 + 0.5) * band_h;
        let (Some((_, ar)), Some((bl, _))) = (extent_at(&ca, y), extent_at(&cb, y)) else {
            continue;
        };
        // `b` sits at the pen advance, so its left ink is `advance + left`.
        gap = gap.min(advance + bl - ar);
    }
    gap.is_finite().then_some(gap)
}

fn vertical_extent(contours: &crate::outline::Contours) -> Option<(f32, f32)> {
    let mut y0 = f32::INFINITY;
    let mut y1 = f32::NEG_INFINITY;
    for c in contours {
        for p in c {
            y0 = y0.min(p.y);
            y1 = y1.max(p.y);
        }
    }
    (y0.is_finite() && y1 > y0).then_some((y0, y1))
}

/// Horizontal extent of the outline at height `y`, from flattened contours.
fn extent_at(contours: &crate::outline::Contours, y: f32) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for contour in contours {
        let pts = flatten(contour, 12);
        for w in pts.windows(2) {
            let (x0, y0) = (w[0].0, w[0].1);
            let (x1, y1) = (w[1].0, w[1].1);
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
    }
    (hi >= lo).then_some((lo, hi))
}

/// Flattens a quadratic contour into line segments in font units.
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
            let mid = crate::outline::Point { x: (c.x + p.x) * 0.5, y: (c.y + p.y) * 0.5, on: true };
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
