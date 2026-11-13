//! `GSUB` substitution: LookupType 1 (`SingleSubst`, formats 1 and 2) for the
//! character features Photoshop applies per run (`smcp`, `c2sc`, `onum`),
//! LookupType 4 (`LigatureSubst`, format 1) for `liga`/`dlig`, and LookupType 6
//! (`ChainContextSubst`, format 3) for contextual rules that fonts put inside
//! `liga`/`dlig` (CCGrimNGritty alternates its letter forms that way), all
//! reached directly or through LookupType 7 extension wrappers. Features are
//! resolved through the ScriptList (default LangSys of the first matching
//! script).
//!
//! Chain-context rules are matched left to right over the shaping group: the
//! input coverage sequence plus backtrack (in reverse logical order) and
//! lookahead coverage all have to match, then the rule's substitution records
//! are applied in order. The nested lookups those records name are applied to
//! the matching glyph; this implementation supports nested single
//! substitutions, which is what the catalogue's fonts use. `ccmp`, `locl`,
//! `ordn` and `frac` contextual rules are not applied because Photoshop does
//! not apply those features by default for Latin runs.
//!
//! Not implemented: multiple, alternate, reverse substitutions, required
//! features, and lookup flags that depend on GDEF (ignore marks, mark
//! attachment, mark filtering sets). Lookups of unsupported types are no-ops.

use super::reader::Reader;
use crate::error::{Error, Result};

pub const TAG_GSUB: [u8; 4] = *b"GSUB";

const TYPE_SINGLE: u16 = 1;
const TYPE_LIGATURE: u16 = 4;
const TYPE_CHAIN: u16 = 6;
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
            return Err(Error::Unsupported {
                what: "GSUB version",
                value: (version >> 16) as i64,
            });
        }
        let script_list = r.u16()? as usize;
        let feature_list = r.u16()? as usize;
        let lookup_list = r.u16()? as usize;
        Ok(Self {
            data,
            script_list,
            feature_list,
            lookup_list,
        })
    }

    /// Absolute offsets of the lookups used by `features` under the first of
    /// `scripts` the font defines (default LangSys only), in lookup-index order.
    pub fn lookups(&self, scripts: &[[u8; 4]], features: &[[u8; 4]]) -> Vec<usize> {
        let Some(langsys) = scripts.iter().find_map(|t| self.default_langsys(*t)) else {
            return Vec::new();
        };
        let (Some(indices), Some(records)) =
            (self.langsys_features(langsys), self.feature_records())
        else {
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
        lookup_indices
            .into_iter()
            .filter_map(|i| self.lookup_offset(i))
            .collect()
    }

    /// Applies one lookup to `spans`. Lookups without SingleSubst subtables
    /// return the input unchanged.  Substitution never changes `start`/`len`:
    /// one glyph stays one glyph.
    pub fn substitute(&self, lookup: usize, spans: &[Span]) -> Vec<Span> {
        let mut cur = spans.to_vec();
        for sub in self.single_subtables(lookup).unwrap_or_default() {
            cur = self.substitute_subtable(sub, &cur);
        }
        cur
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

    /// Applies one lookup of any supported type: `SingleSubst`, `LigatureSubst`
    /// or `ChainContextSubst`, including LookupType 7 extension wrappers.
    /// Contextual subtables of one lookup are tried at each position in table
    /// order and the first match wins, because a rule may depend on a glyph an
    /// earlier rule just substituted.
    pub fn apply(&self, lookup: usize, spans: &[Span]) -> Vec<Span> {
        let mut cur = spans.to_vec();
        let subtables = self.lookup_subtables(lookup).unwrap_or_default();
        let chains: Vec<usize> = subtables
            .iter()
            .filter(|(kind, _)| *kind == TYPE_CHAIN)
            .map(|(_, sub)| *sub)
            .collect();
        if !chains.is_empty() {
            cur = self.chain_lookup(&chains, &cur);
        }
        for (kind, sub) in subtables {
            cur = match kind {
                TYPE_SINGLE => self.substitute_subtable(sub, &cur),
                TYPE_LIGATURE => self.ligate_subtable(sub, &cur),
                _ => cur,
            };
        }
        cur
    }

    /// `(kind, absolute offset)` of every subtable of a lookup, resolving
    /// extension wrappers.
    fn lookup_subtables(&self, lookup: usize) -> Option<Vec<(u16, usize)>> {
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
            out.push((sub_kind, at));
        }
        Some(out)
    }

    /// Applies the contextual subtables of one lookup left to right.
    fn chain_lookup(&self, subtables: &[usize], input: &[Span]) -> Vec<Span> {
        let mut out = input.to_vec();
        let mut i = 0;
        while i < out.len() {
            let mut advance = 1;
            for sub in subtables {
                if let Some((input_len, records)) = self.chain_match(*sub, &out, i) {
                    for (seq, lookup) in records {
                        let at = i + seq;
                        if at >= out.len() {
                            continue;
                        }
                        let mut gid = out[at].gid;
                        for (kind, nested_sub) in
                            self.lookup_subtables_at(lookup).unwrap_or_default()
                        {
                            if kind == TYPE_SINGLE {
                                if let Some(next) = self.substitute_glyph(nested_sub, gid) {
                                    gid = next;
                                }
                            }
                        }
                        out[at].gid = gid;
                    }
                    advance = input_len;
                    break;
                }
            }
            i += advance.max(1);
        }
        out
    }

    /// Matches a ChainContextSubst format 3 subtable at `i`: the input,
    /// backtrack and lookahead coverages all have to hold. Returns the input
    /// glyph count and the substitution records of the matching rule.
    ///
    /// A rule may validly have no records: it then matches and does nothing,
    /// which still matters because `chain_lookup` stops at the first matching
    /// subtable. CCGrimNGritty relies on that: its `liga` lookup 5 has no-op
    /// rules for `i`/`I` after a letter and an unconditional `i`/`I` → `bar`
    /// rule afterwards, so `i` keeps its normal form inside a word but becomes
    /// the bar glyph on its own (measured against Photoshop renders of
    /// `i`, `0i`, `ri`, `ai`, `mi`, `ti` and `i i`).
    fn chain_match(
        &self,
        sub: usize,
        spans: &[Span],
        i: usize,
    ) -> Option<(usize, Vec<(usize, usize)>)> {
        let mut r = Reader::at(self.data, sub).ok()?;
        if r.u16().ok()? != 3 {
            return None;
        }
        let read_coverages = |r: &mut Reader<'_>, base: usize| -> Option<Vec<usize>> {
            let count = r.u16().ok()? as usize;
            let mut out = Vec::with_capacity(count);
            for _ in 0..count {
                out.push(base + r.u16().ok()? as usize);
            }
            Some(out)
        };
        let backtrack = read_coverages(&mut r, sub)?;
        let inputs = read_coverages(&mut r, sub)?;
        let lookahead = read_coverages(&mut r, sub)?;
        let record_count = r.u16().ok()? as usize;
        if inputs.is_empty() {
            return None;
        }
        if i + inputs.len() > spans.len() {
            return None;
        }
        for (k, cov) in inputs.iter().enumerate() {
            if coverage_index(self.data, *cov, spans[i + k].gid).is_none() {
                return None;
            }
        }
        for (k, cov) in backtrack.iter().enumerate() {
            let prev = i.checked_sub(k + 1).and_then(|j| spans.get(j))?;
            if coverage_index(self.data, *cov, prev.gid).is_none() {
                return None;
            }
        }
        for (k, cov) in lookahead.iter().enumerate() {
            let next = spans.get(i + inputs.len() + k)?;
            if coverage_index(self.data, *cov, next.gid).is_none() {
                return None;
            }
        }
        let mut records = Vec::with_capacity(record_count);
        for _ in 0..record_count {
            let seq = r.u16().ok()? as usize;
            let lookup = r.u16().ok()? as usize;
            records.push((seq, lookup));
        }
        Some((inputs.len(), records))
    }

    /// Subtables of a lookup by *lookup index* (the record's `lookupListIndex`),
    /// resolving extension wrappers.
    fn lookup_subtables_at(&self, index: usize) -> Option<Vec<(u16, usize)>> {
        let offset = self.lookup_offset(index as u16)?;
        self.lookup_subtables(offset)
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

    /// Absolute offsets of the SingleSubst subtables of a lookup.
    fn single_subtables(&self, lookup: usize) -> Option<Vec<usize>> {
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
            if sub_kind == TYPE_SINGLE {
                out.push(at);
            }
        }
        Some(out)
    }

    fn substitute_subtable(&self, sub: usize, input: &[Span]) -> Vec<Span> {
        let mut out = input.to_vec();
        for span in out.iter_mut() {
            if let Some(gid) = self.substitute_glyph(sub, span.gid) {
                span.gid = gid;
            }
        }
        out
    }

    /// Replacement glyph for `gid` in a SingleSubst subtable, if covered.
    fn substitute_glyph(&self, sub: usize, gid: u16) -> Option<u16> {
        let mut r = Reader::at(self.data, sub).ok()?;
        let format = r.u16().ok()?;
        let coverage = sub + r.u16().ok()? as usize;
        let index = coverage_index(self.data, coverage, gid)?;
        match format {
            1 => {
                let delta = r.i16().ok()?;
                Some(gid.wrapping_add(delta as u16))
            }
            2 => {
                let count = r.u16().ok()? as usize;
                if index >= count {
                    return None;
                }
                r.skip(2 * index).ok()?;
                r.u16().ok()
            }
            _ => None,
        }
    }

    fn ligate_subtable(&self, sub: usize, input: &[Span]) -> Vec<Span> {
        let mut out = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            match self.match_ligature(sub, &input[i..]) {
                Some((gid, n)) => {
                    let len = input[i..i + n].iter().map(|s| s.len).sum();
                    out.push(Span {
                        gid,
                        start: input[i].start,
                        len,
                    });
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
                (start..=end)
                    .contains(&gid)
                    .then(|| base + (gid - start) as usize)
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal GSUB writer for table-layout tests: header, empty
    /// Script/FeatureLists and a LookupList we fill ourselves.
    struct Builder {
        data: Vec<u8>,
    }

    impl Builder {
        fn new() -> Self {
            let mut data = Vec::new();
            data.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // version 1.0
            data.extend_from_slice(&[0, 0, 0, 0]); // empty Script/FeatureList
            data.extend_from_slice(&[0, 0]); // LookupList offset, patched below
            Self { data }
        }

        fn here(&self) -> usize {
            self.data.len()
        }

        fn u16(&mut self, value: u16) -> usize {
            let at = self.here();
            self.data.extend_from_slice(&value.to_be_bytes());
            at
        }

        fn u32(&mut self, value: u32) -> usize {
            let at = self.here();
            self.data.extend_from_slice(&value.to_be_bytes());
            at
        }

        fn patch(&mut self, at: usize, value: usize) {
            self.data[at..at + 2].copy_from_slice(&(value as u16).to_be_bytes());
        }

        /// Coverage format 1 at the current end; returns its offset.
        fn coverage(&mut self, gids: &[u16]) -> usize {
            let at = self.here();
            self.u16(1);
            self.u16(gids.len() as u16);
            for gid in gids {
                self.u16(*gid);
            }
            at
        }
    }

    fn span(gid: u16) -> Span {
        Span {
            gid,
            start: 0,
            len: 1,
        }
    }

    /// Builds a GSUB with two lookups:
    /// 0: SingleSubst format 2, glyph 10 -> 11.
    /// 1: ChainContextSubst format 3 with three subtables:
    ///    - backtrack {5}, input {10}, no records;
    ///    - backtrack {7}, input {10}, record (0, lookup 0);
    ///    - no backtrack, input {10}, record (0, lookup 0).
    /// The first subtable matching at a position wins, so a preceding 5 blocks
    /// the substitution (even though the rule records nothing) while any other
    /// context falls through to the unconditional rule.
    fn chained_gsub() -> (Gsub<'static>, usize, usize) {
        let mut b = Builder::new();
        let lookup_list = b.here();
        b.patch(8, lookup_list); // LookupListOffset follows version+two list offsets
        b.u16(2);
        let lookup0_offset = b.u16(0);
        let lookup1_offset = b.u16(0);

        // Lookup 0: SingleSubst format 2.
        let lookup0 = b.here();
        b.u16(TYPE_SINGLE);
        b.u16(0);
        b.u16(1);
        let single_subtable = b.u16(0);
        let single = b.here();
        b.patch(single_subtable as usize, single - lookup0);
        b.u16(2);
        let coverage_at = b.u16(0);
        b.u16(1);
        b.u16(11);
        let coverage = b.coverage(&[10]);
        b.patch(coverage_at as usize, coverage - single);
        b.patch(lookup0_offset, lookup0 - lookup_list);

        // Lookup 1: ChainContextSubst format 3 with three subtables.
        let lookup1 = b.here();
        b.u16(TYPE_CHAIN);
        b.u16(0);
        b.u16(3);
        let sub_offsets = [b.u16(0), b.u16(0), b.u16(0)];
        // (backtrack, input, record) for each subtable; `None` stays empty.
        let rules: [(Option<u16>, u16, bool); 3] =
            [(Some(5), 10, false), (Some(7), 10, true), (None, 10, true)];
        for (slot, (backtrack, input, record)) in sub_offsets.iter().zip(rules) {
            let sub = b.here();
            b.patch(*slot as usize, sub - lookup1);
            b.u16(3);
            match backtrack {
                Some(back) => {
                    b.u16(1);
                    let bt_offset = b.u16(0);
                    b.u16(1);
                    let in_offset = b.u16(0);
                    b.u16(0); // lookahead
                    b.u16(u16::from(record)); // record count
                    if record {
                        b.u16(0); // sequence index
                        b.u16(0); // lookup list index (lookup 0)
                    }
                    let bt = b.coverage(&[back]);
                    b.patch(bt_offset as usize, bt - sub);
                    let in_cov = b.coverage(&[input]);
                    b.patch(in_offset as usize, in_cov - sub);
                }
                None => {
                    b.u16(0);
                    b.u16(1);
                    let in_offset = b.u16(0);
                    b.u16(0);
                    b.u16(u16::from(record));
                    if record {
                        b.u16(0);
                        b.u16(0);
                    }
                    let in_cov = b.coverage(&[input]);
                    b.patch(in_offset as usize, in_cov - sub);
                }
            }
        }
        b.patch(lookup1_offset, lookup1 - lookup_list);

        let data: &'static [u8] = Box::leak(b.data.into_boxed_slice());
        (
            Gsub::parse(data).expect("synthetic GSUB parses"),
            lookup0,
            lookup1,
        )
    }

    #[test]
    fn single_subst_substitutes_covered_glyphs() {
        let (gsub, lookup0, _) = chained_gsub();
        let out = gsub.apply(lookup0, &[span(10), span(12)]);
        assert_eq!(out[0].gid, 11);
        assert_eq!(out[1].gid, 12);
    }

    #[test]
    fn chain_context_applies_records_after_their_context() {
        let (gsub, _, chain) = chained_gsub();
        // Preceded by 7: second subtable matches and substitutes.
        let out = gsub.apply(chain, &[span(7), span(10)]);
        assert_eq!(out[0].gid, 7);
        assert_eq!(out[1].gid, 11);
        // Preceded by 9: first two subtables fail, the unconditional one runs.
        let out = gsub.apply(chain, &[span(9), span(10)]);
        assert_eq!(out[1].gid, 11);
        // Glyph on its own: only the unconditional rule matches.
        let out = gsub.apply(chain, &[span(10)]);
        assert_eq!(out[0].gid, 11);
    }

    #[test]
    fn chain_rule_without_records_blocks_later_subtables() {
        let (gsub, _, chain) = chained_gsub();
        // Preceded by 5 the first subtable matches, records nothing, and stops
        // the search: the unconditional rule must not run.
        let out = gsub.apply(chain, &[span(5), span(10)]);
        assert_eq!(out[1].gid, 10);
    }
}
