//! Compositing laws measured from Photoshop (task-17, `out/verify/sync/raster-spec.md` §5).
//!
//! Two *different* laws coexist, and both are measured, not assumed:
//!
//! * Ordinary layers (fills, pixels, ...) composite **linearly in stored 8-bit
//!   space**: `out = bg + (opacity)*(fg - bg)`. Verified over 84 captures
//!   (4 background/foreground pairs x 21 opacities, `opa_*.psd`): max error
//!   1/255.
//! * **Live type layers** do not. `CoolType`/AGM hand Photoshop a linearisation
//!   gamma (`AGMRenderOptionsSetLinearizationGammaProc`, a host callback — see
//!   the spec §1), and the composite is formed in linear light:
//!   `out = E(D(bg) + c*(D(fg) - D(bg)))`, `D(x) = x^g`, `E = D^-1`,
//!   `c` = the stored glyph alpha / 255, `g ≈ 1.5`.
//!   Evidence: with a stored alpha of 17/255, black-on-white gives 244 while
//!   white-on-black gives 38; a single-alpha linear blend would give 11 and 38.
//!   Fitting the law over 196 measured rows (3 AA modes x 4..7 backgrounds)
//!   gives mean |err| 1.7, max 7.4 at `g = 1.5`; `g = 1.0` gives 17.2 / 40.
//!
//! The stored *mask* itself is linear (its level set is a pure quantisation of
//! coverage), so never apply a curve to `alpha` — only to the composite.

/// Gamma used when a **type layer** is composited onto a background.
///
/// Measured, see the module docs: 1.5 is the least-squares fit (1.44 also fits
/// within the 8-bit noise floor). Exposed as a constant so the app can keep it
/// in one place.
pub const TYPE_COMPOSITE_GAMMA: f32 = 1.5;

/// Decode a stored 8-bit value to linear light (`(v/255)^gamma`).
#[inline]
pub fn decode8(v: u8, gamma: f32) -> f32 {
    (v as f32 / 255.0).powf(gamma)
}

/// Encode a linear-light value back to 8 bits (`255 * x^(1/gamma)`), rounded.
#[inline]
pub fn encode8(x: f32, gamma: f32) -> u8 {
    (255.0 * x.max(0.0).powf(1.0 / gamma))
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Composite one channel of a **live type layer** pixel over a background.
///
/// `coverage` is the layer's stored alpha in `0..=1` (straight, unpremultiplied).
/// Implements the measured law `E(D(bg) + c*(D(fg) - D(bg)))`.
#[inline]
pub fn composite_type_pixel(bg: u8, fg: u8, coverage: f32, gamma: f32) -> u8 {
    let c = coverage.clamp(0.0, 1.0);
    let lin = decode8(bg, gamma) + c * (decode8(fg, gamma) - decode8(bg, gamma));
    encode8(lin, gamma)
}

/// Composite one channel of an **ordinary layer** pixel (measured linear law).
#[inline]
pub fn composite_layer_pixel(bg: u8, fg: u8, alpha: f32) -> u8 {
    let a = alpha.clamp(0.0, 1.0);
    (bg as f32 + a * (fg as f32 - bg as f32))
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Composite a straight-alpha **type-layer** pixel over an existing RGBA8
/// background pixel using the measured linear-light law.
///
/// This is the only correct place for the gamma: the layer's *own* stored
/// pixels are linear (colour = text colour, alpha = quantised coverage, no
/// curve — verified against Photoshop's type-layer rasters), while the
/// composite of a type layer onto a background is formed in linear light.
/// Use it in the flatten/composite step, never while building the layer mask.
///
/// `bg` is modified in place; its alpha is treated as opaque coverage of the
/// backdrop (the measured case). When the background pixel is fully
/// transparent the layer colour is written straight, because there is nothing
/// to linearise against.
pub fn composite_type_over_rgba(bg: &mut [u8; 4], fg: [u8; 3], coverage: f32, gamma: f32) {
    let c = coverage.clamp(0.0, 1.0);
    if c <= 0.0 {
        return;
    }
    if bg[3] == 0 {
        for ch in 0..3 {
            bg[ch] = fg[ch];
        }
        bg[3] = encode8(c, 1.0);
        return;
    }
    let bg_a = bg[3] as f32 / 255.0;
    let out_a = (c + bg_a * (1.0 - c)).clamp(0.0, 1.0);
    for ch in 0..3 {
        // Backdrop contribution is weighted by its own coverage so a
        // half-transparent backdrop keeps the right proportion.
        let lin = decode8(bg[ch], gamma) * bg_a * (1.0 - c) + decode8(fg[ch], gamma) * c;
        let value = if out_a > 0.0 { lin / out_a } else { 0.0 };
        bg[ch] = encode8(value.min(1.0), gamma);
    }
    bg[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(bg, fg, out)` for opacity 0,5,...,100 over a solid fill layer.
    /// Measured in Photoshop 20.0.2 (`opa_*.psd`, 84 docs).
    const FILL_SWEEP: &[(u8, u8, &[u8; 21])] = &[
        (
            128,
            0,
            &[
                128, 121, 115, 109, 102, 96, 89, 83, 77, 70, 64, 58, 51, 45, 38, 32, 26, 19, 13, 7,
                0,
            ],
        ),
        (
            0,
            255,
            &[
                0, 13, 26, 38, 51, 64, 77, 89, 102, 115, 128, 140, 153, 166, 179, 191, 204, 217,
                230, 242, 255,
            ],
        ),
        (
            255,
            0,
            &[
                255, 242, 229, 217, 204, 191, 178, 166, 153, 140, 127, 115, 102, 89, 76, 64, 51,
                38, 25, 13, 0,
            ],
        ),
        (
            128,
            255,
            &[
                128, 134, 141, 147, 153, 160, 166, 172, 179, 185, 192, 198, 204, 211, 217, 223,
                230, 236, 243, 249, 255,
            ],
        ),
    ];

    /// `(bg, fg, stored_alpha, merged_composite)` measured on live type layers
    /// (Times New Roman 48 pt `H`, 7 backgrounds, `out/verify/sync/blend_tables.json`).
    const TYPE_SWEEP: &[(u8, u8, u8, u8)] = &[
        (255, 0, 0, 255),     // bw
        (255, 0, 17, 244),    // bw
        (255, 0, 34, 233),    // bw
        (255, 0, 51, 221),    // bw
        (255, 0, 68, 209),    // bw
        (255, 0, 85, 197),    // bw
        (255, 0, 102, 184),   // bw
        (255, 0, 119, 170),   // bw
        (255, 0, 136, 156),   // bw
        (255, 0, 153, 141),   // bw
        (255, 0, 170, 125),   // bw
        (255, 0, 187, 107),   // bw
        (255, 0, 204, 88),    // bw
        (255, 0, 221, 66),    // bw
        (255, 0, 238, 38),    // bw
        (255, 0, 255, 0),     // bw
        (0, 128, 0, 0),       // g0_fg128
        (0, 128, 17, 14),     // g0_fg128
        (0, 128, 34, 28),     // g0_fg128
        (0, 128, 51, 40),     // g0_fg128
        (0, 128, 68, 50),     // g0_fg128
        (0, 128, 85, 59),     // g0_fg128
        (0, 128, 102, 68),    // g0_fg128
        (0, 128, 119, 76),    // g0_fg128
        (0, 128, 136, 83),    // g0_fg128
        (0, 128, 153, 90),    // g0_fg128
        (0, 128, 170, 97),    // g0_fg128
        (0, 128, 187, 104),   // g0_fg128
        (0, 128, 204, 110),   // g0_fg128
        (0, 128, 221, 116),   // g0_fg128
        (0, 128, 238, 122),   // g0_fg128
        (0, 128, 255, 128),   // g0_fg128
        (128, 0, 0, 128),     // g128_fg0
        (128, 0, 17, 122),    // g128_fg0
        (128, 0, 34, 116),    // g128_fg0
        (128, 0, 51, 110),    // g128_fg0
        (128, 0, 68, 104),    // g128_fg0
        (128, 0, 85, 97),     // g128_fg0
        (128, 0, 102, 90),    // g128_fg0
        (128, 0, 119, 83),    // g128_fg0
        (128, 0, 136, 76),    // g128_fg0
        (128, 0, 153, 68),    // g128_fg0
        (128, 0, 170, 59),    // g128_fg0
        (128, 0, 187, 50),    // g128_fg0
        (128, 0, 204, 40),    // g128_fg0
        (128, 0, 221, 28),    // g128_fg0
        (128, 0, 238, 14),    // g128_fg0
        (128, 0, 255, 0),     // g128_fg0
        (128, 255, 0, 128),   // g128_fg255
        (128, 255, 17, 138),  // g128_fg255
        (128, 255, 34, 149),  // g128_fg255
        (128, 255, 51, 158),  // g128_fg255
        (128, 255, 68, 167),  // g128_fg255
        (128, 255, 85, 176),  // g128_fg255
        (128, 255, 102, 185), // g128_fg255
        (128, 255, 119, 194), // g128_fg255
        (128, 255, 136, 202), // g128_fg255
        (128, 255, 153, 210), // g128_fg255
        (128, 255, 170, 218), // g128_fg255
        (128, 255, 187, 226), // g128_fg255
        (128, 255, 204, 233), // g128_fg255
        (128, 255, 221, 241), // g128_fg255
        (128, 255, 238, 248), // g128_fg255
        (128, 255, 255, 255), // g128_fg255
        (128, 64, 0, 128),    // g128_fg64
        (128, 64, 17, 124),   // g128_fg64
        (128, 64, 34, 121),   // g128_fg64
        (128, 64, 51, 117),   // g128_fg64
        (128, 64, 68, 113),   // g128_fg64
        (128, 64, 85, 109),   // g128_fg64
        (128, 64, 102, 105),  // g128_fg64
        (128, 64, 119, 101),  // g128_fg64
        (128, 64, 136, 97),   // g128_fg64
        (128, 64, 153, 93),   // g128_fg64
        (128, 64, 170, 88),   // g128_fg64
        (128, 64, 187, 84),   // g128_fg64
        (128, 64, 204, 79),   // g128_fg64
        (128, 64, 221, 74),   // g128_fg64
        (128, 64, 238, 69),   // g128_fg64
        (128, 64, 255, 64),   // g128_fg64
        (64, 0, 0, 64),       // g64_fg0
        (64, 0, 17, 61),      // g64_fg0
        (64, 0, 34, 58),      // g64_fg0
        (64, 0, 51, 54),      // g64_fg0
        (64, 0, 68, 51),      // g64_fg0
        (64, 0, 85, 47),      // g64_fg0
        (64, 0, 102, 43),     // g64_fg0
        (64, 0, 119, 39),     // g64_fg0
        (64, 0, 136, 35),     // g64_fg0
        (64, 0, 153, 31),     // g64_fg0
        (64, 0, 170, 26),     // g64_fg0
        (64, 0, 187, 21),     // g64_fg0
        (64, 0, 204, 15),     // g64_fg0
        (64, 0, 221, 9),      // g64_fg0
        (64, 0, 238, 4),      // g64_fg0
        (64, 0, 255, 0),      // g64_fg0
        (0, 255, 0, 0),       // wb
        (0, 255, 17, 38),     // wb
        (0, 255, 34, 66),     // wb
        (0, 255, 51, 88),     // wb
        (0, 255, 68, 107),    // wb
        (0, 255, 85, 125),    // wb
        (0, 255, 102, 141),   // wb
        (0, 255, 119, 156),   // wb
        (0, 255, 136, 170),   // wb
        (0, 255, 153, 184),   // wb
        (0, 255, 170, 197),   // wb
        (0, 255, 187, 209),   // wb
        (0, 255, 204, 221),   // wb
        (0, 255, 221, 233),   // wb
        (0, 255, 238, 244),   // wb
        (0, 255, 255, 255),   // wb
    ];

    #[test]
    fn fill_layer_opacity_is_linear_in_stored_space() {
        let mut n = 0;
        for (bg, fg, outs) in FILL_SWEEP {
            for (i, &out) in outs.iter().enumerate() {
                let alpha = (i * 5) as f32 / 100.0;
                let pred = composite_layer_pixel(*bg, *fg, alpha);
                assert!(
                    (pred as i32 - out as i32).abs() <= 1,
                    "bg {bg} fg {fg} p {i}5%: PS {out} vs {pred}"
                );
                n += 1;
            }
        }
        assert_eq!(n, 84, "the opacity sweep must cover all 84 measurements");
    }

    #[test]
    fn type_layer_composite_is_linear_light_not_linear() {
        let g = TYPE_COMPOSITE_GAMMA;
        let mut sum = 0.0f32;
        let mut worst = 0i32;
        let mut linear_worst = 0i32;
        for &(bg, fg, alpha, out) in TYPE_SWEEP {
            let c = alpha as f32 / 255.0;
            let pred = composite_type_pixel(bg, fg, c, g);
            let err = pred as i32 - out as i32;
            sum += err.abs() as f32;
            worst = worst.max(err.abs());
            // the linear law must be clearly worse on the same row
            linear_worst =
                linear_worst.max((composite_layer_pixel(bg, fg, c) as i32 - out as i32).abs());
        }
        let mean = sum / TYPE_SWEEP.len() as f32;
        // The plain linear law is far worse over the same rows (measured 17.2
        // mean / 40 worst), so the linear-light law is the right model even
        // though individual rows can be within a level of each other.
        assert!(
            linear_worst > worst * 2,
            "linear law (worst {linear_worst}) must be clearly worse than linear-light (worst {worst})"
        );
        assert!(
            worst <= 8,
            "worst type-composite error {worst} exceeds 8/255"
        );
        assert!(
            mean <= 2.0,
            "mean type-composite error {mean} exceeds 2/255"
        );
    }

    #[test]
    fn the_rgba_helper_reproduces_the_measured_type_table() {
        let g = TYPE_COMPOSITE_GAMMA;
        for &(bg, fg, alpha, want) in TYPE_SWEEP {
            let mut px = [bg, bg, bg, 255];
            composite_type_over_rgba(&mut px, [fg, fg, fg], alpha as f32 / 255.0, g);
            assert_eq!(px[3], 255, "opaque backdrop must stay opaque");
            let err = (px[0] as i32 - want as i32).abs();
            assert!(
                err <= 8,
                "bg {bg} fg {fg} c {alpha}: PS {want} vs {}",
                px[0]
            );
        }
    }

    #[test]
    fn a_transparent_backdrop_takes_the_layer_colour_straight() {
        let mut px = [0, 0, 0, 0];
        composite_type_over_rgba(&mut px, [17, 34, 51], 0.5, TYPE_COMPOSITE_GAMMA);
        assert_eq!([px[0], px[1], px[2]], [17, 34, 51]);
        assert_eq!(px[3], 128);
    }

    #[test]
    fn compositing_onto_transparent_keeps_the_colour() {
        // A layer built on transparency must store the text colour unchanged at
        // any coverage: the law only applies when there *is* a background.
        for fg in [0u8, 64, 128, 255] {
            for c in [0.0666, 0.5, 0.9333] {
                let lin = c * decode8(fg, TYPE_COMPOSITE_GAMMA);
                let out = encode8(lin, TYPE_COMPOSITE_GAMMA);
                assert!(
                    (out as i32) <= fg as i32 + 1,
                    "fg {fg} c {c} -> {out} brightened"
                );
            }
        }
    }
}
