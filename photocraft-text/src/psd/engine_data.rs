//! Edits the EngineData inside a TySh descriptor without changing its wrapper.

use photocraft_psd::descriptor::{Value as DescValue, VersionedDescriptor};

use crate::engine_data::{self, Value};
use crate::runs::{self, ParagraphRun, RunStyle};

use super::{Error, LayerSpec};

pub(super) fn text_descriptor(block: &[u8]) -> Result<(VersionedDescriptor, usize), Error> {
    let data = block.get(52..).ok_or(Error::TruncatedTySh {
        needed: 52,
        len: block.len(),
    })?;
    Ok(VersionedDescriptor::parse_prefix(data)?)
}

/// Rewrites the `EngineData` blob of a `TySh` block through `edit`.
///
/// The descriptor wrapper (version, transform, text version, warp, bounds) is
/// kept byte for byte; only the `EngineData` value is re-serialised, and the
/// block's even length is preserved.
pub fn with_engine_data(
    block: &[u8],
    edit: impl FnOnce(&mut engine_data::Value),
) -> Result<Vec<u8>, Error> {
    let (mut text_desc, used) = text_descriptor(block)?;
    let version = u16::from_be_bytes([block[0], block[1]]);
    let transform = &block[2..50];
    let text_version = u16::from_be_bytes([block[50], block[51]]);
    // The fixed header has been checked by text_descriptor.
    let warp_at = 52 + used;
    let warp_header = block
        .get(warp_at..warp_at + 2)
        .ok_or(Error::TruncatedTySh {
            needed: warp_at + 2,
            len: block.len(),
        })?;
    let warp_version = u16::from_be_bytes([warp_header[0], warp_header[1]]);
    let (warp_desc, warp_used) = VersionedDescriptor::parse_prefix(&block[warp_at + 2..])?;
    let tail = &block[warp_at + 2 + warp_used..];
    if tail.len() < 16 {
        return Err(Error::TruncatedTySh {
            needed: warp_at + 2 + warp_used + 16,
            len: block.len(),
        });
    }

    let Some(DescValue::RawData(raw)) = text_desc.descriptor.get("EngineData").cloned() else {
        return Err(Error::MissingEngineData);
    };
    let mut ed = engine_data::parse(&raw)?;
    edit(&mut ed);
    let bytes = engine_data::write(&ed);
    for (key, value) in text_desc.descriptor.items.iter_mut() {
        if key.is("EngineData") {
            *value = DescValue::RawData(bytes.clone());
        }
    }

    let mut out = Vec::with_capacity(block.len());
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(transform);
    out.extend_from_slice(&text_version.to_be_bytes());
    out.extend_from_slice(&text_desc.to_bytes());
    out.extend_from_slice(&warp_version.to_be_bytes());
    out.extend_from_slice(&warp_desc.to_bytes());
    out.extend_from_slice(tail);
    if out.len() % 2 != 0 {
        out.push(0);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Building the EngineData a text layer without a `TySh` template needs.
// ---------------------------------------------------------------------------

/// Applies `spec` to an existing `EngineData` tree: fonts, the style and
/// paragraph run tables, the antialias code, fractional widths and the editor
/// text.
///
/// The tree keeps every key this writer does not model (Photoshop's own
/// `Rendered` tree, cookies, per-glyph data), which is why the template path
/// never rebuilds it from scratch.
pub(super) fn apply_engine_data(
    root: &mut Value,
    text: &str,
    spec: &LayerSpec,
    styles: &mut [RunStyle],
    paragraphs: &[ParagraphRun],
) {
    // A template without an `EngineDict` (a `TySh` that carries none, or a
    // root that is not a dictionary at all) has nothing to edit in place:
    // synthesise the engine tree from the spec instead, so the layer stays
    // editable. The template's other top-level keys are kept.
    if root.get("EngineDict").is_none() {
        let fresh = build_engine_data(text, spec, styles, paragraphs);
        match (&mut *root, fresh) {
            (Value::Dict(top), Value::Dict(mut fresh_top)) => {
                for key in ["EngineDict", "ResourceDict", "DocumentResources"] {
                    if let Some(pos) = fresh_top.iter().position(|(k, _)| k == key) {
                        let (_, value) = fresh_top.remove(pos);
                        match top.iter_mut().find(|(k, _)| *k == key) {
                            Some((_, slot)) => *slot = value,
                            None => top.push((key.to_string(), value)),
                        }
                    }
                }
            }
            (slot, fresh) => *slot = fresh,
        }
    }
    for (i, run) in spec.runs.iter().enumerate() {
        if let Some(name) = &run.font
            && let Some(style) = styles.get_mut(i)
        {
            style.font_index = runs::ensure_engine_font(root, name).map(|v| v as i64);
        }
    }
    runs::apply_style_runs(root, styles);
    runs::apply_paragraph_runs(root, paragraphs);
    set_paragraph_extras(root, spec);
    if let Some(aa) = spec.aa.and_then(crate::raster::AntiAlias::from_code) {
        runs::set_antialias(root, aa);
    }
    if let Some(on) = spec.fractional {
        runs::set_fractional_glyph_widths(root, on);
    }
    set_frame(root, frame_of(spec));
    ensure_photoshop_scaffold(root);
    set_fill_flag(root);
    set_editor_text(root, text);
}

/// The frame Photoshop's `EngineData` cookie carries: the explicit
/// [`LayerSpec::frame`], else the bounding box of the stored shape's points.
fn frame_of(spec: &LayerSpec) -> Option<[f32; 4]> {
    if let Some(frame) = spec.frame {
        return Some(frame);
    }
    let points = &spec.shape.as_ref()?.frame.as_ref()?.points;
    let mut x0 = f32::INFINITY;
    let mut y0 = f32::INFINITY;
    let mut x1 = f32::NEG_INFINITY;
    let mut y1 = f32::NEG_INFINITY;
    for pair in points.chunks_exact(2) {
        let (x, y) = (pair[0] as f32, pair[1] as f32);
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    (x1 >= x0 && y1 >= y0).then_some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

/// Writes the layer's frame into the `EngineData` shape tree
/// (`EngineDict/Rendered/Shapes/Children[0]/Cookie/Photoshop`).
///
/// Photoshop composes box/area/path text from the cookie's `BoxBounds` (the
/// frame rectangle in text space, `[left top right bottom]`), not from the
/// `Txt2` frame: a frame change that is not mirrored here is invisible to
/// Photoshop, which keeps recomposing the old box (measured: authored frame
/// widths 479/480/481 all re-composed as 480 until `BoxBounds` followed). Point
/// text carries `PointBase [0 0]` instead and shape type 0. Every other cookie
/// key Photoshop wrote is kept.
pub fn set_frame(root: &mut Value, frame: Option<[f32; 4]>) {
    let Some(engine) = root.get_mut("EngineDict").and_then(Value::as_dict_mut) else {
        return;
    };
    let shape_type = if frame.is_some() { 1 } else { 0 };
    let Some(rendered) = dict_entry(engine, "Rendered") else {
        return;
    };
    let Some(shapes) = dict_entry(rendered, "Shapes") else {
        return;
    };
    if !shapes.iter().any(|(k, _)| k == "WritingDirection") {
        shapes.push(("WritingDirection".to_string(), Value::Int(0)));
    }
    if !shapes.iter().any(|(k, _)| k == "Children") {
        shapes.push(("Children".to_string(), Value::Array(Vec::new())));
    }
    let Some(children) = entry_mut(shapes, "Children").and_then(Value::as_array_mut) else {
        return;
    };
    if children.is_empty() {
        children.push(Value::dict([("ShapeType", Value::Int(shape_type))]));
    }
    let Some(child) = children.first_mut().and_then(Value::as_dict_mut) else {
        return;
    };
    set_entry(child, "ShapeType", Value::Int(shape_type));
    let Some(cookie) = dict_entry(child, "Cookie") else {
        return;
    };
    let Some(photoshop) = dict_entry(cookie, "Photoshop") else {
        return;
    };
    set_entry(photoshop, "ShapeType", Value::Int(shape_type));
    match frame {
        Some([left, top, right, bottom]) => {
            photoshop.retain(|(k, _)| k != "PointBase");
            let want = [f64::from(left), f64::from(top), f64::from(right), f64::from(bottom)];
            if !entry_mut(photoshop, "BoxBounds").is_some_and(|v| numbers_agree(v, &want)) {
                set_entry(
                    photoshop,
                    "BoxBounds",
                    Value::Array(want.into_iter().map(Value::Float).collect()),
                );
            }
        }
        None => {
            photoshop.retain(|(k, _)| k != "BoxBounds");
            let want = [0.0, 0.0];
            if !entry_mut(photoshop, "PointBase").is_some_and(|v| numbers_agree(v, &want)) {
                set_entry(
                    photoshop,
                    "PointBase",
                    Value::Array(want.into_iter().map(Value::Float).collect()),
                );
            }
        }
    }
}

/// Whether `value` is a number array equal to `want` (so an unchanged frame keeps
/// Photoshop's own formatting and an unedited block stays byte-identical).
fn numbers_agree(value: &Value, want: &[f64]) -> bool {
    let Value::Array(values) = value else {
        return false;
    };
    values.len() == want.len()
        && values.iter().zip(want).all(|(value, want)| match value {
            Value::Float(v) => (v - want).abs() <= 1e-6,
            Value::Int(i) => (*i as f64 - want).abs() <= 1e-6,
            _ => false,
        })
}

/// The entry `key` of a dictionary, created (as an empty dictionary) when absent.
fn dict_entry<'a>(
    dict: &'a mut Vec<(String, Value)>,
    key: &str,
) -> Option<&'a mut Vec<(String, Value)>> {
    if !dict.iter().any(|(k, _)| k == key) {
        dict.push((key.to_string(), Value::Dict(Vec::new())));
    }
    entry_mut(dict, key).and_then(Value::as_dict_mut)
}

fn entry_mut<'a>(
    dict: &'a mut Vec<(String, Value)>,
    key: &str,
) -> Option<&'a mut Value> {
    dict.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Replaces `key` in a dictionary, creating the entry when absent.
fn set_entry(entries: &mut Vec<(String, Value)>, key: &str, value: Value) {
    match entries.iter_mut().find(|(k, _)| k == key) {
        Some((_, slot)) => *slot = value,
        None => entries.push((key.to_string(), value)),
    }
}

/// Writes the paragraph properties `ParagraphRun` does not carry: `Burasagari`
/// (CJK hanging punctuation) and `KinsokuOrder`. They exist only in
/// `EngineData`; `Txt2` has no measured slot for either.
fn set_paragraph_extras(root: &mut Value, spec: &LayerSpec) {
    let Some(items) = root
        .get_mut("EngineDict")
        .and_then(|engine| engine.get_mut("ParagraphRun"))
        .and_then(|run| run.get_mut("RunArray"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for (index, item) in items.iter_mut().enumerate() {
        let para = super::author::paragraph_spec(spec, index);
        let Some(properties) = item
            .get_mut("ParagraphSheet")
            .and_then(|sheet| sheet.get_mut("Properties"))
            .and_then(Value::as_dict_mut)
        else {
            continue;
        };
        set_property(properties, "Burasagari", Value::Bool(para.burasagari));
        set_property(properties, "KinsokuOrder", Value::Int(para.kinsoku_order));
    }
}

/// Replaces `key` in an `EngineData` property dict, leaving the existing value
/// untouched when it already says the same thing (so an untouched block keeps
/// its bytes).
fn set_property(properties: &mut Vec<(String, Value)>, key: &str, value: Value) {
    let same = |existing: &Value| match (existing, &value) {
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        _ => false,
    };
    match properties.iter_mut().find(|(k, _)| k == key) {
        Some((_, slot)) if !same(slot) => *slot = value,
        Some(_) => {}
        None => properties.push((key.to_string(), value)),
    }
}

/// Writes `EngineDict/Editor/Text` (`text` plus the terminal U+000D Photoshop
/// keeps).
fn set_editor_text(root: &mut Value, text: &str) {
    let value = Value::Str(format!("{text}\r"));
    if let Some(Value::Dict(entries)) = root.get_mut("EngineDict").and_then(|d| d.get_mut("Editor"))
    {
        match entries.iter_mut().find(|(k, _)| k == "Text") {
            Some((_, slot)) => *slot = value,
            None => entries.push(("Text".to_string(), value)),
        }
    }
}

/// The `ParagraphSheet` item `apply_paragraph_runs` fills in from its template.
fn paragraph_template() -> Value {
    Value::dict([(
        "ParagraphSheet",
        Value::dict([
            ("DefaultStyleSheet", Value::Int(0)),
            ("Properties", Value::dict([("LeadingType", Value::Int(0))])),
        ]),
    )])
}


/// Photoshop 20.0.2's text resources: the superscript/subscript metrics and the small-cap size
/// every `EngineData` block carries. CoolType reads them even for plain Latin text; a block
/// without them aborts Photoshop's open ("General Photoshop error occurred…").
const RESOURCE_SCALARS: [(&str, f64); 5] = [
    ("SuperscriptSize", 0.583),
    ("SuperscriptPosition", 0.333),
    ("SubscriptSize", 0.583),
    ("SubscriptPosition", 0.333),
    ("SmallCapSize", 0.7),
];

/// `EngineDict/GridInfo`: Photoshop's grid settings for the layer (off, but present).
fn grid_info() -> Value {
    let black = || {
        Value::dict([
            ("Type", Value::Int(1)),
            ("Values", Value::Array(vec![Value::Float(0.0), Value::Float(0.0), Value::Float(0.0), Value::Float(1.0)])),
        ])
    };
    Value::dict([
        ("GridIsOn", Value::Bool(false)),
        ("ShowGrid", Value::Bool(false)),
        ("GridSize", Value::Float(18.0)),
        ("GridLeading", Value::Float(22.0)),
        ("GridColor", black()),
        ("GridLeadingFillColor", black()),
        ("AlignLineHeightToGridFlags", Value::Bool(false)),
    ])
}

/// `ParagraphRun/DefaultRunData`: the default paragraph sheet and the per-axis adjustments.
fn default_paragraph_run_data() -> Value {
    Value::dict([
        (
            "ParagraphSheet",
            Value::dict([
                ("DefaultStyleSheet", Value::Int(0)),
                ("Properties", Value::Dict(Vec::new())),
            ]),
        ),
        (
            "Adjustments",
            Value::dict([
                ("Axis", Value::Array(vec![Value::Float(1.0), Value::Float(0.0), Value::Float(1.0)])),
                ("XY", Value::Array(vec![Value::Float(0.0), Value::Float(0.0)])),
            ]),
        ),
    ])
}

/// Photoshop's Kinsoku (line-breaking) sets, as CoolType reads them from `ResourceDict`.
fn kinsoku_set() -> Value {
    Value::Array(vec![
        Value::dict([
            ("Name", Value::Str("PhotoshopKinsokuHard".to_string())),
            (
                "NoStart",
                Value::Str("、。，．・：；？！ー―’”）〕］｝〉》」』】ヽヾゝゞ々ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮヵヶ゛゜?!)]},.:;℃℉¢％‰".to_string()),
            ),
            ("NoEnd", Value::Str("‘“（〔［｛〈《「『【([{￥＄£＠§〒＃".to_string())),
            ("Keep", Value::Str("―‥".to_string())),
            ("Hanging", Value::Str("、。.,".to_string())),
        ]),
        Value::dict([
            ("Name", Value::Str("PhotoshopKinsokuSoft".to_string())),
            ("NoStart", Value::Str("、。，．・：；？！’”）〕］｝〉》」』】ヽヾゝゞ々".to_string())),
            ("NoEnd", Value::Str("‘“（〔［｛〈《「『【".to_string())),
            ("Keep", Value::Str("―‥".to_string())),
            ("Hanging", Value::Str("、。.,".to_string())),
        ]),
    ])
}

/// The MojiKumi (kerning) set names; Photoshop stores the sets in its own resources and only
/// names them here.
fn mojikumi_set() -> Value {
    Value::Array(
        (1..=4)
            .map(|n| Value::dict([("InternalName", Value::Str(format!("Photoshop6MojiKumiSet{n}")))]))
            .collect(),
    )
}


/// Replaces a sparse default character or paragraph sheet an older writer wrote (CoolType reads
/// it as the fallback for every run that names it) with Photoshop's full default. A Photoshop
/// sheet is already complete and is never touched.
fn ensure_default_sheets(resources: &mut Vec<(String, Value)>) {
    let full_style = default_style_sheet;
    let full_paragraph = default_paragraph_sheet;
    for (key, default, sheet_key, minimum) in [
        ("StyleSheetSet", full_style as fn() -> Value, "StyleSheetData", 20usize),
        ("ParagraphSheetSet", full_paragraph as fn() -> Value, "Properties", 15usize),
    ] {
        let Some(Value::Array(sheets)) = entry_mut(resources, key) else {
            continue;
        };
        let Some(Value::Dict(entry)) = sheets.first_mut() else {
            continue;
        };
        let sparse = entry
            .iter()
            .find(|(k, _)| k == sheet_key)
            .and_then(|(_, value)| value.as_dict())
            .is_none_or(|data| data.len() < minimum);
        if sparse {
            match entry.iter_mut().find(|(k, _)| k == sheet_key) {
                Some((_, slot)) => *slot = default(),
                None => entry.push((sheet_key.to_string(), default())),
            }
        }
    }
}

/// Adds the keys Photoshop's CoolType requires and an older writer's block can lack.
///
/// A fresh `EngineData` without them opens in PhotoCraft, but Photoshop 20 aborts on open with
/// "General Photoshop error occurred. This functionality may not be available in this version of
/// Photoshop." A value that is already there is never overwritten: a Photoshop-authored template
/// keeps its own bytes.
pub(super) fn ensure_photoshop_scaffold(root: &mut Value) {
    let Some(top) = root.as_dict_mut() else {
        return;
    };
    if let Some(engine) = dict_entry(top, "EngineDict") {
        if !engine.iter().any(|(k, _)| k == "GridInfo") {
            engine.push(("GridInfo".to_string(), grid_info()));
        }
        if let Some(paragraph_run) = dict_entry(engine, "ParagraphRun")
            && !paragraph_run.iter().any(|(k, _)| k == "DefaultRunData")
        {
            paragraph_run.push(("DefaultRunData".to_string(), default_paragraph_run_data()));
        }
    }
    for key in ["ResourceDict", "DocumentResources"] {
        let Some(resources) = dict_entry(top, key) else {
            continue;
        };
        ensure_default_sheets(resources);
        if !resources.iter().any(|(k, _)| k == "KinsokuSet") {
            resources.push(("KinsokuSet".to_string(), kinsoku_set()));
        }
        if !resources.iter().any(|(k, _)| k == "MojiKumiSet") {
            resources.push(("MojiKumiSet".to_string(), mojikumi_set()));
        }
        for (name, value) in RESOURCE_SCALARS {
            if !resources.iter().any(|(k, _)| k == name) {
                resources.push((name.to_string(), Value::Float(value)));
            }
        }
        for name in ["TheNormalStyleSheet", "TheNormalParagraphSheet"] {
            if !resources.iter().any(|(k, _)| k == name) {
                resources.push((name.to_string(), Value::Int(0)));
            }
        }
    }
    // `DocumentResources` mirrors the resource tables (Photoshop writes both copies).
    let mirrored: Vec<(String, Value)> = top
        .iter()
        .find(|(k, _)| k == "ResourceDict")
        .map(|(_, value)| value.as_dict().map(<[(String, Value)]>::to_vec).unwrap_or_default())
        .unwrap_or_default();
    if let Some(document) = dict_entry(top, "DocumentResources") {
        for (name, value) in mirrored {
            let mirrored_key = matches!(
                name.as_str(),
                "StyleSheetSet"
                    | "ParagraphSheetSet"
                    | "FontSet"
                    | "KinsokuSet"
                    | "MojiKumiSet"
                    | "SuperscriptSize"
                    | "SuperscriptPosition"
                    | "SubscriptSize"
                    | "SubscriptPosition"
                    | "SmallCapSize"
                    | "TheNormalStyleSheet"
                    | "TheNormalParagraphSheet"
            );
            if mirrored_key && !document.iter().any(|(k, _)| *k == name) {
                document.push((name, value));
            }
        }
    }
}


/// The shared default character sheet Photoshop writes in
/// `ResourceDict/StyleSheetSet` (and mirrors in `DocumentResources`). CoolType reads it as the
/// fallback when a run names a sheet that carries no data of its own.
fn default_style_sheet() -> Value {
    Value::dict([
        ("Font", Value::Int(0)),
        ("FontSize", Value::Float(12.0)),
        ("FauxBold", Value::Bool(false)),
        ("FauxItalic", Value::Bool(false)),
        ("AutoLeading", Value::Bool(true)),
        ("Leading", Value::Float(0.0)),
        ("HorizontalScale", Value::Float(1.0)),
        ("VerticalScale", Value::Float(1.0)),
        ("Tracking", Value::Int(0)),
        ("AutoKerning", Value::Bool(true)),
        ("Kerning", Value::Int(0)),
        ("BaselineShift", Value::Float(0.0)),
        ("FontCaps", Value::Int(0)),
        ("FontBaseline", Value::Int(0)),
        ("Underline", Value::Bool(false)),
        ("Strikethrough", Value::Bool(false)),
        ("Ligatures", Value::Bool(true)),
        ("DLigatures", Value::Bool(false)),
        ("BaselineDirection", Value::Int(2)),
        ("Tsume", Value::Float(0.0)),
        ("StyleRunAlignment", Value::Int(2)),
        ("Language", Value::Int(0)),
        ("NoBreak", Value::Bool(false)),
        ("FillColor", Value::dict([
            ("Type", Value::Int(1)),
            ("Values", Value::Array(vec![Value::Float(1.0), Value::Float(0.0), Value::Float(0.0), Value::Float(0.0)])),
        ])),
        ("StrokeColor", Value::dict([
            ("Type", Value::Int(1)),
            ("Values", Value::Array(vec![Value::Float(1.0), Value::Float(0.0), Value::Float(0.0), Value::Float(0.0)])),
        ])),
        ("FillFlag", Value::Bool(true)),
        ("StrokeFlag", Value::Bool(false)),
        ("FillFirst", Value::Bool(true)),
        ("YUnderline", Value::Int(1)),
        ("OutlineWidth", Value::Float(1.0)),
        ("CharacterDirection", Value::Int(0)),
        ("HindiNumbers", Value::Bool(false)),
        ("Kashida", Value::Int(1)),
        ("DiacriticPos", Value::Int(2)),
    ])
}

/// The shared default paragraph sheet (`ParagraphSheetSet/TheNormalParagraphSheet`).
fn default_paragraph_sheet() -> Value {
    Value::dict([
        ("Justification", Value::Int(0)),
        ("FirstLineIndent", Value::Float(0.0)),
        ("StartIndent", Value::Float(0.0)),
        ("EndIndent", Value::Float(0.0)),
        ("SpaceBefore", Value::Float(0.0)),
        ("SpaceAfter", Value::Float(0.0)),
        ("AutoHyphenate", Value::Bool(true)),
        ("HyphenatedWordSize", Value::Int(6)),
        ("PreHyphen", Value::Int(2)),
        ("PostHyphen", Value::Int(2)),
        ("ConsecutiveHyphens", Value::Int(8)),
        ("Zone", Value::Float(36.0)),
        ("WordSpacing", Value::Array(vec![Value::Float(0.8), Value::Float(1.0), Value::Float(1.33)])),
        ("LetterSpacing", Value::Array(vec![Value::Float(0.0), Value::Float(0.0), Value::Float(0.0)])),
        ("GlyphSpacing", Value::Array(vec![Value::Float(1.0), Value::Float(1.0), Value::Float(1.0)])),
        ("AutoLeading", Value::Float(1.2)),
        ("LeadingType", Value::Int(0)),
        ("Hanging", Value::Bool(false)),
        ("Burasagari", Value::Bool(false)),
        ("KinsokuOrder", Value::Int(0)),
        ("EveryLineComposer", Value::Bool(false)),
    ])
}

/// `Rendered/Shapes/Children[i]/Cookie/Photoshop`: the shape base CoolType needs to place the
/// text. Photoshop writes `PointBase` for point text and `BoxBounds` for paragraph text, always
/// with an identity `Base` transform in text space.
fn shape_cookie(shape_type: i64, bounds: Option<[f32; 4]>) -> Value {
    let mut items = vec![("ShapeType".to_string(), Value::Int(shape_type))];
    match bounds {
        Some([_, _, width, height]) if shape_type != 0 => items.push((
            "BoxBounds".to_string(),
            Value::Array(vec![
                Value::Float(0.0),
                Value::Float(0.0),
                Value::Float(f64::from(width)),
                Value::Float(f64::from(height)),
            ]),
        )),
        _ => items.push((
            "PointBase".to_string(),
            Value::Array(vec![Value::Float(0.0), Value::Float(0.0)]),
        )),
    }
    items.push((
        "Base".to_string(),
        Value::dict([
            ("ShapeType", Value::Int(shape_type)),
            (
                "TransformPoint0",
                Value::Array(vec![Value::Float(1.0), Value::Float(0.0)]),
            ),
            (
                "TransformPoint1",
                Value::Array(vec![Value::Float(0.0), Value::Float(1.0)]),
            ),
            (
                "TransformPoint2",
                Value::Array(vec![Value::Float(0.0), Value::Float(0.0)]),
            ),
        ]),
    ));
    Value::dict(items)
}

/// A complete `EngineData` tree for a text layer that has no `TySh` template
/// (a layer added by a writer that left the block empty).
///
/// The shape is the one Photoshop 20.0.2 writes for a plain layer: `EngineDict`
/// with `Editor`, `StyleRun`, `ParagraphRun`, `AntiAlias`, fractional widths and
/// a `Rendered/Shapes` tree naming the layer's shape (point or box) and writing
/// direction, plus `ResourceDict` with the font set and the shared default
/// sheets.
pub(super) fn build_engine_data(
    text: &str,
    spec: &LayerSpec,
    styles: &mut [RunStyle],
    paragraphs: &[ParagraphRun],
) -> Value {
    let mut root = Value::dict([
        ("EngineDict", Value::Dict(Vec::new())),
        (
            "ResourceDict",
            Value::dict([("FontSet", Value::Array(Vec::new()))]),
        ),
    ]);
    for (i, run) in spec.runs.iter().enumerate() {
        if let Some(name) = &run.font
            && let Some(style) = styles.get_mut(i)
        {
            style.font_index = runs::ensure_engine_font(&mut root, name).map(|v| v as i64);
        }
    }

    let style_run = runs::to_engine_data(styles);
    let mut paragraph_run = Value::dict([
        ("RunArray", Value::Array(vec![paragraph_template()])),
        ("RunLengthArray", Value::Array(Vec::new())),
        ("IsJoinable", Value::Int(1)),
    ]);
    // Fill the template through the shared writer so both paths produce the
    // same sheet shape.
    let mut seed = Value::dict([(
        "EngineDict",
        Value::dict([("ParagraphRun", paragraph_run.clone())]),
    )]);
    runs::apply_paragraph_runs(&mut seed, paragraphs);
    if let Some(filled) = seed
        .get("EngineDict")
        .and_then(|e| e.get("ParagraphRun"))
        .cloned()
    {
        paragraph_run = filled;
    }

    let vertical = spec.orientation == Some(1);
    let writing = if vertical { 2 } else { 0 };
    // A frame that is not finite (an area path with a NaN/infinite node) degrades to
    // point text: Photoshop cannot place a box there and the layer says so.
    let frame = frame_of(spec).filter(|b| b.iter().all(|v| v.is_finite()));
    let shape_type = if frame.is_some() { 1 } else { 0 };
    let child = Value::dict([
        ("ShapeType", Value::Int(shape_type)),
        ("Procession", Value::Int(i64::from(vertical))),
        (
            "Lines",
            Value::dict([
                ("WritingDirection", Value::Int(writing)),
                ("Children", Value::Array(Vec::new())),
            ]),
        ),
        (
            "Cookie",
            Value::dict([("Photoshop", shape_cookie(shape_type, frame))]),
        ),
    ]);
    let rendered = Value::dict([
        ("Version", Value::Int(1)),
        (
            "Shapes",
            Value::dict([
                ("WritingDirection", Value::Int(writing)),
                ("Children", Value::Array(vec![child])),
            ]),
        ),
    ]);

    let aa = spec
        .aa
        .and_then(crate::raster::AntiAlias::from_code)
        .unwrap_or_default()
        .code();
    let engine = Value::dict([
        (
            "Editor",
            Value::dict([("Text", Value::Str(format!("{text}\r")))]),
        ),
        ("ParagraphRun", paragraph_run),
        ("StyleRun", style_run),
        ("AntiAlias", Value::Int(aa)),
        (
            "UseFractionalGlyphWidths",
            Value::Bool(spec.fractional.unwrap_or(true)),
        ),
        ("Rendered", rendered),
    ]);
    if let Some(slot) = root.get_mut("EngineDict") {
        *slot = engine;
    }
    set_frame(&mut root, frame_of(spec));
    set_paragraph_extras(&mut root, spec);
    // The shared default sheets Photoshop keeps beside the fonts. CoolType reads them as the
    // fallback for a run that names a sheet with no data; a sparse one aborts the open.
    let default_sheet = default_style_sheet();
    let default_paragraph = default_paragraph_sheet();
    if let Some(Value::Dict(entries)) = root.get_mut("ResourceDict") {
        let push = |entries: &mut Vec<(String, Value)>, key: &str, value: Value| match entries
            .iter_mut()
            .find(|(k, _)| k == key)
        {
            Some((_, slot)) => *slot = value,
            None => entries.push((key.to_string(), value)),
        };
        push(
            entries,
            "StyleSheetSet",
            Value::Array(vec![Value::dict([
                ("Name", Value::Str("Normal RGB".to_string())),
                ("StyleSheetData", default_sheet.clone()),
            ])]),
        );
        push(
            entries,
            "ParagraphSheetSet",
            Value::Array(vec![Value::dict([
                ("Name", Value::Str("Normal RGB".to_string())),
                ("DefaultStyleSheet", Value::Int(0)),
                ("Properties", default_paragraph),
            ])]),
        );
        push(entries, "TheNormalStyleSheet", Value::Int(0));
        push(entries, "TheNormalParagraphSheet", Value::Int(0));
    }
    // Photoshop keeps a second copy of the shared tables in `DocumentResources` and its text
    // engine aborts on a block without the scaffold (`GridInfo`, `ParagraphRun/DefaultRunData`,
    // the resource scalars, the Kinsoku/MojiKumi sets): write the full set for a fresh layer.
    ensure_photoshop_scaffold(&mut root);
    // Last, so it also covers the `StyleSheetSet` entries written above.
    set_fill_flag(&mut root);
    root
}

/// Enables the text fill Photoshop paints from: every style sheet's
/// `FillFlag`. Photopea (and Photoshop's own text renderer) paint nothing for a
/// run whose sheet carries `FillColor` but no `FillFlag`, and a legacy export
/// written before this key existed has none, so an edit must add it. An
/// explicit stored value is never flipped (a disabled fill stays disabled).
pub fn set_fill_flag(root: &mut Value) {
    let Some(engine) = root.get_mut("EngineDict").and_then(Value::as_dict_mut) else {
        return;
    };
    if let Some(runs) = dict_entry(engine, "StyleRun")
        .and_then(|sr| entry_mut(sr, "RunArray"))
        .and_then(Value::as_array_mut)
    {
        for run in runs {
            let Some(sheet) = run
                .get_mut("StyleSheet")
                .and_then(Value::as_dict_mut)
                .and_then(|s| entry_mut(s, "StyleSheetData"))
                .and_then(Value::as_dict_mut)
            else {
                continue;
            };
            if !sheet.iter().any(|(k, _)| k == "FillFlag") {
                sheet.push(("FillFlag".to_string(), Value::Bool(true)));
            }
        }
    }
    for key in ["ResourceDict", "DocumentResources"] {
        let Some(sheets) = root
            .get_mut(key)
            .and_then(Value::as_dict_mut)
            .and_then(|d| entry_mut(d, "StyleSheetSet"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for sheet in sheets {
            let Some(entry) = sheet.as_dict_mut() else {
                continue;
            };
            let Some(data) = entry_mut(entry, "StyleSheetData").and_then(Value::as_dict_mut) else {
                continue;
            };
            if !data.iter().any(|(k, _)| k == "FillFlag") {
                data.push(("FillFlag".to_string(), Value::Bool(true)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RESOURCE_SCALARS, build_engine_data, with_engine_data};
    use crate::engine_data::{self, Value};
    use crate::runs::RunStyle;
    use crate::psd::LayerSpec;

    /// The key set of the dictionary at `path`, sorted.
    fn keys(value: &Value, path: &[&str]) -> Vec<String> {
        let mut node = value;
        for key in path {
            node = node
                .get(key)
                .unwrap_or_else(|| panic!("missing {key:?} under {path:?}"));
        }
        let mut keys: Vec<String> = node
            .as_dict()
            .unwrap_or_else(|| panic!("not a dictionary at {path:?}"))
            .iter()
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort();
        keys
    }

    /// A fresh layer's `EngineData` must carry the same scaffold Photoshop 20 writes: CoolType
    /// aborts the open of a block without `GridInfo`, `ParagraphRun/DefaultRunData`, the resource
    /// scalars and the Kinsoku/MojiKumi sets. `point_plain.engine.bin` is a recorded
    /// Photoshop-authored block, so it is the reference.
    #[test]
    fn fresh_engine_data_has_the_photoshop_scaffold() {
        let reference =
            engine_data::parse(include_bytes!("../../tests/assets/models/point_plain.engine.bin"))
                .expect("reference EngineData");
        let spec = LayerSpec::default();
        let mut styles = vec![RunStyle::default()];
        let fresh = build_engine_data("Hi", &spec, &mut styles, &[]);
        for path in [
            &[][..],
            &["EngineDict"][..],
            &["ResourceDict"][..],
            &["DocumentResources"][..],
            &["EngineDict", "ParagraphRun"][..],
            &["EngineDict", "GridInfo"][..],
            &["EngineDict", "ParagraphRun", "DefaultRunData"][..],
        ] {
            assert_eq!(keys(&fresh, path), keys(&reference, path), "key set at {path:?}");
        }
        // The scalars are Photoshop's own values, in both copies of the resource table.
        for (name, want) in RESOURCE_SCALARS {
            for table in ["ResourceDict", "DocumentResources"] {
                let got = fresh
                    .get(table)
                    .and_then(|table| table.get(name))
                    .and_then(Value::as_f32)
                    .unwrap_or_else(|| panic!("{table}/{name}"));
                assert!((got - want as f32).abs() <= 1e-6, "{table}/{name} = {got}");
            }
        }
        // `DocumentResources` mirrors `ResourceDict`, as Photoshop writes it.
        assert_eq!(
            keys(&fresh, &["ResourceDict"]),
            keys(&fresh, &["DocumentResources"]),
            "the two resource tables carry the same keys"
        );
    }

    /// A template from an older writer that carries a partial resource table is completed too;
    /// keys already present are never overwritten.
    #[test]
    fn an_old_template_is_completed_without_overwriting_its_values() {
        let mut root = Value::dict([
            ("EngineDict", Value::dict([("AntiAlias", Value::Int(3))])),
            (
                "ResourceDict",
                Value::dict([
                    ("SuperscriptSize", Value::Float(0.5)),
                    ("StyleSheetSet", Value::Array(vec![Value::dict([("Name", Value::Str("Old".into()))])])),
                ]),
            ),
        ]);
        super::ensure_photoshop_scaffold(&mut root);
        assert_eq!(
            root.get("ResourceDict")
                .and_then(|table| table.get("SuperscriptSize"))
                .and_then(Value::as_f32),
            Some(0.5),
            "an existing value is kept"
        );
        let mut node = &root;
        for key in ["DocumentResources", "KinsokuSet"] {
            node = node.get(key).unwrap_or_else(|| panic!("missing {key}"));
        }
        assert!(node.as_array().is_some_and(|set| !set.is_empty()), "DocumentResources/KinsokuSet");
        for path in [
            &["EngineDict", "GridInfo"][..],
            &["ResourceDict", "KinsokuSet"][..],
            &["ResourceDict", "MojiKumiSet"][..],
            &["ResourceDict", "SmallCapSize"][..],
            &["DocumentResources", "StyleSheetSet"][..],
        ] {
            let mut node = &root;
            for key in path {
                node = node.get(key).unwrap_or_else(|| panic!("missing {key:?} under {path:?}"));
            }
        }
        let _ = &mut node;
        assert!(
            root.get("DocumentResources")
                .and_then(|doc| doc.get("StyleSheetSet"))
                .and_then(Value::as_array)
                .is_some_and(|sheets| sheets.len() == 1),
            "the resource tables are mirrored"
        );
    }

    fn tysh(raw: &[u8]) -> Vec<u8> {
        photocraft_psd::TySh::new_text(
            "Hi",
            raw,
            [1.0, 0.0, 0.0, 1.0, 12.5, 40.0],
            [0, 0, 100, 100],
        )
        .to_bytes()
    }

    #[test]
    fn every_truncated_tysh_prefix_returns_an_error() {
        let block = tysh(&engine_data::write(&Value::Dict(Vec::new())));
        for len in 0..block.len() {
            assert!(
                with_engine_data(&block[..len], |_| {}).is_err(),
                "prefix {len}"
            );
        }
    }

    #[test]
    fn no_op_edit_preserves_the_wrapper_bytes() {
        let mut block = tysh(&engine_data::write(&Value::Dict(Vec::new())));
        if !block.len().is_multiple_of(2) {
            block.push(0);
        }
        assert_eq!(with_engine_data(&block, |_| {}).unwrap(), block);
    }

    #[test]
    fn parser_errors_keep_their_sources() {
        let error = with_engine_data(&[0; 52], |_| {}).unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
        let error = with_engine_data(&tysh(b"<< /broken [ >>"), |_| {}).unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
    }
}
