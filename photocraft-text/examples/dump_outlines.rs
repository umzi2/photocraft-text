//! Dumps a face's outlines/metrics as the integer-font-unit JSON the raster
//! (CoolType) side consumes (`out/verify/sync/outlines/<face_id>.json`).
//!
//! Usage:
//! ```text
//! dump_outlines <font> [--chars Hn] [--gids 36,78] [--index 0] [--out FILE]
//! ```
//!
//! Coordinates are integers in font units, y up. CFF contours carry cubic
//! segments after subr/bias expansion and the Type 2 hint operators; `glyf`
//! contours carry lines/quads as stored (`glyf_points` is the stored on/off
//! point list, `segments` materialises the implied midpoints). TT glyph
//! instruction bytes are reported so a grid-fitter can consume them.

use std::collections::HashMap;
use std::path::Path;

use photocraft_text::font::Face;
use photocraft_text::outline::{Seg, glyph_contours, glyph_path, path_bbox};

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn bei16(d: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn bei32(d: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// `loca`/`glyf` slice of one glyph plus the raw instruction bytes.
fn glyf_instructions(face: &Face<'_>, gid: u16) -> Option<Vec<u8>> {
    let loca = face.table(*b"loca")?;
    let glyf = face.table(*b"glyf")?;
    let head = face.table(*b"head")?;
    let long = bei16(head, 50)? != 0;
    let gid = gid as usize;
    let (start, end) = if long {
        (
            be32(loca, gid * 4)? as usize,
            be32(loca, gid * 4 + 4)? as usize,
        )
    } else {
        (
            be16(loca, gid * 2)? as usize * 2,
            be16(loca, gid * 2 + 2)? as usize * 2,
        )
    };
    if end <= start || end > glyf.len() {
        return None;
    }
    let data = &glyf[start..end];
    let n = bei16(data, 0)?;
    if n >= 0 {
        let at = 10 + 2 * n as usize;
        let len = be16(data, at)? as usize;
        Some(data.get(at + 2..at + 2 + len)?.to_vec())
    } else {
        // Composite: walk the components to the instruction length.
        let mut at = 10usize;
        loop {
            let flags = be16(data, at)?;
            at += 4; // flags + glyphIndex
            at += if flags & 0x0001 != 0 { 4 } else { 2 }; // ARG_1_AND_2_ARE_WORDS
            if flags & 0x0008 != 0 {
                at += 2; // WE_HAVE_A_SCALE
            } else if flags & 0x0040 != 0 {
                at += 4; // WE_HAVE_AN_X_AND_Y_SCALE
            } else if flags & 0x0080 != 0 {
                at += 8; // WE_HAVE_A_TWO_BY_TWO
            }
            if flags & 0x0020 == 0 {
                break; // MORE_COMPONENTS
            }
        }
        if be16(data, at).is_some() {
            let len = be16(data, at)? as usize;
            Some(data.get(at + 2..at + 2 + len)?.to_vec())
        } else {
            Some(Vec::new())
        }
    }
}

const MAC_GLYPHS: [&str; 258] = [
    ".notdef",
    ".null",
    "nonmarkingreturn",
    "space",
    "exclam",
    "quotedbl",
    "numbersign",
    "dollar",
    "percent",
    "ampersand",
    "quotesingle",
    "parenleft",
    "parenright",
    "asterisk",
    "plus",
    "comma",
    "hyphen",
    "period",
    "slash",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "colon",
    "semicolon",
    "less",
    "equal",
    "greater",
    "question",
    "at",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "bracketleft",
    "backslash",
    "bracketright",
    "asciicircum",
    "underscore",
    "grave",
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "braceleft",
    "bar",
    "braceright",
    "asciitilde",
    "Adieresis",
    "Aring",
    "Ccedilla",
    "Eacute",
    "Ntilde",
    "Odieresis",
    "Udieresis",
    "aacute",
    "agrave",
    "acircumflex",
    "adieresis",
    "atilde",
    "aring",
    "ccedilla",
    "eacute",
    "egrave",
    "ecircumflex",
    "edieresis",
    "iacute",
    "igrave",
    "icircumflex",
    "idieresis",
    "ntilde",
    "oacute",
    "ograve",
    "ocircumflex",
    "odieresis",
    "otilde",
    "uacute",
    "ugrave",
    "ucircumflex",
    "udieresis",
    "dagger",
    "degree",
    "cent",
    "sterling",
    "section",
    "bullet",
    "paragraph",
    "germandbls",
    "registered",
    "copyright",
    "trademark",
    "acute",
    "dieresis",
    "notequal",
    "AE",
    "Oslash",
    "infinity",
    "plusminus",
    "lessequal",
    "greaterequal",
    "yen",
    "mu",
    "partialdiff",
    "summation",
    "product",
    "pi",
    "integral",
    "ordfeminine",
    "ordmasculine",
    "Omega",
    "ae",
    "oslash",
    "questiondown",
    "exclamdown",
    "logicalnot",
    "radical",
    "florin",
    "approxequal",
    "Delta",
    "guillemotleft",
    "guillemotright",
    "ellipsis",
    "nonbreakingspace",
    "Agrave",
    "Atilde",
    "Otilde",
    "OE",
    "oe",
    "endash",
    "emdash",
    "quotedblleft",
    "quotedblright",
    "quoteleft",
    "quoteright",
    "divide",
    "lozenge",
    "ydieresis",
    "Ydieresis",
    "fraction",
    "currency",
    "guilsinglleft",
    "guilsinglright",
    "fi",
    "fl",
    "daggerdbl",
    "periodcentered",
    "quotesinglbase",
    "quotedblbase",
    "perthousand",
    "Acircumflex",
    "Ecircumflex",
    "Aacute",
    "Edieresis",
    "Egrave",
    "Iacute",
    "Icircumflex",
    "Idieresis",
    "Igrave",
    "Oacute",
    "Ocircumflex",
    "apple",
    "Ograve",
    "Uacute",
    "Ucircumflex",
    "Ugrave",
    "dotlessi",
    "circumflex",
    "tilde",
    "macron",
    "breve",
    "dotaccent",
    "ring",
    "cedilla",
    "hungarumlaut",
    "ogonek",
    "caron",
    "Lslash",
    "lslash",
    "Scaron",
    "scaron",
    "Zcaron",
    "zcaron",
    "brokenbar",
    "Eth",
    "eth",
    "Yacute",
    "yacute",
    "Thorn",
    "thorn",
    "minus",
    "multiply",
    "onesuperior",
    "twosuperior",
    "threesuperior",
    "onehalf",
    "onequarter",
    "threequarters",
    "franc",
    "Gbreve",
    "gbreve",
    "Idotaccent",
    "Scedilla",
    "scedilla",
    "Cacute",
    "cacute",
    "Ccaron",
    "ccaron",
    "dcroat",
];

/// Glyph names from a version 2 `post` table (first 258 are the Mac order).
fn post_names(face: &Face<'_>) -> HashMap<u16, String> {
    let mut out = HashMap::new();
    let Some(post) = face.table(*b"post") else {
        return out;
    };
    if be32(post, 0) != Some(0x0002_0000) {
        return out;
    }
    let Some(n) = be16(post, 32) else {
        return out;
    };
    let Some(index_len) = (n as usize).checked_mul(2) else {
        return out;
    };
    let mut names = Vec::new();
    let mut at = 34 + index_len;
    while let Some(len) = post.get(at) {
        let len = *len as usize;
        let Some(bytes) = post.get(at + 1..at + 1 + len) else {
            break;
        };
        names.push(String::from_utf8_lossy(bytes).into_owned());
        at += 1 + len;
    }
    let mut indices = Vec::new();
    for i in 0..n as usize {
        indices.push(be16(post, 34 + i * 2).unwrap_or(0));
    }
    for (gid, idx) in indices.into_iter().enumerate() {
        let name = if idx < 258 {
            MAC_GLYPHS.get(idx as usize).map(|s| (*s).to_string())
        } else {
            names.get(idx as usize - 258).cloned()
        };
        if let Some(name) = name {
            out.insert(gid as u16, name);
        }
    }
    out
}

fn post_metadata(face: &Face<'_>) -> serde_json::Value {
    let Some(post) = face.table(*b"post") else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "italic_angle": bei32(post, 4).map(|v| v as f64 / 65536.0),
        "underline_position": bei16(post, 8),
        "underline_thickness": bei16(post, 10),
    })
}

fn fvar_info(face: &Face<'_>) -> serde_json::Value {
    let Some(fvar) = face.table(*b"fvar") else {
        return serde_json::Value::Null;
    };
    let axes = bei16(fvar, 8).unwrap_or(0);
    let instances = bei16(fvar, 12).unwrap_or(0);
    serde_json::json!({"axes": axes, "named_instances": instances})
}

fn contour_json(path: &photocraft_text::outline::Path) -> serde_json::Value {
    let mut out = Vec::new();
    for contour in path {
        let mut segs = Vec::new();
        for seg in &contour.segs {
            segs.push(match *seg {
                Seg::Line { to } => {
                    serde_json::json!({"line": [to.x.round() as i64, to.y.round() as i64]})
                }
                Seg::Quad { ctrl, to } => serde_json::json!({"quad": [
                    [ctrl.x.round() as i64, ctrl.y.round() as i64],
                    [to.x.round() as i64, to.y.round() as i64]
                ]}),
                Seg::Cubic { c1, c2, to } => serde_json::json!({"cubic": [
                    [c1.x.round() as i64, c1.y.round() as i64],
                    [c2.x.round() as i64, c2.y.round() as i64],
                    [to.x.round() as i64, to.y.round() as i64]
                ]}),
            });
        }
        out.push(serde_json::json!({
            "closed": true,
            "start": [contour.start.x.round() as i64, contour.start.y.round() as i64],
            "segments": segs,
        }));
    }
    serde_json::Value::Array(out)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: dump_outlines <font> [--chars S] [--gids L] [--out F]");
    let mut chars = String::from("HnoA.liT");
    let mut gids: Option<Vec<u16>> = None;
    let mut out_path: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--chars" => chars = args.next().expect("--chars value"),
            "--gids" => {
                gids = Some(
                    args.next()
                        .expect("--gids value")
                        .split(',')
                        .filter_map(|s| s.trim().parse().ok())
                        .collect(),
                );
            }
            "--out" => out_path = args.next(),
            other => panic!("unknown argument {other}"),
        }
    }
    let bytes = std::fs::read(&path).expect("read font");
    let face = Face::parse(&bytes).expect("parse font");
    let ps_name = face.postscript_name().unwrap_or_else(|| {
        Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let is_ttc = bytes.get(0..4) == Some(b"ttcf");
    let face_id = if is_ttc {
        let stem = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{}-i0", slug(&stem))
    } else {
        slug(&ps_name)
    };
    let names = post_names(&face);

    // head bbox
    let head_raw = face.table(*b"head");
    let head_bbox = head_raw.and_then(|h| {
        Some([
            bei16(h, 36)? as i64,
            bei16(h, 38)? as i64,
            bei16(h, 40)? as i64,
            bei16(h, 42)? as i64,
        ])
    });
    let maxp = face.table(*b"maxp");
    let num_glyphs = maxp.and_then(|m| be16(m, 4)).unwrap_or(0);

    // hmtx: advances and lsbs
    let hhea_raw = face.table(*b"hhea");
    let nhm = face.hhea.num_h_metrics as usize;
    let hmtx = face.table(*b"hmtx");
    let last_adv = if nhm > 0 && hmtx.is_some() {
        be16(hmtx.unwrap(), (nhm - 1) * 4).unwrap_or(0)
    } else {
        0
    };
    let metric = |gid: u16| -> (i64, i64) {
        let g = gid as usize;
        let Some(h) = hmtx else {
            return (0, 0);
        };
        if g < nhm {
            (
                be16(h, g * 4).unwrap_or(0) as i64,
                bei16(h, g * 4 + 2).unwrap_or(0) as i64,
            )
        } else {
            let lsb_at = nhm * 4 + (g - nhm) * 2;
            (last_adv as i64, bei16(h, lsb_at).unwrap_or(0) as i64)
        }
    };

    let os2 = face.os2.as_ref().map(|o| {
        serde_json::json!({
            "version": o.version,
            "typo_ascender": o.s_typo_ascender,
            "typo_descender": o.s_typo_descender,
            "typo_line_gap": o.s_typo_line_gap,
            "win_ascent": face.table(*b"OS/2").and_then(|t| be16(t, 74)),
            "win_descent": face.table(*b"OS/2").and_then(|t| be16(t, 76)),
            "s_cap_height": o.s_cap_height,
            "s_x_height": o.s_x_height,
        })
    });

    let cff_json = face.cff().map(|cff| {
        serde_json::json!({
            "font_matrix": cff.font_matrix(),
            "bbox": cff.font_bbox(),
        })
    });
    let tt_json = serde_json::json!({
        "has_prep": face.table(*b"prep").is_some(),
        "has_fpgm": face.table(*b"fpgm").is_some(),
        "has_cvt": face.table(*b"cvt ").is_some(),
        "fpgm_bytes": face.table(*b"fpgm").map(|t| t.len()).unwrap_or(0),
        "maxp_max_stack": maxp.and_then(|m| be16(m, 26)),
        "maxp_max_size_of_instructions": maxp.and_then(|m| be16(m, 30)),
    });

    let gids: Vec<u16> =
        gids.unwrap_or_else(|| chars.chars().filter_map(|c| face.glyph_index(c)).collect());
    let mut glyphs = Vec::new();
    for gid in gids {
        let (advance, lsb) = metric(gid);
        let path = glyph_path(&face, gid).expect("outline");
        let bbox = path_bbox(&path).map(|(x0, y0, x1, y1)| {
            [
                x0.round() as i64,
                y0.round() as i64,
                x1.round() as i64,
                y1.round() as i64,
            ]
        });
        let points = if face.has_glyf() {
            glyph_contours(&face, gid).ok().map(|contours| {
                contours
                    .iter()
                    .map(|c| {
                        c.iter()
                            .map(|p| {
                                serde_json::json!([p.x.round() as i64, p.y.round() as i64, p.on])
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
        } else {
            None
        };
        let code = chars.chars().find(|&c| face.glyph_index(c) == Some(gid));
        let cff_hints = face.cff().and_then(|cff| {
            cff.outline_with_hints(gid).ok().map(|(_, hints)| {
                serde_json::json!({
                    "hstem": hints.hstem,
                    "vstem": hints.vstem,
                    "hintmask": hints.hintmask,
                    "cntrmask": hints.cntrmask,
                    "flex": hints.flex,
                    "seac": hints.seac.map(|s| serde_json::json!({
                        "bchar": s.bchar,
                        "achar": s.achar,
                        "adx": s.adx.round() as i64,
                        "ady": s.ady.round() as i64,
                    })),
                })
            })
        });
        let instructions = if face.has_glyf() {
            glyf_instructions(&face, gid)
                .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>())
        } else {
            None
        };
        glyphs.push(serde_json::json!({
            "gid": gid,
            "name": names.get(&gid),
            "code": code.map(|c| c as u32),
            "advance": advance,
            "lsb": lsb,
            "bbox": bbox,
            "contours": contour_json(&path),
            "glyf_points": points,
            "cff_hints": cff_hints,
            "tt_instructions_hex": instructions,
        }));
    }

    let doc = serde_json::json!({
        "face": {
            "path": std::fs::canonicalize(&path).map(|p| p.to_string_lossy().into_owned()).unwrap_or(path),
            "index": 0,
            "face_id": face_id,
            "family": face.family_name(),
            "style": ps_name.rsplit('-').next(),
            "outline_kind": if face.has_cff() { "cff" } else { "truetype" },
            "outline_engine": if face.has_cff() { "raw-cff-charstring" } else { "raw-glyf" },
            "upem": face.units_per_em(),
            "head_bbox": head_bbox,
            "hhea": {
                "ascender": face.hhea.ascender,
                "descender": face.hhea.descender,
                "line_gap": face.hhea.line_gap,
            },
            "os2": os2,
            "post": post_metadata(&face),
            "cff": cff_json,
            "tt": tt_json,
            "fvar": fvar_info(&face),
            "num_glyphs": num_glyphs,
        },
        "glyphs": glyphs,
    });
    let text = serde_json::to_string_pretty(&doc).expect("serialize");
    match out_path {
        Some(p) => std::fs::write(p, text).expect("write"),
        None => println!("{text}"),
    }
}
