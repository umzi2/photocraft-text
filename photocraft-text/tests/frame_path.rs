//! Frame paths (`Txt2 /0 /8 /0 [frame] /0`): type on a path / non-rectangular
//! area frames, read into text space and written back in the same encoding
//! Photoshop uses (points at `/1 /0`, kind `/2 /0`, matrix `/2 /2`, range
//! `/2 /6`, flip `/2 /11 /0`).

use photocraft_text::txt2::{self, FramePath};

/// A document block with one text object and one path frame. The stored matrix
/// translates by `(5, 5)`, so the reader must return the points shifted and
/// report the matrix.
fn block() -> Vec<u8> {
    let mut block = Vec::new();
    block.extend_from_slice(
        b"/0 << /8 << /0 [ << /0 << \
          /1 << /0 [ 0 0 10 0 20 0 20 10 30 10 40 10 40 20 ] >> \
          /2 << /0 2 /2 [ 1 0 0 1 5 5 ] /6 [ 0.0 -1.0 ] /9 7 /11 << /0 false /4 -1 /7 false >> >> \
          >> >> ] >> >>",
    );
    block.extend_from_slice(b"/1 << /1 [ << /0 << /0 (");
    block.extend_from_slice(&txt2::utf16("Hi"));
    block.extend_from_slice(
        b") /5 << /0 [ << /0 << /0 << /5 << /0 0 >> >> >> /1 3 >> ] >> \
          /6 << /0 [ << /0 << /0 << /6 << /0 0 /1 48.0 >> >> >> /1 3 >> ] >> >> \
          /1 << /0 [ << /0 0 >> ] >> >> ] >>",
    );
    block.extend_from_slice(b"/2 << >> /3 << >> /98 << /0 7 >>");
    block
}

#[test]
fn a_path_frame_reads_into_text_space() {
    let block = block();
    assert_eq!(txt2::object_frame_index(&block, 0), Some(0));
    let path = txt2::frame_path(&block, 0).expect("path frame");
    // Stored points `(0,0) (10,0) (20,0) (20,10) (30,10) (40,10) (40,20)` move by
    // the stored `(5,5)` translation.
    assert_eq!(
        path.points,
        vec![
            5.0, 5.0, 15.0, 5.0, 25.0, 5.0, 25.0, 15.0, 35.0, 15.0, 45.0, 15.0, 45.0, 25.0
        ]
    );
    assert_eq!(path.kind, 2);
    assert_eq!(path.range, [0.0, -1.0]);
    assert_eq!(path.flip, Some(false));
    assert_eq!(path.matrix, Some([1.0, 0.0, 0.0, 1.0, 5.0, 5.0]));
    // The same path through the object view names the same frame.
    assert_eq!(txt2::object_frame_path(&block, 0), Some(path));
}

#[test]
fn writing_the_same_path_keeps_the_bytes() {
    let block = block();
    let path = txt2::object_frame_path(&block, 0).expect("path");
    assert_eq!(
        txt2::set_object_frame_path(&block, 0, &path).unwrap(),
        block
    );
}

#[test]
fn writing_a_changed_path_keeps_unmodelled_keys() {
    let block = block();
    let mut path = txt2::object_frame_path(&block, 0).expect("path");
    path.points = vec![0.0, 0.0, 50.0, 0.0, 100.0, 0.0, 100.0, 100.0];
    path.range = [0.0, 0.5];
    path.flip = Some(true);
    let written = txt2::set_object_frame_path(&block, 0, &path).unwrap();
    let text = String::from_utf8_lossy(&written);
    // New points/kind/matrix/range/flip in the text-space encoding.
    assert!(
        text.contains("/0 [ 0.0 0.0 50.0 0.0 100.0 0.0 100.0 100.0 ]"),
        "{text}"
    );
    assert!(text.contains("/0 2 "), "{text}");
    assert!(text.contains("/2 [ 1.0 0.0 0.0 1.0 0.0 0.0 ]"), "{text}");
    assert!(text.contains("/6 [ 0.0 0.5 ]"), "{text}");
    assert!(text.contains("/0 true"), "{text}");
    // Keys the writer does not model survive: `/2 /9` and the other `/11` flags.
    assert!(text.contains("/9 7 "), "{text}");
    assert!(text.contains("/4 -1 "), "{text}");
    assert!(text.contains("/7 false"), "{text}");
    // Re-reading returns exactly what was written, and re-writing is a no-op.
    let read = txt2::object_frame_path(&written, 0).expect("read back");
    assert_eq!(
        read,
        FramePath {
            matrix: None,
            ..path.clone()
        }
    );
    assert_eq!(
        txt2::set_object_frame_path(&written, 0, &read).unwrap(),
        written
    );
}

#[test]
fn a_photoshop_box_frame_reads_as_a_straight_closed_path() {
    // Photoshop writes a text box as four straight cubic segments, eight numbers
    // per segment (`tests/assets/inputs/box_left.bin`): the same `/1 /0` array a
    // path frame uses, with kind `/2 /0 = 1`.
    let file = fixture("box_left.bin");
    let block = file.txt2().expect("Txt2").to_vec();
    let path = txt2::object_frame_path(&block, 0).expect("box frame path");
    assert_eq!(path.kind, 1, "box frames are kind 1, type-on-a-path is 2");
    assert_eq!(path.points.len(), 32, "four segments, eight numbers each");
    assert_eq!(path.points[0..2], [0.0, 0.0]);
    assert_eq!(
        path.points[30..32],
        [0.0, 0.0],
        "the last point closes the box"
    );
    assert_eq!(path.range, [-2.0, -2.0]);
    assert_eq!(path.matrix, None);
}

#[test]
fn a_point_frame_has_no_path() {
    // Point text stores only `/2` (no `/1` points), so there is no path to read.
    let file = fixture("plain_latin.bin");
    let block = file.txt2().expect("Txt2").to_vec();
    assert_eq!(txt2::object_frame_index(&block, 0), Some(0));
    assert_eq!(txt2::frame_path(&block, 0), None);
    assert_eq!(txt2::object_frame_path(&block, 0), None);
}

#[test]
fn a_singular_matrix_makes_the_frame_unreadable() {
    let block = String::from(
        "/0 << /8 << /0 [ << /0 << /1 << /0 [ 0 0 10 0 ] >> /2 << /0 2 /2 [ 0 0 0 0 0 0 ] >> >> >> ] >> >> \
         /1 << /1 [ << /0 << /0 () /5 << >> /6 << >> >> /1 << /0 [ << /0 0 >> ] >> >> ] /2 << >> /3 << >> >> \
         /98 << /0 7 >>",
    );
    assert_eq!(txt2::frame_path(block.as_bytes(), 0), None);
}

fn fixture(name: &str) -> photocraft_psd::PsdFile {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/assets/inputs")
        .join(name);
    photocraft_psd::PsdFile::from_bytes(&std::fs::read(path).expect("fixture")).expect("parse")
}
