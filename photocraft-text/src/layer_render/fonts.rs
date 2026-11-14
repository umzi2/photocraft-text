//! Font ownership, name resolution and explicit-style rendering.

use std::path::{Path, PathBuf};

use crate::area::{first_baseline_ratio, layout_area, layout_area_runs};
use crate::engine_data::Value;
use crate::error::Error;
use crate::font::Face;
use crate::runs::{self, FontSet, RunStyle};

use super::horizontal::{render_text_layer_with_atlases, text_style_of};
use super::{LayerPixels, LayerTextStyle, RenderOptions};

/// Font files keyed by the family/style name Photoshop stores in `FontSet`.
///
/// `Face` borrows its font bytes, so the bytes have to outlive every layout;
/// keeping them here lets a caller render a layer without owning them itself.
#[derive(Default)]
pub struct FaceStore {
    entries: Vec<(String, Vec<u8>, PathBuf)>,
    /// Names a font is also known by -- its PostScript, family and full names
    /// from the `name` table -- mapped to the entry index. Photoshop names a
    /// run's font by its PostScript name (`TimesNewRomanPSMT`), while installed
    /// files are named by family (`times.ttf`), so both must resolve.
    aliases: Vec<(String, usize)>,
    /// Photoshop-captured glyph bitmaps; when set, glyphs they cover skip the
    /// outline rasteriser and come out byte-identical to Photoshop's.  One
    /// entry per captured pen phase: CoolType's Crisp AA positions glyphs on a
    /// quarter-pixel grid, so a glyph's bitmap depends on `frac(pen x)`.
    atlases: Vec<crate::atlas::Atlas>,
}

impl FaceStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches one glyph atlas (see [`crate::atlas`]), replacing any others.
    pub fn set_atlas(&mut self, atlas: crate::atlas::Atlas) {
        self.atlases.clear();
        self.atlases.push(atlas);
    }

    /// Attaches every phase of a captured glyph atlas (typically four entries
    /// at pen phases 0, 0.25, 0.5 and 0.75).
    pub fn set_atlases(&mut self, atlases: Vec<crate::atlas::Atlas>) {
        self.atlases = atlases;
    }

    /// The attached glyph atlases.
    pub fn atlases(&self) -> &[crate::atlas::Atlas] {
        &self.atlases
    }

    /// The first attached glyph atlas, if any.
    pub fn atlas(&self) -> Option<&crate::atlas::Atlas> {
        self.atlases.first()
    }

    /// Lays out and rasterises `text` with explicit runs and layer style.
    ///
    /// This is the direct API behind the live editor: it resolves the run
    /// fonts from this store and uses the attached atlas when it covers a
    /// glyph, so a captured atlas renders byte-identically to Photoshop.
    pub fn render_spec(
        &self,
        text: &str,
        runs: &[RunStyle],
        spec: &LayerTextStyle,
    ) -> Result<LayerPixels, Error> {
        self.render_spec_with(text, runs, spec, RenderOptions::default())
    }

    /// As [`FaceStore::render_spec`] with explicit rasteriser options.
    pub fn render_spec_with(
        &self,
        text: &str,
        runs: &[RunStyle],
        spec: &LayerTextStyle,
        options: RenderOptions,
    ) -> Result<LayerPixels, Error> {
        let mut wanted: Vec<String> = runs.iter().filter_map(|r| r.font.clone()).collect();
        wanted.sort();
        wanted.dedup();
        let faces = if wanted.is_empty() {
            // No run names a font: prefer the atlas font, then the first face
            // that can actually produce outlines (CFF-only faces parse but have
            // no `loca`, so "parses" is not enough).
            let name = self
                .atlas()
                .map(|a| a.font.clone())
                .filter(|n| self.bytes(n).is_some())
                .or_else(|| self.first_usable_name())
                .unwrap_or_default();
            self.font_set(&[name])?
        } else {
            self.font_set(&wanted)?
        };
        let atlases: &[crate::atlas::Atlas] = if options.use_atlas {
            &self.atlases
        } else {
            &[]
        };
        // Point text (`box_size` 0) has no frame: measure it unwrapped, exactly
        // like `render_from_engine_data` does when rewriting a PSD.
        let mut spec = spec.clone();
        if spec.box_size.0 <= 0.0 || spec.box_size.1 <= 0.0 {
            let face = runs
                .first()
                .and_then(|r| r.font.as_deref())
                .and_then(|n| faces.get(n))
                .unwrap_or_else(|| faces.default_face());
            let ascender = first_baseline_ratio(face);
            // The probe must use the run's style, or tracking/horizontal scale
            // shrink the measured width and the text wraps.
            let probe_style = runs
                .first()
                .map(|r| text_style_of(r, &spec.style))
                .unwrap_or_else(|| spec.style.clone());
            // Alignment is meaningless while measuring: with a 1e7 pt frame a
            // centred line would be pushed 5e6 pt right and the canvas cannot
            // hold it. Point text is measured flush left, then laid out for
            // real against the measured width.
            let mut probe_para = spec.paragraph.clone();
            probe_para.justification = crate::area::Justification::Left;
            probe_para.first_line_indent = 0.0;
            probe_para.start_indent = 0.0;
            probe_para.end_indent = 0.0;
            let probe = if runs.is_empty() {
                layout_area(
                    faces.default_face(),
                    text,
                    &probe_style,
                    &probe_para,
                    1.0e7,
                    1.0e7,
                    spec.origin,
                    ascender,
                )
            } else {
                // Measure the *shaped* text: caps and faux bold widen glyphs, so
                // measuring the raw string wraps the first line.
                layout_area_runs(
                    &faces,
                    text,
                    runs,
                    &probe_style,
                    &probe_para,
                    1.0e7,
                    1.0e7,
                    spec.origin,
                    ascender,
                )
            };
            let width = probe
                .lines
                .iter()
                .flat_map(|l| l.glyphs.iter())
                .map(|g| g.x + g.advance)
                .fold(1.0f32, f32::max);
            let (mut top, mut bottom) = (f32::MAX, f32::MIN);
            for line in &probe.lines {
                top = top.min(line.baseline);
                bottom = bottom.max(line.baseline);
            }
            let biggest = runs
                .iter()
                .map(|r| r.size.unwrap_or(spec.style.size))
                .fold(spec.style.size, f32::max);
            spec.box_size = (width + 2.0, (bottom - top) + biggest * 2.0 + biggest);
        }
        render_text_layer_with_atlases(&faces, text, runs, &spec, atlases, options.aa)
    }

    /// Loads every `.ttf`/`.otf` in `dir`, registering the file stem as the name.
    pub fn load_dir(&mut self, dir: &Path) -> usize {
        let Ok(read) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut loaded = 0;
        for entry in read.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext != "ttf" && ext != "otf" {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if self.load_file(stem, &path).is_ok() {
                    loaded += 1;
                }
            }
        }
        loaded
    }

    /// Loads one named font from `dir`; the name is the file stem.
    pub fn load(&mut self, dir: &Path, name: &str) -> Result<(), Error> {
        for ext in ["ttf", "otf"] {
            let path = dir.join(format!("{name}.{ext}"));
            if path.exists() {
                return self.load_file(name, &path);
            }
        }
        Err(Error::BadFont("font file not found"))
    }

    fn load_file(&mut self, name: &str, path: &Path) -> Result<(), Error> {
        let bytes = std::fs::read(path)?;
        let face = Face::parse(&bytes)?;
        let index = self.entries.len();
        for alias in [face.postscript_name(), face.full_name(), face.family_name()]
            .into_iter()
            .flatten()
        {
            if !alias.is_empty() && !self.aliases.iter().any(|(a, _)| *a == alias) {
                self.aliases.push((alias, index));
            }
        }
        self.entries
            .push((name.to_string(), bytes, path.to_path_buf()));
        Ok(())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.bytes(name).is_some()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(n, _, _)| n.as_str())
    }

    /// One registered face by its `FontSet` name.
    pub fn face(&self, name: &str) -> Option<Face<'_>> {
        Face::parse(self.bytes(name)?).ok()
    }

    /// The first registered face, used when a run names an unknown font.
    pub fn first_face(&self) -> Option<Face<'_>> {
        let (_, bytes, _) = self.entries.first()?;
        Face::parse(bytes).ok()
    }

    /// First registered face that can outline a plain Latin glyph.
    fn first_usable_name(&self) -> Option<String> {
        self.entries.iter().map(|e| e.0.clone()).find(|name| {
            let Some(bytes) = self.bytes(name) else {
                return false;
            };
            let Ok(face) = Face::parse(bytes) else {
                return false;
            };
            let Some(gid) = face.glyph_index('A') else {
                return false;
            };
            crate::outline::glyph_contours(&face, gid)
                .map(|c| !c.is_empty())
                .unwrap_or(false)
        })
    }

    fn bytes(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(n, _, _)| n == name)
            .or_else(|| {
                // Photoshop's `Txt2`/`EngineData` store PostScript names
                // (`CCGrimNGritty-Regular`) while installed files are named
                // with spaces (`CCGrimNGritty Regular.otf`). Fall back to a
                // separator-insensitive match so non-standard fonts resolve.
                let key = font_key(name);
                self.entries.iter().find(|(n, _, _)| font_key(n) == key)
            })
            .map(|(_, b, _)| b.as_slice())
            .or_else(|| {
                let key = font_key(name);
                self.aliases
                    .iter()
                    .find(|(alias, _)| *alias == name || font_key(alias) == key)
                    .and_then(|(_, index)| self.entries.get(*index))
                    .map(|(_, b, _)| b.as_slice())
            })
    }

    /// Builds a `FontSet` whose default face is the first name that loads.
    /// Panics never: an empty store is an error.
    pub(super) fn font_set(&self, wanted: &[String]) -> Result<FontSet<'_>, Error> {
        let fallback;
        let first = if let Some(name) = wanted.iter().find(|n| self.bytes(n).is_some()) {
            name
        } else {
            fallback = self
                .first_usable_name()
                .ok_or(Error::BadFont("no run font available in the store"))?;
            &fallback
        };
        let mut set = FontSet::new(
            first.as_str(),
            Face::parse(
                self.bytes(first)
                    .ok_or(Error::BadFont("font vanished from the store"))?,
            )?,
        );
        for name in wanted {
            if name == first {
                continue;
            }
            // A run that names a font the store lacks cannot be rendered: the
            // layout looks the face up by that name and panics if the `FontSet`
            // silently dropped it. Report it instead, so callers fall back to
            // their own renderer rather than dying on a missing font.
            let bytes = self
                .bytes(name)
                .ok_or_else(|| Error::BadFont("a run font is not in the store"))?;
            set.insert(name.as_str(), Face::parse(bytes)?);
        }
        Ok(set)
    }
}

/// Document-space distance from a point-text origin (the `TySh` transform
/// translation) to the first baseline, for the layer's first style run.
pub fn first_baseline_offset_of(store: &FaceStore, ed: &Value) -> Option<f32> {
    let style_runs = runs::from_engine_data(ed);
    let first = style_runs.first()?;
    let face = first
        .font
        .as_deref()
        .and_then(|n| store.face(n))
        .or_else(|| store.first_face())?;
    let size = first.size.unwrap_or(12.0);
    Some(crate::area::first_baseline_offset(
        first_baseline_ratio(&face),
        size,
    ))
}

/// [`first_baseline_offset_of`] for a Photoshop [`FirstBaseline`] choice
/// (`Txt2`'s frame setting; see [`crate::area::FirstBaseline`]).
pub fn first_baseline_offset_of_kind(
    store: &FaceStore,
    ed: &Value,
    kind: crate::area::FirstBaseline,
) -> Option<f32> {
    let style_runs = runs::from_engine_data(ed);
    let first = style_runs.first()?;
    let face = first
        .font
        .as_deref()
        .and_then(|n| store.face(n))
        .or_else(|| store.first_face())?;
    let size = first.size.unwrap_or(12.0);
    Some(crate::area::first_baseline_offset(
        crate::area::first_baseline_ratio_kind(&face, kind),
        size,
    ))
}

/// The first-baseline ratio [`first_baseline_offset_of_kind`] multiplies by the
/// run's size; the render entry points take it to lay the lines out with the
/// same choice.
pub fn first_baseline_ratio_of_kind(
    store: &FaceStore,
    ed: &Value,
    kind: crate::area::FirstBaseline,
) -> Option<f32> {
    let style_runs = runs::from_engine_data(ed);
    let first = style_runs.first()?;
    let face = first
        .font
        .as_deref()
        .and_then(|n| store.face(n))
        .or_else(|| store.first_face())?;
    Some(crate::area::first_baseline_ratio_kind(&face, kind))
}

/// Separator-insensitive font-name key (`CCGrimNGritty-Regular` ==
/// `CCGrimNGritty Regular`), lowercased, alphanumerics only.
fn font_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}
