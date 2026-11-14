use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use photocraft_text::Face;
use photocraft_text::atlas::Atlas;
use photocraft_text::engine_data;
use photocraft_text::layer_render::{self, FaceStore, LayerPixels, LayerTextStyle, RenderOptions};
use photocraft_text::layout::TextStyle;
use photocraft_text::outline::glyph_contours;
use photocraft_text::psd::LayerSpec;
use photocraft_text::raster::Canvas;
use photocraft_text::runs::RunStyle;

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    spec: LayerSpec,
}

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

fn check_bytes(name: &str, actual: &[u8]) {
    let path = assets().join("goldens").join(name);
    let expected = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("required baseline {}: {error}", path.display()));
    assert!(actual == expected, "{}: byte mismatch", path.display());
}

fn check_pixels(name: &str, pixels: &LayerPixels) {
    assert_eq!(
        pixels.rgba.len(),
        pixels.width as usize * pixels.height as usize * 4
    );
    assert!(pixels.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
    let geometry: Vec<u8> = [
        pixels.width,
        pixels.height,
        pixels.offset.0.to_bits(),
        pixels.offset.1.to_bits(),
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    check_bytes(&format!("{name}.pixels.bin"), &geometry);
    check_bytes(&format!("{name}.rgba.bin"), &pixels.rgba);
}

fn measure<T>(name: &str, iterations: u32, mut render: impl FnMut() -> T) {
    const SAMPLES: usize = 7;
    for _ in 0..iterations {
        drop(black_box(render()));
    }
    let mut samples = [0u128; SAMPLES];
    for sample in &mut samples {
        let start = Instant::now();
        for _ in 0..iterations {
            drop(black_box(render()));
        }
        *sample = start.elapsed().as_nanos();
    }
    samples.sort_unstable();
    println!(
        "{name}: median {:.0} ns/op ({SAMPLES} samples x {iterations} iterations)",
        samples[SAMPLES / 2] as f64 / f64::from(iterations)
    );
}

fn main() {
    for (key, _) in std::env::vars_os() {
        assert!(
            !key.to_string_lossy().starts_with("PHOTOCRAFT_"),
            "benchmark requires no PHOTOCRAFT_* overrides: {key:?}"
        );
    }
    // Cargo supplies --bench to harness=false executables.
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|arg| arg != "--bench")
        .collect();
    assert!(
        filters.len() <= 1,
        "usage: cargo bench --bench render -- [case substring]"
    );
    let selected = |name: &str| filters.first().is_none_or(|filter| name.contains(filter));
    let names = [
        "point_plain",
        "area_paragraphs",
        "area_composer",
        "point_styles",
        "atlas_phases",
        "raster_glyph_B48",
        "ink_bounds",
    ];
    assert!(
        names.iter().any(|name| selected(name)),
        "unknown benchmark filter: {filters:?}"
    );

    let cases: Vec<Case> = serde_json::from_str(include_str!("../tests/assets/cases.json"))
        .expect("checked-in render cases");
    let mut store = FaceStore::new();
    for name in ["NotoSans-Regular", "NotoSans-Bold"] {
        store
            .load(&assets().join("fonts"), name)
            .expect("checked-in font");
    }
    for name in &names[..4] {
        if !selected(name) {
            continue;
        }
        let case = cases
            .iter()
            .find(|case| case.name == *name)
            .expect("named render case");
        let raw = std::fs::read(assets().join("models").join(format!("{name}.engine.bin")))
            .expect("checked-in EngineData input");
        let model = engine_data::parse(&raw).expect("EngineData parse");
        let text = case.spec.text.replace("\r\n", "\r").replace('\n', "\r");
        let text = text.trim_end_matches(['\r', '\n']);
        let size = case
            .spec
            .frame
            .map(|[l, t, r, b]| (r - l, b - t))
            .unwrap_or((0.0, 0.0));
        let render = || {
            layer_render::render_from_engine_data(
                black_box(&store),
                black_box(&model),
                black_box(text),
                black_box(size),
                black_box((0.25, 1.5)),
            )
            .expect("EngineData render")
        };
        check_pixels(name, &render());
        measure(name, if *name == "point_plain" { 100 } else { 50 }, render);
    }

    if selected("atlas_phases") {
        let mut atlases = Vec::new();
        for (phase, varying) in [(0.5, false), (0.25, true), (0.0, false), (0.75, true)] {
            let mut atlas =
                Atlas::load(&assets().join(if varying { "atlas/vary" } else { "atlas" }))
                    .expect("checked-in atlas");
            atlas.phase = phase;
            atlases.push(atlas);
        }
        store.set_atlases(atlases);
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
        let options = RenderOptions {
            use_atlas: true,
            ..Default::default()
        };
        let render = || {
            black_box(&store)
                .render_spec_with(
                    black_box("Hamburgefonstiv AVTRy."),
                    black_box(&runs),
                    black_box(&spec),
                    black_box(options),
                )
                .expect("phased atlas render")
        };
        check_pixels("atlas_phases", &render());
        measure("atlas_phases", 100, render);
    }

    if selected("raster_glyph_B48") {
        let font = std::fs::read(assets().join("fonts/NotoSans-Regular.ttf"))
            .expect("checked-in raster font");
        let face = Face::parse(&font).expect("raster font parse");
        let contours = glyph_contours(&face, face.glyph_index('B').expect("glyph B"))
            .expect("glyph B contours");
        let scale = 48.0 / f32::from(face.head.units_per_em);
        let render = || {
            let mut canvas = Canvas::new(black_box(64), black_box(64));
            canvas.mask_contours(
                black_box(&contours),
                black_box(scale),
                black_box(8.0),
                black_box(52.0),
                black_box([25, 70, 130]),
                black_box(1.0),
            );
            canvas
        };
        let pixels = render();
        assert_ne!(pixels.ink_bounds(), (0, 0, 0, 0), "glyph B has no ink");
        assert_eq!(
            pixels.pixels,
            render().pixels,
            "raster must be deterministic"
        );
        let checksum = pixels
            .pixels
            .iter()
            .fold(0xcbf29ce484222325u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            });
        println!("raster_glyph_B48: RGBA FNV-1a {checksum:016x} (64x64)");
        measure("raster_glyph_B48", 200, render);
    }
    if selected("ink_bounds") {
        let mut canvas = Canvas::new(1024, 256);
        for y in 64..192usize {
            for x in 128..896usize {
                canvas.pixels[(y * 1024 + x) * 4 + 3] = 255;
            }
        }
        assert_eq!(canvas.ink_bounds(), (128, 64, 896, 192));
        measure("ink_bounds", 200, || black_box(&canvas).ink_bounds());
    }
}
