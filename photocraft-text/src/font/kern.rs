//! `kern` table: the Microsoft/Windows (version 0) and Apple (version 1) layouts, subtables of
//! format 0 (sorted glyph pairs) and format 2 (left/right glyph classes and a value matrix).
//!
//! Photoshop applies this legacy table for the fonts that ship one — Times New Roman, Arial,
//! Trebuchet MS and Verdana among the faces the sync matrix uses — with values in font units
//! (`-264` for `AV` in Times = `-3.87 px` at 30 pt). Only horizontal, non-minimum, non-cross-stream
//! subtables affect normal text layout; vertical and cross-stream kerning belong to the vertical
//! composer and are skipped here.
//!
//! A face's `kern` table wins over its GPOS `kern` feature when it has a usable subtable
//! ([`Kern::has_subtables`]); [`crate::font::Face::kern_pair`] implements that precedence.

use super::reader::Reader;
use crate::error::Result;

/// The header layout a table uses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    /// Microsoft/Windows (`version 0`): subtables of `version, length: u16, coverage`.
    Windows,
    /// Apple (`0x00010000`): subtables of `length: u32, coverage, tupleIndex`.
    Apple,
}

impl Variant {
    /// Bytes of a subtable header before its format-specific fields.
    fn header_len(self) -> usize {
        match self {
            Self::Windows => 6,
            Self::Apple => 8,
        }
    }

    /// The subtable's format number.
    fn format(self, coverage: u16) -> u16 {
        match self {
            Self::Windows => coverage >> 8,
            Self::Apple => coverage & 0x00FF,
        }
    }

    /// Whether the subtable kerns horizontal text (`kern` coverage: Windows bit 0, Apple bit 15
    /// being *vertical*).
    fn horizontal(self, coverage: u16) -> bool {
        match self {
            Self::Windows => coverage & 0x0001 != 0,
            Self::Apple => coverage & 0x8000 == 0,
        }
    }

    /// Minimum-kerning subtables ("don't add values that increase the total") are not applied.
    fn minimum(self, coverage: u16) -> bool {
        matches!(self, Self::Windows) && coverage & 0x0002 != 0
    }

    /// Cross-stream kerning (vertical text positioned horizontally) is not applied.
    fn cross_stream(self, coverage: u16) -> bool {
        match self {
            Self::Windows => coverage & 0x0004 != 0,
            Self::Apple => coverage & 0x4000 != 0,
        }
    }

    /// Windows coverage bit 3: this subtable's value replaces the accumulated one.
    fn overrides(self, coverage: u16) -> bool {
        matches!(self, Self::Windows) && coverage & 0x0008 != 0
    }
}

/// One kerning subtable, kept as slices of the font data (nothing is copied).
enum Subtable<'a> {
    /// Format 0: sorted `(left, right, value)` records, six bytes each.
    Pairs { pairs: &'a [u8], count: usize, sorted: bool, override_all: bool },
    /// Format 2: glyph class tables and a row-major `left class × right class` value matrix.
    Classes { left: Classes<'a>, right: Classes<'a>, values: &'a [u8], columns: usize, override_all: bool },
}

/// A format 2 glyph class table: the first glyph it covers and one `u16` class per glyph.
struct Classes<'a> {
    first: u16,
    records: &'a [u8],
}

impl Classes<'_> {
    /// The glyph's class; glyphs outside the table are class 0 (the spec's "not kerned" class).
    fn get(&self, gid: u16) -> usize {
        gid.checked_sub(self.first)
            .map(usize::from)
            .and_then(|index| index.checked_mul(2))
            .and_then(|at| self.records.get(at..at.checked_add(2)?))
            .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
            .unwrap_or(0)
    }

    /// The largest class this table assigns (class 0 is the fallback, so at least 0).
    fn max_class(&self) -> usize {
        self.records.as_chunks::<2>().0.iter().map(|b| usize::from(u16::from_be_bytes(*b))).max().unwrap_or(0)
    }
}

impl<'a> Subtable<'a> {
    /// The subtable's kerning value for `(left, right)` in font units, `None` when the pair has no
    /// record (format 0) or the matrix cell is out of bounds.
    fn value(&self, left: u16, right: u16) -> Option<i32> {
        match self {
            Self::Pairs { pairs, count, sorted, .. } => {
                if *sorted {
                    pairs_binary(pairs, *count, left, right)
                } else {
                    pairs_linear(pairs, *count, left, right)
                }
                .map(i32::from)
            }
            Self::Classes { left: left_classes, right: right_classes, values, columns, .. } => {
                let row = left_classes.get(left);
                let column = right_classes.get(right);
                let index = row.checked_mul(*columns)?.checked_add(column)?;
                let at = index.checked_mul(2)?;
                let bytes = values.get(at..at.checked_add(2)?)?;
                Some(i32::from(i16::from_be_bytes([bytes[0], bytes[1]])))
            }
        }
    }

    fn overrides(&self) -> bool {
        match self {
            Self::Pairs { override_all, .. } | Self::Classes { override_all, .. } => *override_all,
        }
    }
}

/// The `kern` table of one face.
#[derive(Default)]
pub struct Kern<'a> {
    subtables: Vec<Subtable<'a>>,
}

impl<'a> Kern<'a> {
    /// Parses the table. Unsupported subtables (vertical, minimum, cross-stream, formats other
    /// than 0 and 2) are skipped, and a truncated table yields the subtables parsed so far: a font
    /// with a broken legacy table still lays out, with GPOS or without kerning.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let version = r.u16()?;
        let mut subtables = Vec::new();
        if version == 0 {
            let count = r.u16()?;
            for _ in 0..count {
                match parse_subtable(data, &mut r, Variant::Windows) {
                    Some(sub) => subtables.extend(sub),
                    None => break,
                }
            }
        } else if version == 1 && r.u16().unwrap_or(1) == 0 {
            // `0x00010000`: Apple's version 1.0 layout.
            let count = r.u32()?;
            for _ in 0..count {
                match parse_subtable(data, &mut r, Variant::Apple) {
                    Some(sub) => subtables.extend(sub),
                    None => break,
                }
            }
        }
        Ok(Self { subtables })
    }

    /// True when the font has at least one subtable that applies to horizontal text: the signal to
    /// use this table instead of GPOS.
    pub fn has_subtables(&self) -> bool {
        !self.subtables.is_empty()
    }

    /// The accumulated kerning for a pair in font units, `None` when no subtable records it.
    /// Subtables that override replace the accumulated value (Windows coverage bit 3).
    pub fn pair(&self, left: u16, right: u16) -> Option<i32> {
        let mut total = 0i32;
        let mut any = false;
        for subtable in &self.subtables {
            if let Some(value) = subtable.value(left, right) {
                if subtable.overrides() {
                    total = value;
                } else {
                    total = total.saturating_add(value);
                }
                any = true;
            }
        }
        any.then_some(total)
    }
}

/// Parses one subtable at the reader's position and leaves the reader after it. `Some(None)` when
/// the subtable was skipped (unsupported or truncated), `None` when the table itself is unusable.
fn parse_subtable<'a>(data: &'a [u8], r: &mut Reader<'a>, variant: Variant) -> Option<Option<Subtable<'a>>> {
    let start = r.pos();
    let length = match variant {
        Variant::Windows => {
            r.u16().ok()?; // subtable version (0)
            usize::from(r.u16().ok()?)
        }
        Variant::Apple => r.u32().ok()? as usize,
    };
    let coverage = r.u16().ok()?;
    if variant == Variant::Apple {
        r.u16().ok()?; // tupleIndex (variation index); no variable kern table is used here
    }
    let end = start.saturating_add(length.max(variant.header_len())).min(data.len());
    // Advance past the subtable even when it is skipped, so one broken record cannot hide the rest.
    let next = end.max(r.pos());
    r.seek(end).ok()?;
    let usable = variant.horizontal(coverage) && !variant.minimum(coverage) && !variant.cross_stream(coverage);
    if !usable {
        return Some(None);
    }
    let format = variant.format(coverage);
    let override_all = variant.overrides(coverage);
    let body = start.checked_add(variant.header_len())?;
    let end = next;
    match format {
        0 => {
            let counts = data.get(body..body.checked_add(4)?)?;
            let n_pairs = usize::from(u16::from_be_bytes([counts[0], counts[1]]));
            let pairs_start = body.checked_add(8)?;
            let available = end.saturating_sub(pairs_start) / 6;
            let count = n_pairs.min(available);
            let pairs = data.get(pairs_start..pairs_start.checked_add(count.checked_mul(6)?)?)?;
            Some(Some(Subtable::Pairs { pairs, count, sorted: pairs_sorted(pairs, count), override_all }))
        }
        2 => {
            let offsets = data.get(body..body.checked_add(6)?)?;
            let left_at = start.checked_add(usize::from(u16::from_be_bytes([offsets[0], offsets[1]])))?;
            let right_at = start.checked_add(usize::from(u16::from_be_bytes([offsets[2], offsets[3]])))?;
            let values_at = start.checked_add(usize::from(u16::from_be_bytes([offsets[4], offsets[5]])))?;
            let left = classes_at(data, left_at, end)?;
            let right = classes_at(data, right_at, end)?;
            let columns = right.max_class().checked_add(1)?;
            let values = data.get(values_at..end)?;
            Some(Some(Subtable::Classes { left, right, values, columns, override_all }))
        }
        _ => Some(None),
    }
}

/// A format 2 class table at `at`, bounded by the subtable's end.
fn classes_at(data: &[u8], at: usize, end: usize) -> Option<Classes<'_>> {
    let header = data.get(at..at.checked_add(4)?)?;
    let first = u16::from_be_bytes([header[0], header[1]]);
    let count = usize::from(u16::from_be_bytes([header[2], header[3]]));
    let records = data.get(at.checked_add(4)?..at.checked_add(4)?.checked_add(count.checked_mul(2)?)?)?;
    (at.checked_add(4)?.checked_add(count.checked_mul(2)?)? <= end).then_some(Classes { first, records })
}

/// Format 0 records are sorted by `(left << 16) | right` (the spec requires it); checking lets a
/// non-conforming font fall back to a linear scan instead of losing its pairs.
fn pairs_sorted(pairs: &[u8], count: usize) -> bool {
    let mut previous: Option<u32> = None;
    for i in 0..count {
        let Some(at) = i.checked_mul(6) else { return false };
        let Some(record) = pairs.get(at..at + 6) else { return false };
        let key = (u32::from(u16::from_be_bytes([record[0], record[1]])) << 16) | u32::from(u16::from_be_bytes([record[2], record[3]]));
        if previous.is_some_and(|p| key < p) {
            return false;
        }
        previous = Some(key);
    }
    true
}

/// Format 0 binary search: the records are sorted by `(left << 16) | right`.
fn pairs_binary(pairs: &[u8], count: usize, left: u16, right: u16) -> Option<i16> {
    let mut lo = 0usize;
    let mut hi = count;
    let wanted = (u32::from(left) << 16) | u32::from(right);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let record = pairs.get(mid.checked_mul(6)?..mid.checked_mul(6)?.checked_add(6)?)?;
        let key = (u32::from(u16::from_be_bytes([record[0], record[1]])) << 16) | u32::from(u16::from_be_bytes([record[2], record[3]]));
        match key.cmp(&wanted) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => return Some(i16::from_be_bytes([record[4], record[5]])),
        }
    }
    None
}

/// Linear search for a font whose format 0 records are not sorted.
fn pairs_linear(pairs: &[u8], count: usize, left: u16, right: u16) -> Option<i16> {
    for i in 0..count {
        let at = i.checked_mul(6)?;
        let record = pairs.get(at..at.checked_add(6)?)?;
        if u16::from_be_bytes([record[0], record[1]]) == left && u16::from_be_bytes([record[2], record[3]]) == right {
            return Some(i16::from_be_bytes([record[4], record[5]]));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Microsoft version 0 table with one format 0 subtable.
    fn windows_format0(pairs: &[(u16, u16, i16)], coverage: u16) -> Vec<u8> {
        let length = 14 + pairs.len() * 6;
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_be_bytes()); // version
        data.extend_from_slice(&1u16.to_be_bytes()); // nTables
        data.extend_from_slice(&0u16.to_be_bytes()); // subtable version
        data.extend_from_slice(&u16::try_from(length).unwrap().to_be_bytes());
        data.extend_from_slice(&coverage.to_be_bytes());
        data.extend_from_slice(&u16::try_from(pairs.len()).unwrap().to_be_bytes()); // nPairs
        data.extend_from_slice(&[0; 6]); // searchRange, entrySelector, rangeShift
        for (left, right, value) in pairs {
            data.extend_from_slice(&left.to_be_bytes());
            data.extend_from_slice(&right.to_be_bytes());
            data.extend_from_slice(&value.to_be_bytes());
        }
        data
    }

    /// A Microsoft version 0 table with one format 2 subtable: `left`/`right` map glyphs to
    /// classes, `values` is the row-major class matrix.
    fn windows_format2(left: &(u16, &[(u16, u16)]), right: &(u16, &[(u16, u16)]), values: &[i16]) -> Vec<u8> {
        let left_at = 12usize;
        let left_len = 4 + left.1.len() * 2;
        let right_at = left_at + left_len;
        let right_len = 4 + right.1.len() * 2;
        let values_at = right_at + right_len;
        let length = values_at + values.len() * 2;
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes()); // subtable version
        data.extend_from_slice(&u16::try_from(length).unwrap().to_be_bytes());
        data.extend_from_slice(&0x0201u16.to_be_bytes()); // format 2, horizontal
        data.extend_from_slice(&u16::try_from(left_at).unwrap().to_be_bytes());
        data.extend_from_slice(&u16::try_from(right_at).unwrap().to_be_bytes());
        data.extend_from_slice(&u16::try_from(values_at).unwrap().to_be_bytes());
        for (first, classes) in [left, right] {
            data.extend_from_slice(&first.to_be_bytes());
            data.extend_from_slice(&u16::try_from(classes.len()).unwrap().to_be_bytes());
            for (_, class) in *classes {
                data.extend_from_slice(&class.to_be_bytes());
            }
        }
        for value in values {
            data.extend_from_slice(&value.to_be_bytes());
        }
        data
    }

    /// An Apple version 1.0 table with one format 0 subtable.
    fn apple_format0(pairs: &[(u16, u16, i16)]) -> Vec<u8> {
        let length = 16 + pairs.len() * 6;
        let mut data = Vec::new();
        data.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&u32::try_from(length).unwrap().to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes()); // coverage: format 0, horizontal (bit 15 clear)
        data.extend_from_slice(&0u16.to_be_bytes()); // tupleIndex
        data.extend_from_slice(&u16::try_from(pairs.len()).unwrap().to_be_bytes()); // nPairs
        data.extend_from_slice(&[0; 6]);
        for (left, right, value) in pairs {
            data.extend_from_slice(&left.to_be_bytes());
            data.extend_from_slice(&right.to_be_bytes());
            data.extend_from_slice(&value.to_be_bytes());
        }
        data
    }

    #[test]
    fn reads_windows_format0_pairs_and_ignores_missing_ones() {
        let table = windows_format0(&[(36, 55, -227), (36, 57, -264), (55, 79, -71)], 0x0001);
        let kern = Kern::parse(&table).unwrap();
        assert!(kern.has_subtables());
        assert_eq!(kern.pair(36, 57), Some(-264));
        assert_eq!(kern.pair(36, 55), Some(-227));
        assert_eq!(kern.pair(55, 79), Some(-71));
        // A pair without a record: Photoshop applies no adjustment.
        assert_eq!(kern.pair(36, 36), None);
        assert_eq!(kern.pair(57, 36), None);
    }

    #[test]
    fn reads_windows_format2_classes() {
        // Class tables cover a glyph range: glyph 10 is class 1 and glyph 11 class 2 on the left,
        // glyph 30 is class 1 on the right. The matrix has three rows (0, 1, 2) and two columns.
        let table = windows_format2(&(10, &[(10, 1), (11, 2)]), &(30, &[(30, 1)]), &[0, -100, 0, -264, 0, 0]);
        let kern = Kern::parse(&table).unwrap();
        assert!(kern.has_subtables());
        assert_eq!(kern.pair(10, 30), Some(-264), "class (1,1)");
        assert_eq!(kern.pair(11, 30), Some(0), "class (2,1) is an explicit zero");
        assert_eq!(kern.pair(12, 30), Some(-100), "a glyph outside the class tables is class 0");
        assert_eq!(kern.pair(10, 31), Some(0), "class (1,0) is zero");
    }

    #[test]
    fn reads_the_apple_version1_header() {
        let table = apple_format0(&[(36, 57, -264)]);
        let kern = Kern::parse(&table).unwrap();
        assert_eq!(kern.pair(36, 57), Some(-264));
    }

    #[test]
    fn vertical_minimum_cross_stream_and_unknown_formats_are_skipped() {
        // Windows coverage: bit 0 horizontal, bit 1 minimum, bit 2 cross-stream, format in the high byte.
        for coverage in [0x0000u16, 0x0003, 0x0005, 0x0101] {
            let table = windows_format0(&[(36, 57, -264)], coverage);
            let kern = Kern::parse(&table).unwrap();
            assert!(!kern.has_subtables(), "coverage 0x{coverage:04x} must not apply to horizontal text");
            assert_eq!(kern.pair(36, 57), None);
        }
    }

    #[test]
    fn a_truncated_table_never_panics() {
        let table = windows_format0(&[(36, 55, -227), (36, 57, -264)], 0x0001);
        for cut in 0..table.len() {
            let kern = Kern::parse(&table[..cut]).unwrap_or_default();
            let _ = kern.has_subtables();
            let _ = kern.pair(36, 57);
        }
        // A record count larger than the subtable's length is clamped, not read out of bounds.
        let mut lying = windows_format0(&[(36, 57, -264)], 0x0001);
        lying[6..8].copy_from_slice(&500u16.to_be_bytes());
        let kern = Kern::parse(&lying).unwrap();
        assert_eq!(kern.pair(36, 57), Some(-264));
        assert_eq!(kern.pair(1, 2), None);
    }

    #[test]
    fn unsorted_pairs_are_still_found() {
        let table = windows_format0(&[(36, 57, -264), (36, 55, -227)], 0x0001);
        let kern = Kern::parse(&table).unwrap();
        assert_eq!(kern.pair(36, 55), Some(-227));
        assert_eq!(kern.pair(36, 57), Some(-264));
    }

    #[test]
    fn override_subtables_replace_the_accumulated_value() {
        // Two horizontal format 0 subtables: -100 then an override of -264.
        let first = windows_format0(&[(36, 57, -100)], 0x0001);
        let second = windows_format0(&[(36, 57, -264)], 0x0009); // bit 3 override
        // Rebuild one table with two subtables: header + both subtables.
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&2u16.to_be_bytes());
        data.extend_from_slice(&first[4..]);
        data.extend_from_slice(&second[4..]);
        let kern = Kern::parse(&data).unwrap();
        assert_eq!(kern.pair(36, 57), Some(-264), "the override replaces, it does not add");
        let mut accumulating = Vec::new();
        accumulating.extend_from_slice(&0u16.to_be_bytes());
        accumulating.extend_from_slice(&2u16.to_be_bytes());
        accumulating.extend_from_slice(&first[4..]);
        accumulating.extend_from_slice(&windows_format0(&[(36, 57, -264)], 0x0001)[4..]);
        let kern = Kern::parse(&accumulating).unwrap();
        assert_eq!(kern.pair(36, 57), Some(-100 - 264), "plain subtables accumulate");
    }
}
