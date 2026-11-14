//! Photoshop's fitted glyph coordinates land on a quarter-pixel lattice.
//!
//! Measured from the type-layer rasters captured for task-17 (Times 48/30/12,
//! Arial 22, Myriad 48/100 pt, pen on a whole pixel, AA Crisp unless noted).
//! The edge of a stem is recovered per row as `col + (1 - alpha/255)` from the
//! first ink column and `col + alpha/255` from the last; the median over the
//! rows that are stem-only gives the edge for the whole glyph.
//!
//! **Scope: ppem <= 100.** Above 100 ppem the Sharp/Crisp level map changes
//! (`17*k` -> `16*n` for CFF, unquantised for hinted TT) and the fit stops
//! being a quarter-pixel nudge, so these assertions must not be extended to
//! large sizes without a fresh measurement (see `out/verify/sync/raster-spec.md`
//! sections 6b-6d).
//!
//! Each row is `(capture, size_pt, left_edge, right_edge)` in document pixels.
//! Measured residuals against the 1/4 px lattice: 0.000, 0.017, 0.033, 0.017,
//! 0.017, 0.033, 0.067, 0.033, 0.017 (max at `arial22_H_sharp_q0` right edge,
//! where the last ink column is a single sample wide). The tolerance is 0.07 px
//! so it covers that estimator noise while still failing a whole-pixel or
//! half-pixel error (0.25 / 0.5).

const QUARTER: f64 = 0.25;

/// Measured stem edges, all at ppem <= 100.
const FITTED_STEM_EDGES: &[(&str, f64, f64, f64)] = &[
    ("times48_H_crisp_q0", 48.0, 105.000, 129.267),
    ("times48_H_crisp_q2", 48.0, 106.000, 130.267),
    ("times30_H_crisp_q0", 30.0, 103.267, 118.467),
    ("times30_H_crisp_q3", 30.0, 104.267, 119.467),
    ("times12_H_crisp_q0", 12.0, 101.267, 107.467),
    ("arial22_H_crisp_q0", 22.0, 101.733, 114.267),
    ("arial22_H_sharp_q0", 22.0, 101.800, 114.067),
    ("myriad48_H_crisp_q0", 48.0, 103.533, 127.733),
    ("myriad100_H_sharp", 100.0, 67.267, 117.733),
];

/// Distance to the nearest multiple of `step`.
fn residue(value: f64, step: f64) -> f64 {
    (value - (value / step).round() * step).abs()
}

#[test]
fn every_fitted_stem_edge_sits_on_a_quarter_pixel() {
    for &(name, ppem, left, right) in FITTED_STEM_EDGES {
        assert!(
            ppem <= 100.0,
            "{name}: this law is only measured up to 100 ppem"
        );
        for (label, edge) in [("left", left), ("right", right)] {
            let r = residue(edge, QUARTER);
            assert!(
                r <= 0.07,
                "{name} {label} edge {edge} is {r:.3} px off the quarter-pixel lattice"
            );
        }
    }
}

#[test]
fn every_fitted_stem_width_sits_on_a_quarter_pixel() {
    for &(name, ppem, left, right) in FITTED_STEM_EDGES {
        assert!(
            ppem <= 100.0,
            "{name}: this law is only measured up to 100 ppem"
        );
        let width = right - left;
        let r = residue(width, QUARTER);
        // Widths follow from two lattice edges, so the tolerance is the same
        // measurement noise, not a doubled one.
        assert!(
            r <= 0.07,
            "{name} stem width {width:.3} is {r:.3} px off the quarter-pixel lattice"
        );
    }
}

#[test]
fn the_quarter_pixel_lattice_is_not_a_power_of_two_of_the_pixel() {
    // Guard against someone "simplifying" the lattice to whole or half pixels:
    // measured widths 24.267 (Times 48) and 15.200 (Times 30) are not within
    // 0.05 of a half pixel, so the step really is 1/4.
    let times48_width = 129.267 - 105.000;
    let times30_width = 118.467 - 103.267;
    assert!(
        residue(times48_width, 0.5) > 0.07,
        "the 48 pt width fits a half-pixel step"
    );
    assert!(
        residue(times30_width, 0.5) > 0.07,
        "the 30 pt width fits a half-pixel step"
    );
}
