use crate::atlas::{Atlas, GlyphRef};

pub(super) struct GlyphQuery<'a> {
    pub font: &'a str,
    pub size: f32,
    pub aa: i64,
    pub bold: bool,
    pub italic: bool,
    pub horizontal_scale: f32,
    pub vertical_scale: f32,
    pub cap_scale: f32,
    pub phase: i32,
    pub gid: u16,
    pub ch: char,
    pub nominal: bool,
}

impl GlyphQuery<'_> {
    fn matches(&self, atlas: &Atlas) -> bool {
        // Synthetic small caps accept a capture made at the run's nominal size:
        // Photoshop hints those scaled capitals differently from smaller type.
        let effective_size = self.size * self.cap_scale;
        atlas.font == self.font
            && ((atlas.size - effective_size).abs() < 0.01
                || (self.cap_scale < 1.0
                    && atlas.small_caps
                    && (atlas.size - self.size).abs() < 0.01))
            && atlas.aa == self.aa
            && atlas.bold == self.bold
            && atlas.italic == self.italic
            && (atlas.hscale - self.horizontal_scale).abs() < 0.001
            && (self.vertical_scale - self.cap_scale).abs() < 0.001
    }

    pub(super) fn select<'a>(&self, atlases: &'a [Atlas]) -> Option<(GlyphRef<'a>, bool)> {
        let mut first: Option<GlyphRef<'a>> = None;
        let mut count = 0usize;
        let mut invariant = true;
        let mut phase_zero = None;
        let mut phase_match = None;
        for atlas in atlases {
            if !self.matches(atlas) {
                continue;
            }
            // Substituted glyphs must never reuse the nominal character's cell.
            let Some(cell) = atlas.glyph_gid(self.gid).or_else(|| {
                if self.nominal {
                    atlas.glyph(self.ch)
                } else {
                    None
                }
            }) else {
                continue;
            };
            if let Some(first) = first {
                invariant = invariant
                    && cell.glyph.w == first.glyph.w
                    && cell.glyph.h == first.glyph.h
                    && cell.alpha == first.alpha;
            } else {
                first = Some(cell);
            }
            count += 1;
            if phase_zero.is_none() && (atlas.phase * 4.0).round() == 0.0 {
                phase_zero = Some((cell, false));
            }
            if phase_match.is_none() && ((atlas.phase * 4.0).round() as i32 & 3) == self.phase {
                phase_match = Some((cell, true));
            }
        }
        // Identical captured phases use the first phase-zero bitmap and an
        // integer pen; genuinely phased cells keep the quarter-pixel pen.
        let selected = if count > 1 && invariant {
            phase_zero
        } else {
            phase_match
        };
        // A lone cell without the requested phase uses the legacy integer pen.
        selected.or_else(|| {
            if count == 1 {
                first.map(|cell| (cell, false))
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::GlyphQuery;
    use crate::atlas::Atlas;

    fn atlas() -> Atlas {
        Atlas::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets/atlas/gid"),
        )
        .expect("synthetic atlas")
    }

    fn query() -> GlyphQuery<'static> {
        GlyphQuery {
            font: "NotoSans-Regular",
            size: 48.0,
            aa: 4,
            bold: false,
            italic: false,
            horizontal_scale: 1.0,
            vertical_scale: 1.0,
            cap_scale: 1.0,
            phase: 0,
            gid: 0xffff,
            ch: 'A',
            nominal: true,
        }
    }

    #[test]
    fn glyph_id_takes_priority_over_a_nominal_character_cell() {
        let atlases = [atlas()];
        let (cell, phased) = query().select(&atlases).expect("gid cell");
        assert_eq!(cell.glyph.dx, 7);
        assert_eq!(cell.alpha, &[81]);
        assert!(phased);
    }

    #[test]
    fn substituted_glyph_without_a_gid_cell_cannot_reuse_nominal_ink() {
        let atlases = [atlas()];
        let mut query = query();
        query.gid = 0xfffe;
        query.nominal = false;
        assert!(query.select(&atlases).is_none());
        query.nominal = true;
        let (cell, _) = query.select(&atlases).expect("nominal cell");
        assert_eq!(cell.alpha, &[17]);
    }

    #[test]
    fn synthetic_small_caps_accept_only_matching_capture_metadata() {
        let mut atlas = atlas();
        let mut query = query();
        query.cap_scale = 0.8;
        query.vertical_scale = 0.8;
        assert!(query.select(std::slice::from_ref(&atlas)).is_none());
        atlas.small_caps = true;
        assert!(query.select(std::slice::from_ref(&atlas)).is_some());
        for mismatch in 0..4 {
            let mut other = atlas.clone();
            match mismatch {
                0 => other.aa = 1,
                1 => other.bold = true,
                2 => other.italic = true,
                _ => other.hscale = 1.5,
            }
            assert!(
                query.select(&[other]).is_none(),
                "metadata field {mismatch}"
            );
        }
    }
}
