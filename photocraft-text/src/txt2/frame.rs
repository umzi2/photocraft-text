use crate::error::Error;

use super::path::{Step, format_real, replace_container, resolve};
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
    geometry_at(&toks, arr)
}

fn geometry_at(toks: &[Token], arr: usize) -> Option<FrameGeometry> {
    let pos = frame_values(toks, arr)?;
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

/// Token index of the `/99 /F` dict's `/1` rectangle inside the value opened
/// at `open`, if that value holds one.
fn frame_array_in(toks: &[Token], open: usize) -> Option<usize> {
    let end = skip_value(toks, open)?;
    (open + 1..end)
        .find(|&i| matches!(toks[i].kind, Kind::DictOpen) && is_frame_dict(toks, i))
        .and_then(|dict| {
            dict_entries(toks, dict)?
                .into_iter()
                .find(|(k, _)| k.as_str() == "1")
                .map(|(_, v)| v)
        })
        .filter(|&arr| matches!(toks[arr].kind, Kind::ArrayOpen))
}

fn is_frame_dict(toks: &[Token], open: usize) -> bool {
    let Some(entries) = dict_entries(toks, open) else {
        return false;
    };
    entries.iter().any(|(k, v)| {
        k.as_str() == "99" && matches!(&toks[*v].kind, Kind::Name(n) if n.as_str() == "F")
    }) && entries
        .iter()
        .any(|(k, v)| k.as_str() == "1" && matches!(toks[*v].kind, Kind::ArrayOpen))
}

/// Frame list entry `frame` of the block's `/0 /8 /0` list: its `/0 /1 /0`
/// rectangle, i.e. the box Photoshop draws area text in.
pub fn frame_list_entry(block: &[u8], frame: usize) -> Option<FrameGeometry> {
    let toks = tokens(block).ok()?;
    let arr = resolve(
        &toks,
        &[
            Step::Key("0"),
            Step::Key("8"),
            Step::Key("0"),
            Step::Index(frame),
            Step::Key("0"),
            Step::Key("1"),
            Step::Key("0"),
        ],
    )?;
    geometry_at(&toks, arr)
}

/// The frame rectangle of text object `object` (area text), `None` for point
/// text.
///
/// The object's own composed layout holds the frame in a `/99 /F` dict when it
/// has one; otherwise the frame list entry its view names
/// (`/1 /1 [object] /1 /0 [ << /0 frame >> ]`) supplies the box, which is how
/// Photoshop stores an area object whose cache was dropped.
pub fn object_frame_geometry(block: &[u8], object: usize) -> Option<FrameGeometry> {
    let toks = tokens(block).ok()?;
    let open = resolve(
        &toks,
        &[Step::Key("1"), Step::Key("1"), Step::Index(object)],
    )?;
    if let Some(arr) = frame_array_in(&toks, open) {
        return geometry_at(&toks, arr);
    }
    let frame = resolve(
        &toks,
        &[
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(object),
            Step::Key("1"),
            Step::Key("0"),
            Step::Index(0),
            Step::Key("0"),
        ],
    )
    .and_then(|v| toks[v].kind.as_int())
    .and_then(|v| usize::try_from(v).ok())?;
    frame_list_entry(block, frame)
}

/// Writes the frame rectangle in place, keeping the stored order and every
/// other byte of the block.
pub fn set_frame_geometry(block: &[u8], g: FrameGeometry) -> Result<Vec<u8>, Error> {
    set_frame_geometry_at(block, 0, g)
}

/// [`set_frame_geometry`] for text object `object`: writes the rectangle in
/// the object's own `/99 /F` frame dict, leaving every other byte alone.
pub fn set_frame_geometry_at(
    block: &[u8],
    object: usize,
    g: FrameGeometry,
) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let open = resolve(
        &toks,
        &[Step::Key("1"), Step::Key("1"), Step::Index(object)],
    )
    .ok_or_else(|| Error::Txt2("text object not found".into()))?;
    let arr = frame_array_in(&toks, open)
        .ok_or_else(|| Error::Txt2("frame geometry not found".into()))?;
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

// ---------------------------------------------------------------------------
// Frame paths (type on a path and non-rectangular area frames)
// ---------------------------------------------------------------------------

/// A text frame's path as the document frame list stores it (`/0 /8 /0 [frame]
/// /0`): the shape Photoshop draws the type along.
///
/// Points are cubic segments in `TySh` text space: eight numbers per segment,
/// `anchor, out_ctrl, next_in_ctrl, next_anchor`, with the shared anchor
/// repeated between segments. `range` is the type's start/end in fractional
/// segment units (`-1.0` = the stored default) and `flip` is `/2 /11 /0`
/// ("type runs backwards"); `kind` is `/2 /0` (`2` = type on a path).
#[derive(Clone, Debug, PartialEq)]
pub struct FramePath {
    /// `/1 /0` points in text space (the stored matrix already applied).
    pub points: Vec<f64>,
    /// `/2 /0` frame kind (`2` = type on a path).
    pub kind: i64,
    /// `/2 /6` start/end in fractional segment units, `-1.0` for "default".
    pub range: [f64; 2],
    /// `/2 /11 /0`; `None` keeps the stored value.
    pub flip: Option<bool>,
    /// The stored `/2 /2` frame→text matrix when it is not the identity, so a
    /// caller can tell that a stored frame already describes the text-space
    /// points it was given. Never written: new frames use the identity.
    pub matrix: Option<[f64; 6]>,
}

/// Photoshop's point-text frame entry (what a fresh document pads its frame
/// list with): no `/1` path, the default `/6` range and the point kind details.
const POINT_FRAME: &str = "<< /0 << /2 << /6 [ -1.0 -1.0 ] /11 << /4 -1 /7 false >> >> >> >>";

/// Grows the document frame list (`/0 /8 /0 [frame] /0`) to `count` entries,
/// appending point-text frames for the missing ones. A block without a frame
/// list is left as it is. The writer calls this before giving a new object a
/// shape: a view can only name a frame the list has.
pub fn ensure_frame_list(block: &[u8], count: usize) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let Some(arr) = resolve(
        &toks,
        &[Step::Key("0"), Step::Key("8"), Step::Key("0")],
    ) else {
        return Ok(block.to_vec());
    };
    if !matches!(toks[arr].kind, Kind::ArrayOpen) {
        return Ok(block.to_vec());
    }
    let mut at = arr + 1;
    let mut entries = 0usize;
    while !matches!(toks.get(at).map(|t| &t.kind), Some(Kind::ArrayClose)) {
        at = skip_value(&toks, at)
            .ok_or_else(|| Error::Txt2("frame list is malformed".into()))?;
        entries += 1;
    }
    if entries >= count {
        return Ok(block.to_vec());
    }
    let mut text = String::with_capacity((count - entries) * POINT_FRAME.len());
    for _ in entries..count {
        text.push(' ');
        text.push_str(POINT_FRAME);
    }
    let mut out = block.to_vec();
    out.splice(toks[at].start..toks[at].start, text.into_bytes());
    Ok(out)
}

/// Turns the frame of text object `object` back into a point-text frame: the
/// path (`/1 /0`) and its kind/range/flip details go, leaving the entry the
/// small point frame Photoshop writes. Used when a layer is taken off its path.
pub fn set_point_frame(block: &[u8], object: usize) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let name = object_frame_name(&toks, object)
        .ok_or_else(|| Error::Txt2("text object has no frame".into()))?;
    let frame = usize::try_from(
        toks[name]
            .kind
            .as_int()
            .ok_or_else(|| Error::Txt2("frame name is not an integer".into()))?,
    )
    .map_err(|_| Error::Txt2("frame name is out of range".into()))?;
    frame_dict(&toks, frame).ok_or_else(|| Error::Txt2("frame entry not found".into()))?;
    replace_container(
        block,
        &[
            Step::Key("0"),
            Step::Key("8"),
            Step::Key("0"),
            Step::Index(frame),
            Step::Key("0"),
        ],
        b"<< /2 << /6 [ -1.0 -1.0 ] /11 << /4 -1 /7 false >> >> >>",
    )
}

/// Token index of frame entry `frame`'s `/0` dict in the document frame list.
fn frame_dict(toks: &[Token], frame: usize) -> Option<usize> {
    let open = resolve(
        toks,
        &[
            Step::Key("0"),
            Step::Key("8"),
            Step::Key("0"),
            Step::Index(frame),
            Step::Key("0"),
        ],
    )?;
    matches!(toks[open].kind, Kind::DictOpen).then_some(open)
}

/// Offset of the text object's `/1 /0 [0] /0` frame name inside its view, if it
/// has one.
fn object_frame_name(toks: &[Token], object: usize) -> Option<usize> {
    resolve(
        toks,
        &[
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(object),
            Step::Key("1"),
            Step::Key("0"),
            Step::Index(0),
            Step::Key("0"),
        ],
    )
}

/// The frame index text object `object`'s view names, if it names one.
pub fn object_frame_index(block: &[u8], object: usize) -> Option<usize> {
    let toks = tokens(block).ok()?;
    let name = object_frame_name(&toks, object)?;
    usize::try_from(toks[name].kind.as_int()?).ok()
}

/// Reads the path of frame list entry `frame` (`/1 /0` points, `/2 /0` kind,
/// `/2 /6` range, `/2 /11 /0` flip), mapping the points through the stored
/// `/2 /2` matrix into text space.
///
/// `None` when the entry is not a path frame (a rectangle's `/1` is the four
/// box numbers, not a `<< /0 [points] >>`), when the points are malformed, or
/// when the matrix is singular.
pub fn frame_path(block: &[u8], frame: usize) -> Option<FramePath> {
    let toks = tokens(block).ok()?;
    frame_dict(&toks, frame)?;
    // `/1 /0`: the points, only for path frames.
    let points_at = resolve(
        &toks,
        &[
            Step::Key("0"),
            Step::Key("8"),
            Step::Key("0"),
            Step::Index(frame),
            Step::Key("0"),
            Step::Key("1"),
            Step::Key("0"),
        ],
    )?;
    if !matches!(toks[points_at].kind, Kind::ArrayOpen) {
        return None;
    }
    let mut raw = Vec::new();
    let mut i = points_at + 1;
    while i < toks.len() {
        if matches!(toks[i].kind, Kind::ArrayClose) {
            break;
        }
        raw.push(number(&toks[i].kind)?);
        i = skip_value(&toks, i)?;
    }
    if raw.is_empty() || raw.len() % 2 != 0 {
        return None;
    }
    // `/2 /2`: the frame→text matrix, applied to the stored points.
    let matrix = resolve(
        &toks,
        &[
            Step::Key("0"),
            Step::Key("8"),
            Step::Key("0"),
            Step::Index(frame),
            Step::Key("0"),
            Step::Key("2"),
            Step::Key("2"),
        ],
    )
    .filter(|&m| matches!(toks[m].kind, Kind::ArrayOpen))
    .and_then(|m| {
        let mut values = Vec::new();
        let mut i = m + 1;
        while i < toks.len() {
            if matches!(toks[i].kind, Kind::ArrayClose) {
                break;
            }
            values.push(number(&toks[i].kind)?);
            i = skip_value(&toks, i)?;
        }
        <[f64; 6]>::try_from(values).ok()
    });
    let points = match matrix {
        None => raw,
        Some(m) => {
            let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
            let (a, b, c, d, e, f) = (m[0], m[1], m[2], m[3], m[4], m[5]);
            let det = a * d - b * c;
            if !det.is_finite() || det.abs() <= 1e-12 || m == identity {
                if m != identity {
                    return None;
                }
                raw
            } else {
                (0..raw.len() / 2)
                    .flat_map(|i| {
                        let [x, y] = [raw[2 * i], raw[2 * i + 1]];
                        [a * x + c * y + e, b * x + d * y + f]
                    })
                    .collect()
            }
        }
    };
    let mut range = [-1.0, -1.0];
    if let Some(at) = resolve(
        &toks,
        &[
            Step::Key("0"),
            Step::Key("8"),
            Step::Key("0"),
            Step::Index(frame),
            Step::Key("0"),
            Step::Key("2"),
            Step::Key("6"),
        ],
    ) && matches!(toks[at].kind, Kind::ArrayOpen)
    {
        let mut values = Vec::new();
        let mut i = at + 1;
        while i < toks.len() {
            if matches!(toks[i].kind, Kind::ArrayClose) {
                break;
            }
            values.push(number(&toks[i].kind)?);
            i = skip_value(&toks, i)?;
        }
        if values.len() == 2 {
            range = [values[0], values[1]];
        }
    }
    let kind = resolve(
        &toks,
        &[
            Step::Key("0"),
            Step::Key("8"),
            Step::Key("0"),
            Step::Index(frame),
            Step::Key("0"),
            Step::Key("2"),
            Step::Key("0"),
        ],
    )
    .and_then(|k| toks[k].kind.as_int())
    .unwrap_or(0);
    let flip = resolve(
        &toks,
        &[
            Step::Key("0"),
            Step::Key("8"),
            Step::Key("0"),
            Step::Index(frame),
            Step::Key("0"),
            Step::Key("2"),
            Step::Key("11"),
            Step::Key("0"),
        ],
    )
    .and_then(|f| match &toks[f].kind {
        Kind::Bool(b) => Some(*b),
        _ => None,
    });
    Some(FramePath {
        points,
        kind,
        range,
        flip,
        matrix: matrix.filter(|m| *m != [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
    })
}

/// The path of the frame text object `object`'s view names, if it has one.
pub fn object_frame_path(block: &[u8], object: usize) -> Option<FramePath> {
    let toks = tokens(block).ok()?;
    let name = object_frame_name(&toks, object)?;
    let frame = usize::try_from(toks[name].kind.as_int()?).ok()?;
    frame_path(block, frame)
}

/// Writes `path` into the frame list entry text object `object` names, creating
/// the path keys a rectangle frame lacks.
///
/// Unknown keys of the existing frame (other `/2 /11` flags, extra `/2` entries)
/// are kept; the frame's `/1` becomes `<< /0 [points] >>` and its `/2` gets the
/// kind, the identity `/2 /2` matrix (points are text space), the `/6` range and
/// `/11 /0` flip. Writing a path equal to the stored one returns the block
/// unchanged.
pub fn set_object_frame_path(
    block: &[u8],
    object: usize,
    path: &FramePath,
) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let name = object_frame_name(&toks, object)
        .ok_or_else(|| Error::Txt2("text object has no frame".into()))?;
    let frame = usize::try_from(
        toks[name]
            .kind
            .as_int()
            .ok_or_else(|| Error::Txt2("frame name is not an integer".into()))?,
    )
    .map_err(|_| Error::Txt2("frame name is out of range".into()))?;
    let dict =
        frame_dict(&toks, frame).ok_or_else(|| Error::Txt2("frame entry not found".into()))?;
    if object_frame_path(block, object).as_ref() == Some(path) {
        return Ok(block.to_vec());
    }
    let entries = dict_entries(&toks, dict)
        .ok_or_else(|| Error::Txt2("frame entry is not a dictionary".into()))?;
    // One entry's original bytes (`/key value`), so keys this writer does not
    // model survive the rewrite.
    let entry_text = |value: usize| -> Result<String, Error> {
        let key_at = value
            .checked_sub(1)
            .ok_or_else(|| Error::Txt2("frame entry has no key".into()))?;
        let end =
            skip_value(&toks, value).ok_or_else(|| Error::Txt2("frame entry has no end".into()))?;
        String::from_utf8(block[toks[key_at].start..toks[end - 1].end].to_vec())
            .map_err(|_| Error::Txt2("frame entry is not UTF-8".into()))
    };
    let data_at = resolve(
        &toks,
        &[
            Step::Key("0"),
            Step::Key("8"),
            Step::Key("0"),
            Step::Index(frame),
            Step::Key("0"),
            Step::Key("2"),
        ],
    );
    let mut flags_at = None;
    let mut data = String::from("<<");
    if let Some(at) = data_at
        && matches!(toks[at].kind, Kind::DictOpen)
        && let Some(inner) = dict_entries(&toks, at)
    {
        for (key, value) in &inner {
            match key.as_str() {
                "11" => flags_at = Some(*value),
                "0" | "2" | "6" => {}
                _ => {
                    data.push(' ');
                    data.push_str(&entry_text(*value)?);
                }
            }
        }
    }
    data.push_str(&format!(" /0 {} ", path.kind));
    data.push_str("/2 [ 1.0 0.0 0.0 1.0 0.0 0.0 ]");
    data.push_str(&format!(
        " /6 [ {} {} ]",
        format_real(path.range[0]),
        format_real(path.range[1])
    ));
    if flags_at.is_some() || path.flip.is_some() {
        let mut flags = String::from("<<");
        if let Some(at) = flags_at
            && matches!(toks[at].kind, Kind::DictOpen)
            && let Some(inner) = dict_entries(&toks, at)
        {
            for (key, value) in &inner {
                if key == "0" {
                    continue;
                }
                flags.push(' ');
                flags.push_str(&entry_text(*value)?);
            }
        }
        if let Some(flip) = path.flip {
            flags.push_str(if flip { " /0 true" } else { " /0 false" });
        }
        flags.push_str(" >>");
        data.push_str(" /11 ");
        data.push_str(&flags);
    }
    data.push_str(" >>");
    let mut entry = String::from("<< /1 << /0 [");
    for value in &path.points {
        entry.push(' ');
        entry.push_str(&format_real(*value));
    }
    entry.push_str(" ] >> /2 ");
    entry.push_str(&data);
    for (key, value) in &entries {
        if key == "1" || key == "2" {
            continue;
        }
        entry.push(' ');
        entry.push_str(&entry_text(*value)?);
    }
    entry.push_str(" >>");
    replace_container(
        block,
        &[
            Step::Key("0"),
            Step::Key("8"),
            Step::Key("0"),
            Step::Index(frame),
            Step::Key("0"),
        ],
        entry.as_bytes(),
    )
}
