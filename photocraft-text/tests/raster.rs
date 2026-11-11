//!
//! * `overlay_*` tests re-render `out/overlay/spec_{1,2,3}.json` and compare the
//!   alpha of our layer against Photoshop's `text{i}` type layer. They skip when
//!   the overlay files or the font are absent.
//! * The synthetic tests need no external files.
//!
//! Reproduce the measurement table:
//! `cargo test -p photocraft-text --test raster -- --nocapture overlay_sweep`

use photocraft_psd::PsdFile;
use photocraft_text::area::{first_baseline_ratio, layout_area, Justification, ParagraphStyle};
use photocraft_text::font::Face;
use photocraft_text::layout::TextStyle;
use photocraft_text::outline::{glyph_contours, Contours, Point};
use photocraft_text::raster::{AlphaCurve, Canvas, Coverage};
use std::path::{Path, PathBuf};

const W: u32 = 1200;
const H: u32 = 800;
const FONT: &str = "/media/H/anime-set/text_detect/fontas/v_CCWildButNotCrazy.ttf";

struct Spec {
    text: String,
    size: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    justification: String,
    tracking: i16,
    leading: f32,
}

fn load_spec(path: &Path) -> Spec {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).expect("read spec")).expect("parse spec");
    let num = |k: &str, d: f32| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(d);
    Spec {
        text: v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        size: num("size", 36.0),
        x: num("x", 0.0),
        y: num("y", 0.0),
        width: num("width", 600.0),
        height: num("height", 200.0),
        justification: v.get("justification").and_then(|x| x.as_str()).unwrap_or("left").to_string(),
        tracking: num("tracking", 0.0) as i16,
        leading: num("leading", 0.0),
    }
}

fn parse_just(s: &str) -> Justification {
    match s {
        "center" => Justification::Center,
        "right" => Justification::Right,
        "justify" => Justification::JustifyLastLeft,
        _ => Justification::Left,
    }
}

/// Alpha plane of a named PSD layer placed on a W x H canvas.
fn alpha_plane(psd: &PsdFile, name: &str) -> Vec<u8> {
    let mut plane = vec![0u8; (W * H) as usize];
    let layer = psd.iter_layers().find(|l| l.name() == name).unwrap_or_else(|| panic!("layer {name} missing"));
    let img = layer.rgba8().expect("decode layer");
    for y in 0..img.height as i32 {
        for x in 0..img.width as i32 {
            let (cx, cy) = (img.left + x, img.top + y);
            if cx < 0 || cy < 0 || cx >= W as i32 || cy >= H as i32 {
                continue;
            }
            let src = ((y as u32 * img.width + x as u32) * 4 + 3) as usize;
            plane[(cy as u32 * W + cx as u32) as usize] = img.data[src];
        }
    }
    plane
}

/// Re-renders a spec with the given rasteriser settings; returns the alpha plane.
fn render(face: &Face<'_>, spec: &Spec, coverage: Coverage, curve: AlphaCurve, snap: bool) -> Vec<u8> {
    render_shifted(face, spec, coverage, curve, snap, (0.0, 0.0))
}

/// As [`render`], with every glyph origin moved by `shift` device pixels.
fn render_shifted(face: &Face<'_>, spec: &Spec, coverage: Coverage, curve: AlphaCurve, snap: bool, shift: (f32, f32)) -> Vec<u8> {
    let upm = face.units_per_em() as f32;
    let ascender = first_baseline_ratio(face);
    let style = TextStyle {
        size: spec.size,
        tracking: spec.tracking,
        leading: Some(if spec.leading > 0.0 { spec.leading } else { spec.size * 1.2 }),
        ..Default::default()
    };
    let para = ParagraphStyle { justification: parse_just(&spec.justification), ..Default::default() };
    let layout = layout_area(face, &spec.text, &style, &para, spec.width, spec.height, (spec.x, spec.y), ascender);
    let mut canvas = Canvas::new(W, H);
    canvas.coverage = coverage;
    canvas.curve = curve;
    canvas.snap = snap;
    let first_baseline = layout.lines.first().map(|l| l.baseline).unwrap_or(0.0);
    for line in &layout.lines {
        let screen_baseline = spec.y + ascender * spec.size + (first_baseline - line.baseline);
        for g in &line.glyphs {
            let contours = glyph_contours(face, g.gid).expect("outline");
            if contours.is_empty() {
                continue;
            }
            canvas.mask_contours(&contours, spec.size / upm, g.x + shift.0, screen_baseline + shift.1, [0, 0, 0], 1.0);
        }
    }
    canvas.pixels.chunks_exact(4).map(|p| p[3]).collect()
}

/// (mean |Δ| over all pixels, count of pixels with |Δ| > 32).
fn score(ours: &[u8], reference: &[u8]) -> (f64, usize) {
    let mut sum = 0u64;
    let mut over = 0usize;
    for (a, b) in ours.iter().zip(reference) {
        let d = (*a as i32 - *b as i32).unsigned_abs();
        sum += d as u64;
        if d > 32 {
            over += 1;
        }
    }
    (sum as f64 / ours.len() as f64, over)
}

struct Pair {
    spec: Spec,
    photoshop: Vec<u8>,
    rust_layer: Vec<u8>,
}

/// Loads overlay pairs 1..=3, or `None` when inputs are absent.
fn load_pairs() -> Option<Vec<Pair>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/overlay");
    if !Path::new(FONT).exists() {
        eprintln!("skip: font {FONT} absent");
        return None;
    }
    let mut pairs = Vec::new();
    for i in 1..=3 {
        let spec_path = dir.join(format!("spec_{i}.json"));
        let psd_path = dir.join(format!("overlay_{i}.psd"));
        if !spec_path.exists() || !psd_path.exists() {
            eprintln!("skip: {} or {} absent", spec_path.display(), psd_path.display());
            return None;
        }
        let psd = PsdFile::open(&psd_path).expect("io").expect("parse psd");
        pairs.push(Pair {
            spec: load_spec(&spec_path),
            photoshop: alpha_plane(&psd, &format!("text{i}")),
            rust_layer: alpha_plane(&psd, "rust_render"),
        });
    }
    Some(pairs)
}

fn configs() -> Vec<(String, Coverage, AlphaCurve, bool)> {
    let mut out = Vec::new();
    for snap in [false, true] {
        for coverage in [Coverage::Scanlines(4), Coverage::Scanlines(16), Coverage::Area] {
            let mut curves = vec![AlphaCurve::Linear, AlphaCurve::Srgb];
            for g in [0.4f32, 0.5, 0.6, 0.7, 0.8, 0.9, 1.1, 1.2] {
                curves.push(AlphaCurve::Power(g));
            }
            for curve in curves {
                out.push((format!("{coverage:?} {curve:?} snap={snap}"), coverage, curve, snap));
            }
        }
    }
    out
}

/// Re-render of overlay spec with the current Canvas defaults must match the
/// `rust_render` layer produced by the pipeline that built the overlay.
#[test]
fn overlay_baseline_reproduces_rust_layer() {
    let Some(pairs) = load_pairs() else { return };
    let font = std::fs::read(FONT).expect("font");
    let face = Face::parse(&font).expect("face");
    for (i, p) in pairs.iter().enumerate() {
        let ours = render(&face, &p.spec, Coverage::Scanlines(4), AlphaCurve::Linear, false);
        let (mean, over) = score(&ours, &p.rust_layer);
        println!("pair {} rust_layer vs re-render: mean {mean:.3} over32 {over}", i + 1);
    }
}

/// Sweeps rasteriser configurations against Photoshop's type-layer alpha.
/// Asserts the current defaults are no worse than the original 4-scanline linear baseline.
#[test]
fn overlay_sweep() {
    let Some(pairs) = load_pairs() else { return };
    let font = std::fs::read(FONT).expect("font");
    let face = Face::parse(&font).expect("face");
    let defaults = Canvas::new(1, 1);
    let default_cfg = (defaults.coverage, defaults.curve, defaults.snap);

    let baseline: Vec<(f64, usize)> = pairs
        .iter()
        .map(|p| score(&render(&face, &p.spec, Coverage::Scanlines(4), AlphaCurve::Linear, false), &p.photoshop))
        .collect();
    println!("baseline Scanlines(4) Linear snap=false:");
    for (i, (m, o)) in baseline.iter().enumerate() {
        println!("  pair {}: mean {m:.3} over32 {o}", i + 1);
    }

    for (name, coverage, curve, snap) in configs() {
        let mut line = format!("{name:<45}");
        for p in &pairs {
            let (m, o) = score(&render(&face, &p.spec, coverage, curve, snap), &p.photoshop);
            line.push_str(&format!(" | {m:6.3} {o:6}"));
        }
        println!("{line}");
    }

    let default_scores: Vec<(f64, usize)> = pairs
        .iter()
        .map(|p| score(&render(&face, &p.spec, default_cfg.0, default_cfg.1, default_cfg.2), &p.photoshop))
        .collect();
    println!("defaults {default_cfg:?}:");
    for (i, ((dm, _), (bm, _))) in default_scores.iter().zip(&baseline).enumerate() {
        println!("  pair {}: default mean {dm:.3} baseline mean {bm:.3}", i + 1);
        assert!(dm <= bm, "pair {}: default worse than baseline ({dm} > {bm})", i + 1);
    }
}

fn pts(list: &[(f32, f32, bool)]) -> Contours {
    vec![list.iter().map(|&(x, y, on)| Point { x, y, on }).collect()]
}

fn alpha_of(contours: &Contours, coverage: Coverage) -> Vec<u8> {
    let mut canvas = Canvas::new(200, 200);
    canvas.coverage = coverage;
    canvas.mask_contours(contours, 1.0, 20.0, 180.0, [0, 0, 0], 1.0);
    canvas.pixels.chunks_exact(4).map(|p| p[3]).collect()
}

fn alpha_at(plane: &[u8], x: u32, y: u32) -> u8 {
    plane[(y * 200 + x) as usize]
}

/// A closed contour with quadratic off-curve points must render identically
/// regardless of which point it starts from, including off-curve starts.
#[test]
fn off_curve_start_is_rotation_invariant() {
    let base = [
        (0.0, 0.0, true),
        (50.0, -30.0, false),
        (100.0, 0.0, true),
        (130.0, 50.0, false),
        (100.0, 100.0, true),
        (0.0, 100.0, true),
    ];
    let all_off = [(0.0, 0.0, false), (100.0, 0.0, false), (100.0, 100.0, false), (0.0, 100.0, false)];
    for list in [&base[..], &all_off[..]] {
        for coverage in [Coverage::Scanlines(4), Coverage::Area] {
            let reference = alpha_of(&pts(list), coverage);
            assert!(reference.iter().any(|&a| a == 255), "shape must be filled");
            for k in 1..list.len() {
                let mut rot = list.to_vec();
                rot.rotate_left(k);
                let got = alpha_of(&pts(&rot), coverage);
                let max_diff = reference.iter().zip(&got).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
                assert!(max_diff <= 2, "rotation {k} with {coverage:?}: max diff {max_diff}");
            }
        }
    }
}

/// Two same-direction overlapping squares fill their union at full coverage
/// (no double-counting above 1); opposite-direction overlap is a hole under the
/// non-zero rule.
#[test]
fn overlapping_contours_follow_nonzero_rule() {
    let square = |x0: f32, y0: f32, x1: f32, y1: f32, cw: bool| {
        let mut v = vec![(x0, y0, true), (x1, y0, true), (x1, y1, true), (x0, y1, true)];
        if !cw {
            v.reverse();
        }
        v
    };
    for coverage in [Coverage::Scanlines(4), Coverage::Area] {
        let same: Contours = vec![
            square(10.0, 10.0, 60.0, 60.0, true).iter().map(|&(x, y, on)| Point { x, y, on }).collect(),
            square(40.0, 40.0, 90.0, 90.0, true).iter().map(|&(x, y, on)| Point { x, y, on }).collect(),
        ];
        let plane = alpha_of(&same, coverage);
        // Device x = 20 + x, y = 180 - y. A -> x 30..80, rows 120..170; B -> x 60..110, rows 90..140.
        assert_eq!(alpha_at(&plane, 70, 130), 255, "overlap {coverage:?}");
        assert_eq!(alpha_at(&plane, 35, 165), 255, "A only {coverage:?}");
        assert_eq!(alpha_at(&plane, 100, 100), 255, "B only {coverage:?}");
        assert_eq!(alpha_at(&plane, 100, 150), 0, "outside {coverage:?}");

        let opposite: Contours = vec![
            square(10.0, 10.0, 60.0, 60.0, true).iter().map(|&(x, y, on)| Point { x, y, on }).collect(),
            square(40.0, 40.0, 90.0, 90.0, false).iter().map(|&(x, y, on)| Point { x, y, on }).collect(),
        ];
        let plane = alpha_of(&opposite, coverage);
        assert_eq!(alpha_at(&plane, 70, 130), 0, "opposite-winding overlap is a hole {coverage:?}");
        assert_eq!(alpha_at(&plane, 35, 165), 255, "A only {coverage:?}");
    }
}

/// Finds the glyph-origin shift (1/8 px grid over [-1, 1]) that best matches
/// Photoshop per pair, with snapping off. A best shift at a whole-pixel offset
/// means PS rounds the origin; a fractional best shift means it does not.
#[test]
fn overlay_phase_search() {
    let Some(pairs) = load_pairs() else { return };
    let font = std::fs::read(FONT).expect("font");
    let face = Face::parse(&font).expect("face");
    for (i, p) in pairs.iter().enumerate() {
        let mut best = (f64::MAX, 0usize, (0.0f32, 0.0f32));
        for iy in -8i32..=8 {
            for ix in -8i32..=8 {
                let shift = (ix as f32 / 8.0, iy as f32 / 8.0);
                let ours = render_shifted(&face, &p.spec, Coverage::Scanlines(4), AlphaCurve::Linear, false, shift);
                let (m, o) = score(&ours, &p.photoshop);
                if m < best.0 {
                    best = (m, o, shift);
                }
            }
        }
        println!("pair {}: best shift {:?} mean {:.3} over32 {}", i + 1, best.2, best.0, best.1);
    }
}

/// Ink bounding box and centroid of an alpha plane (alpha > 127).
fn ink_stats(plane: &[u8]) -> (u32, u32, u32, u32, f64, f64) {
    let (mut x0, mut y0, mut x1, mut y1) = (W, H, 0u32, 0u32);
    let (mut sx, mut sy, mut n) = (0f64, 0f64, 0f64);
    for (i, &a) in plane.iter().enumerate() {
        if a > 127 {
            let (x, y) = (i as u32 % W, i as u32 / W);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            sx += x as f64;
            sy += y as f64;
            n += 1.0;
        }
    }
    (x0, y0, x1, y1, sx / n, sy / n)
}

#[test]
fn overlay_ink_geometry() {
    let Some(pairs) = load_pairs() else { return };
    let font = std::fs::read(FONT).expect("font");
    let face = Face::parse(&font).expect("face");
    for (i, p) in pairs.iter().enumerate() {
        let ours = render(&face, &p.spec, Coverage::Scanlines(4), AlphaCurve::Linear, false);
        println!("pair {} PS   bbox/centroid {:?}", i + 1, ink_stats(&p.photoshop));
        println!("pair {} ours bbox/centroid {:?}", i + 1, ink_stats(&ours));
    }
}

/// Every glyph in the reference font must parse, including composites
/// (`numberOfContours < 0`), and yield closed, non-empty outlines where it has ink.
#[test]
fn all_glyphs_parse() {
    if !Path::new(FONT).exists() {
        eprintln!("skip: font {FONT} absent");
        return;
    }
    let font = std::fs::read(FONT).expect("font");
    let face = Face::parse(&font).expect("face");
    let mut composites = 0usize;
    for gid in 0..face.num_glyphs() {
        let contours = glyph_contours(&face, gid).unwrap_or_else(|e| panic!("gid {gid}: {e:?}"));
        for c in &contours {
            assert!(c.len() >= 2 || c.is_empty(), "gid {gid}: degenerate contour");
        }
        if !contours.is_empty() && contours.len() > 1 {
            composites += 1;
        }
    }
    println!("glyphs {} multi-contour {composites}", face.num_glyphs());
}

fn rgba_at(canvas: &Canvas, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * canvas.width + x) * 4) as usize;
    [canvas.pixels[i], canvas.pixels[i + 1], canvas.pixels[i + 2], canvas.pixels[i + 3]]
}

/// Unit square (font units == device pixels) with its bottom-left at (20, 180) on a 200x200 canvas.
fn square(w: f32, h: f32) -> Contours {
    pts(&[(0.0, 0.0, true), (w, 0.0, true), (w, h, true), (0.0, h, true)])
}

/// `sy = 2` doubles the glyph height in y, leaves the left edge where it was, and keeps the baseline.
#[test]
fn scaled_vertical_stretch_doubles_height_not_left_edge() {
    let c = square(10.0, 10.0);
    let mut base = Canvas::new(200, 200);
    base.mask_contours(&c, 1.0, 20.0, 180.0, [0, 0, 0], 1.0);
    let mut stretched = Canvas::new(200, 200);
    stretched.mask_contours_scaled(&c, 1.0, 2.0, 20.0, 180.0, [0, 0, 0], 1.0);

    let (bl, bt, br, bb) = base.ink_bounds();
    let (sl, st, sr, sb) = stretched.ink_bounds();
    assert_eq!((bl, bt, br, bb), (20, 170, 30, 180));
    assert_eq!(sl, bl, "left edge moved");
    assert_eq!(sr, br, "width changed");
    assert_eq!(sb, bb, "baseline moved");
    assert_eq!(sb - st, 2 * (bb - bt), "height is not doubled");
}

/// `mask_contours` is `mask_contours_scaled` with `sx == sy == scale`.
#[test]
fn uniform_scale_matches_scaled_path() {
    let c = square(10.0, 10.0);
    let mut a = Canvas::new(200, 200);
    a.mask_contours(&c, 1.5, 20.0, 180.0, [10, 20, 30], 0.75);
    let mut b = Canvas::new(200, 200);
    b.mask_contours_transformed(&c, 1.5, 1.5, 0.0, 20.0, 180.0, [10, 20, 30], 0.75);
    assert_eq!(a.pixels, b.pixels);
}

/// Skew shifts each row by `skew * py`: the top row sits `skew * height` right of the bottom row.
#[test]
fn skew_turns_rectangle_into_parallelogram() {
    let height = 20.0f32;
    let skew = 1.0f32;
    let c = square(10.0, height);
    let mut canvas = Canvas::new(200, 200);
    canvas.mask_contours_transformed(&c, 1.0, 1.0, skew, 20.0, 180.0, [0, 0, 0], 1.0);

    // Alpha-weighted centre of a row.
    let centre = |y: u32| {
        let (mut sum, mut wsum) = (0.0f64, 0.0f64);
        for x in 0..200u32 {
            let a = rgba_at(&canvas, x, y)[3] as f64 / 255.0;
            sum += a * (x as f64 + 0.5);
            wsum += a;
        }
        assert!(wsum > 0.0, "row {y} empty");
        sum / wsum
    };
    // Row 160 spans py 19..20 (top); row 179 spans py 0..1 (bottom).
    let top = centre(160);
    let bottom = centre(179);
    let shift = top - bottom;
    // Row centres are 0.5 px inside the top (py=20) and bottom (py=0) edges.
    let expected = skew as f64 * (height as f64 - 1.0);
    assert!((shift - expected).abs() < 0.5, "top-bottom centre shift {shift}, expected {expected}");
    // Bottom row samples py 0..1, so its centre is 25 + skew * 0.5.
    assert!((bottom - (25.0 + skew as f64 * 0.5)).abs() < 0.5, "bottom centre {bottom}");
}

/// Glyphs in different colours composite over each other without bleeding:
/// the first glyph keeps its colour where the second does not reach, and the
/// second glyph's colour wins where it overlaps at full alpha.
#[test]
fn per_glyph_colours_do_not_bleed() {
    let mut canvas = Canvas::new(200, 200);
    canvas.mask_contours(&square(10.0, 10.0), 1.0, 20.0, 180.0, [255, 0, 0], 1.0);
    canvas.mask_contours(&square(10.0, 10.0), 1.0, 25.0, 180.0, [0, 0, 255], 1.0);

    assert_eq!(rgba_at(&canvas, 21, 175), [255, 0, 0, 255], "first glyph lost its colour");
    assert_eq!(rgba_at(&canvas, 27, 175), [0, 0, 255, 255], "overlap should show second colour");
    assert_eq!(rgba_at(&canvas, 33, 175), [0, 0, 255, 255], "second glyph colour wrong");
    assert_eq!(rgba_at(&canvas, 36, 175), [0, 0, 0, 0], "ink outside both glyphs");
}

/// An underline covers exactly its x range and the thickness rows, and touches no other row.
#[test]
fn underline_spans_x_range_and_thickness_only() {
    let mut canvas = Canvas::new(200, 200);
    // baseline 50, offset 3 -> bar occupies rows 53 and 54 (thickness 2).
    canvas.underline(10.0, 30.0, 50.0, 2.0, 3.0, [255, 255, 255], 1.0);

    for y in [52u32, 55, 56, 49] {
        for x in 0..200u32 {
            assert_eq!(rgba_at(&canvas, x, y)[3], 0, "row {y} touched");
        }
    }
    for y in [53u32, 54] {
        assert_eq!(rgba_at(&canvas, 9, y)[3], 0, "column 9 touched");
        assert_eq!(rgba_at(&canvas, 30, y)[3], 0, "column 30 touched");
        for x in 10..30u32 {
            assert_eq!(rgba_at(&canvas, x, y), [255, 255, 255, 255], "gap at ({x},{y})");
        }
    }
}

/// Fractional top and bottom edges give partial coverage on those rows only.
#[test]
fn underline_antialiases_top_and_bottom_rows() {
    let mut canvas = Canvas::new(200, 200);
    canvas.underline(10.0, 30.0, 50.0, 2.0, 3.5, [255, 255, 255], 1.0);
    // Bar spans 53.5..55.5: row 53 half, row 54 full, row 55 half.
    assert_eq!(rgba_at(&canvas, 15, 53)[3], 128);
    assert_eq!(rgba_at(&canvas, 15, 54)[3], 255);
    assert_eq!(rgba_at(&canvas, 15, 55)[3], 128);
    assert_eq!(rgba_at(&canvas, 15, 52)[3], 0);
    assert_eq!(rgba_at(&canvas, 15, 56)[3], 0);
}

#[test]
fn grid_coverage_counts_point_samples() {
    let full = pts(&[(0.0, 0.0, true), (10.0, 0.0, true), (10.0, 10.0, true), (0.0, 10.0, true), (0.0, 0.0, true)]);
    for coverage in [Coverage::Grid(16), Coverage::Grid(15), Coverage::Area, Coverage::Scanlines(4)] {
        let plane = alpha_of(&full, coverage);
        assert_eq!(alpha_at(&plane, 25, 175), 255, "{coverage:?} interior");
        assert_eq!(alpha_at(&plane, 15, 175), 0, "{coverage:?} outside");
    }
    // Half-pixel stem: every integrator must land on ~50 % alpha.
    let half = pts(&[(0.0, 0.0, true), (0.5, 0.0, true), (0.5, 10.0, true), (0.0, 10.0, true), (0.0, 0.0, true)]);
    for coverage in [Coverage::Grid(16), Coverage::Grid(15), Coverage::Area, Coverage::Scanlines(4)] {
        let plane = alpha_of(&half, coverage);
        // Odd grids straddle the edge (7/15 or 8/15 samples), then Sharp's
        // 15-step quantisation rounds; both stay within a couple of levels.
        let a = alpha_at(&plane, 20, 175);
        assert!((110..=140).contains(&a), "{coverage:?} half stem {a}");
    }
}
