//! `kern` table (both the Microsoft v0 and Apple v1 layouts).

use super::reader::Reader;
use crate::error::Result;

#[derive(Default)]
pub struct Kern<'a> {
    subtables: Vec<(usize, u16)>,
    data: &'a [u8],
}

impl<'a> Kern<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let mut subtables = Vec::new();
        let first = r.u16()?;
        if first == 0 {
            let count = r.u16()?;
            for _ in 0..count {
                let start = r.pos();
                let version = r.u16()?;
                let length = r.u16()?;
                let coverage = r.u16()?;
                // Only horizontal, non-minimum, non-cross-stream format 0 subtables.
                if version == 0 && (coverage & 0x1) == 0 && ((coverage >> 8) & 0xFF) == 0 {
                    subtables.push((start, length));
                }
                let _ = r.seek(start + length.max(6) as usize);
            }
        } else {
            // Apple: u32 version 0x00010000, u32 nTables
            let mut r = Reader::new(data);
            let _version = r.u32()?;
            let count = r.u32()?;
            for _ in 0..count {
                let start = r.pos();
                let length = r.u32()?;
                let coverage = r.u16()?;
                if (coverage & 0x1) == 0 && ((coverage >> 8) & 0xFF) == 0 {
                    subtables.push((start, u16::try_from(length).unwrap_or(u16::MAX)));
                }
                let _ = r.seek(start + length.max(8) as usize);
            }
        }
        Ok(Self { subtables, data })
    }

    pub fn pair(&self, left: u16, right: u16) -> Option<i32> {
        let key = ((left as u32) << 16) | right as u32;
        for (start, _len) in &self.subtables {
            let mut r = Reader::at(self.data, *start).ok()?;
            let _v = r.u16().ok()?;
            let _l = r.u16().ok()?;
            let _c = r.u16().ok()?;
            let n_pairs = r.u16().ok()?;
            let _ = r.skip(6).ok()?;
            let mut lo = 0u32;
            let mut hi = n_pairs as u32;
            while lo < hi {
                let mid = (lo + hi) / 2;
                let _ = r.seek(14 + start + mid as usize * 6);
                let l = r.u16().ok()?;
                let rt = r.u16().ok()?;
                let k = ((l as u32) << 16) | rt as u32;
                match k.cmp(&key) {
                    std::cmp::Ordering::Less => lo = mid + 1,
                    std::cmp::Ordering::Greater => hi = mid,
                    std::cmp::Ordering::Equal => return Some(r.i16().ok()? as i32),
                }
            }
        }
        None
    }
}
