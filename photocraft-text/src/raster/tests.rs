use super::{Edge, scanlines, span};

// Frozen scalar implementation: compare f32 bits before alpha quantisation.
fn reference_scanlines(edges: &[Edge], w: usize, h: usize, sub: usize) -> Vec<f32> {
    let mut cov = vec![0f32; w * h];
    let weight = 1.0 / sub as f32;
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for row in 0..h {
        for s in 0..sub {
            let y = row as f32 + (s as f32 + 0.5) / sub as f32;
            xs.clear();
            for e in edges {
                let (y0, y1) = (e.y0, e.y1);
                if (y0 <= y && y1 > y) || (y1 <= y && y0 > y) {
                    let t = (y - y0) / (y1 - y0);
                    xs.push((e.x0 + t * (e.x1 - e.x0), if y1 > y0 { 1 } else { -1 }));
                }
            }
            if xs.len() < 2 {
                continue;
            }
            xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut winding = 0;
            let mut span_start = 0f32;
            for &(x, dir) in &xs {
                let was = winding;
                winding += dir;
                if was == 0 && winding != 0 {
                    span_start = x;
                } else if was != 0 && winding == 0 {
                    span(span_start, x, row * w, w, &mut cov, weight);
                }
            }
        }
    }
    cov
}

fn assert_same_coverage(edges: &[Edge], w: usize, h: usize, sub: usize) {
    let expected = reference_scanlines(edges, w, h, sub);
    let actual = scanlines(edges, w, h, sub);
    assert_eq!(
        actual.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        expected.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        "{w}x{h}, {sub} subscanlines, {} edges",
        edges.len(),
    );
}

#[test]
fn scanline_coverage_matches_scalar_for_clipped_and_crossing_edges() {
    let mut seed = 0x8b17_4d39u32;
    let mut coordinate = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed % 1024) as f32 / 32.0 - 5.0
    };
    for count in [0, 1, 2, 7, 32, 127] {
        for _ in 0..8 {
            let edges: Vec<Edge> = (0..count)
                .map(|_| Edge {
                    x0: coordinate(),
                    y0: coordinate(),
                    x1: coordinate(),
                    y1: coordinate(),
                })
                .collect();
            for sub in [1, 2, 3, 4, 8, 15, 16, 64] {
                assert_same_coverage(&edges, 17, 19, sub);
            }
        }
    }
}

#[test]
fn scanline_row_boundaries_keep_half_open_intersections_and_tie_order() {
    let mut edges = Vec::new();
    for y in [-1.0f32, -0.0, 0.0, 0.125, 0.5, 0.875, 1.0, 1.125, 6.5, 7.0] {
        for end in [y.next_down(), y, y.next_up(), y + 1.0] {
            edges.push(Edge {
                x0: 2.25,
                y0: y,
                x1: 5.75,
                y1: end,
            });
            edges.push(Edge {
                x0: 5.75,
                y0: end,
                x1: 2.25,
                y1: y,
            });
            edges.push(Edge {
                x0: 2.25,
                y0: end,
                x1: 2.25,
                y1: y,
            });
        }
    }
    for sub in [1, 2, 3, 4, 8, 15, 16, 64, 257] {
        assert_same_coverage(&edges, 9, 9, sub);
    }
}

fn glyph_edges(ch: char, size: f32) -> Vec<Edge> {
    let face = crate::font::Face::parse(include_bytes!(
        "../../tests/assets/fonts/NotoSans-Regular.ttf"
    ))
    .unwrap();
    let gid = face.glyph_index(ch).unwrap();
    let contours = crate::outline::glyph_contours(&face, gid).unwrap();
    let scale = size / f32::from(face.units_per_em());
    super::flatten(&contours, &|x, y| {
        (4.25 + x * scale + 0.2 * y * scale, size - y * scale)
    })
}

#[test]
fn scanline_glyph_masks_keep_unquantised_coverage_bits() {
    for ch in ['A', 'B', 'g', 'é', ' '] {
        for size in [9.0, 24.0, 48.0, 96.0] {
            let edges = glyph_edges(ch, size);
            for sub in [1, 3, 4, 16] {
                assert_same_coverage(&edges, 128, 128, sub);
            }
        }
    }
}

fn reference_over(pixel: &mut [u8; 4], color: [u8; 3], a: f32) {
    let prev = pixel[3] as f32 / 255.0;
    let out = (a + prev * (1.0 - a)).clamp(0.0, 1.0);
    if out > 0.0 {
        for c in 0..3 {
            let mixed = (color[c] as f32 * a + pixel[c] as f32 * prev * (1.0 - a)) / out;
            pixel[c] = mixed.round().clamp(0.0, 255.0) as u8;
        }
        pixel[3] = (out * 255.0).round() as u8;
    }
}

#[test]
fn alpha_compositing_keeps_scalar_bytes_for_all_stored_alpha_pairs() {
    let mut canvas = super::Canvas::new(1, 1);
    for previous in 0..=255u8 {
        for source in 0..=255u8 {
            let color = [source, source.wrapping_add(73), source.wrapping_mul(179)];
            let mut expected = [
                previous.wrapping_mul(137),
                source.wrapping_add(29),
                255,
                previous,
            ];
            canvas.pixels.copy_from_slice(&expected);
            let alpha = f32::from(source) / 255.0;
            reference_over(&mut expected, color, alpha);
            canvas.over(0, color, alpha);
            assert_eq!(
                canvas.pixels, expected,
                "source={source} previous={previous}"
            );
        }
    }
}

#[test]
fn alpha_compositing_preserves_extreme_fractional_values_and_hidden_rgb() {
    let mut canvas = super::Canvas::new(1, 1);
    for alpha in [
        -0.0,
        0.0,
        f32::from_bits(1),
        f32::MIN_POSITIVE,
        0.5f32.next_down(),
        0.5,
        0.5f32.next_up(),
        1.0f32.next_down(),
        1.0,
        f32::NAN,
    ] {
        for previous in [0, 1, 16, 127, 128, 254, 255] {
            for source in [0, 1, 128, 254, 255] {
                let color = [source, 129, 255];
                let mut expected = [255, 17, 0, previous];
                canvas.pixels.copy_from_slice(&expected);
                reference_over(&mut expected, color, alpha);
                canvas.over(0, color, alpha);
                assert_eq!(
                    canvas.pixels, expected,
                    "source={source} previous={previous} alpha={alpha:?}"
                );
            }
        }
    }
}
