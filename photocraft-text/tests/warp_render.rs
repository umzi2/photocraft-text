//! The one-shot warped-layer adapter: it must own the flat→warped geometry
//! hand-off (padded flat render, remap into the stored rectangle) so a caller
//! cannot mix up the flat layout box and the warped layer rectangle.

use photocraft_text::engine_data::Value;
use photocraft_text::layer_render::{FaceStore, RectTarget, render_from_engine_data_warped};
use photocraft_text::runs::{RunStyle, to_engine_data};
use photocraft_text::warp::{self, FlatGeometry, FlatSurface, Style, Warp, placement};

fn store() -> FaceStore {
    let mut store = FaceStore::new();
    let dir = std::env::var_os("HOME").map(|h| {
        std::path::PathBuf::from(h).join(".local/share/photocraft/wine/drive_c/windows/Fonts")
    });
    if let Some(dir) = dir {
        store.load_dir(&dir);
    }
    store
}

fn engine_data(runs: Vec<RunStyle>, text: &str) -> Value {
    let covered: usize = runs.iter().map(|r| r.len).sum();
    assert_eq!(covered, text.encode_utf16().count());
    Value::dict([
        (
            "EngineDict",
            Value::dict([("StyleRun", to_engine_data(&runs))]),
        ),
        (
            "ResourceDict",
            Value::dict([(
                "FontSet",
                Value::Array(vec![Value::dict([
                    ("Name", Value::Str("NotoSans-Regular".into())),
                    ("Script", Value::Int(0)),
                ])]),
            )]),
        ),
    ])
}

const TEXT: &str = "Warp";

fn runs() -> Vec<RunStyle> {
    vec![RunStyle {
        len: TEXT.encode_utf16().count(),
        font: Some("NotoSans-Regular".into()),
        size: Some(24.0),
        ..Default::default()
    }]
}

fn target(size: (u32, u32)) -> RectTarget {
    RectTarget {
        origin: (0, 0),
        size,
        anchor: (10.0, 40.0),
    }
}

#[test]
fn warped_render_covers_exactly_the_stored_rectangle() {
    let store = store();
    let ed = engine_data(runs(), TEXT);
    let warp = Warp {
        style: Style::Arc,
        value: 50.0,
        ..Default::default()
    };
    let flat = FlatGeometry {
        bounds: (10.0, 20.0, 110.0, 44.0),
        baseline: 40.0,
    };
    let out = render_from_engine_data_warped(
        &store,
        &ed,
        TEXT,
        &[],
        &[],
        (0.0, 0.0),
        &warp,
        flat,
        target((120, 60)),
    )
    .expect("warped render");
    assert_eq!(out.rect, (0, 0, 120, 60));
    assert_eq!((out.pixels.width, out.pixels.height), (120, 60));
    assert_eq!(out.pixels.offset, (0.0, 0.0));
    assert_eq!(out.pixels.rgba.len(), 120 * 60 * 4);
    assert!(
        out.pixels.rgba.chunks_exact(4).any(|px| px[3] != 0),
        "warped surface is empty"
    );
}

#[test]
fn unwarped_style_renders_the_flat_layout_into_the_rectangle() {
    let store = store();
    let ed = engine_data(runs(), TEXT);
    let warp = Warp::default();
    let flat = FlatGeometry {
        bounds: (10.0, 20.0, 110.0, 44.0),
        baseline: 40.0,
    };
    let out = render_from_engine_data_warped(
        &store,
        &ed,
        TEXT,
        &[],
        &[],
        (0.0, 0.0),
        &warp,
        flat,
        target((120, 60)),
    )
    .expect("flat render");
    assert_eq!(out.rect, (0, 0, 120, 60));
    assert!(out.pixels.rgba.chunks_exact(4).any(|px| px[3] != 0));
}

#[test]
fn an_empty_flat_box_is_rejected() {
    let store = store();
    let ed = engine_data(runs(), TEXT);
    let warp = Warp {
        style: Style::Arc,
        value: 50.0,
        ..Default::default()
    };
    let flat = FlatGeometry {
        bounds: (0.0, 0.0, 0.0, 0.0),
        baseline: 40.0,
    };
    let err = render_from_engine_data_warped(
        &store,
        &ed,
        TEXT,
        &[],
        &[],
        (0.0, 0.0),
        &warp,
        flat,
        target((120, 60)),
    )
    .expect_err("degenerate bounds must not silently map the text into one pixel");
    assert!(matches!(err, photocraft_text::Error::Warp(_)));
}

#[test]
fn arch_style_uses_the_fitted_patch_and_covers_the_rectangle() {
    let store = store();
    let ed = engine_data(runs(), TEXT);
    let warp = Warp {
        style: Style::Arch,
        value: 50.0,
        ..Default::default()
    };
    let flat = FlatGeometry {
        bounds: (10.0, 20.0, 110.0, 44.0),
        baseline: 40.0,
    };
    let out = render_from_engine_data_warped(
        &store,
        &ed,
        TEXT,
        &[],
        &[],
        (0.0, 0.0),
        &warp,
        flat,
        target((120, 60)),
    )
    .expect("arch render");
    assert_eq!(out.rect, (0, 0, 120, 60));
    let ink: u64 = out
        .pixels
        .rgba
        .chunks_exact(4)
        .map(|px| u64::from(px[3] > 0))
        .sum();
    assert!(ink > 50, "arch surface has too little ink: {ink}");
}

#[test]
fn arch_bend_zero_keeps_the_flat_layout() {
    let store = store();
    let ed = engine_data(runs(), TEXT);
    let warp = Warp {
        style: Style::Arch,
        value: 0.0,
        ..Default::default()
    };
    let flat = FlatGeometry {
        bounds: (10.0, 20.0, 110.0, 44.0),
        baseline: 40.0,
    };
    let out = render_from_engine_data_warped(
        &store,
        &ed,
        TEXT,
        &[],
        &[],
        (0.0, 0.0),
        &warp,
        flat,
        target((120, 60)),
    )
    .expect("flat arch render");
    assert_eq!(out.rect, (0, 0, 120, 60));
    assert!(out.pixels.rgba.chunks_exact(4).any(|px| px[3] != 0));
}

/// The arch bends the baseline up in the middle: the topmost painted row is the
/// crown, so its ink sits in the middle of the rectangle while the ends sit
/// low. A resampled flat bitmap could not produce that asymmetry.
#[test]
fn arch_crown_is_centred_in_the_stored_rectangle() {
    let store = store();
    let ed = engine_data(runs(), TEXT);
    let warp = Warp {
        style: Style::Arch,
        value: 50.0,
        ..Default::default()
    };
    let flat = FlatGeometry {
        bounds: (10.0, 20.0, 110.0, 44.0),
        baseline: 40.0,
    };
    let (w, h) = (120usize, 60usize);
    let out = render_from_engine_data_warped(
        &store,
        &ed,
        TEXT,
        &[],
        &[],
        (0.0, 0.0),
        &warp,
        flat,
        target((w as u32, h as u32)),
    )
    .expect("arch render");
    let alpha = |x: usize, y: usize| out.pixels.rgba[(y * w + x) * 4 + 3];
    let top = (0..h)
        .find(|&y| (0..w).any(|x| alpha(x, y) > 0))
        .expect("arch ink");
    let mid = (0..w).filter(|&x| alpha(x, top) > 0);
    let (lo, hi) = (mid.clone().min().unwrap(), mid.max().unwrap());
    assert!(
        lo > w / 6 && hi < w * 5 / 6,
        "crown at row {top} spans {lo}..={hi}, expected the middle of {w} columns"
    );
}

#[test]
fn placement_needs_a_fitted_bend() {
    let flat = vec![0u8; 40 * 40 * 4];
    let surface = FlatSurface {
        origin: (0, 0),
        width: 40,
        height: 40,
    };
    let geom = FlatGeometry {
        bounds: (0.0, 0.0, 40.0, 40.0),
        baseline: 30.0,
    };
    let no_bend = Warp {
        style: Style::Arc,
        value: 0.0,
        ..Default::default()
    };
    assert!(placement(&flat, surface, geom, &no_bend).is_none());
    let other = Warp {
        style: Style::Other,
        value: 50.0,
        ..Default::default()
    };
    assert!(placement(&flat, surface, geom, &other).is_none());
    let arc = Warp {
        style: Style::Arc,
        value: 50.0,
        ..Default::default()
    };
    let p = placement(&flat, surface, geom, &arc).expect("arc placement");
    // The warp is a bicubic in the flat box: every mapped boundary point must
    // sit at or past the returned anchor on both axes.
    for (x, y) in [(0.0, 0.0), (40.0, 0.0), (0.0, 40.0), (40.0, 40.0)] {
        let (wx, wy) = p.forward(geom, x, y);
        assert!(wx >= p.min.0 - 1.0e-3 && wy >= p.min.1 - 1.0e-3);
    }
}

/// The style map is computed, not a constant patch: the arch grid at 50 %
/// bend and the box aspect Photoshop's layer 13 uses must be exactly the
/// closed form (`geom::warp::preset_grid`) engine-parity measured, whose
/// forward map tracks Photoshop's band centroids within ~1 px.
///
/// Pinned numbers: arch, b = +0.5, box 224.28 x 37.38 (h = 1/6).
#[test]
fn the_arch_map_is_the_closed_form() {
    let bounds = (0.0f32, 0.0, 224.28, 37.38);
    let warp = Warp {
        style: warp::Style::Arch,
        value: 50.0,
        perspective: 0.0,
        other: 0.0,
    };
    let grid = warp::map_for(&warp, bounds).expect("arch map");
    let expected = [
        [[0.0, 0.0], [0.0, 1.0 / 3.0], [0.0, 2.0 / 3.0], [0.0, 1.0]],
        [
            [0.276142, -1.656854],
            [0.276142, -1.323521],
            [0.276142, -0.990188],
            [0.276142, -0.656854],
        ],
        [
            [0.723858, -1.656854],
            [0.723858, -1.323521],
            [0.723858, -0.990188],
            [0.723858, -0.656854],
        ],
        [[1.0, 0.0], [1.0, 1.0 / 3.0], [1.0, 2.0 / 3.0], [1.0, 1.0]],
    ];
    for (row, want_row) in grid.iter().zip(expected) {
        for (point, want) in row.iter().zip(want_row) {
            assert!(
                (point[0] - want[0]).abs() < 1e-5 && (point[1] - want[1]).abs() < 1e-5,
                "arch control point {point:?} != {want:?}"
            );
        }
    }
    // Only the two inner columns move: the arch's vertical lines stay straight.
    for j in 0..4 {
        assert!((grid[0][j][0] - 0.0).abs() < 1e-6);
        assert!((grid[3][j][0] - 1.0).abs() < 1e-6);
        assert!((grid[2][j][0] - grid[1][j][0] - (0.723858 - 0.276142)).abs() < 1e-5);
    }
    // Forward samples engine-parity measured (box units; v = 0, 0.5, 1).
    let (w, h) = (bounds.2, bounds.3);
    let sample = |u: f32, v: f32| {
        let (x, y) = warp::forward(&grid, bounds, w * u, h * v);
        (x / w, y / h)
    };
    for (u, want) in [
        (0.25f32, [(0.233915, -0.931981), (0.233915, -0.431981), (0.233915, 0.068019)]),
        (0.5, [(0.5, -1.242641), (0.5, -0.742641), (0.5, -0.242641)]),
    ] {
        for (v, want) in [0.0f32, 0.5, 1.0].into_iter().zip(want) {
            let (x, y) = sample(u, v);
            assert!(
                (x - want.0).abs() < 1e-5 && (y - want.1).abs() < 1e-5,
                "arch forward ({u}, {v}) = ({x}, {y}) != {want:?}"
            );
        }
    }
}

/// `warpArc` keeps the per-layer fitted grid: engine-parity measured the
/// generic chord-normalised cylinder at `.8981` IoU on `orig_show.psd` layer 12
/// against `.9323` for the fitted `ARC_PATCH`, so only the arch style uses the
/// computed map.
#[test]
fn the_arc_style_keeps_the_fitted_grid() {
    let bounds = (0.0f32, 0.0, 204.78, 37.38);
    let arc = |value: f32| {
        warp::map_for(
            &Warp {
                style: Style::Arc,
                value,
                perspective: 0.0,
                other: 0.0,
            },
            bounds,
        )
        .expect("arc map")
    };
    // At the fitted bend the grid is the checked-in fit.
    let fitted = arc(50.0);
    for (point, want) in [
        (fitted[0][0], (-0.13648, 1.31338)),
        (fitted[1][0], (0.22029, -0.54010)),
        (fitted[3][3], (0.98153, 2.17508)),
    ] {
        assert!(
            (point[0] - want.0).abs() < 1e-4 && (point[1] - want.1).abs() < 1e-4,
            "fitted arc control point {point:?} != {want:?}"
        );
    }
    // The bend interpolation runs from the identity grid at 0 % to the fitted
    // grid at 50 %.
    let half = arc(25.0);
    for (i, row) in half.iter().enumerate() {
        for (j, point) in row.iter().enumerate() {
            let identity = [i as f32 / 3.0, j as f32 / 3.0];
            let want = [
                identity[0] + (fitted[i][j][0] - identity[0]) * 0.5,
                identity[1] + (fitted[i][j][1] - identity[1]) * 0.5,
            ];
            assert!(
                (point[0] - want[0]).abs() < 1e-5 && (point[1] - want[1]).abs() < 1e-5,
                "arc bend interpolation at ({i},{j}): {point:?} != {want:?}"
            );
        }
    }
}

/// Unwarped layers have no map, and a style without a ported map has none.
#[test]
fn unwarped_layers_have_no_map() {
    let flat = Warp {
        style: warp::Style::Arch,
        value: 0.0,
        perspective: 0.0,
        other: 0.0,
    };
    assert!(warp::map_for(&flat, (0.0, 0.0, 100.0, 20.0)).is_none());
    let unsupported = Warp {
        style: warp::Style::Other,
        value: 50.0,
        perspective: 0.0,
        other: 0.0,
    };
    assert!(warp::map_for(&unsupported, (0.0, 0.0, 100.0, 20.0)).is_none());
}
