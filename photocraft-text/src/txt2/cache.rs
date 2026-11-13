use crate::error::Error;

use super::frame::{frame_entries, frame_values, number};
use super::path::{Step, format_real, replace_container, resolve, value_span};
use super::syntax::{Kind, Token, dict_entries, skip_value, tokens};

/// Empties the composed-layout cache of text object `object` (the `/2` of its
/// view), so the next application recomposes from the text and the run tables.
///
/// The key stays: Photoshop 2025 crashes on a block in which no object has a
/// `/2`, while an empty array is accepted and simply means "not laid out".
pub fn clear_cache(block: &[u8], object: usize) -> Result<Vec<u8>, Error> {
    // Area text keeps its frame rectangle inside the layout container: the
    // `/99 /F` dict holds the box in `/1` and the composed glyphs in `/6`.
    // Emptying the whole container deletes the box, and emptying only the
    // glyph cache makes Photoshop hang on open, so area text keeps its layout
    // container; a changed `/1` rectangle is what forces a recomposition.
    let toks = tokens(block)?;
    if frame_entries(&toks).is_some() {
        return Ok(block.to_vec());
    }
    let steps = vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("1"),
        Step::Key("2"),
    ];
    replace_container(block, &steps, b"[ ]")
}

// ---------------------------------------------------------------------------
// Composed-layout cache invalidation
// ---------------------------------------------------------------------------

/// One byte span of a block and the bytes that replace it.
struct CacheEdit {
    start: usize,
    end: usize,
    bytes: Vec<u8>,
}

/// Whether the dict opened at `open` is an area-text frame (`/99 /F`) carrying
/// a `/1` rectangle.
fn is_frame_dict(toks: &[Token], open: usize) -> bool {
    let Some(entries) = dict_entries(toks, open) else {
        return false;
    };
    let name = entries
        .iter()
        .find(|(k, _)| k == "99")
        .is_some_and(|(_, v)| matches!(&toks[*v].kind, Kind::Name(n) if n == "F"));
    name && entries
        .iter()
        .any(|(k, v)| k == "1" && matches!(toks[*v].kind, Kind::ArrayOpen))
}

/// Token index of the `/1` rectangle of the area frame inside the subtree
/// opened at `open` (the object's composed layout), if it holds one. Point-text
/// caches have no frame rectangle.
fn frame_in_subtree(toks: &[Token], open: usize) -> Option<usize> {
    let end = skip_value(toks, open)?;
    let dict = (open + 1..end)
        .find(|&i| matches!(toks[i].kind, Kind::DictOpen) && is_frame_dict(toks, i))?;
    let entries = dict_entries(toks, dict)?;
    let arr = entries.iter().find(|(k, _)| k == "1")?.1;
    matches!(toks[arr].kind, Kind::ArrayOpen).then_some(arr)
}

/// The path from a block's top level to text object `object`'s composed
/// layout: `/1` (text engine) → `/1` (objects) → `[object]` → `/1` (cache).
fn cache_path(object: usize) -> [Step<'static>; 4] {
    [
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("1"),
    ]
}

/// Number of text objects in `/1 /1` of a block.
fn object_count(toks: &[Token]) -> Option<usize> {
    let arr = resolve(toks, &[Step::Key("1"), Step::Key("1")])?;
    if !matches!(toks[arr].kind, Kind::ArrayOpen) {
        return None;
    }
    let end = skip_value(toks, arr)?;
    let mut count = 0;
    let mut i = arr + 1;
    while i < end && !matches!(toks[i].kind, Kind::ArrayClose) {
        i = skip_value(toks, i)?;
        count += 1;
    }
    Some(count)
}

/// The edit that makes Photoshop recompose object `object`'s layout from the
/// editing model, or `None` when the object has no composed layout to touch.
///
/// * Area text: the cached frame rectangle's top edge moves one point. The
///   model's frame (the `TySh` shape) wins on the next open and Photoshop
///   rewrites the cache with the new line boxes.
/// * Point text: the composed layout (`/2`) becomes `[ ]`, the "not laid out"
///   state Photoshop recomposes from.
fn cache_edit(toks: &[Token], object: usize) -> Option<CacheEdit> {
    let cache = resolve(toks, &cache_path(object))?;
    if !matches!(toks[cache].kind, Kind::DictOpen) {
        return None;
    }
    if let Some(arr) = frame_in_subtree(toks, cache) {
        let pos = frame_values(toks, arr)?;
        let top = number(&toks[pos[1]].kind)?;
        return Some(CacheEdit {
            start: toks[pos[1]].start,
            end: toks[pos[1]].end,
            bytes: format_real(top + 1.0).into_bytes(),
        });
    }
    let path = [
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("1"),
        Step::Key("2"),
    ];
    let (start, end) = value_span(toks, &path)?;
    Some(CacheEdit {
        start,
        end,
        bytes: b"[ ]".to_vec(),
    })
}

/// Edits of [`invalidate_composed_layouts`], applied back to front so the byte
/// offsets of the spans still to replace stay valid.
fn apply_edits(block: &[u8], mut edits: Vec<CacheEdit>) -> Vec<u8> {
    if edits.is_empty() {
        return block.to_vec();
    }
    edits.sort_by_key(|e| e.start);
    let mut out = block.to_vec();
    for e in edits.into_iter().rev() {
        out.splice(e.start..e.end, e.bytes);
    }
    out
}

/// Makes Photoshop recompose object `object`'s text layout from its editing
/// model on the next open.
///
/// Photoshop draws the composed layout cached in `Txt2` (`/1/1[i]/1`) rather
/// than the model, so a save that changed the text or its properties without
/// rewriting the cache shows the old layout, and the first text edit asks
/// whether to accept the new one. Touching the cache the way Photoshop's own
/// relayout check notices makes it recompose while opening instead:
///
/// * area text (`/99 /F` rectangle in the cache): move the cached frame's top
///   edge one point — the model's frame wins and the cache is rewritten with the
///   model's line boxes. Measured on Photoshop 20.0.2 (`out/verify/
///   existing_depth_framebump.psd` opened and saved again gave exactly the cache
///   of the same layer recomposed from its model, frame included);
/// * point text: empty the composed layout (`/2`), the "not laid out" state
///   Photoshop recomposes from (`docs/layout-update.md`).
///
/// `Ok(block)` unchanged when `object` has no composed layout (a native block
/// or an object past the array).
pub fn invalidate_composed_layout(block: &[u8], object: usize) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let edits = cache_edit(&toks, object).into_iter().collect();
    Ok(apply_edits(block, edits))
}

/// [`invalidate_composed_layout`] for every text object of a document block,
/// so Photoshop recomposes each layer it draws from a cache that may no longer
/// match the model (the block a save writes after text, style or paragraph
/// edits, including the ones `EngineData` cannot describe).
pub fn invalidate_composed_layouts(block: &[u8]) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let count = object_count(&toks).unwrap_or(0);
    let edits = (0..count).filter_map(|i| cache_edit(&toks, i)).collect();
    Ok(apply_edits(block, edits))
}

/// Types of the tokens of the complete value at `path` (container included), for comparing
/// two blocks' models without depending on their whitespace or number formatting.
fn value_kinds(toks: &[Token], path: &[Step<'_>]) -> Option<Vec<Kind>> {
    let v = resolve(toks, path)?;
    let end = skip_value(toks, v)?;
    Some(toks[v..end].iter().map(|t| t.kind.clone()).collect())
}

/// [`invalidate_composed_layouts`], skipped when no pre-existing text object's
/// editing model (the `/0` text, style runs and paragraph runs) differs from
/// `previous`'s object at the same slot.
///
/// A writer cannot tell which layers were edited, but the block it replaces
/// can: if every object both blocks hold has the same model, the file is being
/// saved unchanged and its caches, glyph pen positions (`/21 /1`) and other
/// extras are returned byte for byte.  As soon as one object's model changed,
/// every object's cache is invalidated: an old file may carry a stale cache
/// for a layer this save did not touch (an earlier save rewrote its model and
/// kept the cache), and nothing marks those apart, so refreshing all of them is
/// what Photoshop's "Update All Text Layers" does.  Objects `previous` does not
/// hold (new layers) have no older cache and do not count as a change.
pub fn invalidate_changed_composed_layouts(
    current: &[u8],
    previous: &[u8],
) -> Result<Vec<u8>, Error> {
    let toks = tokens(current)?;
    let count = object_count(&toks).unwrap_or(0);
    let Ok(prev) = tokens(previous) else {
        return invalidate_composed_layouts(current);
    };
    let path = |i: usize| {
        [
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(i),
            Step::Key("0"),
        ]
    };
    let changed = (0..count).any(|i| {
        let p = path(i);
        match (value_kinds(&prev, &p), value_kinds(&toks, &p)) {
            // In both blocks: compare the models.
            (Some(before), Some(now)) => before != now,
            // New in this save: no older cache to invalidate.
            (None, _) => false,
            // The replaced block held an object this one lost.
            (Some(_), None) => true,
        }
    });
    if changed {
        invalidate_composed_layouts(current)
    } else {
        Ok(current.to_vec())
    }
}
