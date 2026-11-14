//! Corpus scan: Type 2 hint-operator / `seac` statistics for CFF faces and
//! glyph-instruction statistics for `glyf` faces.
//!
//! `cargo run --example outline_scan -- <font>...`

use photocraft_text::font::Face;
use photocraft_text::outline::{glyph_path, path_bbox};

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn bei16(d: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

/// Walks a composite glyph's components to the `WE_HAVE_INSTRUCTIONS` bytes.
fn instructions(face: &Face<'_>, gid: u16) -> Option<usize> {
    let loca = face.table(*b"loca")?;
    let glyf = face.table(*b"glyf")?;
    let head = face.table(*b"head")?;
    let long = bei16(head, 50)? != 0;
    let g = gid as usize;
    let (start, end) = if long {
        (be32(loca, g * 4)? as usize, be32(loca, g * 4 + 4)? as usize)
    } else {
        (
            be16(loca, g * 2)? as usize * 2,
            be16(loca, g * 2 + 2)? as usize * 2,
        )
    };
    if end <= start || end > glyf.len() {
        return None;
    }
    let data = &glyf[start..end];
    let n = bei16(data, 0)?;
    if n >= 0 {
        let at = 10 + 2 * n as usize;
        return Some(be16(data, at)? as usize);
    }
    let mut at = 10usize;
    loop {
        let flags = be16(data, at)?;
        at += 4;
        at += if flags & 0x0001 != 0 { 4 } else { 2 };
        if flags & 0x0008 != 0 {
            at += 2;
        } else if flags & 0x0040 != 0 {
            at += 4;
        } else if flags & 0x0080 != 0 {
            at += 8;
        }
        if flags & 0x0020 == 0 {
            break;
        }
    }
    Some(be16(data, at)? as usize)
}

fn main() {
    for path in std::env::args().skip(1) {
        let Ok(bytes) = std::fs::read(&path) else {
            println!("{path}: missing");
            continue;
        };
        let Ok(face) = Face::parse(&bytes) else {
            println!("{path}: parse error");
            continue;
        };
        let n = face.num_glyphs();
        if let Some(cff) = face.cff() {
            let (mut hstem, mut vstem, mut mask, mut flex, mut seac) = (0, 0, 0, 0, 0);
            let mut seac_samples = Vec::new();
            for gid in 0..n {
                let Ok((_, hints)) = cff.outline_with_hints(gid) else {
                    continue;
                };
                if hints.hstem > 0 {
                    hstem += 1;
                }
                if hints.vstem > 0 {
                    vstem += 1;
                }
                if hints.hintmask || hints.cntrmask {
                    mask += 1;
                }
                if hints.flex {
                    flex += 1;
                }
                if let Some(s) = hints.seac {
                    seac += 1;
                    if seac_samples.len() < 4 {
                        let bbox = glyph_path(&face, gid)
                            .ok()
                            .and_then(|p| path_bbox(&p))
                            .map(|(a, b, c, d)| [a, b, c, d]);
                        seac_samples.push(format!(
                            "gid {gid} b{:02x}a{:02x} adx{:.1} ady{:.1} bbox {bbox:?}",
                            s.bchar, s.achar, s.adx, s.ady
                        ));
                    }
                }
            }
            println!(
                "{path}\n  CFF upem {} glyphs {n}: hstem {hstem} vstem {vstem} hintmask {mask} flex {flex} seac {seac}",
                face.units_per_em()
            );
            for s in seac_samples {
                println!("  seac {s}");
            }
        }
        if face.has_glyf() {
            let mut with = 0usize;
            let mut bytes_total = 0usize;
            let mut max = 0usize;
            for gid in 0..n {
                if let Some(len) = instructions(&face, gid)
                    && len > 0
                {
                    with += 1;
                    bytes_total += len;
                    max = max.max(len);
                }
            }
            println!(
                "{path}\n  glyf upem {} glyphs {n}: instructed {with} ({:.1}%), instruction bytes {bytes_total}, max {max}",
                face.units_per_em(),
                100.0 * with as f32 / n as f32
            );
        }
    }
}
