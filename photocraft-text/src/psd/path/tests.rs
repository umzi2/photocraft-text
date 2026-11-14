//! Ported from the fork's `crates/text/src/psd_path/tests.rs`: the fixture
//! frames come from Photoshop 2026 and ag-psd's MIT test corpus, so the
//! expected values are measured, not invented.

use super::*;

fn frame(src: &str) -> E {
    crate::engine_data::parse(src.as_bytes()).unwrap()
}

fn read(src: &str) -> Option<ShapeSpec> {
    let frame = frame(src);
    read_shape(FrameSource::EngineData(&frame))
}

fn read_path(src: &str) -> PathSpec {
    read(src).unwrap_or_else(|| panic!("unreadable frame {src:.24}")).path.expect("path")
}

/// The frame of `text/path-circle.psd` (Photoshop 2026).
const CIRCLE: &str = "<< /1 << /0 [ 32.0 128.0 32.0 74.98067 74.98067 32.0 128.0 32.0 128.0 32.0 181.01933 32.0 224.0 74.98067 224.0 128.0 224.0 128.0 224.0 181.01933 181.01933 224.0 128.0 224.0 128.0 224.0 74.98067 224.0 32.0 181.01933 32.0 128.0 ] >> /2 << /0 2 /2 [ 1.0 0.0 0.0 1.0 -32.0 -32.0 ] /6 [ -1.0 -1.0 ] >> >>";
/// The frame of `text/path-wave-open.psd` (Photoshop 2026).
const WAVE: &str = "<< /1 << /0 [ 16.0 100.0 46.0 40.0 120.0 40.0 160.0 80.0 160.0 80.0 200.0 120.0 274.0 100.0 304.0 60.0 ] >> /2 << /0 2 /2 [ 1.0 0.0 0.0 1.0 -16.0 -52.12246 ] /6 [ -1.0 -1.0 ] >> >>";
/// ag-psd's `test/read/text-path/src.psd` (CS6, MIT): "some text" with its end
/// a third into the first segment, so only "so" is drawn.
const AG_SHORT: &str = "<< /1 << /0 [ 0.0 10.17448 30.0 -13.99219 63.5 5.17448 83.0 13.17448 83.0 13.17448 102.5 21.17448 138.0 18.67448 150.0 3.17448 ] >> /2 << /0 2 /6 [ 0.0 0.32983 ] /11 << /3 0 /5 0.0 /7 false >> >> >>";
/// ag-psd's `test/read/text-path-26/src.psd` (2024, MIT): the start moved, the
/// end at the path's end.
const AG_MOVED: &str = "<< /1 << /0 [ 438.0 529.0 438.0 529.0 632.0 -39.0 790.0 545.0 790.0 545.0 948.0 1129.0 1564.0 987.0 1684.0 605.0 ] >> /2 << /0 2 /2 [ 1.0 0.0 0.0 1.0 -438.0 -281.23035 ] /6 [ 0.07689 2.0 ] /11 << /3 0 /5 0.0 >> >> >>";

fn close(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() <= 1e-3 && (a[1] - b[1]).abs() <= 1e-3
}

fn dist(a: Pt, b: Pt) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn frame_point(src: &str, i: usize, t: f64) -> Pt {
    let frame = frame(src);
    let (segs, _) = segments(FrameSource::EngineData(&frame)).expect("segments");
    cubic(&segs[i], t)
}

/// The point `arc` px along `path`, measured with the same flattening the
/// reader uses.
fn sample(path: &PathSpec, arc: f64) -> Pt {
    let sub = Sub::from_path(path);
    let segs = sub.segments();
    let measure = Measure::new(&segs);
    let p = measure.position(arc);
    let i = (p.floor() as usize).min(segs.len() - 1);
    cubic(&segs[i], p - i as f64)
}

/// The point at placement `arc` of `path`, honoring its flip flag the way the
/// type is laid out: flipped type measures from the path's end.
fn place(path: &PathSpec, arc: f64) -> Pt {
    let sub = Sub::from_path(path);
    let measure = Measure::new(&sub.segments());
    let length = measure.length();
    sample(path, if path.flip { length - arc } else { arc })
}

fn shape_of(path: &PathSpec, start: f32, end: Option<f32>, flip: bool) -> ShapeSpec {
    ShapeSpec {
        frame: None,
        path: Some(PathSpec {
            knots: path.knots.clone(),
            closed: path.closed,
            start,
            end,
            flip,
            matrix: None,
            kind: Some(if path.closed { "box".into() } else { "path".into() }),
        }),
    }
}

fn same_frame(a: &txt2::FramePath, b: &txt2::FramePath) -> bool {
    a.kind == b.kind
        && (a.range[0] - b.range[0]).abs() <= 0.01
        && (a.range[1] - b.range[1]).abs() <= 0.01
        && a.flip == b.flip
        && a.matrix == b.matrix
        && a.points.len() == b.points.len()
        && a.points.iter().zip(&b.points).all(|(x, y)| (x - y).abs() <= 0.01)
}

fn same_knots(a: &[PathKnot], b: &[PathKnot]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            close(x.anchor, y.anchor)
                && close(x.in_ctrl, y.in_ctrl)
                && close(x.out_ctrl, y.out_ctrl)
        })
}

#[test]
fn photoshop_path_frames_read_into_text_space() {
    let wave = read_path(WAVE);
    assert!(!wave.closed);
    assert_eq!(wave.knots.len(), 3);
    let p = |x: f32, y: f32| [x - 16.0, y - 52.12246];
    let k = &wave.knots;
    assert!(
        close(k[0].anchor, p(16.0, 100.0))
            && close(k[0].in_ctrl, p(16.0, 100.0))
            && close(k[0].out_ctrl, p(46.0, 40.0))
    );
    assert!(
        close(k[1].in_ctrl, p(120.0, 40.0))
            && close(k[1].anchor, p(160.0, 80.0))
            && close(k[1].out_ctrl, p(200.0, 120.0))
            && k[1].smooth == Some(true)
    );
    assert!(close(k[2].in_ctrl, p(274.0, 100.0)) && close(k[2].anchor, p(304.0, 60.0)));

    let circle = read_path(CIRCLE);
    assert!(circle.closed);
    assert_eq!(circle.knots.len(), 4);
    // The type starts at the bottom anchor (Photoshop's glyph 0) and runs to
    // the left one.
    assert!(close(circle.knots[0].anchor, [96.0, 192.0]));
    assert!(close(circle.knots[1].anchor, [0.0, 96.0]));
    assert!(circle.knots.iter().all(|k| k.smooth == Some(true)));
}

#[test]
fn reference_ranges_read_in_segment_units() {
    let short = read_path(AG_SHORT);
    assert!(!short.closed && short.knots.len() == 3, "the whole path: {short:?}");
    assert_eq!(short.start, 0.0);
    let end = f64::from(short.end.expect("explicit end"));
    let want = frame_point(AG_SHORT, 0, 0.32983);
    assert!(dist(sample(&short, end), want) < 0.5, "{:?} vs {want:?}", sample(&short, end));
    // About 30 px: "so" fits at 30 pt, "som" does not.
    assert!((28.0..34.0).contains(&end), "{end}");

    let moved = read_path(AG_MOVED);
    // The file's end (`2.0`) is the path's end, kept as an explicit end.
    let frame = frame(AG_MOVED);
    let (segs, _) = segments(FrameSource::EngineData(&frame)).expect("segments");
    let want_end = Measure::new(&segs).length();
    let moved_end = f64::from(moved.end.expect("explicit end"));
    assert!((moved_end - want_end).abs() < 0.01, "{moved_end} vs {want_end}");
    assert!(!moved.closed);
    let want = frame_point(AG_MOVED, 0, 0.07689);
    assert!(dist(sample(&moved, f64::from(moved.start)), want) < 0.5);

    // The default range reads as before.
    for src in [CIRCLE, WAVE] {
        let shape = read_path(src);
        assert_eq!(shape.start, 0.0, "{src:.12}");
        assert_eq!(shape.end, None, "{src:.12}");
        assert!(!shape.flip, "{src:.12}");
    }
}

#[test]
fn a_reversed_frame_reads_as_flipped_type() {
    let src = AG_MOVED.replace("/11 << /3 0", "/11 << /0 true /3 0");
    let shape = read_path(&src);
    assert!(shape.flip);
    assert!(f64::from(shape.start).abs() < 0.01, "{}", shape.start);
    let end = f64::from(shape.end.expect("explicit end"));
    let want = frame_point(&src, 0, 0.07689);
    assert!(dist(place(&shape, end), want) < 0.5, "{:?} vs {want:?}", place(&shape, end));
}

fn rev_path(path: &PathSpec) -> PathSpec {
    let mut knots: Vec<PathKnot> = path
        .knots
        .iter()
        .rev()
        .map(|k| PathKnot {
            anchor: k.anchor,
            in_ctrl: k.out_ctrl,
            out_ctrl: k.in_ctrl,
            smooth: k.smooth,
        })
        .collect();
    if path.closed && !knots.is_empty() {
        knots.rotate_right(1);
    }
    PathSpec { knots, closed: path.closed, start: 0.0, end: None, flip: false, matrix: None, kind: None }
}

/// Ranges written to a frame read back placing the type the same; an open path
/// keeps all of itself and a moved start / flipped type are baked into the path.
#[test]
fn ranges_round_trip_through_frames() {
    let wave = read_path(WAVE);
    let circle = read_path(CIRCLE);
    let cases: [(&PathSpec, f32, Option<f32>, bool); 8] = [
        (&wave, 30.0, Some(150.0), false),
        (&wave, 30.0, None, false),
        (&wave, 0.0, Some(100.0), false),
        (&wave, 25.0, Some(120.0), true),
        (&circle, 40.0, Some(200.0), false),
        (&circle, 0.0, Some(300.0), false),
        (&circle, 250.0, Some(500.0), true),
        (&circle, 60.0, None, false),
    ];
    for (path, start, end, flip) in cases {
        let parts = to_frame_parts(&shape_of(path, start, end, flip)).expect("frame parts");
        let back = read_shape(FrameSource::Frame(&parts)).expect("read back").path.expect("path");
        // Writing the read-back shape again gives the same frame: the canonical
        // form is stable.
        let again = to_frame_parts(&shape_of(&back, back.start, back.end, back.flip)).expect("again");
        assert!(same_frame(&again, &parts), "start {start} end {end:?} flip {flip}: {again:?} vs {parts:?}");
        // The placement is the same: the base path (reversed when flipped)
        // sampled at the start/end arc.
        let base = if flip { rev_path(path) } else { path.clone() };
        let back_start = f64::from(back.start);
        let want_start = if path.closed { 0.0 } else { f64::from(start) };
        assert!((back_start - want_start).abs() < 0.05, "start {start} flip {flip}: {back_start}");
        assert!(dist(sample(&back, back_start), sample(&base, f64::from(start))) < 0.5);
        assert!(!back.flip, "flipped type is baked into the path");
        let measure = Measure::new(&Sub::from_path(&base).segments());
        let length = measure.length();
        if let (Some(want), Some(got)) = (end, back.end) {
            // A closed range keeps its span (`end - start`); an open one is
            // absolute on the path, clamped to the start.
            let want_arc = if path.closed {
                (f64::from(want) - f64::from(start)).clamp(0.0, length)
            } else {
                f64::from(want).clamp(f64::from(start), length)
            };
            let got_arc = f64::from(got);
            assert!((got_arc - want_arc).abs() < 0.05, "start {start} end {want}: {got_arc} vs {want_arc}");
        }
        // An open path keeps all of itself.
        if !path.closed {
            assert_eq!(back.knots.len(), base.knots.len(), "start {start} flip {flip}");
            assert!(!back.closed);
        }
    }
}

#[test]
fn flip_and_start_are_baked_into_the_frame() {
    for src in [CIRCLE, WAVE] {
        let path = read_path(src);
        for (start, flip) in [(40.0, false), (0.0, true), (25.0, true)] {
            let parts = to_frame_parts(&shape_of(&path, start, None, flip)).expect("frame parts");
            let back = read_shape(FrameSource::Frame(&parts)).expect("read back").path.expect("path");
            assert!(!back.flip, "{src:.12} start {start} flip {flip}: flip was not baked");
            // A closed path takes the start into its curve; an open one keeps
            // it in the range, so it survives the save as a start.
            let want_start = if path.closed { 0.0 } else { f64::from(start) };
            assert!(
                (f64::from(back.start) - want_start).abs() < 1e-3,
                "{src:.12} start {start} flip {flip}: start {} vs {want_start}",
                back.start
            );
            assert_eq!(back.end, None, "{src:.12}: the default end stays default");
            // The baked path starts where the original type did.
            let base = if flip { rev_path(&path) } else { path.clone() };
            assert!(
                dist(sample(&back, f64::from(back.start)), sample(&base, f64::from(start))) < 0.5,
                "{src:.12} start {start} flip {flip}: {:?} vs {:?}",
                sample(&back, f64::from(back.start)),
                sample(&base, f64::from(start))
            );
        }
    }
}

/// Updating an unreadable/unchanged shape keeps the stored frame verbatim; a
/// moved decoded curve reads back as the moved curve.
#[test]
fn written_frames_read_back() {
    for src in [CIRCLE, WAVE] {
        let stored = read(src).expect("shape");
        let frame = stored.frame.clone().expect("stored frame");
        assert_eq!(
            to_frame_parts(&stored),
            Some(txt2::FramePath {
                points: frame.points.clone(),
                kind: frame.kind,
                range: frame.range,
                flip: frame.flip,
                matrix: frame.matrix,
            }),
            "an unchanged shape must re-emit the stored frame"
        );

        let path = stored.path.expect("path");
        let mut moved = path.clone();
        for k in &mut moved.knots {
            for p in [&mut k.anchor, &mut k.in_ctrl, &mut k.out_ctrl] {
                p[0] += 5.0;
                p[1] -= 3.0;
            }
        }
        let spec = ShapeSpec { frame: None, path: Some(moved.clone()) };
        let parts = to_frame_parts(&spec).expect("moved frame");
        let back = read_shape(FrameSource::Frame(&parts)).expect("read back").path.expect("path");
        assert!(same_knots(&back.knots, &moved.knots), "{src:.12}: {back:?} vs {moved:?}");
        assert_eq!(back.closed, moved.closed);
    }
}

#[test]
fn box_frames_emit_32_numbers_and_keep_their_kind() {
    // The four straight cubic segments Photoshop writes for a box: eight
    // numbers per segment with the shared anchors repeated.
    let corners = [
        [10.0, 20.0],
        [410.0, 20.0],
        [410.0, 220.0],
        [10.0, 220.0],
        [10.0, 20.0],
    ];
    let mut points = Vec::new();
    for pair in corners.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        for point in [a, a, b, b] {
            points.extend_from_slice(&point);
        }
    }
    let shape = ShapeSpec {
        frame: Some(FrameSpec { points: points.clone(), kind: BOX_FRAME, range: [-2.0, -2.0], flip: None, matrix: None }),
        path: None,
    };
    let parts = to_frame_parts(&shape).expect("box frame");
    assert_eq!(parts.kind, BOX_FRAME, "a box frame stays a box frame");
    assert_eq!(parts.points.len(), 32, "4 segments x 8 numbers");
    assert_eq!(parts.points, points);
    assert_eq!(parts.range, [-2.0, -2.0]);
    // The decoded curve is a closed four-knot rectangle, informational only.
    let read = read_shape(FrameSource::Frame(&parts)).expect("read back").path.expect("path");
    assert!(read.closed);
    assert_eq!(read.knots.len(), 4);
    assert_eq!(read.kind.as_deref(), Some("box"));
    assert!(read.knots.iter().all(|k| k.smooth == Some(false)));
    assert!(close(read.knots[0].anchor, [10.0, 20.0]) || close(read.knots[0].anchor, [10.0, 220.0]));
}

#[test]
fn open_ranges_use_segment_units_and_the_default_sentinel() {
    let wave = read_path(WAVE);
    let parts = to_frame_parts(&shape_of(&wave, 30.0, None, false)).expect("frame");
    assert!(parts.range[0] > 0.0, "a moved start is in the range: {:?}", parts.range);
    assert_eq!(parts.range[1], -1.0, "a default end keeps -1");
    assert_eq!(parts.flip, Some(false));
    assert_eq!(parts.kind, PATH_FRAME);
    assert_eq!(parts.matrix, None);

    let default = to_frame_parts(&shape_of(&wave, 0.0, None, false)).expect("frame");
    assert_eq!(default.range, [-1.0, -1.0]);

    let limited = to_frame_parts(&shape_of(&wave, 0.0, Some(100.0), false)).expect("frame");
    let measure = Measure::new(&Sub::from_path(&wave).segments());
    assert_eq!(limited.range[0], 0.0);
    assert!((limited.range[1] - measure.position(100.0)).abs() < 0.05, "{:?}", limited.range);

    // A closed path with a lap or more is an explicit end at the seam.
    let circle = read_path(CIRCLE);
    let lap = to_frame_parts(&shape_of(&circle, 0.0, Some(100_000.0), false)).expect("frame");
    assert_eq!(lap.range, [0.0, 4.0]);
    let none = to_frame_parts(&shape_of(&circle, 0.0, None, false)).expect("frame");
    assert_eq!(none.range, [-1.0, -1.0]);
}

#[test]
fn hostile_ranges_read_as_the_default() {
    for range in ["[ 9999 -5 ]", "[ (x) 1 ]", "[ 9 99 ]", "[ -3 ]", "( )", "[ 1.5 0.5 ]"] {
        let src = WAVE.replace("/6 [ -1.0 -1.0 ]", &format!("/6 {range}"));
        let shape = read_path(&src);
        assert!(shape.start.is_finite() && shape.start >= 0.0, "{range}: {}", shape.start);
        assert!(
            shape.end.is_none_or(|e| e.is_finite() && e >= shape.start),
            "{range}: {:?}",
            shape.end
        );
    }
}

#[test]
fn hostile_frames_are_errors() {
    let huge = "9".to_string() + &"0".repeat(330); // overflows f64 to infinity
    let many = "0 0 1 1 2 2 0 0 ".repeat(MAX_SEGMENTS + 1);
    let cases = [
        "<< /1 << /0 [ ] >> /2 << /0 2 >> >>".to_string(),
        "<< /1 << /0 [ 0 0 1 ] >> /2 << /0 2 >> >>".to_string(),
        "<< /1 << /0 [ 0 0 (x) 0 20 10 30 10 ] >> /2 << /0 2 >> >>".to_string(),
        "<< /1 << /0 [ 0 0 10 0 20 10 30 10 ] >> /2 << /0 2 /2 [ 0 0 0 0 0 0 ] >> >>".to_string(),
        "<< /1 << /0 [ 0 0 10 0 20 10 30 10 ] >> /2 << /0 2 /2 [ 1 0 0 ] >> >>".to_string(),
        // Two pieces: the second segment does not start at the first's end.
        "<< /1 << /0 [ 0 0 10 0 20 10 30 10 40 40 50 50 60 60 70 70 ] >> /2 << /0 2 >> >>".to_string(),
        // No length: every point is the same.
        "<< /1 << /0 [ 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 ] >> /2 << /0 2 >> >>".to_string(),
        format!("<< /1 << /0 [ {huge} 0 1 0 2 1 3 1 ] >> /2 << /0 2 >> >>"),
        format!("<< /1 << /0 [ {many} ] >> /2 << /0 2 >> >>"),
        // No path at all.
        "<< /2 << /0 2 >> >>".to_string(),
    ];
    for src in cases {
        let frame = frame(&src);
        assert!(segments(FrameSource::EngineData(&frame)).is_err(), "read {src:.60}");
        assert!(read(&src).is_none(), "shape for {src:.60}");
    }
}
