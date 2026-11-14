//! sfnt (TrueType/OpenType) font access: metrics, cmap, kerning.
//!
//! Parsing is done by hand so the behaviour is fully controlled: every value the
//! layout engine uses comes from an explicit table read here.

pub mod cff;
pub mod cmap;
pub mod gpos;
pub mod gsub;
pub mod kern;
pub mod reader;

use crate::error::{Error, Result};
use reader::Reader;

pub const TAG_HEAD: [u8; 4] = *b"head";
pub const TAG_HHEA: [u8; 4] = *b"hhea";
pub const TAG_MAXP: [u8; 4] = *b"maxp";
pub const TAG_HMTX: [u8; 4] = *b"hmtx";
pub const TAG_CMAP: [u8; 4] = *b"cmap";
pub const TAG_KERN: [u8; 4] = *b"kern";
pub const TAG_GPOS: [u8; 4] = *b"GPOS";
pub const TAG_OS2: [u8; 4] = *b"OS/2";
pub const TAG_POST: [u8; 4] = *b"post";

/// Horizontal metrics for one glyph, in font units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphMetrics {
    pub advance: u16,
    pub lsb: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    pub units_per_em: u16,
    pub mac_style: u16,
    pub index_to_loc_format: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hhea {
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub num_h_metrics: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Os2 {
    pub version: u16,
    pub weight_class: u16,
    pub width_class: u16,
    pub fs_selection: u16,
    pub s_typo_ascender: i16,
    pub s_typo_descender: i16,
    pub s_typo_line_gap: i16,
    pub s_cap_height: Option<i16>,
    pub s_x_height: Option<i16>,
}

pub struct Face<'a> {
    data: &'a [u8],
    tables: Vec<([u8; 4], usize, usize)>,
    pub head: Head,
    pub hhea: Hhea,
    pub os2: Option<Os2>,
    advances: Vec<u16>,
    lsbs: Vec<i16>,
    cmap_sub: (usize, u16),
    num_glyphs: u16,
    pub kern: kern::Kern<'a>,
    pub gpos: Option<gpos::Gpos<'a>>,
    cff: Option<cff::Cff<'a>>,
}

impl<'a> Face<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let version = r.u32()?;
        let base = if version == 0x7474_6366 {
            // 'ttcf': first face in the collection
            let _ver = r.u32()?;
            let num_fonts = r.u32()?;
            if num_fonts == 0 {
                return Err(Error::BadFont("empty font collection"));
            }
            r.u32()? as usize
        } else {
            0
        };
        let mut r = Reader::at(data, base)?;
        let sfnt_version = r.u32()?;
        if sfnt_version != 0x0001_0000 && sfnt_version != 0x4F54_544F && sfnt_version != 0x7472_7565
        {
            return Err(Error::BadFont("unknown sfnt version"));
        }
        let num_tables = r.u16()?;
        r.skip(6)?;
        let mut tables = Vec::with_capacity(num_tables as usize);
        for _ in 0..num_tables {
            let tag = r.tag()?;
            let _checksum = r.u32()?;
            let offset = r.u32()? as usize;
            let length = r.u32()? as usize;
            tables.push((tag, offset, length));
        }

        let table = |tag: [u8; 4]| -> Option<&'a [u8]> {
            tables
                .iter()
                .find(|(t, _, _)| *t == tag)
                .and_then(|(_, off, len)| {
                    let end = off.checked_add(*len)?;
                    data.get(*off..end)
                })
        };

        let head_data = table(TAG_HEAD).ok_or(Error::MissingTable("head"))?;
        let mut hr = Reader::new(head_data);
        hr.skip(18)?;
        let units_per_em = hr.u16()?;
        hr.skip(24)?; // created, modified, bbox
        let mac_style = hr.u16()?;
        hr.skip(4)?; // lowestRecPPEM, fontDirectionHint
        let index_to_loc_format = hr.i16()?;
        let head = Head {
            units_per_em,
            mac_style,
            index_to_loc_format,
        };
        if units_per_em == 0 {
            return Err(Error::BadFont("unitsPerEm is zero"));
        }

        let hhea_data = table(TAG_HHEA).ok_or(Error::MissingTable("hhea"))?;
        let mut hr = Reader::new(hhea_data);
        hr.skip(4)?;
        let ascender = hr.i16()?;
        let descender = hr.i16()?;
        let line_gap = hr.i16()?;
        hr.skip(24)?;
        let num_h_metrics = hr.u16()?;
        let hhea = Hhea {
            ascender,
            descender,
            line_gap,
            num_h_metrics,
        };

        let maxp_data = table(TAG_MAXP).ok_or(Error::MissingTable("maxp"))?;
        let mut mr = Reader::new(maxp_data);
        mr.skip(4)?;
        let num_glyphs = mr.u16()?;

        let (advances, lsbs) = Self::read_hmtx(
            table(TAG_HMTX).ok_or(Error::MissingTable("hmtx"))?,
            num_h_metrics,
            num_glyphs,
        )?;

        let (cmap_start, cmap_len) = tables
            .iter()
            .find(|(t, _, _)| *t == TAG_CMAP)
            .map(|(_, o, l)| (*o, *l))
            .ok_or(Error::MissingTable("cmap"))?;
        let cmap_data = data
            .get(
                cmap_start
                    ..cmap_start
                        .checked_add(cmap_len)
                        .ok_or(Error::BadFont("cmap length"))?,
            )
            .ok_or(Error::MissingTable("cmap"))?;
        let (sub_off, sub_fmt) = cmap::pick_subtable(cmap_data)?;
        let cmap_sub = (cmap_start + sub_off, sub_fmt);

        let os2 = table(TAG_OS2).map(parse_os2);
        let kern = table(TAG_KERN)
            .map(|d| kern::Kern::parse(d))
            .transpose()?
            .unwrap_or_default();
        let gpos = table(TAG_GPOS).map(|d| gpos::Gpos::parse(d)).transpose()?;
        // `OTTO` faces carry `CFF ` instead of `glyf`; CFF2 is not supported.
        let cff = table(*b"CFF ")
            .map(|d| cff::Cff::parse(d, units_per_em))
            .transpose()?;

        Ok(Self {
            data,
            tables,
            head,
            hhea,
            os2,
            advances,
            lsbs,
            cmap_sub,
            num_glyphs,
            kern,
            gpos,
            cff,
        })
    }

    /// True when the face has TrueType `glyf` outlines.
    pub fn has_glyf(&self) -> bool {
        self.tables.iter().any(|(tag, _, _)| *tag == *b"glyf")
    }

    /// True when the face has a parsed CFF table.
    pub fn has_cff(&self) -> bool {
        self.cff.is_some()
    }

    /// The parsed `CFF ` table, if the face has one.
    pub fn cff(&self) -> Option<&cff::Cff<'a>> {
        self.cff.as_ref()
    }

    /// Ink bounding box of `gid` in font units, y up, as
    /// `(x_min, y_min, x_max, y_max)`.
    ///
    /// Follows the drawn curve: composite `glyf` components are expanded and
    /// quadratic/cubic extrema are solved, so the result is the true outline
    /// bound rather than the `glyf` header or control-point hull.
    pub fn glyph_bbox(&self, gid: u16) -> Option<(f32, f32, f32, f32)> {
        crate::outline::glyph_bbox(self, gid).ok().flatten()
    }

    /// Photoshop's area-text first-baseline ratio (the distance from the box
    /// top to the first baseline, divided by the point size).
    ///
    /// Measured against Photoshop 2026 on synthetic single-glyph box layers
    /// (`H`, `d`, `h`, `l` at 12-400 pt, box top on an integer pixel, ink
    /// bounding boxes compared on the rasterized duplicate; see
    /// `out/verify/text-baseline-offset.md`): Photoshop's ratio is the
    /// **smallest** of the ink `yMax` of the `H` and `d` reference glyphs,
    /// per em:
    ///
    /// | font | `H` | `d` | Photoshop |
    /// |---|---|---|---|
    /// | Times New Roman (`glyf`) | 0.6621 | 0.6943 | 0.662 |
    /// | CCGrimNGritty (CFF) | 0.800 | 0.781 | 0.780-0.781 |
    /// | CCMeanwhile (CFF) | 0.8303 | 0.813 | 0.809-0.813 |
    /// | Vampire95 (`glyf`) | 0.605 | 0.466 | 0.465 |
    ///
    /// The old rule was `d`'s ink `yMax` alone, which is the *ink-top* distance
    /// for a line whose tallest glyph is `d` — that is what the PDF-based
    /// calibration actually measured, not the box-top distance. It is right for
    /// fonts where `d` is the shorter reference and wrong where it overshoots
    /// the cap (Times: `d` 1422 vs `H` 1356 units of 2048, i.e. the box layer
    /// drawn 1-2 px low at 30-60 pt — the user's "text renders 2 px lower than
    /// Photoshop").
    ///
    /// Falls back to the remaining ascender/cap glyphs (`E`, `I`, `T`, `b`,
    /// `l`) for faces without `H`/`d` (symbol fonts), and finally to
    /// `hhea.ascender / unitsPerEm`.
    pub fn ascent_ratio(&self) -> f32 {
        let upm = self.head.units_per_em as f32;
        // Only the two reference glyphs are used. The declared `OS/2.sCapHeight`
        // is deliberately not consulted: `v_CCWildButNotCrazy` declares 0.700
        // while Photoshop's overlay sweep matches `d`'s 0.772, so the declared
        // cap height is not what the engine uses.
        let mut best: Option<f32> = None;
        let mut reference = false;
        for ch in ['H', 'd', 'E', 'I', 'T', 'b', 'l'] {
            let primary = matches!(ch, 'H' | 'd');
            let Some(gid) = self.glyph_index(ch) else {
                continue;
            };
            let Some((_, _, _, y_max)) = self.glyph_bbox(gid) else {
                continue;
            };
            let ratio = y_max / upm;
            if ratio.is_finite() && ratio > 0.0 {
                if primary {
                    reference = true;
                    best = Some(best.map_or(ratio, |b: f32| b.min(ratio)));
                } else if !reference {
                    // Only faces without both reference glyphs fall back to a
                    // different ascender letter; a shorter `T`/`E` must not
                    // override the cap the reference glyphs give.
                    best = Some(best.map_or(ratio, |b: f32| b.min(ratio)));
                }
            }
        }
        best.unwrap_or(self.hhea.ascender as f32 / upm)
    }

    fn read_hmtx(data: &[u8], num_h_metrics: u16, num_glyphs: u16) -> Result<(Vec<u16>, Vec<i16>)> {
        let mut r = Reader::new(data);
        let n = num_h_metrics.max(1) as usize;
        let mut advances = Vec::with_capacity(num_glyphs as usize);
        let mut lsbs = Vec::with_capacity(num_glyphs as usize);
        let mut last_advance = 0u16;
        for _ in 0..n {
            last_advance = r.u16()?;
            let lsb = r.i16()?;
            advances.push(last_advance);
            lsbs.push(lsb);
        }
        let total = num_glyphs as usize;
        for _ in n..total {
            advances.push(last_advance);
            lsbs.push(r.i16().unwrap_or(0));
        }
        advances.truncate(total.max(n));
        lsbs.truncate(total.max(n));
        Ok((advances, lsbs))
    }

    pub fn cmap_subtable(&self) -> (usize, u16) {
        self.cmap_sub
    }

    pub fn units_per_em(&self) -> u16 {
        self.head.units_per_em
    }

    pub fn num_glyphs(&self) -> u16 {
        self.num_glyphs
    }

    pub fn table(&self, tag: [u8; 4]) -> Option<&'a [u8]> {
        self.tables
            .iter()
            .find(|(t, _, _)| *t == tag)
            .and_then(|(_, off, len)| self.data.get(*off..off.checked_add(*len)?))
    }

    /// Family name from the `name` table (Windows platform, else Macintosh).
    pub fn family_name(&self) -> Option<String> {
        self.name_record(1)
    }

    /// PostScript name from the `name` table (name ID 6), the name Photoshop's
    /// `Txt2` block and `EngineData` use for a run's font.
    pub fn postscript_name(&self) -> Option<String> {
        self.name_record(6)
    }

    /// Full name from the `name` table (name ID 4).
    pub fn full_name(&self) -> Option<String> {
        self.name_record(4)
    }

    /// One `name` table record (preferring Windows, else Macintosh).
    fn name_record(&self, wanted: u16) -> Option<String> {
        let data = self.table(*b"name")?;
        let count = u16::from_be_bytes([*data.get(2)?, *data.get(3)?]) as usize;
        let storage = u16::from_be_bytes([*data.get(4)?, *data.get(5)?]) as usize;
        let mut fallback = None;
        for i in 0..count {
            let at = 6 + i * 12;
            let platform = u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]);
            let name_id = u16::from_be_bytes([*data.get(at + 6)?, *data.get(at + 7)?]);
            let len = u16::from_be_bytes([*data.get(at + 8)?, *data.get(at + 9)?]) as usize;
            let off = u16::from_be_bytes([*data.get(at + 10)?, *data.get(at + 11)?]) as usize;
            if name_id != wanted {
                continue;
            }
            let start = storage.checked_add(off)?;
            let bytes = data.get(start..start.checked_add(len)?)?;
            let text = if platform == 3 {
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16(&units).ok()
            } else {
                Some(bytes.iter().map(|b| *b as char).collect())
            };
            match (platform, text) {
                (3, Some(t)) => return Some(t),
                (1, Some(t)) => fallback = Some(t),
                _ => {}
            }
        }
        fallback
    }

    pub fn glyph_index(&self, c: char) -> Option<u16> {
        cmap::lookup(self.data, self.cmap_sub, c as u32)
    }

    pub fn metrics(&self, gid: u16) -> GlyphMetrics {
        let i = gid as usize;
        GlyphMetrics {
            advance: self.advances.get(i).copied().unwrap_or(0),
            lsb: self.lsbs.get(i).copied().unwrap_or(0),
        }
    }

    pub fn advance(&self, gid: u16) -> u16 {
        self.metrics(gid).advance
    }

    /// Kerning adjustment in font units for a glyph pair.
    ///
    /// A face that ships a usable legacy `kern` subtable is kerned from it — that is the table
    /// Photoshop applies for Times New Roman, Arial, Trebuchet MS and Verdana, whose values have no
    /// GPOS equivalent at all. GPOS pair kerning is the fallback for faces without a `kern` table
    /// (Myriad Pro, Minion Pro, the Noto faces) and stays the only path for them.
    pub fn kern_pair(&self, left: u16, right: u16) -> i32 {
        if self.kern.has_subtables() {
            return self.kern.pair(left, right).unwrap_or(0);
        }
        if let Some(g) = &self.gpos {
            if let Some(v) = g.pair(left, right) {
                return v;
            }
        }
        0
    }
}

fn parse_os2(data: &[u8]) -> Os2 {
    let mut r = Reader::new(data);
    let mut os2 = Os2 {
        version: 0,
        weight_class: 400,
        width_class: 5,
        fs_selection: 0,
        s_typo_ascender: 0,
        s_typo_descender: 0,
        s_typo_line_gap: 0,
        s_cap_height: None,
        s_x_height: None,
    };
    let Ok(version) = r.u16() else { return os2 };
    os2.version = version;
    // The table's fixed offsets (OpenType `OS/2`, v0-v5):
    // 2 xAvgCharWidth, 4 usWeightClass, 6 usWidthClass, 8 fsType,
    // 62 fsSelection, 64 usFirstCharIndex, 66 usLastCharIndex,
    // 68 sTypoAscender, 70 sTypoDescender, 72 sTypoLineGap,
    // 74 usWinAscent, 76 usWinDescent, 78/82 ulCodePageRange1/2 (v1+),
    // 86 sxHeight, 88 sCapHeight (v2+).
    if r.seek(4).is_err() {
        return os2;
    }
    os2.weight_class = r.u16().unwrap_or(400);
    os2.width_class = r.u16().unwrap_or(5);
    if r.seek(62).is_err() {
        return os2;
    }
    os2.fs_selection = r.u16().unwrap_or(0);
    if r.seek(68).is_err() {
        return os2;
    }
    os2.s_typo_ascender = r.i16().unwrap_or(0);
    os2.s_typo_descender = r.i16().unwrap_or(0);
    os2.s_typo_line_gap = r.i16().unwrap_or(0);
    if version >= 2 {
        if r.seek(86).is_err() {
            return os2;
        }
        os2.s_x_height = r.i16().ok();
        os2.s_cap_height = r.i16().ok();
    }
    os2
}
