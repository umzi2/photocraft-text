//! Glyph outlines from a TrueType `glyf` table.
//!
//! Returns closed contours of quadratic segments in font units, y up, exactly as
//! stored: on-curve points carry `on = true`, control points `on = false`.

use crate::error::{Error, Result};
use crate::font::Face;

/// One outline point in font units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub on: bool,
}

/// A glyph as a list of closed contours.
pub type Contours = Vec<Vec<Point>>;

const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const REPEAT: u8 = 0x08;
const X_SAME: u8 = 0x10;
const Y_SAME: u8 = 0x20;

const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const ARGS_ARE_XY_VALUES: u16 = 0x0002;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;

fn be16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn bei16(d: &[u8], at: usize) -> Option<i16> {
    be16(d, at).map(|v| v as i16)
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn f2dot14(v: i16) -> f32 {
    v as f32 / 16384.0
}

/// Outline of `gid`, in font units.
pub fn glyph_contours(face: &Face<'_>, gid: u16) -> Result<Contours> {
    glyph_contours_at(face, gid, 0)
}

fn glyph_contours_at(face: &Face<'_>, gid: u16, depth: usize) -> Result<Contours> {
    let loca = face.table(*b"loca").ok_or(Error::MissingTable("loca"))?;
    let glyf = face.table(*b"glyf").ok_or(Error::MissingTable("glyf"))?;
    let head = face.table(*b"head").ok_or(Error::MissingTable("head"))?;
    let long = bei16(head, 50).unwrap_or(0) != 0;
    let gid = gid as usize;
    let (start, end) = if long {
        (
            be32(loca, gid * 4).ok_or(Error::OutOfBounds { needed: gid * 4 + 4, len: loca.len() })? as usize,
            be32(loca, (gid + 1) * 4).ok_or(Error::OutOfBounds { needed: gid * 4 + 8, len: loca.len() })? as usize,
        )
    } else {
        (
            be16(loca, gid * 2).ok_or(Error::OutOfBounds { needed: gid * 2 + 2, len: loca.len() })? as usize * 2,
            be16(loca, (gid + 1) * 2).ok_or(Error::OutOfBounds { needed: gid * 2 + 4, len: loca.len() })? as usize * 2,
        )
    };
    if end <= start || end > glyf.len() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    parse_glyph(face, &glyf[start..end], depth, &mut out)?;
    Ok(out)
}

fn parse_glyph(face: &Face<'_>, data: &[u8], depth: usize, out: &mut Contours) -> Result<()> {
    if depth > 8 {
        return Err(Error::BadFont("composite glyph nesting"));
    }
    let num = bei16(data, 0).ok_or(Error::OutOfBounds { needed: 2, len: data.len() })?;
    if num >= 0 {
        return parse_simple(data, num as usize, out);
    }
    parse_composite(face, data, depth, out)
}

fn parse_simple(data: &[u8], contours: usize, out: &mut Contours) -> Result<()> {
    if contours == 0 {
        return Ok(());
    }
    let mut at = 10usize;
    let mut ends = Vec::with_capacity(contours);
    for _ in 0..contours {
        ends.push(be16(data, at).ok_or(Error::OutOfBounds { needed: at + 2, len: data.len() })? as usize);
        at += 2;
    }
    let points = ends.last().copied().unwrap_or(0) + 1;
    let instr = be16(data, at).ok_or(Error::OutOfBounds { needed: at + 2, len: data.len() })? as usize;
    at += 2 + instr;

    let mut flags = Vec::with_capacity(points);
    while flags.len() < points {
        let f = *data.get(at).ok_or(Error::OutOfBounds { needed: at + 1, len: data.len() })?;
        at += 1;
        flags.push(f);
        if f & REPEAT != 0 {
            let n = *data.get(at).ok_or(Error::OutOfBounds { needed: at + 1, len: data.len() })?;
            at += 1;
            for _ in 0..n {
                flags.push(f);
            }
        }
    }
    flags.truncate(points);

    let mut xs = Vec::with_capacity(points);
    let mut x = 0f32;
    for &f in &flags {
        if f & X_SHORT != 0 {
            let d = *data.get(at).ok_or(Error::OutOfBounds { needed: at + 1, len: data.len() })? as f32;
            at += 1;
            x += if f & X_SAME != 0 { d } else { -d };
        } else if f & X_SAME == 0 {
            x += bei16(data, at).ok_or(Error::OutOfBounds { needed: at + 2, len: data.len() })? as f32;
            at += 2;
        }
        xs.push(x);
    }
    let mut ys = Vec::with_capacity(points);
    let mut y = 0f32;
    for &f in &flags {
        if f & Y_SHORT != 0 {
            let d = *data.get(at).ok_or(Error::OutOfBounds { needed: at + 1, len: data.len() })? as f32;
            at += 1;
            y += if f & Y_SAME != 0 { d } else { -d };
        } else if f & Y_SAME == 0 {
            y += bei16(data, at).ok_or(Error::OutOfBounds { needed: at + 2, len: data.len() })? as f32;
            at += 2;
        }
        ys.push(y);
    }

    let mut start = 0usize;
    for end in ends {
        let contour: Vec<Point> = (start..=end.min(points.saturating_sub(1)))
            .map(|i| Point { x: xs[i], y: ys[i], on: flags[i] & ON_CURVE != 0 })
            .collect();
        if contour.len() > 1 {
            out.push(contour);
        }
        start = end + 1;
    }
    Ok(())
}

fn parse_composite(face: &Face<'_>, data: &[u8], depth: usize, out: &mut Contours) -> Result<()> {
    let mut at = 10usize;
    loop {
        let flags = be16(data, at).ok_or(Error::OutOfBounds { needed: at + 2, len: data.len() })?;
        let gid = be16(data, at + 2).ok_or(Error::OutOfBounds { needed: at + 4, len: data.len() })?;
        at += 4;
        let word = flags & ARG_1_AND_2_ARE_WORDS != 0;
        let (r1, r2) = if word {
            (be16(data, at).unwrap_or(0) as u32, be16(data, at + 2).unwrap_or(0) as u32)
        } else {
            (*data.get(at).unwrap_or(&0) as u32, *data.get(at + 1).unwrap_or(&0) as u32)
        };
        at += if word { 4 } else { 2 };
        let xy = flags & ARGS_ARE_XY_VALUES != 0;
        // Offset interpretation: signed XY, or unsigned point indices (parent, child).
        let (sx, sy) = if word { (r1 as u16 as i16 as f32, r2 as u16 as i16 as f32) } else { (r1 as u8 as i8 as f32, r2 as u8 as i8 as f32) };

        let (mut a, mut b, mut c, mut d) = (1.0f32, 0.0f32, 0.0f32, 0.0f32);
        if flags & WE_HAVE_A_SCALE != 0 {
            let s = f2dot14(bei16(data, at).unwrap_or(16384));
            at += 2;
            a = s;
            d = s;
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            a = f2dot14(bei16(data, at).unwrap_or(16384));
            d = f2dot14(bei16(data, at + 2).unwrap_or(16384));
            at += 4;
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            a = f2dot14(bei16(data, at).unwrap_or(16384));
            b = f2dot14(bei16(data, at + 2).unwrap_or(0));
            c = f2dot14(bei16(data, at + 4).unwrap_or(0));
            d = f2dot14(bei16(data, at + 6).unwrap_or(16384));
            at += 8;
        }

        let mut inner: Contours = glyph_contours_at(face, gid, depth + 1)?
            .into_iter()
            .map(|contour| contour.into_iter().map(|p| Point { x: a * p.x + c * p.y, y: b * p.x + d * p.y, on: p.on }).collect())
            .collect();
        let (dx, dy) = if xy {
            (sx, sy)
        } else {
            let parent = out.iter().flatten().nth(r1 as usize).ok_or(Error::BadFont("composite parent point index"))?;
            let child = inner.iter().flatten().nth(r2 as usize).ok_or(Error::BadFont("composite component point index"))?;
            (parent.x - child.x, parent.y - child.y)
        };
        for p in inner.iter_mut().flatten() {
            p.x += dx;
            p.y += dy;
        }
        for contour in inner {
            out.push(contour);
        }

        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    Ok(())
}
