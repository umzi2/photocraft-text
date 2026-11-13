//! Composed-layout cache invalidation: what makes Photoshop recompose a text
//! layer from its editing model instead of drawing stale glyphs.
//!
//! The area-text half was measured on Photoshop 20.0.2 (`out/verify/
//! existing_depth_framebump.psd`: opening and saving it again produced exactly
//! the cache of the same layer recomposed from its model, frame restored); the
//! point-text half is the `[ ]` "not laid out" state documented in
//! `docs/layout-update.md` and already used by [`txt2::clear_cache`].

use photocraft_psd::PsdFile;
use photocraft_text::txt2::{self, Step};

fn corpus_block(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
    let bytes = std::fs::read(dir.join(format!("{name}.psd"))).expect("corpus psd");
    let file = PsdFile::from_bytes(&bytes).expect("parse psd");
    file.global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .expect("Txt2 block")
        .data
        .clone()
}

fn object_view(block: &[u8], object: usize) -> String {
    let (start, end) = txt2::value_range(
        block,
        &[
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(object),
            Step::Key("1"),
        ],
    )
    .expect("object cache");
    String::from_utf8_lossy(&block[start..end]).into_owned()
}

/// Every text object's editing model (text, run tables) is untouched by the
/// invalidation, whatever the block holds.
fn assert_model_untouched(before: &[u8], after: &[u8]) {
    txt2::tokens(after).expect("invalidated block still parses");
    assert_eq!(
        txt2::read_text(after),
        txt2::read_text(before),
        "text changed"
    );
    assert_eq!(
        txt2::run_lengths(after).unwrap(),
        txt2::run_lengths(before).unwrap(),
        "style runs changed"
    );
    assert_eq!(
        txt2::paragraph_run_lengths(after).unwrap(),
        txt2::paragraph_run_lengths(before).unwrap(),
        "paragraph runs changed"
    );
}

#[test]
fn point_text_cache_is_emptied() {
    let block = corpus_block("plain_latin");
    let out = txt2::invalidate_composed_layouts(&block).expect("invalidate");
    assert_model_untouched(&block, &out);

    let view = object_view(&out, 0);
    assert!(
        view.contains("/2 [ ]"),
        "composed layout not emptied: {view}"
    );
    assert!(!view.contains("/99 /PC"), "stale glyphs survived: {view}");
}

#[test]
fn area_text_cached_frame_top_moves_one_point() {
    let block = corpus_block("box_left");
    let before = txt2::frame_geometry(&block).expect("area frame");
    let out = txt2::invalidate_composed_layouts(&block).expect("invalidate");
    assert_model_untouched(&block, &out);

    let after = txt2::frame_geometry(&out).expect("area frame");
    assert_eq!(after.left, before.left, "left edge moved");
    assert_eq!(after.right, before.right, "right edge moved");
    assert_eq!(after.bottom, before.bottom, "bottom edge moved");
    assert!(
        (after.top - (before.top + 1.0)).abs() < 1e-3,
        "cached top edge did not move one point: {} -> {}",
        before.top,
        after.top
    );
}

#[test]
fn area_text_keeps_its_frame_rectangle() {
    let block = corpus_block("box_justify");
    let out = txt2::invalidate_composed_layouts(&block).expect("invalidate");
    // The cache must keep the frame dict itself: removing it hangs Photoshop.
    let view = object_view(&out, 0);
    assert!(view.contains("/99 /F"), "area frame dict lost: {view}");
    assert!(txt2::frame_geometry(&out).is_some());
}

#[test]
fn invalidating_one_object_leaves_the_others_alone() {
    let block = corpus_block("plain_latin");
    let out = txt2::invalidate_composed_layout(&block, 0).expect("invalidate");
    assert_model_untouched(&block, &out);
    assert_eq!(txt2::invalidate_composed_layout(&block, 9).unwrap(), block);
}

#[test]
fn every_corpus_block_invalidates_without_touching_the_model() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("corpus dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("psd") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read psd");
        let Ok(file) = PsdFile::from_bytes(&bytes) else {
            continue;
        };
        for block in file.global_blocks.iter().filter(|b| &b.key == b"Txt2") {
            let out = txt2::invalidate_composed_layouts(&block.data).expect("invalidate");
            assert_model_untouched(&block.data, &out);
            checked += 1;
        }
    }
    assert!(checked > 10, "only {checked} corpus Txt2 blocks checked");
}

#[test]
fn an_unchanged_save_keeps_the_cache_and_a_changed_model_drops_it() {
    let block = corpus_block("plain_latin");
    // Same model: a save keeps the object (and Photoshop's glyph pen positions) untouched.
    assert_eq!(
        txt2::invalidate_changed_composed_layouts(&block, &block).unwrap(),
        block
    );
    // A style edit changes `/0`, so its cache must go.
    let edited = txt2::set_run_font_size(&block, 0, 96.0).expect("resize run");
    let out = txt2::invalidate_changed_composed_layouts(&edited, &block).expect("invalidate");
    assert_ne!(out, edited, "the edited object kept its stale cache");
    assert_model_untouched(&edited, &out);
}

/// A block with the corpus block's single text object written twice, so the
/// all-objects invalidation can be observed. PostScript arrays are
/// space-separated: a comma is a malformed number, as in Photoshop's blocks.
fn two_object_block(block: &[u8]) -> Vec<u8> {
    let (start, end) =
        txt2::value_range(block, &[Step::Key("1"), Step::Key("1")]).expect("objects array");
    let inner = &block[start + 1..end - 1];
    let mut out = block[..start].to_vec();
    out.push(b'[');
    out.extend_from_slice(inner);
    out.push(b' ');
    out.extend_from_slice(inner);
    out.push(b']');
    out.extend_from_slice(&block[end..]);
    out
}

#[test]
fn one_changed_object_invalidates_every_cache() {
    let block = two_object_block(&corpus_block("plain_latin"));
    txt2::tokens(&block).expect("block parses");
    let edited = txt2::set_run_font_size(&block, 0, 96.0).expect("resize run");
    let out = txt2::invalidate_changed_composed_layouts(&edited, &block).expect("invalidate");
    assert_model_untouched(&edited, &out);
    // Object 0's model changed, so both objects' caches go (the old file has no marker of
    // which layer is stale).
    let view0 = object_view(&out, 0);
    let view1 = object_view(&out, 1);
    assert!(
        view0.contains("/2 [ ]"),
        "changed object kept its cache: {view0}"
    );
    assert!(
        view1.contains("/2 [ ]"),
        "sibling object kept a possibly stale cache: {view1}"
    );
}
