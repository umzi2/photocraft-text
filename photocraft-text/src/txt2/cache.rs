use crate::error::Error;

use super::frame::{frame_values, number};
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
    // Only *this* object's own frame matters: another object's `/99 /F` box in
    // a multi-layer block must not stop this one's cache from being cleared.
    let open = resolve(
        &toks,
        &[Step::Key("1"), Step::Key("1"), Step::Index(object)],
    );
    if let Some(open) = open
        && frame_in_subtree(&toks, open).is_some()
    {
        return Ok(block.to_vec());
    }
    let steps = vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("1"),
        Step::Key("2"),
    ];
    // A hand-built/minimal object may have no view: nothing to clear.
    if resolve(&toks, &steps).is_none() {
        return Ok(block.to_vec());
    }
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

// ---------------------------------------------------------------------------
// Reading the composed-layout cache
// ---------------------------------------------------------------------------

/// Which way the text of a composed cache runs.
///
/// The cache has two frames. The **relative** one — a line's `/0` origin, `/1`
/// box, `/10` baseline and the run's `/1` bounds — is always *x = along the
/// line, y = across it* (the engine's y-up text space). The **absolute** boxes
/// `/8` and `/9` are in document space, so they flip for vertical text:
/// x = across, y = along. A reader that does not know the orientation therefore
/// reports a vertical line's origin as its *across* inset while the along start
/// it prints came from the absolute box, i.e. two different axes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    /// `Ornt = Hrzn`: lines run left to right, `/8` is `[x0, y0, x1, y1]`.
    #[default]
    Horizontal,
    /// `Ornt = Vrtc`: columns run right to left, `/8` is
    /// `[across_x0, along_y0, across_x1, along_y1]`.
    Vertical,
}

/// One glyph run of a cached line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComposedRun {
    /// Line-space origin of the run: the pen start Photoshop used, which is the
    /// line origin plus its indents.
    ///
    /// Horizontal: the `/8` box's `(x0, y0)`, x = along, y = across. Vertical:
    /// the same box **transposed** to `(y0, x0)` so that x stays the along
    /// axis and y the across axis, matching every other field here.
    pub origin: (f32, f32),
    /// Glyph ids of the run, in text order.
    pub gids: Vec<u16>,
    /// The run's box `[x0, y0, x1, y1]` from `/1`, in the line's relative
    /// frame: the pen start, the descent below the baseline, the ink end
    /// (Photoshop stores the right side bearing out of it), the ascent above it.
    /// Relative frame, so x = along and y = across for both orientations.
    pub bounds: [f32; 4],
    /// Advance adjustments recorded for the run, in cache order.
    ///
    /// Photoshop's every-line composer writes one record per *adjustable
    /// glyph* (the word spaces it balances); `Some(delta)` is that record's
    /// `/3` advance correction in points, `None` a record without one. A
    /// caller that knows which shaped glyphs are spaces can zip the deltas onto
    /// them in order; [`ComposedLine::deltas`] returns the corrections alone.
    pub adjustments: Vec<Option<f32>>,
}

/// One line of Photoshop's composed-layout cache.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComposedLine {
    /// Line origin in the line's relative frame: x = along the line, y = across
    /// it (points, y-up).
    ///
    /// Horizontal: the first run's document `x0` (the pen start) and the
    /// baseline `/10`, exactly as Photoshop writes them. Vertical: the line's
    /// own `/0` origin, `[along_start, across_inset]` — for the measured
    /// `box_v` cache that is `[0.0, 17.90369]`, i.e. the first column's
    /// baseline sits 17.90369 pt inside the frame's right edge.
    pub origin: (f32, f32),
    /// Baseline (`/10`) in points, y-up.
    ///
    /// Horizontal: the first baseline's distance below the frame's top edge.
    /// Vertical: the **across** inset of this column's baseline from the
    /// frame's right edge (the column pitch, `17.90369, 75.50369, …` at 48 pt).
    pub baseline: f32,
    /// Ascent (`/15`) and descent (`/14`, stored negative y-up) of the line's
    /// tallest run, points.
    ///
    /// Horizontal: the font ascent/descent above and below the baseline.
    /// Vertical: the column's half width on each side of the baseline, which
    /// Photoshop makes exactly half an em (`±size/2`), so the pair does not
    /// describe the font.
    pub ascent: f32,
    pub descent: f32,
    /// The line's glyph runs in visual order.
    pub runs: Vec<ComposedRun>,
}

impl ComposedLine {
    /// Every glyph id of the line, in text order.
    pub fn gids(&self) -> impl Iterator<Item = u16> + '_ {
        self.runs.iter().flat_map(|run| run.gids.iter().copied())
    }

    /// The line's advance corrections that carry a value, in cache order.
    pub fn deltas(&self) -> Vec<f32> {
        self.runs
            .iter()
            .flat_map(|run| run.adjustments.iter())
            .filter_map(|delta| *delta)
            .collect()
    }
}

/// The composed-layout cache of one text object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComposedLayout {
    /// The orientation the fields were read in (see [`Orientation`]).
    pub orientation: Orientation,
    /// Lines in visual order; empty when the object has not been laid out.
    pub lines: Vec<ComposedLine>,
}

impl ComposedLayout {
    /// `true` when the cache holds no line (Photoshop has not composed the
    /// object, or a writer cleared the cache).
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Glyph count across every line.
    pub fn glyph_count(&self) -> usize {
        self.lines
            .iter()
            .map(|line| line.runs.iter().map(|r| r.gids.len()).sum::<usize>())
            .sum()
    }

    /// Every advance correction of the object, in line and cache order.
    ///
    /// This is the shape [`crate::layer_render::RunFeatures::word_spacing_deltas`]
    /// wants: the every-line composer adjusts word spaces line by line, so the
    /// deltas are the per-space corrections in text order.
    pub fn deltas(&self) -> Vec<f32> {
        self.lines.iter().flat_map(ComposedLine::deltas).collect()
    }
}

/// Reads the composed-layout cache of text object `object` (the `/2` of its
/// view) as Photoshop stored it.
///
/// The cache is Photoshop's own composition: per line, its origin, baseline,
/// ascent/descent, glyph ids and per-glyph advance corrections. It is the
/// oracle for "did Photoshop compose this differently from the engine" — the
/// raster can only show ink, and glyph ids plus `/3` corrections are the
/// composition itself.
///
/// Reads the cache as **horizontal** text ([`Orientation::Horizontal`]). A
/// vertical layer must be read with [`composed_layout_vertical`] (or
/// [`composed_layout_oriented`]): its absolute `/8`/`/9` boxes are transposed,
/// so without the orientation a vertical line's origin x comes back as the
/// column's across inset and the along start is lost.
///
/// Returns an empty layout when the object has no cache, and an error only when
/// the block is not valid `Txt2` syntax.
pub fn composed_layout(block: &[u8], object: usize) -> Result<ComposedLayout, Error> {
    composed_layout_oriented(block, object, Orientation::Horizontal)
}

/// Reads a **vertical** (`Ornt = Vrtc`) cache: [`composed_layout`] with
/// [`Orientation::Vertical`], which transposes the absolute `/8`/`/9` boxes so
/// the reported origin is `[along, across]` like every other field.
pub fn composed_layout_vertical(block: &[u8], object: usize) -> Result<ComposedLayout, Error> {
    composed_layout_oriented(block, object, Orientation::Vertical)
}

/// As [`composed_layout`], for text whose orientation the caller knows (from
/// `TySh` `Ornt` or the engine data's writing direction).
pub fn composed_layout_oriented(
    block: &[u8],
    object: usize,
    orientation: Orientation,
) -> Result<ComposedLayout, Error> {
    let toks = tokens(block)?;
    let steps = [
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("1"),
        Step::Key("2"),
    ];
    let Some(cache) = resolve(&toks, &steps) else {
        return Ok(ComposedLayout::default());
    };
    let end = skip_value(&toks, cache).unwrap_or(toks.len());
    let mut lines = Vec::new();
    for i in cache..end {
        if !matches!(toks[i].kind, Kind::DictOpen) || !is_kind(&toks, i, "L") {
            continue;
        }
        if let Some(line) = composed_line(&toks, i, end, orientation) {
            lines.push(line);
        }
    }
    Ok(ComposedLayout { orientation, lines })
}

/// `true` when the dict at `open` records `/99 /<kind>`.
fn is_kind(toks: &[Token], open: usize, kind: &str) -> bool {
    dict_entries(toks, open).is_some_and(|entries| {
        entries.iter().any(|(key, value)| {
            key == "99"
                && matches!(toks.get(*value).map(|t| &t.kind), Some(Kind::Name(name)) if name == kind)
        })
    })
}

fn entry(entries: &[(String, usize)], key: &str) -> Option<usize> {
    entries
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| *value)
}

fn float_at(toks: &[Token], index: usize) -> Option<f32> {
    number(&toks.get(index)?.kind).map(|v| v as f32)
}

/// The numbers of the array at `open`, in order.
fn numbers(toks: &[Token], open: usize) -> Vec<f64> {
    let mut out = Vec::new();
    let mut i = open + 1;
    while let Some(tok) = toks.get(i) {
        match &tok.kind {
            Kind::ArrayClose => break,
            kind => {
                if let Some(value) = number(kind) {
                    out.push(value);
                }
                i += 1;
            }
        }
    }
    out
}

fn composed_line(
    toks: &[Token],
    open: usize,
    end: usize,
    orientation: Orientation,
) -> Option<ComposedLine> {
    let entries = dict_entries(toks, open)?;
    let mut line = ComposedLine::default();
    let relative_origin = entry(&entries, "0")
        .and_then(|dict| dict_entries(toks, dict).and_then(|inner| entry(&inner, "0")))
        .map(|origin| numbers(toks, origin))
        .filter(|values| values.len() >= 2)
        .map(|values| (values[0] as f32, values[1] as f32));
    if let Some(origin) = relative_origin {
        line.origin = origin;
    }
    line.baseline = entry(&entries, "10")
        .and_then(|i| float_at(toks, i))
        .unwrap_or(0.0);
    line.ascent = entry(&entries, "15")
        .and_then(|i| float_at(toks, i))
        .unwrap_or(0.0);
    line.descent = entry(&entries, "14")
        .and_then(|i| float_at(toks, i))
        .map_or(0.0, f32::abs);
    let line_end = skip_value(toks, open).unwrap_or(end);
    for i in open..line_end {
        if !matches!(toks[i].kind, Kind::DictOpen) || !is_kind(toks, i, "G") {
            continue;
        }
        let Some(run_entries) = dict_entries(toks, i) else {
            continue;
        };
        let mut run = ComposedRun::default();
        if let Some(doc_box) = entry(&run_entries, "8") {
            let values = numbers(toks, doc_box);
            if values.len() >= 2 {
                run.origin = match orientation {
                    Orientation::Horizontal => (values[0] as f32, values[1] as f32),
                    // Document space is x = across, y = along for vertical
                    // text; swap so the run origin matches the line fields.
                    Orientation::Vertical => (values[1] as f32, values[0] as f32),
                };
            }
        }
        if let Some(bounds) = entry(&run_entries, "1") {
            let values = numbers(toks, bounds);
            for (slot, value) in run.bounds.iter_mut().zip(values) {
                *slot = value as f32;
            }
        }
        if let Some(gids) = entry(&run_entries, "5") {
            run.gids = numbers(toks, gids)
                .into_iter()
                .filter(|value| (0.0..=f64::from(u16::MAX)).contains(value))
                .map(|value| value as u16)
                .collect();
        }
        if let Some(records) = entry(&run_entries, "6")
            .and_then(|dict| dict_entries(toks, dict))
            .and_then(|entries| entry(&entries, "0"))
        {
            let mut i = records + 1;
            while let Some(tok) = toks.get(i) {
                match &tok.kind {
                    Kind::ArrayClose => break,
                    Kind::DictOpen => {
                        let delta = dict_entries(toks, i)
                            .and_then(|record| entry(&record, "3"))
                            .and_then(|value| float_at(toks, value));
                        run.adjustments.push(delta);
                        i = skip_value(toks, i).unwrap_or(i + 1);
                    }
                    _ => i += 1,
                }
            }
        }
        line.runs.push(run);
    }
    match orientation {
        // A horizontal line's pen start is the first run's document x, and its
        // origin y is the baseline (the `/0` record leaves x at 0).
        Orientation::Horizontal => {
            if let Some(first) = line.runs.first() {
                line.origin = (first.origin.0, line.baseline);
            }
        }
        // A vertical line's `/0` origin already is `[along, across]`; keep it
        // instead of borrowing the run's *across* coordinate as an x.
        Orientation::Vertical => {
            if let Some(origin) = relative_origin {
                line.origin = origin;
            }
        }
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-line cache in the shape Photoshop writes it: a view dict holding
    /// the `/2` cache, one `/99 /L` line record per line, each with a glyph run
    /// (`/99 /G`) that carries the gids, the document box and the per-glyph
    /// advance corrections.
    const BLOCK: &[u8] = b"/1 << /1 [ << /1 << /2 [ << /99 /L /10 13.6799 /14 -15.4259 /15 7.00214 /0 << /0 [ 0.0 13.6799 ] >> /1 [ 0.0 -15.4259 430.19693 7.00214 ] /6 [ << /99 /S /15 << /0 58 >> >> << /0 << /0 [ 42.0 0.0 ] >> /6 [ << /99 /G /1 [ 0.0 -15.4259 430.19693 7.00214 ] /5 [ 41 76 3 85 ] /6 << /0 [ << /3 -.0468 >> << >> ] /1 [ 4 2 ] >> /8 [ 42.0 -1.746 472.19693 20.68204 ] >> ] >> ] >> << /99 /L /10 40.6799 /14 -15.4259 /15 7.00214 /0 << /0 [ 0.0 40.6799 ] >> /1 [ 0.0 -15.4259 442.92755 7.00214 ] /6 [ << /99 /S /15 << /0 52 >> >> << /0 << /0 [ 18.0 0.0 ] >> /6 [ << /99 /G /1 [ 0.0 -15.4259 442.92755 7.00214 ] /5 [ 76 81 71 ] /6 << /0 [ << >> << >> << >> ] >> /8 [ 18.0 25.254 460.92755 47.68204 ] >> ] >> ] >> ] >> >> ] >>";

    #[test]
    fn reads_the_lines_origins_and_advance_corrections() {
        let layout = composed_layout(BLOCK, 0).expect("cache");
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(layout.glyph_count(), 7);
        let first = &layout.lines[0];
        assert_eq!(first.origin, (42.0, 13.6799));
        assert!((first.baseline - 13.6799).abs() < 1e-3);
        assert!((first.ascent - 7.00214).abs() < 1e-3);
        assert!((first.descent - 15.4259).abs() < 1e-3);
        assert_eq!(first.runs.len(), 1);
        assert_eq!(first.runs[0].gids, vec![41, 76, 3, 85]);
        assert_eq!(first.runs[0].origin, (42.0, -1.746));
        // The corrections are recorded per adjustable glyph, in cache order.
        assert_eq!(first.runs[0].adjustments, vec![Some(-0.0468), None]);
        assert_eq!(first.deltas(), vec![-0.0468]);
        let second = &layout.lines[1];
        assert_eq!(second.origin, (18.0, 40.6799));
        assert!(second.deltas().is_empty());
        assert_eq!(layout.deltas(), vec![-0.0468]);
    }

    #[test]
    fn an_object_without_a_cache_is_empty() {
        let layout = composed_layout(BLOCK, 3).expect("cache");
        assert!(layout.is_empty());
        assert_eq!(layout.glyph_count(), 0);
        assert!(layout.deltas().is_empty());
    }
}
