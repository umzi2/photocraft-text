//! Text layer `TySh` block: the layer-level text description written by
//! Photoshop, and a builder for a minimal one.
//!
//! Layout (Adobe Photoshop File Formats, TySh; all big-endian):
//! `u16 version (1)`, `6 × f64 transform`, `u16 textVersion`,
//! `VersionedDescriptor text`, `u16 warpVersion (1)`,
//! `VersionedDescriptor warp`, `4 × i32 bounds (left, top, right, bottom)`.

use crate::descriptor::{Descriptor, UnicodeString, Value, VersionedDescriptor};
use crate::error::{PsdError, Result};
use crate::io::Reader;

/// `TySh` version written and accepted.
pub const TY_SH_VERSION: u16 = 1;
/// Warp descriptor version written and accepted.
pub const WARP_VERSION: u16 = 1;
/// Text version written by [`TySh::new_text`].
pub const TEXT_VERSION: u16 = 1;

/// A parsed `TySh` block.
#[derive(Debug, Clone, PartialEq)]
pub struct TySh {
    /// Affine transform `[xx, xy, yx, yy, tx, ty]`.
    pub transform: [f64; 6],
    /// Text version (1).
    pub text_version: u16,
    /// Text descriptor (`TxLr` class).
    pub descriptor: VersionedDescriptor,
    /// Warp descriptor (`warp` class).
    pub warp: VersionedDescriptor,
    /// Bounds as `[left, top, right, bottom]` (order per Adobe spec; [INFERENCE] for exact field meaning).
    pub bounds: [i32; 4],
    /// Bytes after the bounds, preserved verbatim. The corpus file `plain_latin.psd` carries 2 zero
    /// bytes here ([INFERENCE]: padding; the Adobe spec excerpt used does not list them).
    pub trailing: Vec<u8>,
}

impl TySh {
    /// Builds a text layer `TySh` from the text, its engine data (`EngineData`, `tdta`
    /// payload, produced by the caller's text engine), a transform and bounds.
    pub fn new_text(text: &str, engine_data: &[u8], transform: [f64; 6], bounds: [i32; 4]) -> Self {
        TySh {
            transform,
            text_version: TEXT_VERSION,
            descriptor: text_descriptor(text, engine_data, transform, bounds),
            warp: warp_descriptor(bounds),
            bounds,
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
}

/// Builds the text descriptor (`TxLr`) for a `TySh` block.
///
/// Keys: `Txt ` (text as given; caller supplies any `\r` paragraph separators),
/// `textVersion`, `textDescriptorVersion`, `EngineData` (raw `tdta`), `bounds` and
/// `boundingBox` (`Rctn`, px), `textGridding`, `Ornt`, `AntA`, `transform`.
/// Key/enum names beyond `Txt ` and `EngineData` are [INFERENCE] from the Adobe spec
/// and have not been verified against a Photoshop-written file.
pub fn text_descriptor(text: &str, engine_data: &[u8], transform: [f64; 6], bounds: [i32; 4]) -> VersionedDescriptor {
    let rect = rctn(bounds);
    let [xx, xy, yx, yy, tx, ty] = transform;
    let transform_desc = Descriptor::new("transform")
        .with("xx", Value::Double(xx))
        .with("xy", Value::Double(xy))
        .with("yx", Value::Double(yx))
        .with("yy", Value::Double(yy))
        .with("tx", Value::Double(tx))
        .with("ty", Value::Double(ty));
    let desc = Descriptor::new("TxLr")
        .with("Txt ", Value::Text(UnicodeString::new(text)))
        .with("textVersion", Value::Integer(1))
        .with("textDescriptorVersion", Value::Integer(16))
        .with("EngineData", Value::RawData(engine_data.to_vec()))
        .with("bounds", Value::Descriptor(rect.clone()))
        .with("boundingBox", Value::Descriptor(rect))
        .with("textGridding", Value::Enumerated { type_id: crate::descriptor::Id::new("textGridding"), value: crate::descriptor::Id::new("None") })
        .with("Ornt", Value::Enumerated { type_id: crate::descriptor::Id::new("Ornt"), value: crate::descriptor::Id::new("Hrzn") })
        .with("AntA", Value::Enumerated { type_id: crate::descriptor::Id::new("Annt"), value: crate::descriptor::Id::new("AnSm") })
        .with("transform", Value::Descriptor(transform_desc));
    VersionedDescriptor::new(desc)
}

/// Default (no warp) warp descriptor for `bounds`.
fn warp_descriptor(bounds: [i32; 4]) -> VersionedDescriptor {
    let desc = Descriptor::new("warp")
        .with("warpStyle", Value::Enumerated { type_id: crate::descriptor::Id::new("warpStyle"), value: crate::descriptor::Id::new("warpNone") })
        .with("warpValue", Value::Double(0.0))
        .with("warpPerspective", Value::Double(0.0))
        .with("warpPerspectiveOther", Value::Double(0.0))
        .with("warpRotate", Value::Enumerated { type_id: crate::descriptor::Id::new("Ornt"), value: crate::descriptor::Id::new("Hrzn") })
        .with("bounds", Value::Descriptor(rctn(bounds)))
        .with("uOrder", Value::Integer(4))
        .with("vOrder", Value::Integer(4));
    VersionedDescriptor::new(desc)
}

/// `Rctn` descriptor in pixels from `[left, top, right, bottom]`.
fn rctn(b: [i32; 4]) -> Descriptor {
    let px = |v: i32| Value::UnitFloat { unit: *b"#Pxl", value: f64::from(v) };
    Descriptor::new("Rctn")
        .with("Top ", px(b[1]))
        .with("Left", px(b[0]))
        .with("Btom", px(b[3]))
        .with("Rght", px(b[2]))
}
