//! Block-level builders for callers that assemble PSD data themselves.
//!
//! [`crate::psd::author`] rewrites a layer inside a [`photocraft_psd::PsdFile`];
//! a caller that keeps its own document model (the fork's `TextLayer`) needs the
//! same EngineData and `TySh` bytes without a file or a [`crate::layer_render::FaceStore`]:
//! [`build_engine_data`] and [`build_tysh`] produce them from a [`LayerSpec`],
//! reusing the writer [`crate::psd::author`] uses, so both paths write identical
//! sheets.

use photocraft_psd::text_layer::TySh;

use super::engine_data;
use super::model::LayerSpec;
use super::{Error, author};
use crate::engine_data::write;
use crate::psd::pixels;
use crate::txt2;

/// The `EngineData` tree a layer's `TySh` carries for `spec` (the engine's
/// `ResourceDict`, style and paragraph runs, antialias, fractional widths and
/// shape).
pub fn build_engine_data(spec: &LayerSpec) -> crate::engine_data::Value {
    let text = author::spec_text(spec);
    let mut styles = author::run_styles(&spec.runs);
    let paragraphs = paragraph_runs(&text, spec);
    let mut root = engine_data::build_engine_data(&text, spec, &mut styles, &paragraphs);
    patch_paragraph_keys(&mut root, spec, paragraphs.len());
    root
}

/// One paragraph run per paragraph split of `text`, taking the per-paragraph
/// spec when [`LayerSpec::paragraphs`] has one (a short list repeats its last
/// entry, the fallback is [`LayerSpec::paragraph`]).
fn paragraph_runs(text: &str, spec: &LayerSpec) -> Vec<crate::runs::ParagraphRun> {
    text.split('\r')
        .enumerate()
        .map(|(i, piece)| {
            let para = spec
                .paragraphs
                .get(i)
                .or_else(|| spec.paragraphs.last())
                .unwrap_or(&spec.paragraph);
            // Each paragraph owns its break, so its run length is the piece
            // plus the U+000D (the last one the terminal break).
            author::paragraph_run(piece, para)
        })
        .collect()
}

/// The two paragraph properties the shared paragraph model does not carry:
/// `Burasagari` and `KinsokuOrder` go straight into each `ParagraphSheet`.
fn patch_paragraph_keys(root: &mut crate::engine_data::Value, spec: &LayerSpec, count: usize) {
    let Some(items) = root
        .get_mut("EngineDict")
        .and_then(|e| e.get_mut("ParagraphRun"))
        .and_then(|r| r.get_mut("RunArray"))
        .and_then(crate::engine_data::Value::as_array_mut)
    else {
        return;
    };
    for (i, item) in items.iter_mut().take(count).enumerate() {
        let para = spec
            .paragraphs
            .get(i)
            .or_else(|| spec.paragraphs.last())
            .unwrap_or(&spec.paragraph);
        let Some(props) = item
            .get_mut("ParagraphSheet")
            .and_then(|s| s.get_mut("Properties"))
            .and_then(crate::engine_data::Value::as_dict_mut)
        else {
            continue;
        };
        put(
            props,
            "Burasagari",
            crate::engine_data::Value::Bool(para.burasagari),
        );
        put(
            props,
            "KinsokuOrder",
            crate::engine_data::Value::Int(para.kinsoku_order),
        );
    }
}

fn put(
    props: &mut Vec<(String, crate::engine_data::Value)>,
    key: &str,
    value: crate::engine_data::Value,
) {
    match props.iter_mut().find(|(k, _)| k == key) {
        Some((_, slot)) => *slot = value,
        None => props.push((key.to_owned(), value)),
    }
}

/// The `TySh` block body for `spec`, without touching a file or rasterising.
///
/// `bounds` is the descriptor's ink rectangle in points (Photoshop's layer
/// record); `None` uses a rectangle around the text's origin, which is what a
/// point-text layer the caller has not measured wants. Errors mirror
/// [`crate::psd::author`]: empty runs or run lengths that do not cover the text
/// plus its terminator.
pub fn build_tysh(spec: &LayerSpec, bounds: Option<[f32; 4]>) -> Result<Vec<u8>, Error> {
    let text = author::spec_text(spec);
    author::validate_runs(&text, &spec.runs)?;
    let engine = build_engine_data(spec);
    let bytes = write(&engine);
    let bounds = bounds.unwrap_or([0.0, 0.0, 0.0, 0.0]);
    let rect = [
        f64::from(bounds[0]),
        f64::from(bounds[1]),
        f64::from(bounds[2]),
        f64::from(bounds[3]),
    ];
    let mut tysh = TySh::new_text_points(
        &text,
        &bytes,
        spec.transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        rect,
    );
    if let Some(orientation) = spec.orientation {
        tysh.set_orientation(if orientation == 1 {
            photocraft_psd::text_layer::Orientation::Vertical
        } else {
            photocraft_psd::text_layer::Orientation::Horizontal
        });
    }
    if let Some(transform) = spec.transform {
        tysh.set_transform(transform);
    }
    if let Some([left, top, right, bottom]) = spec.frame {
        tysh.set_layout_bounds([
            f64::from(left),
            f64::from(top),
            f64::from(right),
            f64::from(bottom),
        ]);
    }
    Ok(tysh.to_bytes())
}

/// The pixel refresh a caller needs after [`build_tysh`] (Photoshop stores the
/// layer's pre-rendered pixels beside the block).
pub fn refresh_pixels(
    layer: &mut photocraft_psd::layer::LayerRecord,
    spec: &LayerSpec,
    store: &crate::layer_render::FaceStore,
    depth: u16,
    version: photocraft_psd::Version,
) -> Result<(), Error> {
    let text = author::spec_text(spec);
    let frame = spec
        .frame
        .map(|[left, top, right, bottom]| crate::txt2::FrameGeometry {
            left,
            top,
            right,
            bottom,
        });
    pixels::refresh_layer(layer, &text, store, frame, depth, version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::psd::{ParagraphSpec, RunSpec};

    /// A one-layer file carrying `tysh` and `txt2` (the blocks `read_layer` reads).
    fn file_with_blocks(tysh: &[u8], txt2: Option<Vec<u8>>) -> photocraft_psd::PsdFile {
        use photocraft_psd::builder::{PixelData, PsdBuilder, TextLayerSpec};
        let mut builder = PsdBuilder::new(1, 1);
        builder
            .push_text_layer(TextLayerSpec {
                name: "T".into(),
                rect: photocraft_psd::layer::Rect {
                    top: 0,
                    left: 0,
                    bottom: 1,
                    right: 1,
                },
                pixels: PixelData::Rgba8(vec![0, 0, 0, 0]),
                ty_sh: tysh.to_vec(),
                txt2,
            })
            .expect("text layer");
        builder.build().expect("file")
    }

    /// A one-layer file carrying `tysh`, the input `read_layer` wants.
    fn file_with_tysh(tysh: &[u8]) -> photocraft_psd::PsdFile {
        use photocraft_psd::builder::{PixelData, PsdBuilder, TextLayerSpec};
        let mut builder = PsdBuilder::new(1, 1);
        builder
            .push_text_layer(TextLayerSpec {
                name: "T".into(),
                rect: photocraft_psd::layer::Rect {
                    top: 0,
                    left: 0,
                    bottom: 1,
                    right: 1,
                },
                pixels: PixelData::Rgba8(vec![0, 0, 0, 0]),
                ty_sh: tysh.to_vec(),
                txt2: None,
            })
            .expect("text layer");
        builder.build().expect("file")
    }

    fn spec() -> LayerSpec {
        LayerSpec {
            text: "Hi area".into(),
            runs: vec![RunSpec {
                len: 8,
                size: Some(24.0),
                font: Some("NotoSans-Regular".into()),
                color: Some([255, 0, 0]),
                ..Default::default()
            }],
            paragraph: ParagraphSpec {
                justification: 2,
                ..Default::default()
            },
            aa: Some(3),
            frame: Some([10.0, 20.0, 210.0, 120.0]),
            ..Default::default()
        }
    }

    /// A fresh box-text spec authors its `Txt2` object at the spec's index and
    /// reads its frame back: the object has no composed `/99 /F` frame
    /// dictionary (Photoshop builds that on open, from the `BoxBounds` the
    /// writer puts in the `EngineData`), so `read_layer` takes the frame from
    /// those bounds instead of inventing an object frame.
    #[test]
    fn fresh_box_object_reads_back_its_frame() {
        let spec = spec();
        let tysh = build_tysh(&spec, Some([10.0, 20.0, 210.0, 120.0])).expect("tysh");
        let txt2 = build_txt2(&[(0, &spec)], None).expect("txt2");
        assert_eq!(
            crate::txt2::object_text(&txt2, 0).as_deref(),
            Some(spec.text.as_str()),
            "the authored object is missing"
        );
        assert!(
            crate::txt2::object_frame_geometry(&txt2, 0).is_none(),
            "a fresh object must not carry a zeroed composed frame"
        );
        let file = file_with_blocks(&tysh, Some(txt2));
        let read = crate::psd::read_layer(&file, 0).expect("read");
        assert_eq!(read.text, spec.text);
        assert_eq!(read.frame, spec.frame);
    }

    /// `preserve_trailing_break` keeps the empty final paragraph an editor's
    /// trailing newline means: the default trims it (`"Hi\n"` -> `"Hi"`), the
    /// flag re-appends one U+000D per trimmed break and the run lengths must
    /// cover it (`"Hi\r"` -> 4 units with the terminator).
    #[test]
    fn trailing_break_survives_only_when_asked() {
        let mut spec = spec();
        spec.text = "Hi\n".into();
        spec.runs = vec![RunSpec {
            len: 3,
            ..Default::default()
        }];
        let trimmed = build_txt2(&[(0, &spec)], None).expect("trimmed");
        assert_eq!(
            crate::txt2::object_text(&trimmed, 0).as_deref(),
            Some("Hi"),
            "the default must trim the trailing break"
        );
        spec.preserve_trailing_break = true;
        spec.runs[0].len = 4;
        let kept = build_txt2(&[(0, &spec)], None).expect("kept");
        assert_eq!(
            crate::txt2::object_text(&kept, 0).as_deref(),
            Some("Hi\r"),
            "the flag must keep one paragraph break"
        );
        // Two hard-broken paragraphs: "Hi" and the empty one after it.
        let tysh = build_tysh(&spec, Some([10.0, 20.0, 210.0, 120.0])).expect("tysh");
        let file = file_with_blocks(&tysh, Some(kept));
        let read = crate::psd::read_layer(&file, 0).expect("read");
        assert_eq!(read.paragraphs.len(), 2, "{:?}", read.paragraphs);
        assert_eq!(read.text, "Hi");
    }

    /// Walks the first `Rendered` shape's `Procession`.
    fn first_shape<'a>(
        engine: &'a crate::engine_data::Value,
        key: &str,
    ) -> Option<&'a crate::engine_data::Value> {
        let shapes = engine
            .get("EngineDict")?
            .get("Rendered")?
            .get("Shapes")?;
        if key == "WritingDirection" {
            return shapes.get(key);
        }
        match shapes.get("Children")? {
            crate::engine_data::Value::Array(items) => items.first()?.get(key),
            _ => None,
        }
    }

    /// Authoring must take the orientation from one source: `TySh` `Ornt` and the engine's
    /// `WritingDirection`/`Procession` all come from `spec.orientation`. A document whose
    /// vertical engine data says `WritingDirection 2` while `Ornt` says `Hrzn` makes Photoshop
    /// refuse to open it ("Could not complete your request because of a program error").
    #[test]
    fn vertical_spec_writes_consistent_orientation() {
        use photocraft_psd::text_layer::{Orientation, TySh};
        for frame in [None, Some([10.0, 20.0, 210.0, 120.0])] {
            for (orientation, want, direction, procession) in [
                (Some(1), Orientation::Vertical, Some(2), Some(1)),
                (Some(0), Orientation::Horizontal, Some(0), Some(0)),
                (None, Orientation::Horizontal, Some(0), Some(0)),
            ] {
                let layer_spec = LayerSpec {
                    orientation,
                    frame,
                    ..spec()
                };
                let tysh = build_tysh(&layer_spec, frame).expect("tysh");
                let parsed = TySh::from_bytes(&tysh).expect("parse");
                assert_eq!(parsed.orientation(), want, "frame {frame:?}");
                let engine = crate::engine_data::parse(parsed.engine_data().expect("engine data"))
                    .expect("parse engine data");
                assert_eq!(
                    first_shape(&engine, "WritingDirection").and_then(crate::engine_data::Value::as_i64),
                    direction,
                    "frame {frame:?} orientation {orientation:?}"
                );
                assert_eq!(
                    first_shape(&engine, "Procession").and_then(crate::engine_data::Value::as_i64),
                    procession,
                    "frame {frame:?} orientation {orientation:?}"
                );
            }
        }
    }

    /// A stale `Ornt` cannot make the reader flip a vertical layer: the engine's
    /// `WritingDirection` is authoritative, because that is what composes the glyphs (the user's
    /// broken file carries `Ornt = Hrzn` with `WritingDirection 2`).
    #[test]
    fn engine_data_orientation_wins_over_a_stale_ornt() {
        use photocraft_psd::text_layer::{Orientation, TySh};
        let vertical = LayerSpec {
            orientation: Some(1),
            frame: None,
            ..spec()
        };
        let tysh = build_tysh(&vertical, None).expect("tysh");
        let mut parsed = TySh::from_bytes(&tysh).expect("parse");
        assert_eq!(parsed.orientation(), Orientation::Vertical);
        // What the older writer produced: vertical EngineData under a horizontal `Ornt`.
        parsed.set_orientation(Orientation::Horizontal);
        let file = file_with_tysh(&parsed.to_bytes());
        let read = crate::psd::read_layer(&file, 0).expect("read");
        assert_eq!(read.orientation, Some(1), "the engine data must win");
        // Horizontal on both sides still reads horizontal.
        let horizontal = LayerSpec {
            orientation: Some(0),
            frame: None,
            ..spec()
        };
        let tysh = build_tysh(&horizontal, None).expect("tysh");
        let file = file_with_tysh(&tysh);
        let read = crate::psd::read_layer(&file, 0).expect("read");
        assert_eq!(read.orientation, Some(0));
    }

    /// EngineData numbers are 32-bit floats and Photoshop's parser reads a number token into a
    /// short buffer: a 17-character token (`.800000011920929`, a `f64` widened from `f32`) makes
    /// it abort the open. The writer must print the shortest `f32` form.
    #[test]
    fn engine_data_numbers_stay_inside_photoshops_token_budget() {
        let spec = spec();
        let tysh = build_tysh(&spec, Some([10.0, 20.0, 210.0, 120.0])).expect("tysh");
        let parsed = photocraft_psd::text_layer::TySh::from_bytes(&tysh).expect("parse");
        let blob = parsed.engine_data().expect("engine data");
        let text = String::from_utf8_lossy(blob);
        let token_len = |bytes: &[u8]| bytes.len();
        for token in text.split(|c: char| c.is_whitespace() || "[]()<>/".contains(c)) {
            if token.is_empty() {
                continue;
            }
            if token.parse::<f64>().is_ok() {
                assert!(
                    token_len(token.as_bytes()) <= 12,
                    "number token {token:?} is longer than Photoshop's parser accepts"
                );
            }
        }
        let widened = crate::engine_data::write(&crate::engine_data::Value::Float(
            0.800_000_011_920_929,
        ));
        assert!(
            widened.ends_with(b".8"),
            "the widened f32 must print as `.8`, got {:?}",
            String::from_utf8_lossy(&widened)
        );
    }

    /// The block the builder writes reads back through the reader: same text,
    /// run length, size, colour, frame and justification.
    #[test]
    fn built_tysh_reads_back() {
        let spec = spec();
        let tysh = build_tysh(&spec, Some([10.0, 20.0, 210.0, 120.0])).expect("build");
        let file = file_with_tysh(&tysh);
        let read = crate::psd::read_layer(&file, 0).expect("read");
        assert_eq!(read.text, spec.text);
        assert_eq!(read.runs.len(), 1, "{:?}", read.runs);
        assert_eq!(read.runs[0].len, 8);
        assert_eq!(read.runs[0].size, Some(24.0));
        assert_eq!(read.runs[0].color, Some([255, 0, 0]));
        assert_eq!(read.frame, Some([10.0, 20.0, 210.0, 120.0]));
        assert_eq!(read.paragraph.justification, 2);
        assert_eq!(read.aa, Some(3));
    }

    /// Per-paragraph specs (and the `EngineData`-only `Burasagari`/
    /// `KinsokuOrder`) survive the block: two paragraphs with different
    /// alignment, indents and flags read back as two `ParagraphSpec`s.
    #[test]
    fn paragraph_specs_round_trip() {
        use crate::psd::ParagraphSpec;
        let mut spec = spec();
        spec.text = "First\rSecond".into();
        spec.runs = vec![RunSpec {
            len: 13,
            size: Some(24.0),
            font: Some("NotoSans-Regular".into()),
            ..Default::default()
        }];
        let first = ParagraphSpec {
            justification: 2,
            first_line_indent: 12.0,
            burasagari: true,
            kinsoku_order: 2,
            ..Default::default()
        };
        let second = ParagraphSpec {
            justification: 1,
            burasagari: false,
            kinsoku_order: 0,
            ..Default::default()
        };
        spec.paragraph = first.clone();
        spec.paragraphs = vec![first.clone(), second.clone()];
        let tysh = build_tysh(&spec, None).expect("build");
        let file = file_with_tysh(&tysh);
        let read = crate::psd::read_layer(&file, 0).expect("read");
        assert_eq!(read.paragraphs.len(), 2, "{:?}", read.paragraphs);
        assert_eq!(read.paragraphs[0].justification, first.justification);
        assert_eq!(
            read.paragraphs[0].first_line_indent,
            first.first_line_indent
        );
        assert!(read.paragraphs[0].burasagari);
        assert_eq!(read.paragraphs[0].kinsoku_order, 2);
        assert_eq!(read.paragraphs[1].justification, second.justification);
        assert!(!read.paragraphs[1].burasagari);
        assert_eq!(read.paragraph.justification, first.justification);
    }

    #[test]
    fn empty_runs_are_refused() {
        let mut spec = spec();
        spec.runs.clear();
        assert!(build_tysh(&spec, None).is_err());
    }
}

/// [`build_tysh`] with the template a caller's layer already carries.
///
/// A real Photoshop `TySh` keeps its unmodelled descriptor keys (warp,
/// `textGridding`, the antialias value's stored form) and its trailing bytes;
/// only the text, the `EngineData` mirror, the orientation, the transform and
/// the ink bounds are rewritten. `None` builds a fresh block, exactly like
/// [`build_tysh`].
pub fn build_tysh_from(
    template: Option<&[u8]>,
    spec: &LayerSpec,
    bounds: Option<[f32; 4]>,
) -> Result<Vec<u8>, Error> {
    let text = author::spec_text(spec);
    author::validate_runs(&text, &spec.runs)?;
    let styles = author::run_styles(&spec.runs);
    let paragraphs = author::paragraph_runs_for(&text, spec);
    let tysh = match template.and_then(|raw| TySh::from_bytes(raw).ok()) {
        Some(mut tysh) => {
            tysh.set_text(&text);
            tysh
        }
        None => {
            let mut tysh = TySh::new_text_points(
                &text,
                b"",
                spec.transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
                [0.0; 4],
            );
            tysh.set_orientation(if spec.orientation == Some(1) {
                photocraft_psd::text_layer::Orientation::Vertical
            } else {
                photocraft_psd::text_layer::Orientation::Horizontal
            });
            tysh
        }
    };
    let bytes = author::tysh_block(&tysh, &text, spec, styles, paragraphs)?;
    let mut tysh = TySh::from_bytes(&bytes)?;
    if let Some(orientation) = spec.orientation {
        tysh.set_orientation(if orientation == 1 {
            photocraft_psd::text_layer::Orientation::Vertical
        } else {
            photocraft_psd::text_layer::Orientation::Horizontal
        });
    }
    if let Some(transform) = spec.transform {
        tysh.set_transform(transform);
    }
    if let Some(bounds) = bounds {
        let rect = [
            f64::from(bounds[0]),
            f64::from(bounds[1]),
            f64::from(bounds[2]),
            f64::from(bounds[3]),
        ];
        tysh.set_layout_bounds(rect);
        tysh.set_ink_bounds(rect);
        tysh.set_bounds_points(bounds);
    }
    Ok(tysh.to_bytes())
}

/// The document `Txt2` block for `objects` (`(TextIndex, spec)`), starting from
/// `previous` so Photoshop's own objects and every unmodelled key survive.
///
/// This is the `Txt2` half of [`crate::psd::author_layer`]: each object's text
/// and sheets are written (or kept when they already agree) and the block is
/// returned for a caller that keeps its own document model.
pub fn build_txt2(objects: &[(i32, &LayerSpec)], previous: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    let mut block = previous
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| author::NATIVE_TXT2_SCAFFOLD.to_vec());
    for (index, spec) in objects {
        let text = author::spec_text(spec);
        author::validate_runs(&text, &spec.runs)?;
        let object = usize::try_from(*index).unwrap_or(0);
        let agreed =
            previous.is_some_and(|prev| author::object_agrees(prev, object, &text, spec));
        if !agreed {
            // A view must name a frame the list has, and every object this writer
            // creates names frame `object`: pad the list even for point text, or
            // Photoshop sees an object naming a frame that isn't there.
            block = txt2::ensure_frame_list(&block, object + 1)?;
            block = author::write_object(block, object, &text, spec)?;
        }
    }
    Ok(block)
}

/// Applies `spec` to an existing `EngineData` tree, the tree-level form of the
/// writer [`build_engine_data`] uses: a caller that keeps Photoshop's own
/// `EngineData` (a `TySh` template it did not parse) edits it in place so every
/// unmodelled key survives.
pub fn apply_engine_data(root: &mut crate::engine_data::Value, spec: &LayerSpec) -> Result<(), Error> {
    let text = author::spec_text(spec);
    author::validate_runs(&text, &spec.runs)?;
    let mut styles = author::run_styles(&spec.runs);
    let paragraphs = author::paragraph_runs_for(&text, spec);
    super::engine_data::apply_engine_data(root, &text, spec, &mut styles, &paragraphs);
    Ok(())
}
