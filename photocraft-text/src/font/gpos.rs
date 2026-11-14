//! `GPOS` pair kerning (lookup type 2, plus type 9 extension wrappers).

use super::reader::Reader;
use crate::error::Result;

pub struct Gpos<'a> {
    data: &'a [u8],
    lookups: Vec<usize>,
}

impl<'a> Gpos<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let _version = r.u32()?;
        let script_list = r.u16()? as usize;
        let feature_list = r.u16()? as usize;
        let lookup_list = r.u16()? as usize;
        let _ = script_list;

        let lookups = collect_kern_lookups(data, feature_list, lookup_list).unwrap_or_default();
        Ok(Self { data, lookups })
    }

    pub fn pair(&self, left: u16, right: u16) -> Option<i32> {
        let mut best: Option<i32> = None;
        for lookup in &self.lookups {
            if let Some(v) = self.apply_lookup(*lookup, left, right) {
                // Later lookups accumulate (Photoshop sums adjustments).
                best = Some(best.unwrap_or(0) + v);
            }
        }
        best
    }

    fn apply_lookup(&self, lookup_offset: usize, left: u16, right: u16) -> Option<i32> {
        let mut r = Reader::at(self.data, lookup_offset).ok()?;
        let lookup_type = r.u16().ok()?;
        let _flag = r.u16().ok()?;
        let sub_count = r.u16().ok()?;
        let mut offsets = Vec::with_capacity(sub_count as usize);
        for _ in 0..sub_count {
            offsets.push(r.u16().ok()? as usize);
        }
        let mut total = 0i32;
        let mut any = false;
        for off in offsets {
            let mut sub_at = lookup_offset + off;
            let mut sub_type = lookup_type;
            if lookup_type == 9 {
                let mut sr = Reader::at(self.data, sub_at).ok()?;
                let format = sr.u16().ok()?;
                if format != 1 {
                    continue;
                }
                sub_type = sr.u16().ok()?;
                let ext = sr.u32().ok()? as usize;
                sub_at += ext;
            }
            if sub_type != 2 {
                continue;
            }
            if let Some(v) = self.apply_pair(sub_at, left, right) {
                total += v;
                any = true;
            }
        }
        any.then_some(total)
    }

    fn apply_pair(&self, at: usize, left: u16, right: u16) -> Option<i32> {
        let mut r = Reader::at(self.data, at).ok()?;
        let format = r.u16().ok()?;
        match format {
            1 => self.pair_format1(at, left, right),
            2 => self.pair_format2(at, left, right),
            _ => None,
        }
    }

    fn pair_format1(&self, at: usize, left: u16, right: u16) -> Option<i32> {
        let mut r = Reader::at(self.data, at).ok()?;
        let _format = r.u16().ok()?;
        let coverage = r.u16().ok()? as usize;
        let vf1 = r.u16().ok()?;
        let _vf2 = r.u16().ok()?;
        let pair_set_count = r.u16().ok()?;
        let idx = coverage_index(self.data, at + coverage, left)?;
        if idx >= pair_set_count as usize {
            return None;
        }
        let mut offs = Vec::with_capacity(pair_set_count as usize);
        for _ in 0..pair_set_count {
            offs.push(r.u16().ok()? as usize);
        }
        let mut ps = Reader::at(self.data, at + offs[idx]).ok()?;
        let count = ps.u16().ok()?;
        for _ in 0..count {
            let second = ps.u16().ok()?;
            let v1 = read_value(&mut ps, vf1)?;
            let _v2 = read_value(&mut ps, _vf2)?;
            if second == right {
                return Some(v1);
            }
        }
        None
    }

    fn pair_format2(&self, at: usize, left: u16, right: u16) -> Option<i32> {
        let mut r = Reader::at(self.data, at).ok()?;
        let _format = r.u16().ok()?;
        let coverage = r.u16().ok()? as usize;
        let vf1 = r.u16().ok()?;
        let vf2 = r.u16().ok()?;
        let class1_off = r.u16().ok()? as usize;
        let class2_off = r.u16().ok()? as usize;
        let class1_count = r.u16().ok()? as usize;
        let class2_count = r.u16().ok()? as usize;
        coverage_index(self.data, at + coverage, left)?;
        let c1 = class_of(self.data, at + class1_off, left)? as usize;
        let c2 = class_of(self.data, at + class2_off, right)? as usize;
        if c1 >= class1_count || c2 >= class2_count {
            return None;
        }
        let record_size = value_size(vf1) + value_size(vf2);
        let matrix = r.pos();
        let mut vr = Reader::at(self.data, matrix + (c1 * class2_count + c2) * record_size).ok()?;
        read_value(&mut vr, vf1)
    }
}

fn collect_kern_lookups(
    data: &[u8],
    feature_list: usize,
    lookup_list: usize,
) -> Option<Vec<usize>> {
    let mut fl = Reader::at(data, feature_list).ok()?;
    let feature_count = fl.u16().ok()?;
    let mut records = Vec::new();
    for _ in 0..feature_count {
        let tag = fl.tag().ok()?;
        let off = fl.u16().ok()? as usize;
        records.push((tag, off));
    }
    let mut lookups = Vec::new();
    for (tag, off) in records {
        if &tag != b"kern" {
            continue;
        }
        let mut fr = Reader::at(data, feature_list + off).ok()?;
        let _params = fr.u16().ok()?;
        let count = fr.u16().ok()?;
        for _ in 0..count {
            let idx = fr.u16().ok()? as usize;
            lookups.push(idx);
        }
    }
    let mut ll = Reader::at(data, lookup_list).ok()?;
    let lookup_count = ll.u16().ok()?;
    let mut offsets = Vec::with_capacity(lookup_count as usize);
    for _ in 0..lookup_count {
        offsets.push(ll.u16().ok()? as usize);
    }
    // One lookup can be named by the `kern` feature of several scripts (MyriadPro-Regular's
    // GPOS has nine `kern` feature records, all pointing at lookup 1). Kerning is summed over
    // lookups, so the same lookup must not be applied once per script: `AV` at 48 pt came out
    // 9 x too tight (-477 instead of -53 units, i.e. 59 px instead of 206 px for `AVAVAVAV`).
    let mut seen = std::collections::HashSet::new();
    Some(
        lookups
            .into_iter()
            .filter(|i| seen.insert(*i))
            .filter_map(|i| offsets.get(i).map(|o| lookup_list + o))
            .collect(),
    )
}

fn coverage_index(data: &[u8], at: usize, gid: u16) -> Option<usize> {
    let mut r = Reader::at(data, at).ok()?;
    let format = r.u16().ok()?;
    match format {
        1 => {
            let count = r.u16().ok()? as usize;
            let mut lo = 0usize;
            let mut hi = count;
            while lo < hi {
                let mid = (lo + hi) / 2;
                let mut gr = Reader::at(data, at + 4 + mid * 2).ok()?;
                let g = gr.u16().ok()?;
                match g.cmp(&gid) {
                    std::cmp::Ordering::Less => lo = mid + 1,
                    std::cmp::Ordering::Greater => hi = mid,
                    std::cmp::Ordering::Equal => return Some(mid),
                }
            }
            None
        }
        2 => {
            let count = r.u16().ok()? as usize;
            for i in 0..count {
                let mut rr = Reader::at(data, at + 4 + i as usize * 6).ok()?;
                let start = rr.u16().ok()?;
                let end = rr.u16().ok()?;
                let start_idx = rr.u16().ok()? as usize;
                if gid >= start && gid <= end {
                    return Some(start_idx + (gid - start) as usize);
                }
            }
            None
        }
        _ => None,
    }
}

fn class_of(data: &[u8], at: usize, gid: u16) -> Option<u16> {
    let mut r = Reader::at(data, at).ok()?;
    let format = r.u16().ok()?;
    match format {
        1 => {
            let start = r.u16().ok()?;
            let count = r.u16().ok()?;
            if gid < start || gid - start >= count {
                return Some(0);
            }
            let _ = r.skip((gid - start) as usize * 2).ok()?;
            r.u16().ok()
        }
        2 => {
            let count = r.u16().ok()?;
            for i in 0..count {
                let mut rr = Reader::at(data, at + 4 + i as usize * 6).ok()?;
                let start = rr.u16().ok()?;
                let end = rr.u16().ok()?;
                let class = rr.u16().ok()?;
                if gid >= start && gid <= end {
                    return Some(class);
                }
            }
            Some(0)
        }
        _ => None,
    }
}

fn value_size(vf: u16) -> usize {
    let mut n = 0;
    for bit in 0..8 {
        if vf & (1 << bit) != 0 {
            n += 2;
        }
    }
    n
}

fn read_value(r: &mut Reader<'_>, vf: u16) -> Option<i32> {
    let mut advance = 0;
    for bit in 0..8 {
        if vf & (1 << bit) == 0 {
            continue;
        }
        let v = r.i16().ok()?;
        if bit == 2 {
            advance = v as i32;
        }
    }
    Some(advance)
}
