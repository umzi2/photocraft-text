//! `GSUB` ligature substitution: LookupType 4 (`LigatureSubst`, format 1),
//! reached directly or through LookupType 7 extension wrappers. `liga`/`dlig`
//! are resolved through the ScriptList (default LangSys of the first matching
//! script).
//!
//! Not implemented: every other substitution type (single, multiple,
//! alternate, contextual, chaining, reverse), required features, and lookup
//! flags that depend on GDEF (ignore marks, mark attachment, mark filtering
//! sets). Lookups of unsupported types are no-ops.

use super::reader::Reader;
use crate::error::{Error, Result};

pub const TAG_GSUB: [u8; 4] = *b"GSUB";

const TYPE_LIGATURE: u16 = 4;
const TYPE_EXTENSION: u16 = 7;

/// One glyph of shaping input: its gid and the range of original items it
/// stands for. Ligature substitution merges adjacent spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub gid: u16,
    /// Index of the first original item covered.
    pub start: usize,
    /// Number of original items covered.
    pub len: usize,
}

pub struct Gsub<'a> {
    data: &'a [u8],
    script_list: usize,
    feature_list: usize,
    lookup_list: usize,
}

impl<'a> Gsub<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let version = r.u32()?;
        if version >> 16 != 1 {
            return Err(Error::Unsupported { what: "GSUB version", value: (version >> 16) as i64 });
        }
        let script_list = r.u16()? as usize;
        let feature_list = r.u16()? as usize;
        let lookup_list = r.u16()? as usize;
        Ok(Self { data, script_list, feature_list, lookup_list })
    }

    /// Absolute offsets of the lookups used by `features` under the first of
    /// `scripts` the font defines (default LangSys only), in lookup-index order.
    pub fn lookups(&self, scripts: &[[u8; 4]], features: &[[u8; 4]]) -> Vec<usize> {
        let Some(langsys) = scripts.iter().find_map(|t| self.default_langsys(*t)) else {
            return Vec::new();
        };
        let (Some(indices), Some(records)) = (self.langsys_features(langsys), self.feature_records()) else {
            return Vec::new();
        };
        let mut lookup_indices: Vec<u16> = Vec::new();
        for fi in indices {
            if let Some((tag, abs)) = records.get(fi) {
                if features.contains(tag) {
                    lookup_indices.extend(self.feature_lookup_indices(*abs).unwrap_or_default());
                }
            }
        }
        lookup_indices.sort_unstable();
        lookup_indices.dedup();
        lookup_indices.into_iter().filter_map(|i| self.lookup_offset(i)).collect()
    }

    /// Applies one lookup to `spans`. Lookups without LigatureSubst subtables
    /// return the input unchanged.
    pub fn ligate(&self, lookup: usize, spans: &[Span]) -> Vec<Span> {
        let mut cur = spans.to_vec();
        for sub in self.ligature_subtables(lookup).unwrap_or_default() {
            cur = self.ligate_subtable(sub, &cur);
        }
        cur
    }

    fn default_langsys(&self, tag: [u8; 4]) -> Option<usize> {
        let base = self.script_list;
        let mut r = Reader::at(self.data, base).ok()?;
        let count = r.u16().ok()?;
        for _ in 0..count {
            let t = r.tag().ok()?;
            let off = r.u16().ok()? as usize;
            if t == tag {
                let default = Reader::at(self.data, base + off).ok()?.u16().ok()? as usize;
                return (default != 0).then_some(base + off + default);
            }
        }
        None
    }

    /// Feature indices of a LangSys. The required feature is not included.
    fn langsys_features(&self, langsys: usize) -> Option<Vec<usize>> {
        let mut r = Reader::at(self.data, langsys).ok()?;
        r.u16().ok()?; // lookupOrder (reserved)
        r.u16().ok()?; // requiredFeatureIndex
        let count = r.u16().ok()?;
        (0..count).map(|_| r.u16().ok().map(usize::from)).collect()
    }

    /// `(tag, absolute offset)` of every FeatureRecord.
    fn feature_records(&self) -> Option<Vec<([u8; 4], usize)>> {
        let mut r = Reader::at(self.data, self.feature_list).ok()?;
        let count = r.u16().ok()?;
        (0..count)
            .map(|_| {
                let tag = r.tag().ok()?;
                let off = r.u16().ok()? as usize;
                Some((tag, self.feature_list + off))
            })
            .collect()
    }

    fn feature_lookup_indices(&self, feature: usize) -> Option<Vec<u16>> {
        let mut r = Reader::at(self.data, feature).ok()?;
        r.u16().ok()?; // featureParams
        let count = r.u16().ok()?;
        (0..count).map(|_| r.u16().ok()).collect()
    }

    fn lookup_offset(&self, index: u16) -> Option<usize> {
        let mut r = Reader::at(self.data, self.lookup_list).ok()?;
        let count = r.u16().ok()?;
        if index >= count {
            return None;
        }
        r.skip(2 * index as usize).ok()?;
        Some(self.lookup_list + r.u16().ok()? as usize)
    }

    /// Absolute offsets of the LigatureSubst subtables of a lookup.
    fn ligature_subtables(&self, lookup: usize) -> Option<Vec<usize>> {
        let mut r = Reader::at(self.data, lookup).ok()?;
        let kind = r.u16().ok()?;
        r.u16().ok()?; // lookupFlag
        let count = r.u16().ok()?;
        let mut out = Vec::new();
        for _ in 0..count {
            let sub = lookup + r.u16().ok()? as usize;
            let (sub_kind, at) = if kind == TYPE_EXTENSION {
                let mut er = Reader::at(self.data, sub).ok()?;
                er.u16().ok()?; // format
                let real_kind = er.u16().ok()?;
                let ext = er.u32().ok()? as usize;
                (real_kind, sub + ext)
            } else {
                (kind, sub)
            };
            if sub_kind == TYPE_LIGATURE {
                out.push(at);
            }
        }
        Some(out)
    }

    fn ligate_subtable(&self, sub: usize, input: &[Span]) -> Vec<Span> {
        let mut out = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            match self.match_ligature(sub, &input[i..]) {
                Some((gid, n)) => {
                    let len = input[i..i + n].iter().map(|s| s.len).sum();
                    out.push(Span { gid, start: input[i].start, len });
                    i += n;
                }
                None => {
                    out.push(input[i]);
                    i += 1;
                }
            }
        }
        out
    }

    /// Ligature glyph and component count for the longest-priority match at
    /// the head of `seq`.
    fn match_ligature(&self, sub: usize, seq: &[Span]) -> Option<(u16, usize)> {
        let first = seq.first()?.gid;
        let mut r = Reader::at(self.data, sub).ok()?;
        if r.u16().ok()? != 1 {
            return None;
        }
        let coverage = sub + r.u16().ok()? as usize;
        let set_count = r.u16().ok()? as usize;
        let index = coverage_index(self.data, coverage, first)?;
        if index >= set_count {
            return None;
        }
        r.skip(2 * index).ok()?;
        let set = sub + r.u16().ok()? as usize;
        ligature_in_set(self.data, set, seq)
    }
}

/// LigatureSet: ligatures are tried in stored order; the first full match wins.
fn ligature_in_set(data: &[u8], set: usize, seq: &[Span]) -> Option<(u16, usize)> {
    let mut r = Reader::at(data, set).ok()?;
    let count = r.u16().ok()?;
    for _ in 0..count {
        let off = r.u16().ok()? as usize;
        let mut lr = Reader::at(data, set + off).ok()?;
        let glyph = lr.u16().ok()?;
        let comps = lr.u16().ok()? as usize;
        if comps == 0 || comps > seq.len() {
            continue;
        }
        if (1..comps).all(|j| lr.u16().ok() == Some(seq[j].gid)) {
            return Some((glyph, comps));
        }
    }
    None
}

/// Coverage table lookup: the index of `gid`, if covered.
fn coverage_index(data: &[u8], at: usize, gid: u16) -> Option<usize> {
    let mut r = Reader::at(data, at).ok()?;
    let format = r.u16().ok()?;
    let count = r.u16().ok()? as usize;
    match format {
        1 => {
            let bytes = r.bytes(count * 2).ok()?;
            let (mut lo, mut hi) = (0usize, count);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let g = u16::from_be_bytes([bytes[2 * mid], bytes[2 * mid + 1]]);
                match g.cmp(&gid) {
                    std::cmp::Ordering::Less => lo = mid + 1,
                    std::cmp::Ordering::Greater => hi = mid,
                    std::cmp::Ordering::Equal => return Some(mid),
                }
            }
            None
        }
        2 => {
            let bytes = r.bytes(count * 6).ok()?;
            bytes.chunks_exact(6).find_map(|c| {
                let start = u16::from_be_bytes([c[0], c[1]]);
                let end = u16::from_be_bytes([c[2], c[3]]);
                let base = u16::from_be_bytes([c[4], c[5]]) as usize;
                (start..=end).contains(&gid).then(|| base + (gid - start) as usize)
            })
        }
        _ => None,
    }
}
