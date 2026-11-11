//! Bitmap glyph atlas captured from Photoshop.
//!
//! CoolType hints every glyph onto the pixel grid and places it at an integer
//! origin, so one glyph's rendered bitmap does not depend on where it sits in a
//! line: eight `T` glyphs inside one Photoshop text layer are byte-identical
//! cells even though their pen x differs. A per-(font, size, antialias) bitmap
//! atlas is therefore enough to reproduce Photoshop's pixels exactly; the
//! unhinted outline path cannot, because hinting is not stored anywhere.
//!
//! `tools/make_glyph_atlas.py` builds an atlas by rendering one glyph per
//! Photoshop launch and lifting each layer's alpha. The text format is
//! line-oriented so it needs no JSON dependency:
//!
//! ```text
//! font NotoSans-Regular
//! size 48
//! aa 4
//! bold 0
//! italic 0
//! hscale 1
//! pen_x 40
//! baseline_y 120
//! bin atlas.bin
//! glyph 0048 4 -33 30 35 0
//! ```
//!
//! The `glyph` fields are codepoint, `dx` (ink left minus pen x), `dy` (ink top
//! minus baseline), width, height and the byte offset into `atlas.bin`.

use std::collections::HashMap;
use std::path::Path;

use crate::error::Error;

/// One glyph bitmap inside an [`Atlas`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub ch: char,
    /// Ink left edge minus the pen x, in device pixels.
    pub dx: i32,
    /// Ink top edge minus the baseline row, in device pixels (negative is up).
    pub dy: i32,
    pub w: u32,
    pub h: u32,
    pub offset: usize,
}

/// Photoshop-captured glyph bitmaps for one font at one size and antialias.
#[derive(Clone, Debug)]
pub struct Atlas {
    pub font: String,
    pub size: f32,
    /// Photoshop's antialias code (`4` = Sharp); see [`crate::raster::AntiAlias`].
    pub aa: i64,
    /// Captured with faux bold (glyphs carry Photoshop's synthetic stroke).
    pub bold: bool,
    /// Captured with faux italic (glyphs carry Photoshop's skew).
    pub italic: bool,
    /// Horizontal scale the atlas was captured at (`1.0` = 100 %).
    pub hscale: f32,
    /// Pen x used when the atlas was captured (the bitmaps are relative to it).
    pub pen_x: f32,
    /// Baseline row used when the atlas was captured.
    pub baseline_y: f32,
    glyphs: HashMap<char, Glyph>,
    data: Vec<u8>,
}

impl Atlas {
    /// Parses `atlas.txt` (and its `bin` file) from `dir`.
    pub fn load(dir: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(dir.join("atlas.txt"))
            .map_err(|e| Error::Atlas(format!("{}: {e}", dir.join("atlas.txt").display())))?;
        let mut font = String::new();
        let mut size = 0.0f32;
        let mut aa = 0i64;
        let mut bold = false;
        let mut italic = false;
        let mut hscale = 1.0f32;
        let mut pen_x = 0.0f32;
        let mut baseline_y = 0.0f32;
        let mut bin = None;
        let mut glyphs = HashMap::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let (key, rest) = trimmed
                .split_once(' ')
                .ok_or_else(|| Error::Atlas(format!("bad atlas line: {trimmed}")))?;
            match key {
                "font" => font = rest.trim().to_owned(),
                "size" => size = parse(rest)?,
                "aa" => aa = parse(rest)?,
                "bold" => bold = parse::<i64>(rest)? != 0,
                "italic" => italic = parse::<i64>(rest)? != 0,
                "hscale" => hscale = parse(rest)?,
                "pen_x" => pen_x = parse(rest)?,
                "baseline_y" => baseline_y = parse(rest)?,
                "bin" => bin = Some(rest.trim().to_owned()),
                "glyph" => {
                    let mut parts = rest.split_whitespace();
                    let code: u32 = u32::from_str_radix(
                        parts.next().ok_or_else(|| Error::Atlas("glyph: codepoint".into()))?,
                        16,
                    )
                    .map_err(|e| Error::Atlas(format!("glyph codepoint: {e}")))?;
                    let ch = char::from_u32(code)
                        .ok_or_else(|| Error::Atlas(format!("bad codepoint {code:04X}")))?;
                    let dx = parse(parts.next().unwrap_or("0"))?;
                    let dy = parse(parts.next().unwrap_or("0"))?;
                    let w: u32 = parse(parts.next().unwrap_or("0"))?;
                    let h: u32 = parse(parts.next().unwrap_or("0"))?;
                    let offset: usize = parse(parts.next().unwrap_or("0"))?;
                    glyphs.insert(ch, Glyph { ch, dx, dy, w, h, offset });
                }
                other => return Err(Error::Atlas(format!("unknown atlas key: {other}"))),
            }
        }
        let bin = bin.ok_or_else(|| Error::Atlas("atlas has no bin file".into()))?;
        let data = std::fs::read(dir.join(&bin))
            .map_err(|e| Error::Atlas(format!("{}: {e}", dir.join(&bin).display())))?;
        if glyphs.is_empty() {
            return Err(Error::Atlas("atlas has no glyphs".into()));
        }
        Ok(Self { font, size, aa, bold, italic, hscale, pen_x, baseline_y, glyphs, data })
    }

    /// Number of captured glyphs.
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    /// Whether the atlas has no glyphs at all.
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// The glyph's bitmap, or `None` when the atlas does not cover it.
    pub fn glyph(&self, ch: char) -> Option<GlyphRef<'_>> {
        let g = *self.glyphs.get(&ch)?;
        let start = g.offset;
        let end = start + (g.w as usize) * (g.h as usize);
        let alpha = self.data.get(start..end)?;
        Some(GlyphRef { glyph: g, alpha })
    }
}

fn parse<T: std::str::FromStr>(s: &str) -> Result<T, Error> {
    s.trim()
        .parse()
        .map_err(|_| Error::Atlas(format!("cannot parse {s:?}")))
}

/// A glyph bitmap borrowed from its [`Atlas`].
#[derive(Clone, Copy, Debug)]
pub struct GlyphRef<'a> {
    pub glyph: Glyph,
    pub alpha: &'a [u8],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_small_atlas() {
        let dir = std::env::temp_dir().join(format!("photocraft-atlas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("atlas.txt"),
            "font NotoSans-Regular\nsize 48\naa 4\nbold 0\nitalic 0\npen_x 40\nbaseline_y 120\nbin g.bin\n\
             glyph 0041 4 -33 2 2 0\n",
        )
        .unwrap();
        std::fs::write(dir.join("g.bin"), [0u8, 17, 34, 255]).unwrap();
        let atlas = Atlas::load(&dir).unwrap();
        assert_eq!(atlas.font, "NotoSans-Regular");
        assert_eq!(atlas.size, 48.0);
        assert_eq!(atlas.aa, 4);
        let g = atlas.glyph('A').unwrap();
        assert_eq!((g.glyph.dx, g.glyph.dy, g.glyph.w, g.glyph.h), (4, -33, 2, 2));
        assert_eq!(g.alpha, &[0, 17, 34, 255]);
        assert!(atlas.glyph('B').is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
