use crate::error::Error;

use super::path::format_real;
use super::syntax::{Kind, Token, dict_entries, skip_value, tokens};

/// Area-text frame rectangle, in the order stored in the `/1` array.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameGeometry {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Token index of the `[` opening the `/1` array of the dict whose `/99` is
/// the name `F`. Found by walking every dict; no byte offsets are assumed.
fn frame_array(toks: &[Token]) -> Option<usize> {
    let entries = frame_entries(toks)?;
    let arr = entries.iter().find(|(k, _)| k.as_str() == "1")?.1;
    if matches!(toks[arr].kind, Kind::ArrayOpen) {
        Some(arr)
    } else {
        None
    }
}

/// Entries of the `/99 /F` dict (the area-text frame), if the block has one.
pub(super) fn frame_entries(toks: &[Token]) -> Option<Vec<(String, usize)>> {
    for (i, t) in toks.iter().enumerate() {
        if !matches!(t.kind, Kind::DictOpen) {
            continue;
        }
        let Some(entries) = dict_entries(toks, i) else {
            continue;
        };
        let is_frame = entries.iter().any(|(k, v)| {
            k.as_str() == "99" && matches!(&toks[*v].kind, Kind::Name(n) if n.as_str() == "F")
        });
        // Point text carries a `/99 /F` node too, but only area text stores the
        // box rectangle in its `/1` array.
        let has_rect = entries
            .iter()
            .any(|(k, v)| k.as_str() == "1" && matches!(toks[*v].kind, Kind::ArrayOpen));
        if is_frame && has_rect {
            return Some(entries);
        }
    }
    None
}

/// Token indices of the four elements of the array opened at `arr`.
pub(super) fn frame_values(toks: &[Token], arr: usize) -> Option<[usize; 4]> {
    let mut j = arr + 1;
    let mut pos = [0usize; 4];
    for slot in pos.iter_mut() {
        *slot = j;
        j = skip_value(toks, j)?;
    }
    if !matches!(toks.get(j)?.kind, Kind::ArrayClose) {
        return None;
    }
    Some(pos)
}

pub(super) fn number(kind: &Kind) -> Option<f64> {
    match kind {
        Kind::Real(r) => Some(*r),
        Kind::Int(i) => Some(*i as f64),
        _ => None,
    }
}

/// Frame rectangle of area text. `None` for point text (no `/99 /F` dict).
pub fn frame_geometry(block: &[u8]) -> Option<FrameGeometry> {
    let toks = tokens(block).ok()?;
    let arr = frame_array(&toks)?;
    let pos = frame_values(&toks, arr)?;
    let mut v = [0f32; 4];
    for (slot, &i) in v.iter_mut().zip(pos.iter()) {
        *slot = number(&toks[i].kind)? as f32;
    }
    Some(FrameGeometry {
        left: v[0],
        top: v[1],
        right: v[2],
        bottom: v[3],
    })
}

/// Writes the frame rectangle in place, keeping the stored order and every
/// other byte of the block.
pub fn set_frame_geometry(block: &[u8], g: FrameGeometry) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let arr = frame_array(&toks).ok_or_else(|| Error::Txt2("frame geometry not found".into()))?;
    let pos =
        frame_values(&toks, arr).ok_or_else(|| Error::Txt2("frame array malformed".into()))?;
    let vals = [g.left, g.top, g.right, g.bottom];
    let mut out = block.to_vec();
    // Reverse order keeps the original byte offsets of earlier elements valid.
    for (idx, &p) in pos.iter().enumerate().rev() {
        let t = &toks[p];
        out.splice(
            t.start..t.end,
            format_real(f64::from(vals[idx])).into_bytes(),
        );
    }
    Ok(out)
}
