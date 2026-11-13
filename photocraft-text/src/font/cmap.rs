//! `cmap` table: character to glyph id.

use super::reader::Reader;
use crate::error::{Error, Result};

/// Picks the best Unicode subtable: returns (offset, format).
pub fn pick_subtable(data: &[u8]) -> Result<(usize, u16)> {
    let mut r = Reader::new(data);
    r.skip(2)?;
    let count = r.u16()?;
    let mut best: Option<(u32, usize, u16)> = None;
    for _ in 0..count {
        let platform = r.u16()?;
        let encoding = r.u16()?;
        let offset = r.u32()? as usize;
        if offset + 2 > data.len() {
            continue;
        }
        let format = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let rank = match (platform, encoding, format) {
            (3, 10, 12) => 100,
            (0, 6, 12) => 95,
            (0, 4, 12) => 90,
            (3, 1, 4) => 80,
            (0, 3, 4) => 75,
            (0, 0, 4) => 70,
            (0, 5, 14) => 60,
            (3, 0, 4) => 50,
            (3, 1, 6) => 40,
            (0, 1, 6) => 35,
            (1, 0, 0) => 10,
            _ => 0,
        };
        if rank > 0 && best.map(|(b, _, _)| rank > b).unwrap_or(true) {
            best = Some((rank, offset, format));
        }
    }
    match best {
        Some((_, off, fmt)) => Ok((off, fmt)),
        None => Err(Error::Unsupported {
            what: "cmap subtable",
            value: 0,
        }),
    }
}

pub fn lookup(data: &[u8], (offset, format): (usize, u16), cp: u32) -> Option<u16> {
    let mut r = Reader::at(data, offset).ok()?;
    match format {
        0 => {
            let _ = r.skip(6);
            let idx = usize::try_from(cp).ok()?;
            if idx > 255 {
                return None;
            }
            let _ = r.skip(idx).ok()?;
            let gid = r.u8().ok()?;
            (gid != 0).then_some(gid as u16)
        }
        4 => lookup_format4(&mut r, cp),
        6 => {
            let _ = r.skip(6);
            let first = r.u16().ok()? as u32;
            let count = r.u16().ok()? as u32;
            if cp < first || cp >= first + count {
                return None;
            }
            let _ = r.skip(2 * (cp - first) as usize).ok()?;
            let gid = r.u16().ok()?;
            (gid != 0).then_some(gid)
        }
        12 => {
            let _ = r.skip(12);
            let groups = r.u32().ok()?;
            let mut search_lo = 0u32;
            let mut search_hi = groups;
            while search_lo < search_hi {
                let mid = (search_lo + search_hi) / 2;
                let pos = 16 + mid as usize * 12;
                let mut gr = Reader::at(data, offset + pos).ok()?;
                let start = gr.u32().ok()?;
                let end = gr.u32().ok()?;
                let start_gid = gr.u32().ok()?;
                if cp < start {
                    search_hi = mid;
                } else if cp > end {
                    search_lo = mid + 1;
                } else {
                    let gid = start_gid.checked_add(cp - start)?;
                    return (gid != 0).then_some(gid as u16);
                }
            }
            None
        }
        _ => None,
    }
}

fn lookup_format4(r: &mut Reader<'_>, cp: u32) -> Option<u16> {
    // format(2) length(2) language(2) segCountX2(2) searchRange(2)
    // entrySelector(2) rangeShift(2) then the parallel arrays.
    let _ = r.skip(6).ok()?;
    let seg_count_x2 = r.u16().ok()?;
    let segments = seg_count_x2 / 2;
    let _ = r.skip(6).ok()?;
    let end_codes = r.pos();
    let _ = r.skip(seg_count_x2 as usize + 2).ok()?;
    let start_codes = r.pos();
    let _ = r.skip(seg_count_x2 as usize).ok()?;
    let id_deltas = r.pos();
    let _ = r.skip(seg_count_x2 as usize).ok()?;
    let id_range_offsets = r.pos();

    let mut r2 = Reader::at(r.data(), end_codes).ok()?;
    let mut seg = 0u16;
    while seg < segments {
        let end = r2.u16().ok()? as u32;
        if cp <= end {
            break;
        }
        seg += 1;
    }
    if seg >= segments {
        return None;
    }
    let mut rs = Reader::at(r.data(), start_codes + seg as usize * 2).ok()?;
    let start = rs.u16().ok()? as u32;
    if cp < start {
        return None;
    }
    let mut rd = Reader::at(r.data(), id_deltas + seg as usize * 2).ok()?;
    let delta = rd.i16().ok()? as i32;
    let mut ro = Reader::at(r.data(), id_range_offsets + seg as usize * 2).ok()?;
    let range_offset = ro.u16().ok()? as usize;
    if range_offset == 0 {
        let gid = (cp as i32 + delta) & 0xFFFF;
        return (gid != 0).then_some(gid as u16);
    }
    let glyph_pos = id_range_offsets + seg as usize * 2 + range_offset + (cp - start) as usize * 2;
    let mut rg = Reader::at(r.data(), glyph_pos).ok()?;
    let gid = rg.u16().ok()? as i32;
    if gid == 0 {
        return None;
    }
    let gid = (gid + delta) & 0xFFFF;
    (gid != 0).then_some(gid as u16)
}
