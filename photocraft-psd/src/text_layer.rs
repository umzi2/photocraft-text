//! Text layers: the `TySh` layer block, the document-level `Txt2` block, the
//! `photoshop:TextLayers` XMP list (resource 1060) and the layer's stored raster.
//!
//! ## `TySh`
//!
//! Layout (Adobe Photoshop File Formats, "Type Tool Object Setting"; all
//! big-endian): `u16 version (1)`, `6 × f64 transform`, `u16 text version`,
//! versioned text descriptor, `u16 warp version (1)`, versioned warp descriptor,
//! `4 × 4 bytes bounds`, then up to three bytes that pad the block.
//!
//! Measured against files written by Photoshop 20.0.2 (`corpus/*.psd`,
//! `out/probe5/cjkvert.psd`, `out/probe5/pE.psd`):
//!
//! * version 1, text version 50, warp version 1;
//! * the text descriptor is class `TxLr` with a one-NUL name and the keys, in
//!   order, `Txt `, `textGridding`, `Ornt`, `AntA`, `bounds`, `boundingBox`,
//!   `TextIndex`, `EngineData`. `Txt ` is NUL-terminated UTF-16 (paragraph
//!   breaks inside it are `U+000D`); `bounds` and `boundingBox` are objects of
//!   class `bounds` / `boundingBox` holding `Left`, `Top `, `Rght`, `Btom` as
//!   `#Pnt` unit floats — the layout box and the ink box in points;
//! * `Ornt` is `Hrzn` or `Vrtc`; vertical text also flips the rectangles (a
//!   vertical line is tall and narrow, see `out/probe5/cjkvert.psd`);
//! * the warp descriptor is class `warp` with exactly `warpStyle`, `warpValue`,
//!   `warpPerspective`, `warpPerspectiveOther`, `warpRotate`. `warpNone` is the
//!   unwarped style; `out/probe5/pE.psd` carries `warpArc`/`warpArch` at
//!   value 50;
//! * the 16 bytes after the warp descriptor are four big-endian **f32 point
//!   bounds**, not integers: `out/probe5/pE.psd` layer `warp_arc` stores
//!   `(0.0, -41.1357421875, 339.404296875, 18.67236328125)`, the same rectangle
//!   as its descriptor `bounds`, while unwarped layers store four zero f32.
//!   [`TySh::bounds_points`] reads them as such; the raw [`TySh::bounds`] field
//!   keeps the bits for byte-exact round-trips of any writer;
//! * `TextIndex` is the layer's index among the document's text layers, used to
//!   pair it with its object in `Txt2`.
//!
//! [`TySh::from_bytes`] rejects unknown `TySh` / warp versions and keeps
//! [`TySh::trailing`] so [`TySh::to_bytes`] reproduces every parsed block
//! byte for byte.
//!
//! ## `Txt2` and XMP
//!
//! `Txt2` is a single *document-level* additional-layer-info block, not a
//! per-layer one: `out/probe5/props1.psd` has 43 text layers and one `Txt2`.
//! Photoshop reads a layer's text from it; `TySh` is the modern companion other
//! applications read (see `docs/text-layer-storage.md`). The XMP packet keeps a
//! second copy: `photoshop:TextLayers` holds one `rdf:li` per text layer with
//! `photoshop:LayerName` and `photoshop:LayerText`, in bottom-to-top layer
//! order. Its `LayerText` may contain raw carriage returns
//! (`out/para.psd`) or character references such as `&#xA;` (`out/demo/*`).
//!
//! ## API
//!
//! [`PsdFile::text_layers`] enumerates text layers; [`PsdFile::text_layer_info`]
//! summarizes one. Readers and writers: [`PsdFile::text_layer_tysh`] /
//! [`PsdFile::set_text_layer_tysh`], [`PsdFile::text_layer_engine_data`] /
//! [`PsdFile::set_text_layer_engine_data`], [`PsdFile::text_layer_transform`] /
//! [`PsdFile::set_text_layer_transform`], [`PsdFile::text_layer_orientation`] /
//! [`PsdFile::set_text_layer_orientation`], [`PsdFile::text_layer_warp`] /
//! [`PsdFile::set_text_layer_warp`], [`PsdFile::txt2`] / [`PsdFile::set_txt2`],
//! [`PsdFile::text_layer_raster`] / [`PsdFile::set_text_layer_raster`], and the
//! XMP packet accessors [`PsdFile::xmp_packet`] / [`PsdFile::set_xmp_packet`] /
//! [`PsdFile::xmp_text_layers`] / [`PsdFile::set_xmp_text_layers`] /
//! [`PsdFile::sync_xmp_text_layers`]. Unknown tagged blocks are untouched by
//! every setter, so they survive byte for byte ([`TaggedBlock`] keeps the exact
//! padding that followed them).
//!
//! Setting a raster always encodes it with RLE (Photoshop 20.0.2 rejects
//! ZIP-compressed layers, see `docs/STATUS.md`).

use crate::compression::Compression;
use crate::descriptor::{Descriptor, Id, UnicodeString, Value, VersionedDescriptor};
use crate::error::{PsdError, Result};
use crate::file::PsdFile;
use crate::header::ColorMode;
use crate::io::Reader;
use crate::layer::{CHANNEL_TRANSPARENCY, LayerRecord, Rect};
use crate::resources::{ImageResource, ids};
use crate::tagged::TaggedBlock;

/// `TySh` version written and accepted.
pub const TY_SH_VERSION: u16 = 1;
/// Warp descriptor version written and accepted.
pub const WARP_VERSION: u16 = 1;
/// Text engine version [`TySh::new_text`] writes. Photoshop 20.0.2 writes 50
/// (`corpus/*.psd`, `out/probe5/*.psd`).
pub const TEXT_VERSION: u16 = 50;

/// `TySh` text descriptor key for the layer's text.
const KEY_TEXT: &str = "Txt ";
/// `TySh` text descriptor key for the text orientation (`Hrzn` / `Vrtc`).
const KEY_ORIENTATION: &str = "Ornt";
/// `TySh` text descriptor key for the gridding (`None`).
const KEY_TEXT_GRIDDING: &str = "textGridding";
/// `TySh` text descriptor key for the antialias mode.
const KEY_ANTIALIAS: &str = "AntA";
/// `TySh` text descriptor key for the layout rectangle (points).
const KEY_BOUNDS: &str = "bounds";
/// `TySh` text descriptor key for the ink rectangle (points).
const KEY_BOUNDING_BOX: &str = "boundingBox";
/// `TySh` text descriptor key for the layer's index among the text layers.
const KEY_TEXT_INDEX: &str = "TextIndex";
/// `TySh` text descriptor key for the `EngineData` payload.
const KEY_ENGINE_DATA: &str = "EngineData";
/// `TySh` text descriptor key for the transform some writers mirror the header in.
const KEY_TRANSFORM: &str = "transform";
/// `TySh` warp descriptor key for the style name.
const KEY_WARP_STYLE: &str = "warpStyle";
/// `TySh` warp descriptor key for the warp amount.
const KEY_WARP_VALUE: &str = "warpValue";
/// `TySh` warp descriptor key for the perspective amount.
const KEY_WARP_PERSPECTIVE: &str = "warpPerspective";
/// `TySh` warp descriptor key for the perspective other amount.
const KEY_WARP_PERSPECTIVE_OTHER: &str = "warpPerspectiveOther";
/// `TySh` warp descriptor key for the warp's rotate orientation.
const KEY_WARP_ROTATE: &str = "warpRotate";
/// The unwarped style name.
const WARP_NONE: &str = "warpNone";
/// Unit code Photoshop uses for the descriptor's point rectangles.
const UNIT_POINTS: [u8; 4] = *b"#Pnt";
/// The XMP namespace the `photoshop:TextLayers` list lives in.
const PHOTOSHOP_NAMESPACE: &str = "http://ns.adobe.com/photoshop/1.0/";

/// Text orientation (`TySh` `Ornt`): Photoshop's `Hrzn` / `Vrtc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Orientation {
    /// Horizontal text (`Hrzn`), the default.
    #[default]
    Horizontal,
    /// Vertical text (`Vrtc`).
    Vertical,
}

impl Orientation {
    /// The enumerated value name Photoshop stores.
    pub const fn as_str(self) -> &'static str {
        match self {
            Orientation::Horizontal => "Hrzn",
            Orientation::Vertical => "Vrtc",
        }
    }

    /// Parses an enumerated value name (`Hrzn` / `Vrtc`).
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Hrzn" => Some(Orientation::Horizontal),
            "Vrtc" => Some(Orientation::Vertical),
            _ => None,
        }
    }

    /// Parses an enumerated descriptor value.
    pub fn from_id(id: &Id) -> Option<Self> {
        std::str::from_utf8(id.as_bytes()).ok().and_then(Orientation::from_name)
    }

    /// The enumerated descriptor value (`Hrzn` and `Vrtc` are four-byte codes).
    pub fn as_id(self) -> Id {
        Id::Code(*self.as_bytes())
    }

    /// The four value bytes.
    const fn as_bytes(self) -> &'static [u8; 4] {
        match self {
            Orientation::Horizontal => b"Hrzn",
            Orientation::Vertical => b"Vrtc",
        }
    }
}

/// The text-warp settings of a text layer (`TySh` warp descriptor).
///
/// Photoshop stores no warp in `Txt2`; this is the only copy of the setting.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Warp {
    /// Style name: `warpNone`, `warpArc`, `warpArcLower`, `warpArcUpper`,
    /// `warpArch`, `warpBulge`, `warpShellLower`, `warpShellUpper`, `warpFlag`,
    /// `warpWave`, `warpFish`, `warpRise`, `warpFisheye`, `warpInflate`,
    /// `warpSqueeze` or `warpTwist`.
    pub style: String,
    /// Warp amount (Photoshop's "Bend"), percent.
    pub value: f64,
    /// Horizontal distortion, percent.
    pub perspective: f64,
    /// Vertical distortion, percent.
    pub perspective_other: f64,
    /// Rotate orientation (`Hrzn` / `Vrtc`).
    pub rotate: Orientation,
}

impl Default for Warp {
    fn default() -> Self {
        Warp { style: WARP_NONE.to_string(), value: 0.0, perspective: 0.0, perspective_other: 0.0, rotate: Orientation::Horizontal }
    }
}

impl Warp {
    /// `true` when a warp other than `warpNone` is applied.
    pub fn is_warped(&self) -> bool {
        self.style != WARP_NONE
    }
}

/// A parsed `TySh` block.
#[derive(Debug, Clone, PartialEq)]
pub struct TySh {
    /// Affine transform `[xx, xy, yx, yy, tx, ty]`.
    pub transform: [f64; 6],
    /// Text engine version (50 in Photoshop 20.0.2 files).
    pub text_version: u16,
    /// Text descriptor (`TxLr` class).
    pub descriptor: VersionedDescriptor,
    /// Warp descriptor (`warp` class).
    pub warp: VersionedDescriptor,
    /// The raw 16 bytes after the warp descriptor, kept bit for bit.
    ///
    /// Photoshop 20.0.2 writes four big-endian f32 point bounds here (zero when
    /// the text is not warped); use [`Self::bounds_points`] to read them and
    /// [`Self::set_bounds_points`] to write them. Other writers store integers,
    /// which round-trip as the bits they are.
    pub bounds: [i32; 4],
    /// Bytes after the bounds, preserved verbatim (block padding).
    pub trailing: Vec<u8>,
}

impl TySh {
    /// Builds a text layer `TySh` from the text, its engine data (`EngineData`, the
    /// `tdta` payload produced by the caller's text engine), a transform and bounds.
    ///
    /// `bounds` is `[left, top, right, bottom]` in points; it fills the text
    /// descriptor's layout and ink rectangles. Photoshop 20.0.2 leaves the 16
    /// trailing bytes zero unless the text is warped, and so does this.
    pub fn new_text(text: &str, engine_data: &[u8], transform: [f64; 6], bounds: [i32; 4]) -> Self {
        Self::new_text_points(text, engine_data, transform, bounds.map(f64::from))
    }

    /// [`Self::new_text`] with the descriptor rectangles in points.
    pub fn new_text_points(text: &str, engine_data: &[u8], transform: [f64; 6], bounds: [f64; 4]) -> Self {
        TySh {
            transform,
            text_version: TEXT_VERSION,
            descriptor: text_descriptor(text, engine_data, bounds),
            warp: warp_descriptor(),
            bounds: [0; 4],
            trailing: Vec::new(),
        }
    }

    /// Parses a `TySh` block body. Rejects unknown `TySh`/warp versions. Bytes after the bounds
    /// are kept in [`TySh::trailing`] so that [`TySh::to_bytes`] reproduces the input exactly.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let version = r.u16()?;
        if version != TY_SH_VERSION {
            return Err(PsdError::invalid(format!("TySh version {version}, expected {TY_SH_VERSION}")));
        }
        let mut transform = [0f64; 6];
        for t in &mut transform {
            *t = r.f64()?;
        }
        let text_version = r.u16()?;
        let (descriptor, n) = VersionedDescriptor::parse_prefix(r.peek_rest())?;
        r.skip(n)?;
        let warp_version = r.u16()?;
        if warp_version != WARP_VERSION {
            return Err(PsdError::invalid(format!("TySh warp version {warp_version}, expected {WARP_VERSION}")));
        }
        let (warp, n) = VersionedDescriptor::parse_prefix(r.peek_rest())?;
        r.skip(n)?;
        let mut bounds = [0i32; 4];
        for b in &mut bounds {
            *b = r.i32()?;
        }
        let trailing = r.peek_rest().to_vec();
        Ok(TySh { transform, text_version, descriptor, warp, bounds, trailing })
    }

    /// Serializes the block body.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&TY_SH_VERSION.to_be_bytes());
        for t in self.transform {
            out.extend_from_slice(&t.to_be_bytes());
        }
        out.extend_from_slice(&self.text_version.to_be_bytes());
        out.extend(self.descriptor.to_bytes());
        out.extend_from_slice(&WARP_VERSION.to_be_bytes());
        out.extend(self.warp.to_bytes());
        for b in self.bounds {
            out.extend_from_slice(&b.to_be_bytes());
        }
        out.extend_from_slice(&self.trailing);
        out
    }

    /// Sets the affine transform (the header's six doubles) and, when the text
    /// descriptor carries a `transform` object (some writers mirror it there), its values.
    pub fn set_transform(&mut self, transform: [f64; 6]) {
        self.transform = transform;
        let Some(Value::Descriptor(desc)) = item_mut(&mut self.descriptor.descriptor, KEY_TRANSFORM) else {
            return;
        };
        for (key, value) in ["xx", "xy", "yx", "yy", "tx", "ty"].into_iter().zip(transform) {
            set_item(desc, key, Value::Double(value));
        }
    }

    /// The text descriptor, for direct access to values this API does not type.
    pub fn text_descriptor(&self) -> &Descriptor {
        &self.descriptor.descriptor
    }

    /// The text descriptor, mutably.
    pub fn text_descriptor_mut(&mut self) -> &mut Descriptor {
        &mut self.descriptor.descriptor
    }

    /// The warp descriptor, for direct access.
    pub fn warp_descriptor(&self) -> &Descriptor {
        &self.warp.descriptor
    }

    /// The warp descriptor, mutably.
    pub fn warp_descriptor_mut(&mut self) -> &mut Descriptor {
        &mut self.warp.descriptor
    }

    /// The raw `EngineData` bytes (`tdta` payload), when present.
    pub fn engine_data(&self) -> Option<&[u8]> {
        match self.text_descriptor().get(KEY_ENGINE_DATA)? {
            Value::RawData(data) => Some(data),
            _ => None,
        }
    }

    /// Replaces the `EngineData` payload (appending the key when a writer left it out).
    pub fn set_engine_data(&mut self, engine_data: &[u8]) {
        set_item(self.text_descriptor_mut(), KEY_ENGINE_DATA, Value::RawData(engine_data.to_vec()));
    }

    /// The layer's text, without the trailing NUL Photoshop stores in the descriptor.
    pub fn text(&self) -> Option<String> {
        match self.text_descriptor().get(KEY_TEXT)? {
            Value::Text(text) => Some(text.to_string_lossy()),
            _ => None,
        }
    }

    /// The layer's text as stored (UTF-16 units, including any trailing NUL).
    pub fn text_units(&self) -> Option<&[u16]> {
        match self.text_descriptor().get(KEY_TEXT)? {
            Value::Text(text) => Some(&text.0),
            _ => None,
        }
    }

    /// Replaces the layer's text, storing it NUL-terminated as Photoshop does.
    ///
    /// The text is what other applications show; Photoshop itself composes from
    /// `Txt2` (see `docs/text-layer-storage.md`).
    pub fn set_text(&mut self, text: &str) {
        set_item(self.text_descriptor_mut(), KEY_TEXT, Value::Text(UnicodeString::new_nul(text)));
    }

    /// Text orientation (`Ornt`), horizontal when the key is absent.
    pub fn orientation(&self) -> Orientation {
        enum_value(self.text_descriptor(), KEY_ORIENTATION).and_then(Orientation::from_id).unwrap_or_default()
    }

    /// Sets the text orientation (`Ornt`).
    pub fn set_orientation(&mut self, orientation: Orientation) {
        set_enum(self.text_descriptor_mut(), KEY_ORIENTATION, Id::new(KEY_ORIENTATION), orientation.as_id());
    }

    /// The text gridding mode (`textGridding`, `None` in every Photoshop file).
    pub fn text_gridding(&self) -> Option<String> {
        enum_value(self.text_descriptor(), KEY_TEXT_GRIDDING).map(id_name)
    }

    /// Sets the text gridding mode (`textGridding`).
    pub fn set_text_gridding(&mut self, gridding: &str) {
        set_enum(self.text_descriptor_mut(), KEY_TEXT_GRIDDING, Id::new(KEY_TEXT_GRIDDING), Id::new(gridding));
    }

    /// The antialias mode name (`AntA`), e.g. `AnSm`/`AnCr` or `antiAliasSharp`.
    pub fn antialias(&self) -> Option<String> {
        enum_value(self.text_descriptor(), KEY_ANTIALIAS).map(id_name)
    }

    /// Sets the antialias mode name (`AntA`), keeping the descriptor's stored form
    /// (four-byte code or string) for the value.
    pub fn set_antialias(&mut self, mode: &str) {
        let value = match enum_value(self.text_descriptor(), KEY_ANTIALIAS) {
            Some(Id::Code(_)) => Id::Code(four_byte_code(mode)),
            _ => Id::new(mode),
        };
        set_enum(self.text_descriptor_mut(), KEY_ANTIALIAS, Id::new("Annt"), value);
    }

    /// The layout rectangle in points: `[left, top, right, bottom]` of `bounds`.
    ///
    /// For vertical text the rectangle is tall and narrow (a 12 × 144 point line
    /// in `out/probe5/cjkvert.psd`), for area text it is the text frame.
    pub fn layout_bounds(&self) -> Option<[f64; 4]> {
        rect_value(self.text_descriptor(), KEY_BOUNDS)
    }

    /// Sets the layout rectangle (`bounds`) in points.
    pub fn set_layout_bounds(&mut self, bounds: [f64; 4]) {
        set_rect(self.text_descriptor_mut(), KEY_BOUNDS, bounds);
    }

    /// The ink rectangle in points: `[left, top, right, bottom]` of `boundingBox`.
    pub fn ink_bounds(&self) -> Option<[f64; 4]> {
        rect_value(self.text_descriptor(), KEY_BOUNDING_BOX)
    }

    /// Sets the ink rectangle (`boundingBox`) in points.
    pub fn set_ink_bounds(&mut self, bounds: [f64; 4]) {
        set_rect(self.text_descriptor_mut(), KEY_BOUNDING_BOX, bounds);
    }

    /// The 16 trailing bytes read as Photoshop's four f32 point bounds.
    pub fn bounds_points(&self) -> [f32; 4] {
        self.bounds.map(|bits| f32::from_bits(bits as u32))
    }

    /// Writes four f32 point bounds into the 16 trailing bytes.
    pub fn set_bounds_points(&mut self, bounds: [f32; 4]) {
        self.bounds = bounds.map(|v| v.to_bits() as i32);
    }

    /// The layer's index among the document's text layers (`TextIndex`).
    pub fn text_index(&self) -> Option<i32> {
        match self.text_descriptor().get(KEY_TEXT_INDEX)? {
            Value::Integer(v) => Some(*v),
            _ => None,
        }
    }

    /// Sets the layer's index among the document's text layers (`TextIndex`).
    pub fn set_text_index(&mut self, index: i32) {
        set_item(self.text_descriptor_mut(), KEY_TEXT_INDEX, Value::Integer(index));
    }

    /// The warp settings, with the values Photoshop writes when a key is absent.
    pub fn warp_settings(&self) -> Warp {
        let warp = self.warp_descriptor();
        Warp {
            style: enum_value(warp, KEY_WARP_STYLE).map(id_name).unwrap_or_else(|| WARP_NONE.to_string()),
            value: double_value(warp, KEY_WARP_VALUE).unwrap_or(0.0),
            perspective: double_value(warp, KEY_WARP_PERSPECTIVE).unwrap_or(0.0),
            perspective_other: double_value(warp, KEY_WARP_PERSPECTIVE_OTHER).unwrap_or(0.0),
            rotate: enum_value(warp, KEY_WARP_ROTATE).and_then(Orientation::from_id).unwrap_or_default(),
        }
    }

    /// Writes the warp settings.
    pub fn set_warp_settings(&mut self, warp: &Warp) {
        let desc = self.warp_descriptor_mut();
        set_enum(desc, KEY_WARP_STYLE, Id::new(KEY_WARP_STYLE), Id::Str(warp.style.as_bytes().to_vec()));
        set_item(desc, KEY_WARP_VALUE, Value::Double(warp.value));
        set_item(desc, KEY_WARP_PERSPECTIVE, Value::Double(warp.perspective));
        set_item(desc, KEY_WARP_PERSPECTIVE_OTHER, Value::Double(warp.perspective_other));
        set_enum(desc, KEY_WARP_ROTATE, Id::new("Ornt"), warp.rotate.as_id());
    }
}

/// Builds the text descriptor (`TxLr`) for a `TySh` block, in the shape
/// Photoshop 20.0.2 writes it (see the module documentation).
///
/// `bounds` is `[left, top, right, bottom]` in points and fills both the
/// layout rectangle and the ink rectangle.
pub fn text_descriptor(text: &str, engine_data: &[u8], bounds: [f64; 4]) -> VersionedDescriptor {
    let desc = named_descriptor("TxLr")
        .with(KEY_TEXT, Value::Text(UnicodeString::new_nul(text)))
        .with(KEY_TEXT_GRIDDING, Value::Enumerated { type_id: Id::new(KEY_TEXT_GRIDDING), value: Id::new("None") })
        .with(KEY_ORIENTATION, Value::Enumerated { type_id: Id::new(KEY_ORIENTATION), value: Orientation::Horizontal.as_id() })
        .with(KEY_ANTIALIAS, Value::Enumerated { type_id: Id::new("Annt"), value: Id::new("AnSm") })
        .with(KEY_BOUNDS, Value::Descriptor(rect_descriptor(KEY_BOUNDS, bounds)))
        .with(KEY_BOUNDING_BOX, Value::Descriptor(rect_descriptor(KEY_BOUNDING_BOX, bounds)))
        .with(KEY_TEXT_INDEX, Value::Integer(0))
        .with(KEY_ENGINE_DATA, Value::RawData(engine_data.to_vec()));
    VersionedDescriptor::new(desc)
}

/// Builds the default (no warp) warp descriptor, in the shape Photoshop 20.0.2 writes it.
pub fn warp_descriptor() -> VersionedDescriptor {
    let desc = named_descriptor("warp")
        .with(KEY_WARP_STYLE, Value::Enumerated { type_id: Id::new(KEY_WARP_STYLE), value: Id::Str(WARP_NONE.as_bytes().to_vec()) })
        .with(KEY_WARP_VALUE, Value::Double(0.0))
        .with(KEY_WARP_PERSPECTIVE, Value::Double(0.0))
        .with(KEY_WARP_PERSPECTIVE_OTHER, Value::Double(0.0))
        .with(KEY_WARP_ROTATE, Value::Enumerated { type_id: Id::new("Ornt"), value: Orientation::Horizontal.as_id() });
    VersionedDescriptor::new(desc)
}

/// `true` when `record` carries a `TySh` block, i.e. it is a text layer.
pub fn is_text_layer(record: &LayerRecord) -> bool {
    record.block(b"TySh").is_some()
}

/// A summary of one text layer, as [`PsdFile::text_layer_info`] reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLayerInfo {
    /// Index in [`PsdFile::layers`].
    pub index: usize,
    /// Layer name (prefers the `luni` block).
    pub name: String,
    /// Layer rect, i.e. the bounds of the stored raster.
    pub rect: Rect,
    /// The text stored in `TySh` (`Txt `), without its NUL terminator.
    pub text: String,
    /// The `TySh` transform.
    pub transform: [f64; 6],
    /// Text orientation.
    pub orientation: Orientation,
    /// Warp settings.
    pub warp: Warp,
    /// Layout rectangle in points, when the descriptor stores one.
    pub layout_bounds: Option<[f64; 4]>,
    /// Ink rectangle in points, when the descriptor stores one.
    pub ink_bounds: Option<[f64; 4]>,
    /// Length in bytes of the `EngineData` payload.
    pub engine_data_len: usize,
    /// The layer's text as the XMP `photoshop:TextLayers` list stores it, when
    /// an entry with this layer's name exists.
    pub xmp_text: Option<String>,
    /// `true` when the document carries a `Txt2` block.
    pub has_txt2: bool,
}

/// One `photoshop:TextLayers` entry of the XMP packet (resource 1060).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct XmpTextLayer {
    /// `photoshop:LayerName`.
    pub name: String,
    /// `photoshop:LayerText`.
    pub text: String,
}

/// The stored raster of a text layer at the document's sample depth.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TextRaster {
    /// 8-bit RGBA, four bytes per pixel.
    Rgba8(Vec<u8>),
    /// 16-bit RGBA, four samples per pixel.
    Rgba16(Vec<u16>),
    /// 32-bit float RGBA, four samples per pixel.
    Rgba32(Vec<f32>),
}

impl TextRaster {
    /// Bits per sample this raster holds (8, 16 or 32).
    pub fn depth(&self) -> u16 {
        match self {
            TextRaster::Rgba8(_) => 8,
            TextRaster::Rgba16(_) => 16,
            TextRaster::Rgba32(_) => 32,
        }
    }

    /// Number of interleaved samples (four per pixel).
    pub fn len(&self) -> usize {
        match self {
            TextRaster::Rgba8(v) => v.len(),
            TextRaster::Rgba16(v) => v.len(),
            TextRaster::Rgba32(v) => v.len(),
        }
    }

    /// `true` when the raster covers no pixels.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of pixels the samples cover.
    fn pixel_count(&self) -> usize {
        self.len() / 4
    }
}

/// A text layer's stored raster: its rect and straight-alpha RGBA samples at the
/// document's depth.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TextLayerRaster {
    /// Layer rect in document coordinates.
    pub rect: Rect,
    /// Interleaved straight-alpha RGBA samples, row-major.
    pub raster: TextRaster,
}

impl PsdFile {
    /// The text layers, in file order (bottom-most first) with their indices.
    pub fn text_layers(&self) -> impl Iterator<Item = (usize, &LayerRecord)> {
        self.layers().iter().enumerate().filter(|(_, record)| is_text_layer(record))
    }

    /// Number of text layers.
    pub fn text_layer_count(&self) -> usize {
        self.text_layers().count()
    }

    /// `true` when layer `index` exists and is a text layer.
    pub fn is_text_layer(&self, index: usize) -> bool {
        self.layers().get(index).is_some_and(is_text_layer)
    }

    /// Summary of text layer `index`; `None` when the layer does not exist or is
    /// not a text layer.
    pub fn text_layer_info(&self, index: usize) -> Option<Result<TextLayerInfo>> {
        let record = self.layers().get(index)?;
        if !is_text_layer(record) {
            return None;
        }
        Some((|| {
            let tysh = TySh::from_bytes(&record.block(b"TySh").ok_or_else(|| not_a_text_layer(index))?.data)?;
            let name = record.name();
            let xmp_text = self.xmp_text_layers().into_iter().find(|entry| entry.name == name).map(|entry| entry.text);
            Ok(TextLayerInfo {
                index,
                name,
                rect: record.rect,
                text: tysh.text().unwrap_or_default(),
                transform: tysh.transform,
                orientation: tysh.orientation(),
                warp: tysh.warp_settings(),
                layout_bounds: tysh.layout_bounds(),
                ink_bounds: tysh.ink_bounds(),
                engine_data_len: tysh.engine_data().map_or(0, <[u8]>::len),
                xmp_text,
                has_txt2: self.txt2().is_some(),
            })
        })())
    }

    /// Parses text layer `index`'s `TySh` block; `None` when the layer does not
    /// exist or is not a text layer.
    pub fn text_layer_tysh(&self, index: usize) -> Option<Result<TySh>> {
        let record = self.layers().get(index)?;
        Some(TySh::from_bytes(&record.block(b"TySh")?.data))
    }

    /// Replaces text layer `index`'s `TySh` block. Unknown blocks keep their data
    /// and padding; the edited block's padding is re-derived from the new payload
    /// length (a canonical pad to even).
    pub fn set_text_layer_tysh(&mut self, index: usize, tysh: &TySh) -> Result<()> {
        let data = tysh.to_bytes();
        let block = text_block_mut(self, index)?;
        block.data = data;
        block.padding = None;
        Ok(())
    }

    /// Text layer `index`'s name (prefers the Unicode `luni` name).
    pub fn text_layer_name(&self, index: usize) -> Option<String> {
        let record = self.layers().get(index)?;
        is_text_layer(record).then(|| record.name())
    }

    /// Sets text layer `index`'s name: the legacy Pascal name and the Unicode
    /// `luni` block the layer reads it from.
    pub fn set_text_layer_name(&mut self, index: usize, name: &str) -> Result<()> {
        if !self.is_text_layer(index) {
            return Err(not_a_text_layer(index));
        }
        let unicode = TaggedBlock::unicode_name(name);
        let record = self.layers_mut().get_mut(index).ok_or_else(|| not_a_text_layer(index))?;
        record.name = crate::io::encode_legacy_name(name);
        match record.block_mut(b"luni") {
            Some(block) => {
                block.data = unicode.data;
                block.padding = unicode.padding;
            }
            None => record.blocks.insert(0, unicode),
        }
        Ok(())
    }

    /// Text layer `index`'s `EngineData` payload (`TySh`, the `tdta` blob).
    pub fn text_layer_engine_data(&self, index: usize) -> Option<Result<Vec<u8>>> {
        let tysh = self.text_layer_tysh(index)?;
        Some(tysh.and_then(|tysh| tysh.engine_data().map(<[u8]>::to_vec).ok_or_else(|| PsdError::invalid("TySh has no EngineData"))))
    }

    /// Replaces text layer `index`'s `EngineData` payload (`TySh`).
    ///
    /// Photoshop composes from `Txt2`, so this changes what other applications
    /// and the stored raster path show, not Photoshop's own composition.
    pub fn set_text_layer_engine_data(&mut self, index: usize, engine_data: &[u8]) -> Result<()> {
        let mut tysh = self.text_layer_tysh(index).ok_or_else(|| not_a_text_layer(index))??;
        tysh.set_engine_data(engine_data);
        self.set_text_layer_tysh(index, &tysh)
    }

    /// Text layer `index`'s text (`TySh` `Txt `, without the NUL terminator).
    pub fn text_layer_text(&self, index: usize) -> Option<Result<String>> {
        let tysh = self.text_layer_tysh(index)?;
        Some(tysh.and_then(|tysh| tysh.text().ok_or_else(|| PsdError::invalid("TySh has no Txt value"))))
    }

    /// Replaces text layer `index`'s text (`TySh` `Txt `). The stored bytes end
    /// with the NUL Photoshop writes. Photoshop composes from `Txt2`; see
    /// [`Self::set_text_layer_engine_data`].
    pub fn set_text_layer_text(&mut self, index: usize, text: &str) -> Result<()> {
        let mut tysh = self.text_layer_tysh(index).ok_or_else(|| not_a_text_layer(index))??;
        tysh.set_text(text);
        self.set_text_layer_tysh(index, &tysh)
    }

    /// Text layer `index`'s transform (`TySh` header's six doubles).
    pub fn text_layer_transform(&self, index: usize) -> Option<Result<[f64; 6]>> {
        Some(self.text_layer_tysh(index)?.map(|tysh| tysh.transform))
    }

    /// Replaces text layer `index`'s transform, and the descriptor's mirrored
    /// `transform` object when a writer stored one.
    pub fn set_text_layer_transform(&mut self, index: usize, transform: [f64; 6]) -> Result<()> {
        let mut tysh = self.text_layer_tysh(index).ok_or_else(|| not_a_text_layer(index))??;
        tysh.set_transform(transform);
        self.set_text_layer_tysh(index, &tysh)
    }

    /// Text layer `index`'s orientation (`Ornt`).
    pub fn text_layer_orientation(&self, index: usize) -> Option<Result<Orientation>> {
        Some(self.text_layer_tysh(index)?.map(|tysh| tysh.orientation()))
    }

    /// Sets text layer `index`'s orientation (`Ornt`).
    pub fn set_text_layer_orientation(&mut self, index: usize, orientation: Orientation) -> Result<()> {
        let mut tysh = self.text_layer_tysh(index).ok_or_else(|| not_a_text_layer(index))??;
        tysh.set_orientation(orientation);
        self.set_text_layer_tysh(index, &tysh)
    }

    /// Text layer `index`'s text-warp settings (`TySh` warp descriptor).
    pub fn text_layer_warp(&self, index: usize) -> Option<Result<Warp>> {
        Some(self.text_layer_tysh(index)?.map(|tysh| tysh.warp_settings()))
    }

    /// Sets text layer `index`'s text-warp settings. `Txt2` keeps no warp copy, so
    /// nothing else in the document changes.
    pub fn set_text_layer_warp(&mut self, index: usize, warp: &Warp) -> Result<()> {
        let mut tysh = self.text_layer_tysh(index).ok_or_else(|| not_a_text_layer(index))??;
        tysh.set_warp_settings(warp);
        self.set_text_layer_tysh(index, &tysh)
    }

    /// The document-level `Txt2` block data (the CoolType text model), when present.
    pub fn txt2(&self) -> Option<&[u8]> {
        self.txt2_block().map(|block| &block.data[..])
    }

    /// The document-level `Txt2` block, when present.
    pub fn txt2_block(&self) -> Option<&TaggedBlock> {
        self.global_blocks.iter().find(|block| &block.key == b"Txt2")
    }

    /// Replaces the document's `Txt2` block data, keeping exactly one such block
    /// (Photoshop writes one per document even for many text layers). `None`
    /// removes every `Txt2` block. Unknown global blocks are left alone.
    pub fn set_txt2(&mut self, data: Option<Vec<u8>>) {
        let positions: Vec<usize> = self.global_blocks.iter().enumerate().filter(|(_, block)| &block.key == b"Txt2").map(|(i, _)| i).collect();
        let Some(data) = data else {
            for position in positions.into_iter().rev() {
                self.global_blocks.remove(position);
            }
            return;
        };
        match positions.split_first() {
            Some((&first, duplicates)) => {
                let block = &mut self.global_blocks[first];
                if block.data != data {
                    block.data = data;
                    block.padding = None;
                }
                for &position in duplicates.iter().rev() {
                    self.global_blocks.remove(position);
                }
            }
            None => self.global_blocks.push(TaggedBlock::new(*b"Txt2", data)),
        }
    }

    /// Text layer `index`'s stored raster as straight-alpha RGBA at the
    /// document's sample depth. Supports RGB and grayscale documents; other color
    /// modes report [`PsdError::Unsupported`].
    pub fn text_layer_raster(&self, index: usize) -> Option<Result<TextLayerRaster>> {
        let record = self.layers().get(index)?;
        if !is_text_layer(record) {
            return None;
        }
        Some(self.stored_raster(record))
    }

    /// Replaces text layer `index`'s stored raster and layer rect.
    ///
    /// The raster's depth must equal the document's (8, 16 or 32); color and
    /// transparency channels are re-encoded with RLE (the compression Photoshop
    /// 20.0.2 accepts). Channels the layer lacks are appended; other channels
    /// (masks) are untouched. RGB and grayscale documents are supported; other
    /// color modes report [`PsdError::Unsupported`].
    pub fn set_text_layer_raster(&mut self, index: usize, raster: &TextLayerRaster) -> Result<()> {
        if !self.is_text_layer(index) {
            return Err(not_a_text_layer(index));
        }
        let depth = self.header.depth;
        if raster.raster.depth() != depth {
            return Err(PsdError::invalid(format!("raster is {}-bit, the document is {depth}-bit", raster.raster.depth())));
        }
        let (width, height) = raster.rect.size()?;
        let pixels = width.checked_mul(height).ok_or(PsdError::LimitExceeded("text raster size overflow"))?;
        if raster.raster.pixel_count() != pixels {
            return Err(PsdError::invalid(format!("raster holds {} pixels, the rect covers {pixels}", raster.raster.pixel_count())));
        }
        let color_channels = raster_color_channels(self.header.color_mode)?;
        let (colors, alpha) = split_planes(&raster.raster, color_channels)?;
        let version = self.header.version;
        let planes = std::iter::once((CHANNEL_TRANSPARENCY, alpha)).chain(colors.into_iter().enumerate().map(|(i, plane)| (i as i16, plane)));
        let record = self.layers_mut().get_mut(index).ok_or_else(|| not_a_text_layer(index))?;
        record.rect = raster.rect;
        for (id, plane) in planes {
            let encoded = crate::layer::ChannelData::encode(id, Compression::Rle, &plane, width, height, depth, version)?;
            match record.channels.iter_mut().find(|channel| channel.id == id) {
                Some(channel) => *channel = encoded,
                None => record.channels.push(encoded),
            }
        }
        Ok(())
    }

    /// The XMP packet (image resource 1060) as text.
    pub fn xmp_packet(&self) -> Option<&str> {
        self.resource(ids::XMP)?.xmp_str()
    }

    /// Replaces the XMP packet (image resource 1060), adding the resource when
    /// the document has none. Other resources are untouched.
    pub fn set_xmp_packet(&mut self, packet: &str) {
        match self.resources.iter_mut().find(|resource| resource.id == ids::XMP) {
            Some(resource) => resource.data = packet.as_bytes().to_vec(),
            None => self.resources.push(ImageResource::new(ids::XMP, packet.as_bytes().to_vec())),
        }
    }

    /// The `photoshop:TextLayers` list of the XMP packet, in stored order
    /// (Photoshop writes bottom-to-top layer order).
    pub fn xmp_text_layers(&self) -> Vec<XmpTextLayer> {
        self.xmp_packet().map(parse_xmp_text_layers).unwrap_or_default()
    }

    /// Replaces the `photoshop:TextLayers` list, leaving the rest of the XMP
    /// packet byte for byte. An empty list removes the element; a document with
    /// no XMP packet gets a new one holding the list.
    pub fn set_xmp_text_layers(&mut self, layers: &[XmpTextLayer]) -> Result<()> {
        let packet = self.xmp_packet().unwrap_or_default().to_string();
        let updated = write_xmp_text_layers(&packet, layers)?;
        self.set_xmp_packet(&updated);
        Ok(())
    }

    /// Rewrites the XMP `photoshop:TextLayers` list from the document's text
    /// layers (name and `TySh` text), in layer order. The `Txt2` model is not
    /// consulted: this crate treats it as opaque bytes.
    pub fn sync_xmp_text_layers(&mut self) -> Result<()> {
        let mut entries = Vec::new();
        for (index, record) in self.text_layers() {
            let block = record.block(b"TySh").ok_or_else(|| not_a_text_layer(index))?;
            let tysh = TySh::from_bytes(&block.data)?;
            entries.push(XmpTextLayer { name: record.name(), text: tysh.text().unwrap_or_default() });
        }
        self.set_xmp_text_layers(&entries)
    }

    /// The stored raster of `record`: color channels (RGB or grayscale) plus the
    /// transparency channel, interleaved at the document depth.
    fn stored_raster(&self, record: &LayerRecord) -> Result<TextLayerRaster> {
        let (width, height) = record.rect.size()?;
        let pixels = width.checked_mul(height).ok_or(PsdError::LimitExceeded("text raster size overflow"))?;
        let depth = self.header.depth;
        let version = self.header.version;
        let color_channels = raster_color_channels(self.header.color_mode)?;
        let mut colors = Vec::with_capacity(color_channels);
        for id in 0..color_channels as i16 {
            let plane = match record.channel(id) {
                Some(_) => record.decode_channel(id, depth, version)?,
                None => blank_plane(depth, pixels, 0)?,
            };
            colors.push(plane);
        }
        let alpha = match record.channel(CHANNEL_TRANSPARENCY) {
            Some(_) => record.decode_channel(CHANNEL_TRANSPARENCY, depth, version)?,
            None => blank_plane(depth, pixels, max_sample(depth))?,
        };
        Ok(TextLayerRaster { rect: record.rect, raster: interleave(&colors, &alpha, depth, pixels)? })
    }
}

/// The text-layer channels this API can read and write: RGB and grayscale images.
fn raster_color_channels(mode: ColorMode) -> Result<usize> {
    match mode {
        ColorMode::Rgb => Ok(3),
        ColorMode::Grayscale => Ok(1),
        other => Err(PsdError::Unsupported(format!("text-layer rasters in {other:?} documents"))),
    }
}

/// All-`fill` plane of `pixels` samples at `depth` (planar big-endian).
fn blank_plane(depth: u16, pixels: usize, fill: u16) -> Result<Vec<u8>> {
    let bytes = usize::from(depth / 8);
    if !matches!(depth, 8 | 16 | 32) {
        return Err(PsdError::Unsupported(format!("depth {depth}")));
    }
    let len = pixels.checked_mul(bytes).ok_or(PsdError::LimitExceeded("raster plane size overflow"))?;
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| PsdError::OutOfMemory("not enough memory for a raster plane"))?;
    match depth {
        8 => out.resize(pixels, fill as u8),
        16 => {
            for _ in 0..pixels {
                out.extend_from_slice(&fill.to_be_bytes());
            }
        }
        _ => {
            let value = if fill == 0 { 0.0f32 } else { 1.0 };
            for _ in 0..pixels {
                out.extend_from_slice(&value.to_be_bytes());
            }
        }
    }
    Ok(out)
}

/// Largest sample value at `depth` (`1.0` for float images).
fn max_sample(depth: u16) -> u16 {
    match depth {
        8 => u16::from(u8::MAX),
        16 => u16::MAX,
        _ => 0,
    }
}

/// Interleaves decoded color planes and the alpha plane into an RGBA raster.
fn interleave(colors: &[Vec<u8>], alpha: &[u8], depth: u16, pixels: usize) -> Result<TextRaster> {
    let Some(red) = colors.first() else { return Err(PsdError::invalid("text layer has no color channel")) };
    Ok(match depth {
        8 => {
            let mut out = vec![0u8; pixels * 4];
            for (i, rgba) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (g, b) = match colors.get(1).zip(colors.get(2)) {
                    Some((green, blue)) => (green[i], blue[i]),
                    None => (red[i], red[i]),
                };
                *rgba = [red[i], g, b, alpha[i]];
            }
            TextRaster::Rgba8(out)
        }
        16 => {
            let sample = |plane: &[u8], i: usize| u16::from_be_bytes([plane[i * 2], plane[i * 2 + 1]]);
            let color = |channel: usize, i: usize| colors.get(channel).map_or_else(|| sample(red, i), |plane| sample(plane, i));
            let mut out = vec![0u16; pixels * 4];
            for (i, rgba) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                *rgba = [color(0, i), color(1, i), color(2, i), sample(alpha, i)];
            }
            TextRaster::Rgba16(out)
        }
        32 => {
            let sample = |plane: &[u8], i: usize| f32::from_be_bytes([plane[i * 4], plane[i * 4 + 1], plane[i * 4 + 2], plane[i * 4 + 3]]);
            let color = |channel: usize, i: usize| colors.get(channel).map_or_else(|| sample(red, i), |plane| sample(plane, i));
            let mut out = vec![0f32; pixels * 4];
            for (i, rgba) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                *rgba = [color(0, i), color(1, i), color(2, i), sample(alpha, i)];
            }
            TextRaster::Rgba32(out)
        }
        other => return Err(PsdError::Unsupported(format!("depth {other}"))),
    })
}

/// Splits an RGBA raster into decoded color planes (one per color channel, in
/// channel order) and the alpha plane, as planar big-endian bytes. A grayscale
/// document keeps the red sample.
fn split_planes(raster: &TextRaster, color_channels: usize) -> Result<(Vec<Vec<u8>>, Vec<u8>)> {
    let pixels = raster.pixel_count();
    let sample_bytes = usize::from(raster.depth() / 8);
    let sample_count = match raster {
        TextRaster::Rgba8(samples) => samples.len(),
        TextRaster::Rgba16(samples) => samples.len(),
        TextRaster::Rgba32(samples) => samples.len(),
    };
    if sample_count != pixels * 4 {
        return Err(PsdError::invalid("raster holds a partial pixel"));
    }
    let mut colors: Vec<Vec<u8>> = (0..color_channels)
        .map(|_| {
            let mut plane = Vec::new();
            plane.try_reserve_exact(pixels * sample_bytes).map_err(|_| PsdError::OutOfMemory("not enough memory for a raster plane"))?;
            Ok(plane)
        })
        .collect::<Result<_>>()?;
    let mut alpha = Vec::new();
    alpha.try_reserve_exact(pixels * sample_bytes).map_err(|_| PsdError::OutOfMemory("not enough memory for a raster plane"))?;
    for (i, plane) in colors.iter_mut().enumerate() {
        let channel = i.min(2);
        match raster {
            TextRaster::Rgba8(samples) => plane.extend(samples.as_chunks::<4>().0.iter().map(|rgba| rgba[channel])),
            TextRaster::Rgba16(samples) => plane.extend(samples.as_chunks::<4>().0.iter().flat_map(|rgba| rgba[channel].to_be_bytes())),
            TextRaster::Rgba32(samples) => plane.extend(samples.as_chunks::<4>().0.iter().flat_map(|rgba| rgba[channel].to_be_bytes())),
        }
    }
    match raster {
        TextRaster::Rgba8(samples) => alpha.extend(samples.as_chunks::<4>().0.iter().map(|rgba| rgba[3])),
        TextRaster::Rgba16(samples) => alpha.extend(samples.as_chunks::<4>().0.iter().flat_map(|rgba| rgba[3].to_be_bytes())),
        TextRaster::Rgba32(samples) => alpha.extend(samples.as_chunks::<4>().0.iter().flat_map(|rgba| rgba[3].to_be_bytes())),
    }
    Ok((colors, alpha))
}

/// Error for an index that is not a text layer.
fn not_a_text_layer(index: usize) -> PsdError {
    PsdError::invalid(format!("layer {index} is not a text layer (no TySh block)"))
}

/// Mutable `TySh` block of layer `index`.
fn text_block_mut(file: &mut PsdFile, index: usize) -> Result<&mut TaggedBlock> {
    let record = file.layers_mut().get_mut(index).ok_or_else(|| not_a_text_layer(index))?;
    record.block_mut(b"TySh").ok_or_else(|| not_a_text_layer(index))
}

/// A descriptor with Photoshop's one-NUL display name.
fn named_descriptor(class: &str) -> Descriptor {
    Descriptor { name: UnicodeString(vec![0]), class_id: Id::new(class), items: Vec::new() }
}

/// `bounds`-style point rectangle descriptor.
fn rect_descriptor(class: &str, bounds: [f64; 4]) -> Descriptor {
    let px = |value: f64| Value::UnitFloat { unit: UNIT_POINTS, value };
    named_descriptor(class).with("Left", px(bounds[0])).with("Top ", px(bounds[1])).with("Rght", px(bounds[2])).with("Btom", px(bounds[3]))
}

/// First value with key `key`.
fn item<'a>(descriptor: &'a Descriptor, key: &str) -> Option<&'a Value> {
    descriptor.get(key)
}

/// First value with key `key`, mutably.
fn item_mut<'a>(descriptor: &'a mut Descriptor, key: &str) -> Option<&'a mut Value> {
    descriptor.items.iter_mut().find(|(k, _)| k.is(key)).map(|(_, value)| value)
}

/// Sets key `key` to `value`, keeping the item's position (appending when absent).
fn set_item(descriptor: &mut Descriptor, key: &str, value: Value) {
    match item_mut(descriptor, key) {
        Some(slot) => *slot = value,
        None => descriptor.items.push((Id::new(key), value)),
    }
}

/// The enumerated value of `key`, when the item is an enumeration.
fn enum_value<'a>(descriptor: &'a Descriptor, key: &str) -> Option<&'a Id> {
    match item(descriptor, key)? {
        Value::Enumerated { value, .. } => Some(value),
        _ => None,
    }
}

/// The double value of `key`, when the item is a double.
fn double_value(descriptor: &Descriptor, key: &str) -> Option<f64> {
    match item(descriptor, key)? {
        Value::Double(value) => Some(*value),
        _ => None,
    }
}

/// Sets `key` to an enumerated type/value pair, keeping the item's position.
fn set_enum(descriptor: &mut Descriptor, key: &str, type_id: Id, value: Id) {
    set_item(descriptor, key, Value::Enumerated { type_id, value });
}

/// An identifier's bytes as text (lossy; Photoshop's enum names are ASCII).
fn id_name(id: &Id) -> String {
    String::from_utf8_lossy(id.as_bytes()).into_owned()
}

/// `name` as a four-byte code, space-padded or truncated to four bytes.
fn four_byte_code(name: &str) -> [u8; 4] {
    let mut code = [b' '; 4];
    for (slot, byte) in code.iter_mut().zip(name.as_bytes()) {
        *slot = *byte;
    }
    code
}

/// Reads a `[left, top, right, bottom]` point rectangle from `key`.
fn rect_value(descriptor: &Descriptor, key: &str) -> Option<[f64; 4]> {
    let Value::Descriptor(rect) = item(descriptor, key)? else { return None };
    Some([rect_f64(rect, "Left")?, rect_f64(rect, "Top ")?, rect_f64(rect, "Rght")?, rect_f64(rect, "Btom")?])
}

/// One rectangle component, accepting `UnitFloat` (Photoshop) and `Double`.
fn rect_f64(rect: &Descriptor, key: &str) -> Option<f64> {
    match item(rect, key)? {
        Value::UnitFloat { value, .. } => Some(*value),
        Value::Double(value) => Some(*value),
        _ => None,
    }
}

/// Sets a point rectangle at `key`, keeping the rectangle's class and unit when
/// the descriptor already stores one (Photoshop writes class `bounds` and unit
/// `#Pnt`).
fn set_rect(descriptor: &mut Descriptor, key: &str, bounds: [f64; 4]) {
    let (class_id, unit) = match item_mut(descriptor, key) {
        Some(Value::Descriptor(rect)) => (rect.class_id.clone(), rect_unit(rect)),
        _ => (Id::new(key), UNIT_POINTS),
    };
    let px = |value: f64| Value::UnitFloat { unit, value };
    let rect = Descriptor {
        name: UnicodeString(vec![0]),
        class_id,
        items: vec![
            (Id::new("Left"), px(bounds[0])),
            (Id::new("Top "), px(bounds[1])),
            (Id::new("Rght"), px(bounds[2])),
            (Id::new("Btom"), px(bounds[3])),
        ],
    };
    set_item(descriptor, key, Value::Descriptor(rect));
}

/// The unit of an existing rectangle's components.
fn rect_unit(rect: &Descriptor) -> [u8; 4] {
    match rect.get("Left") {
        Some(Value::UnitFloat { unit, .. }) => *unit,
        _ => UNIT_POINTS,
    }
}

// -------------------------------------------------------------------------
// `photoshop:TextLayers` (XMP, image resource 1060)
// -------------------------------------------------------------------------

/// Parses the `photoshop:TextLayers` list out of an XMP packet.
///
/// Element names match by local name (`TextLayers`, `li`, `LayerName`,
/// `LayerText`), so any namespace prefix works. Character and entity references
/// are decoded; a missing list yields an empty vector.
pub fn parse_xmp_text_layers(packet: &str) -> Vec<XmpTextLayer> {
    let Some(span) = find_element(packet, 0, "TextLayers") else { return Vec::new() };
    let mut layers = Vec::new();
    let mut at = span.content.start;
    while at < span.content.end {
        let Some(item) = find_element(packet, at, "li").filter(|item| item.open < span.content.end) else { break };
        at = item.after;
        let name = child_text(packet, &item, "LayerName").unwrap_or_default();
        let text = child_text(packet, &item, "LayerText").unwrap_or_default();
        layers.push(XmpTextLayer { name, text });
    }
    layers
}

/// Rewrites the `photoshop:TextLayers` list of `packet`, preserving every other
/// byte. An empty `layers` removes the element; an empty `packet` becomes a new
/// packet holding the list.
pub fn write_xmp_text_layers(packet: &str, layers: &[XmpTextLayer]) -> Result<String> {
    if packet.trim().is_empty() {
        return Ok(new_xmp_packet(layers));
    }
    if let Some(span) = find_element(packet, 0, "TextLayers") {
        if layers.is_empty() {
            return Ok(remove_element(packet, &span));
        }
        let prefix = element_prefix(packet, span.open).to_string();
        let indent = line_indent(packet, span.open).to_string();
        let block = render_text_layers(&prefix, &indent, layers);
        // `packet[..span.open]` already ends with the element's indentation.
        let line_start = packet[..span.open].rfind('\n').map_or(0, |i| i + 1);
        let base_end = if line_indent(packet, span.open).len() == span.open - line_start { line_start } else { span.open };
        return Ok(format!("{}{}{}", &packet[..base_end], block, &packet[span.after..]));
    }
    if layers.is_empty() {
        return Ok(packet.to_string());
    }
    insert_text_layers(packet, layers)
}

/// An element's byte span inside a packet.
struct ElementSpan {
    /// Offset of the opening `<`.
    open: usize,
    /// Content bytes (empty for a self-closing element).
    content: std::ops::Range<usize>,
    /// Offset just past the element (closing tag or `/>`).
    after: usize,
    /// The element closes itself (`<x/>`) and has no content.
    self_closing: bool,
}

/// A parsed start/end tag.
struct Tag<'a> {
    /// Qualified name as written (`photoshop:TextLayers`).
    name: &'a str,
    /// Name after the last colon.
    local: &'a str,
    /// Offset just past `>`.
    end: usize,
    /// The tag is a closing tag (`</...>`).
    closing: bool,
    /// The tag ends with `/>`.
    self_closing: bool,
}

/// Finds the first element with local name `want` at or after `from`, skipping
/// comments, processing instructions and declarations.
fn find_element(s: &str, from: usize, want: &str) -> Option<ElementSpan> {
    let mut at = from;
    while let Some(open) = find_next_open_tag(s, at) {
        let tag = parse_tag(s, open)?;
        at = tag.end;
        if tag.closing || tag.local != want {
            continue;
        }
        if tag.self_closing {
            return Some(ElementSpan { open, content: tag.end..tag.end, after: tag.end, self_closing: true });
        }
        let mut inner = tag.end;
        while let Some(next) = find_next_open_tag(s, inner) {
            let inner_tag = parse_tag(s, next)?;
            inner = inner_tag.end;
            if inner_tag.closing && inner_tag.local == want {
                return Some(ElementSpan { open, content: tag.end..next, after: inner_tag.end, self_closing: false });
            }
        }
        return None;
    }
    None
}

/// Offset of the next `<` that starts a tag (not a comment, PI or declaration).
fn find_next_open_tag(s: &str, from: usize) -> Option<usize> {
    let mut at = from;
    loop {
        let open = at.checked_add(s.get(at..)?.find('<')?)?;
        let rest = &s[open..];
        let skip = if rest.starts_with("<!--") {
            rest.find("-->").map(|i| i + 3)
        } else if rest.starts_with("<?") {
            rest.find("?>").map(|i| i + 2)
        } else if rest.starts_with("<!") {
            rest.find('>').map(|i| i + 1)
        } else {
            return Some(open);
        };
        at = open + skip?;
    }
}

/// Parses the tag starting at `open`.
fn parse_tag(s: &str, open: usize) -> Option<Tag<'_>> {
    let rest = s.get(open..)?;
    let bytes = rest.as_bytes();
    let mut i = 1usize;
    let closing = *bytes.get(i)? == b'/';
    if closing {
        i += 1;
    }
    let name_start = i;
    i = name_end(bytes, i);
    let name = rest.get(name_start..i)?;
    if name.is_empty() {
        return None;
    }
    let mut quote = 0u8;
    while i < bytes.len() {
        let byte = bytes[i];
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = byte;
        } else if byte == b'>' {
            break;
        }
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let self_closing = !closing && i > 0 && bytes[i - 1] == b'/';
    Some(Tag { name, local: name.rsplit(':').next().unwrap_or(name), end: open + i + 1, closing, self_closing })
}

/// End offset of the name starting at `from`.
fn name_end(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < bytes.len() && !matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>') {
        i += 1;
    }
    i
}

/// Decoded text of child element `want` of `parent`.
fn child_text(packet: &str, parent: &ElementSpan, want: &str) -> Option<String> {
    let child = find_element(packet, parent.content.start, want)?;
    if child.open >= parent.content.end {
        return None;
    }
    Some(unescape_xml(&packet[child.content.clone()]))
}

/// The prefix of the element at `open` (`photoshop` for `photoshop:TextLayers`).
fn element_prefix(packet: &str, open: usize) -> &str {
    match parse_tag(packet, open) {
        Some(tag) => tag.name.split_once(':').map_or("photoshop", |(prefix, _)| prefix),
        None => "photoshop",
    }
}

/// Whitespace before offset `at` on its line (empty when other text precedes it).
fn line_indent(packet: &str, at: usize) -> &str {
    let line_start = packet[..at].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &packet[line_start..at];
    if prefix.chars().all(|c| c == ' ' || c == '\t') { prefix } else { "" }
}

/// `packet` without `span`'s element (and the line it occupied).
fn remove_element(packet: &str, span: &ElementSpan) -> String {
    let line_start = packet[..span.open].rfind('\n').map_or(0, |i| i + 1);
    let line_only_holds_element = packet[line_start..span.open].chars().all(|c| c == ' ' || c == '\t');
    let start = if line_only_holds_element { line_start } else { span.open };
    let end = if packet[span.after..].starts_with('\n') { span.after + 1 } else { span.after };
    format!("{}{}", &packet[..start], &packet[end..])
}

/// Inserts a `photoshop:TextLayers` element into the packet's first
/// `rdf:Description`, declaring the Photoshop namespace when the packet does not
/// already bind it.
fn insert_text_layers(packet: &str, layers: &[XmpTextLayer]) -> Result<String> {
    let Some(span) = find_element(packet, 0, "Description") else {
        return Err(PsdError::invalid("XMP packet has no rdf:Description to hold photoshop:TextLayers"));
    };
    let bound_prefix = photoshop_prefix(packet);
    let prefix = bound_prefix.clone().unwrap_or_else(|| "photoshop".to_string());
    let declaration = if bound_prefix.is_some() { String::new() } else { format!(" xmlns:photoshop=\"{PHOTOSHOP_NAMESPACE}\"") };
    if span.self_closing {
        let indent = line_indent(packet, span.open);
        let block = render_text_layers(&prefix, &format!("{indent}   "), layers);
        let element = &packet[span.open..span.after];
        let head = element.strip_suffix("/>").map_or(element, str::trim_end);
        return Ok(format!("{}{head}{declaration}>\n{block}\n{indent}</rdf:Description>{}", &packet[..span.open], &packet[span.after..]));
    }
    // Keep any existing description content; put the block right before the
    // closing tag, at the closing tag's indentation plus one level.
    let line_start = packet[..span.content.end].rfind('\n').map_or(0, |i| i + 1);
    let close_indent = &packet[line_start..span.content.end];
    let whitespace_only = close_indent.chars().all(|c| c == ' ' || c == '\t');
    let base_end = if whitespace_only { line_start } else { span.content.end };
    let close_indent = if whitespace_only { close_indent } else { "" };
    let child_indent = format!("{close_indent}   ");
    let block = render_text_layers(&prefix, &child_indent, layers);
    let gt = span.content.start - 1;
    let base = &packet[..base_end];
    let head = format!("{}{declaration}{}", &base[..gt.min(base_end)], &base[gt.min(base_end)..]);
    Ok(format!("{head}{block}\n{close_indent}{}", &packet[span.content.end..]))
}

/// The prefix bound to the Photoshop XMP namespace, when the packet declares one.
fn photoshop_prefix(packet: &str) -> Option<String> {
    let mut at = 0;
    while let Some(found) = packet.get(at..)?.find("xmlns:") {
        let name_start = at + found + "xmlns:".len();
        let name_end = packet.get(name_start..)?.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')).map_or(packet.len(), |i| name_start + i);
        let name = packet.get(name_start..name_end)?;
        let value = packet.get(name_end..)?.trim_start().strip_prefix('=')?.trim_start();
        let quote = value.chars().next()?;
        let namespace = value.get(1..)?.split(quote).next()?;
        if namespace == PHOTOSHOP_NAMESPACE {
            return Some(name.to_string());
        }
        at = name_start;
    }
    None
}

/// Renders the `photoshop:TextLayers` element with `indent` before it.
fn render_text_layers(prefix: &str, indent: &str, layers: &[XmpTextLayer]) -> String {
    if layers.is_empty() {
        return format!("{indent}<{prefix}:TextLayers/>");
    }
    let mut out = format!("{indent}<{prefix}:TextLayers>\n{indent}   <rdf:Bag>");
    for layer in layers {
        out.push_str(&format!(
            "\n{indent}      <rdf:li rdf:parseType=\"Resource\">\n{indent}         <{prefix}:LayerName>{}</{prefix}:LayerName>\n{indent}         <{prefix}:LayerText>{}</{prefix}:LayerText>\n{indent}      </rdf:li>",
            escape_xml(&layer.name),
            escape_xml(&layer.text)
        ));
    }
    out.push_str(&format!("\n{indent}   </rdf:Bag>\n{indent}</{prefix}:TextLayers>"));
    out
}

/// A new XMP packet holding `layers` (used when a document has no XMP resource).
fn new_xmp_packet(layers: &[XmpTextLayer]) -> String {
    let block = render_text_layers("photoshop", "         ", layers);
    let mut packet = String::new();
    packet.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    packet.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
    packet.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    packet.push_str("  <rdf:Description rdf:about=\"\" xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\">\n");
    packet.push_str(&block);
    packet.push_str("\n  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n");
    packet.push_str("<?xpacket end=\"w\"?>");
    packet
}

/// Escapes text for an XML text node, using character references for the
/// control characters XML parsers would otherwise normalize away.
fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xD;"),
            '\n' => out.push_str("&#xA;"),
            '\t' => out.push_str("&#x9;"),
            _ => out.push(c),
        }
    }
    out
}

/// Decodes XML entity and character references.
fn unescape_xml(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(end) = tail.find(';').filter(|&end| end <= 12) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        match named_entity(entity).or_else(|| char_reference(entity)) {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The five predefined XML entities.
fn named_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// A numeric character reference (`&#13;` or `&#xD;`).
fn char_reference(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok().and_then(char::from_u32),
        None => digits.parse::<u32>().ok().and_then(char::from_u32),
    }
}
