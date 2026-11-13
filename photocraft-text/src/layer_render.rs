//! Rendering a text layer's pixels.
//!
//! A Photoshop type layer stores both the editable text (`TySh`/`Txt2`) and a
//! rasterised copy of the glyphs in the layer's pixel channels. Apps without a
//! text engine show that copy, and Photoshop uses it until it re-renders the
//! layer. When we rewrite the text we must rewrite the pixels too, otherwise the
//! file shows the old words in every viewer that does not re-lay-out text.

use std::path::{Path, PathBuf};

use crate::area::{
    ParagraphStyle, first_baseline_ratio, layout_area, layout_area_runs,
    layout_area_runs_paragraphs_point, layout_area_runs_paragraphs_with_advances,
};
use crate::engine_data::Value;
use crate::error::Error;
use crate::font::Face;
use crate::layout::{Kerning, TextStyle};
use crate::outline::glyph_contours;
use crate::raster::Canvas;
use crate::runs::{self, FontSet, RunStyle};
use crate::typography;
use crate::warp::{self, FlatGeometry, FlatSurface, Warp};

/// A rendered text layer: straight (non-premultiplied) RGBA, row-major.
#[derive(Clone, Debug)]
pub struct LayerPixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Top-left corner of the canvas relative to the text box's top-left.
    ///
    /// Photoshop sizes a type layer to the ink, not to the box, so glyphs that
    /// rise above the box top (accents, `Й`) or hang below it still land in the
    /// layer. The caller adds this offset to the box origin to get the layer
    /// rectangle.
    pub offset: (f32, f32),
}

/// Parameters of one type layer, in the same units Photoshop stores.
#[derive(Clone, Debug, Default)]
pub struct LayerTextStyle {
    pub style: TextStyle,
    pub paragraph: ParagraphStyle,
    /// Width and height of the text box in points.
    pub box_size: (f32, f32),
    /// Top-left corner of the text box inside the layer rect, in points.
    pub origin: (f32, f32),
    /// Fill colour, straight RGB.
    pub color: [u8; 3],
}

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
    fn font_set(&self, wanted: &[String]) -> Result<FontSet<'_>, Error> {
        let fallback = self.first_usable_name();
        let first = wanted
            .iter()
            .find(|n| self.bytes(n).is_some())
            .or(fallback.as_ref())
            .ok_or(Error::BadFont("no run font available in the store"))?;
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

fn text_style_of(run: &RunStyle, fallback: &TextStyle) -> TextStyle {
    let kerning = match (run.auto_kerning, run.kerning) {
        (Some(true), _) | (None, None) => Kerning::Metrics,
        (_, Some(k)) => Kerning::Manual(k),
        _ => Kerning::Metrics,
    };
    TextStyle {
        size: run.size.unwrap_or(fallback.size),
        tracking: run.tracking.unwrap_or(fallback.tracking),
        kerning,
        horizontal_scale: run.horizontal_scale.unwrap_or(1.0),
        baseline_shift: run.baseline_shift.unwrap_or(0.0),
        leading: fallback.leading,
        auto_leading: fallback.auto_leading,
    }
}

/// Maps each `EngineData` paragraph run onto the layout's paragraph style.
pub fn paragraph_styles_of(runs: &[runs::ParagraphRun]) -> Vec<ParagraphStyle> {
    runs.iter()
        .map(|p| ParagraphStyle {
            justification: p.justification,
            first_line_indent: p.first_line_indent,
            start_indent: p.start_indent,
            end_indent: p.end_indent,
            space_before: p.space_before,
            space_after: p.space_after,
            word_spacing: p.word_spacing,
            letter_spacing: p.letter_spacing,
            glyph_spacing: p.glyph_scaling,
            auto_leading: p.auto_leading,
            every_line_composer: p.every_line_composer,
            hanging: p.hanging,
            hyphenate: p.hyphenate,
            words_longer_than: p.words_longer_than,
            pre_hyphen: p.pre_hyphen,
            post_hyphen: p.post_hyphen,
            consecutive_hyphens: p.consecutive_hyphens,
            hyphen_zone: p.hyphen_zone,
        })
        .collect()
}

/// Clears the paragraph geometry that must not affect an unwrapped probe:
/// alignment against a huge frame would push centred/indented lines away.
fn probe_paragraphs(paragraphs: &[ParagraphStyle]) -> Vec<ParagraphStyle> {
    let mut out = paragraphs.to_vec();
    if out.is_empty() {
        out.push(ParagraphStyle::default());
    }
    for p in &mut out {
        p.justification = crate::area::Justification::Left;
        p.first_line_indent = 0.0;
        p.start_indent = 0.0;
        p.end_indent = 0.0;
        p.space_before = 0.0;
        p.space_after = 0.0;
    }
    out
}

/// Lays out and rasterises `text` into a canvas the size of the ink.
///
/// `faces` must contain the run fonts; `runs` may be empty, in which case
/// `spec.style` and the default face are used.
/// Options for [`FaceStore::render_spec_with`].
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderOptions {
    /// Antialias mode to rasterise with; `None` keeps [`crate::raster::AntiAlias::Sharp`].
    pub aa: Option<crate::raster::AntiAlias>,
    /// Use the attached glyph atlas for the glyphs it covers.
    pub use_atlas: bool,
}

pub fn render_text_layer(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    spec: &LayerTextStyle,
) -> Result<LayerPixels, Error> {
    render_text_layer_with_atlas(faces, text, runs, spec, None, None)
}

/// As [`render_text_layer`], with one captured glyph atlas and an antialias
/// override.
pub fn render_text_layer_with_atlas(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    spec: &LayerTextStyle,
    atlas: Option<&crate::atlas::Atlas>,
    aa: Option<crate::raster::AntiAlias>,
) -> Result<LayerPixels, Error> {
    render_text_layer_with_atlases(
        faces,
        text,
        runs,
        spec,
        atlas.map(std::slice::from_ref).unwrap_or(&[]),
        aa,
    )
}

/// As [`render_text_layer_with_atlas`], with every captured pen phase attached.
pub fn render_text_layer_with_atlases(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    spec: &LayerTextStyle,
    atlases: &[crate::atlas::Atlas],
    aa: Option<crate::raster::AntiAlias>,
) -> Result<LayerPixels, Error> {
    let face = runs
        .first()
        .and_then(|r| r.font.as_deref())
        .and_then(|name| faces.get(name))
        .unwrap_or_else(|| faces.default_face());
    let ascender = first_baseline_ratio(face);
    let style = runs
        .first()
        .map(|r| text_style_of(r, &spec.style))
        .unwrap_or_else(|| spec.style.clone());

    // The multi-run path carries the per-character attributes (vertical scale,
    // faux italic, caps, colour) into `Line::attrs`; the single-style
    // `layout_area` cannot, so anything that changes between characters needs
    // it. `runs` is usually one run from the live editor, but that run still
    // sets vertical scale and faux styles.
    let layout = if runs.is_empty() {
        layout_area(
            faces.default_face(),
            text,
            &style,
            &spec.paragraph,
            spec.box_size.0,
            spec.box_size.1,
            spec.origin,
            ascender,
        )
    } else {
        layout_area_runs(
            faces,
            text,
            runs,
            &style,
            &spec.paragraph,
            spec.box_size.0,
            spec.box_size.1,
            spec.origin,
            ascender,
        )
    };
    paint(
        faces,
        layout.lines,
        runs,
        spec,
        aa.unwrap_or(crate::raster::AntiAlias::Sharp),
        atlases,
        None,
        None,
    )
}

/// Lays out a layer straight from its `EngineData` and rasterises it.
///
/// The paragraph sheets are read from the layer's own
/// `EngineDict/ParagraphRun` (every run, with its `RunLengthArray`), so indents,
/// space before/after, auto leading, justification, hanging punctuation,
/// every-line composer and hyphenation are applied per paragraph. Point text
/// (`box_size` zero) is measured unwrapped first.
pub fn render_from_engine_data(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    box_size: (f32, f32),
    origin: (f32, f32),
) -> Result<LayerPixels, Error> {
    let paragraphs = runs::paragraphs_from_engine_data(ed);
    render_from_engine_data_with_paragraphs(store, ed, text, &paragraphs, box_size, origin)
}

/// As [`render_from_engine_data`] with paragraph styles supplied by the caller.
///
/// `paragraphs` are indexed by paragraph in text order (`len` in UTF-16 code
/// units, paragraph separators included; the last entry repeats). This is the
/// entry point for a document model that keeps paragraph sheets outside
/// `EngineData`; passing an empty slice falls back to the layer's own sheets.
///
/// Returns the ink-cropped RGBA surface and its offset from the text origin
/// (`LayerPixels::offset`, measured from the first baseline vertically), the
/// same contract as [`render_from_engine_data`].
fn render_impl(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
    target: Option<RectTarget>,
    warp: Option<&WarpDraw<'_>>,
) -> Result<LayerPixels, Error> {
    let mut style_runs = runs::from_engine_data(ed);
    for (i, f) in features.iter().enumerate() {
        if let Some(run) = style_runs.get_mut(i) {
            run.old_style |= f.old_style;
        }
    }
    let aa = runs::antialias(ed);
    let paragraphs = if paragraphs.is_empty() {
        runs::paragraphs_from_engine_data(ed)
    } else {
        paragraphs.to_vec()
    };
    if style_runs.is_empty() {
        return Err(Error::EngineData("no style runs"));
    }
    let mut wanted: Vec<String> = style_runs.iter().filter_map(|r| r.font.clone()).collect();
    wanted.sort();
    wanted.dedup();
    let faces = store.font_set(&wanted)?;

    let base = TextStyle {
        size: style_runs[0].size.unwrap_or(12.0),
        ..Default::default()
    };
    let mut style = text_style_of(&style_runs[0], &base);
    let optical_kerning = features.iter().any(|f| f.optical_kerning);
    let mut runs_owned: Vec<runs::RunStyle>;
    let style_runs: &[runs::RunStyle] = if optical_kerning {
        // Optical mode replaces the font's pair kerning; the per-run
        // `AutoKerning`/`Kerning` fields would otherwise force metrics back on.
        style.kerning = crate::layout::Kerning::Optical;
        runs_owned = style_runs.to_vec();
        for run in &mut runs_owned {
            run.auto_kerning = None;
            run.kerning = None;
        }
        runs_owned.as_slice()
    } else {
        style_runs.as_slice()
    };
    let paras = paragraph_styles_of(&paragraphs);
    let primary = style_runs[0]
        .font
        .as_deref()
        .and_then(|n| faces.get(n))
        .unwrap_or_else(|| faces.default_face());
    let ascender = first_baseline_ratio(primary);

    // Point text has no frame: measure it unwrapped and size the canvas to the
    // laid-out lines instead of to the layer's old ink rectangle. Its lines
    // align against the type origin, not against the widest line.
    let point_text = !(box_size.0 > 0.0 && box_size.1 > 0.0);
    let box_size = if box_size.0 > 0.0 && box_size.1 > 0.0 {
        box_size
    } else {
        // Measure flush left: alignment against a 1e7 pt frame would push a
        // centred line millions of points right and the canvas could not hold it.
        let probe_styles = probe_paragraphs(&paras);
        let probe = layout_area_runs_paragraphs_with_advances(
            &faces,
            text,
            &style_runs,
            &style,
            &probe_styles,
            1.0e7,
            1.0e7,
            origin,
            ascender,
            &features
                .iter()
                .flat_map(|f| f.advance_deltas.iter().copied())
                .collect::<Vec<f32>>(),
        );
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
        let biggest = style_runs
            .iter()
            .map(|r| r.size.unwrap_or(style.size))
            .fold(style.size, f32::max);
        let height = (bottom - top) + biggest * 2.0;
        (width + 2.0, height + biggest)
    };

    let advance_deltas: Vec<f32> = features
        .iter()
        .flat_map(|f| f.advance_deltas.iter().copied())
        .collect();
    let layout = if point_text {
        layout_area_runs_paragraphs_point(&faces, text, &style_runs, &style, &paras, box_size.0, box_size.1, origin, ascender, &advance_deltas)
    } else {
        layout_area_runs_paragraphs_with_advances(&faces, text, &style_runs, &style, &paras, box_size.0, box_size.1, origin, ascender, &advance_deltas)
    };
    let spec = LayerTextStyle {
        style,
        paragraph: paras.first().copied().unwrap_or_default(),
        box_size,
        origin,
        color: style_runs[0].color.unwrap_or([0, 0, 0]),
    };
    let trace_level: u32 = std::env::var("PHOTOCRAFT_TRACE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if trace_level >= 2 {
        eprintln!(
            "layout: size={:?} box={:?} origin={:?} asc={:.5} first_off={:.3} paras={}",
            style.size,
            box_size,
            origin,
            ascender,
            crate::area::first_baseline_offset(ascender, style.size),
            paras.len()
        );
        for (i, line) in layout.lines.iter().enumerate() {
            let text: String = line.glyphs.iter().map(|g| g.ch).collect();
            let last = line.glyphs.last().map(|g| g.x + g.advance).unwrap_or(0.0);
            eprintln!(
                "line {i}: baseline={:.3} x0={:.3} xend={:.3} glyphs={:?} {:?}",
                line.baseline,
                line.glyphs.first().map(|g| g.x).unwrap_or(0.0),
                last,
                line.glyphs.len(),
                text
            );
            if trace_level >= 3 {
                for g in &line.glyphs {
                    eprintln!(
                        "   {:?} x={:.3} adv={:.3} y={:.3}",
                        g.ch, g.x, g.advance, g.y
                    );
                }
            }
        }
    }
    paint(
        &faces,
        layout.lines,
        &style_runs,
        &spec,
        aa,
        store.atlases(),
        target,
        warp,
    )
}

/// As [`render_from_engine_data`] with paragraph styles supplied by the caller.
///
/// `paragraphs` are indexed by paragraph in text order (`len` in UTF-16 code
/// units, paragraph separators included; the last entry repeats). This is the
/// entry point for a document model that keeps paragraph sheets outside
/// `EngineData`; passing an empty slice falls back to the layer's own sheets.
///
/// Returns the ink-cropped RGBA surface and its offset from the text origin
/// (`LayerPixels::offset`, measured from the first baseline vertically), the
/// same contract as [`render_from_engine_data`].
pub fn render_from_engine_data_with_paragraphs(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    box_size: (f32, f32),
    origin: (f32, f32),
) -> Result<LayerPixels, Error> {
    render_impl(
        store,
        ed,
        text,
        paragraphs,
        &[],
        box_size,
        origin,
        None,
        None,
    )
}

/// Lays out and rasterises a layer straight into a document rectangle.
///
/// This is the entry point for comparing against Photoshop's stored layer
/// pixels: `target.anchor` is the document position of the text origin (the
/// `TySh` translation for point text, the frame origin for area text) and
/// `target.origin`/`target.size` are the layer record's rectangle, so glyphs are
/// rasterised with the same subpixel phase Photoshop used. The returned
/// [`LayerPixels::offset`] is `(0, 0)` — the surface already sits at
/// `target.origin`.
pub fn render_from_engine_data_in_rect(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    box_size: (f32, f32),
    origin: (f32, f32),
    target: RectTarget,
) -> Result<LayerPixels, Error> {
    render_impl(
        store,
        ed,
        text,
        paragraphs,
        &[],
        box_size,
        origin,
        Some(target),
        None,
    )
}

/// As [`render_from_engine_data_with_paragraphs`], with per-style-run character
/// features that `EngineData` does not carry (`old_style` figures live only in
/// `Txt2` `/6/30`). `features` is indexed by style run; missing entries mean
/// "feature off".
pub fn render_from_engine_data_with_paragraphs_and_features(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
) -> Result<LayerPixels, Error> {
    render_impl(
        store, ed, text, paragraphs, features, box_size, origin, None, None,
    )
}

/// As [`render_from_engine_data_in_rect`], with the `Txt2`-only character
/// features of [`render_from_engine_data_with_paragraphs_and_features`].
pub fn render_from_engine_data_in_rect_with_features(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
    target: RectTarget,
) -> Result<LayerPixels, Error> {
    render_impl(
        store,
        ed,
        text,
        paragraphs,
        features,
        box_size,
        origin,
        Some(target),
        None,
    )
}

/// A warped type layer's pixels: the RGBA surface is exactly the layer's
/// stored document rectangle, so the caller can paint it at `rect` without
/// touching strides or origins.
#[derive(Clone, Debug)]
pub struct WarpedPixels {
    pub pixels: LayerPixels,
    /// Document rectangle the surface covers: `(x, y, width, height)` in layer
    /// pixels. `pixels.width/height` equal the rectangle's size.
    pub rect: (i32, i32, u32, u32),
}

/// Renders a warped type layer straight into its stored document rectangle.
///
/// Photoshop stores the *flat* layout rectangles (the `TySh` `bounds`
/// descriptor and the transform translation) and bends the rendered glyphs with
/// the `warp` descriptor; the layer's pixel rectangle is the warped result. This
/// wraps the two steps so a caller cannot mix up the flat and warped geometry:
///
/// 1. the text is laid out flat and rasterised on a canvas padded around
///    `target` (the bend pushes ink outside the rectangle — the pad is the
///    first-baseline offset plus 60 px, the same margin the PSD harness uses);
/// 2. [`warp::apply`] remaps that flat raster into `target.size` at
///    `target.origin`.
///
/// Contract:
/// * `flat.bounds` is the flat layout box in **document points** — the `TySh`
///   `bounds` rectangle translated by the transform: `(tx + l, ty + t,
///   tx + r, ty + b)`. It is *not* the warped `boundingBox`. A bounds rectangle
///   with no area is rejected with [`Error::Warp`] instead of silently mapping
///   the text into a one-pixel box.
/// * `flat.baseline` is the document y of the flat first baseline (the `TySh`
///   transform translation `ty` for point text).
/// * `target` is the *stored warped layer rectangle* (`RectTarget::origin` =
///   record `left/top`, `size` = record width/height) and `target.anchor` is the
///   document position of the layout origin (the `TySh` translation for point
///   text, the frame origin plus first baseline for area text), the same anchor
///   [`render_from_engine_data_in_rect`] takes.
/// * `box_size`/`origin` follow the normal render contract (point text passes
///   `(0, 0)` for both; area text passes the frame size).
/// * Horizontal type only: the flat pass is the horizontal renderer.
///
/// The returned [`WarpedPixels::rect`] is `target.origin` with `target.size`,
/// and `pixels.offset` is `(0, 0)`.
#[allow(clippy::too_many_arguments)]
pub fn render_from_engine_data_warped(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    warp: &Warp,
    flat: FlatGeometry,
    target: RectTarget,
) -> Result<WarpedPixels, Error> {
    let width = target.size.0.max(1);
    let height = target.size.1.max(1);
    let rect = (target.origin.0, target.origin.1, width, height);
    let (l, t, r, b) = flat.bounds;
    if ![l, t, r, b, flat.baseline].iter().all(|v| v.is_finite()) || r - l <= 0.5 || b - t <= 0.5 {
        return Err(Error::Warp(format!(
            "flat layout box {:?} with baseline {} is empty; pass the TySh \
             `bounds` rectangle translated by the transform",
            flat.bounds, flat.baseline
        )));
    }
    if !warp.is_warped() {
        // No bend to apply: the flat layout is the layer, so render it directly
        // into the stored rectangle.
        let pixels = render_from_engine_data_in_rect_with_features(
            store,
            ed,
            text,
            paragraphs,
            features,
            box_size,
            (0.0, 0.0),
            target,
        )?;
        return Ok(WarpedPixels { pixels, rect });
    }
    let pad = (first_baseline_offset_of(store, ed).unwrap_or(0.0).abs() + 60.0).ceil();
    let pad = pad.clamp(0.0, 4096.0) as i32;
    let flat_origin = (target.origin.0 - pad, target.origin.1 - pad);
    let flat_size = (
        width.saturating_add(2 * pad as u32),
        height.saturating_add(2 * pad as u32),
    );
    let flat_target = RectTarget {
        origin: flat_origin,
        size: flat_size,
        anchor: target.anchor,
    };
    let mut pixels = render_from_engine_data_in_rect_with_features(
        store,
        ed,
        text,
        paragraphs,
        features,
        box_size,
        (0.0, 0.0),
        flat_target,
    )?;
    let surface = FlatSurface {
        origin: flat_origin,
        width: pixels.width.max(1),
        height: pixels.height.max(1),
    };
    // Photoshop warps the glyph *outlines* and rasterises them in the bent
    // space; resampling the flat bitmap instead doubles every sheared stem
    // (the flat hinted pixels carry the unbent stem pattern, which the warp
    // smears across two device columns). The flat pass above supplies the flat
    // ink bounds, which anchor the warped drawing exactly like the bitmap
    // resampler does; the bent outlines themselves are rasterised in the
    // stored rectangle. `PHOTOCRAFT_WARP_RESAMPLE=1` restores the bitmap path.
    let resample = std::env::var("PHOTOCRAFT_WARP_RESAMPLE").is_ok_and(|v| v != "0");
    if !resample {
        if let Some(placement) = warp::placement(&pixels.rgba, surface, flat, warp) {
            let min = placement.min;
            let anchor = match std::env::var("PHOTOCRAFT_WARP_ANCHOR").as_deref() {
                Ok("ink") => WarpAnchor::Ink,
                Ok("stretch") => WarpAnchor::Stretch,
                _ => WarpAnchor::Caller,
            };
            let flat_origin_f = (flat_origin.0 as f32, flat_origin.1 as f32);
            let map = move |cx: f32, cy: f32| {
                let (wx, wy) = placement.forward(flat, cx + flat_origin_f.0, cy + flat_origin_f.1);
                if anchor == WarpAnchor::Caller {
                    (wx - min.0, wy - min.1)
                } else {
                    (wx, wy)
                }
            };
            let warped_target = RectTarget {
                origin: flat_origin,
                size: (width, height),
                anchor: target.anchor,
            };
            if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "14") {
                eprintln!(
                    "warp: bounds={:?} baseline={:.2} flat=({},{}) min=({:.2},{:.2}) anchor={anchor:?} out={:?}",
                    flat.bounds, flat.baseline, flat_origin.0, flat_origin.1, min.0, min.1, rect
                );
            }
            let draw = WarpDraw { map: &map, anchor };
            let mut warped = render_impl(
                store,
                ed,
                text,
                paragraphs,
                features,
                box_size,
                (0.0, 0.0),
                Some(warped_target),
                Some(&draw),
            )?;
            // The layer record is the warped ink's bounding box; keep the
            // surface spanning it even where the outermost stroke rounds to
            // zero coverage, or the exporter crops and shifts the layer.
            warp::fill_border(
                &mut warped.rgba,
                (warped.width.max(1), warped.height.max(1)),
            );
            warped.offset = (0.0, 0.0);
            return Ok(WarpedPixels {
                pixels: warped,
                rect,
            });
        }
    }
    let surface = FlatSurface {
        origin: flat_origin,
        width: pixels.width.max(1),
        height: pixels.height.max(1),
    };
    let rgba = warp::apply(
        &pixels.rgba,
        surface,
        flat,
        warp,
        target.origin,
        (width, height),
    );
    pixels.rgba = rgba;
    pixels.width = width;
    pixels.height = height;
    pixels.offset = (0.0, 0.0);
    Ok(WarpedPixels { pixels, rect })
}

/// Cap-height ratio (`OS/2.sCapHeight` over the em square), 0.72 fallback.
fn cap_height_ratio(face: &Face<'_>) -> f32 {
    face.os2
        .and_then(|o| o.s_cap_height)
        .map(|c| f32::from(c) / f32::from(face.head.units_per_em))
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(0.72)
}

fn vertical_impl(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
    target: Option<RectTarget>,
) -> Result<LayerPixels, Error> {
    let mut style_runs = runs::from_engine_data(ed);
    let aa = runs::antialias(ed);
    if style_runs.is_empty() {
        return Err(Error::EngineData("no style runs"));
    }
    // `TySh` stores the paragraph terminator as a NUL and drops it from the
    // text, while `EngineData`'s run lengths still count it; trim the trailing
    // units so shaping covers exactly the string we draw.
    let text_units = text.encode_utf16().count();
    let mut excess = style_runs
        .iter()
        .map(|r| r.len)
        .sum::<usize>()
        .saturating_sub(text_units);
    while excess > 0 {
        let Some(last) = style_runs.last_mut() else {
            break;
        };
        let take = last.len.min(excess);
        last.len -= take;
        excess -= take;
        if last.len == 0 {
            style_runs.pop();
        }
    }
    let mut wanted: Vec<String> = style_runs.iter().filter_map(|r| r.font.clone()).collect();
    wanted.sort();
    wanted.dedup();
    let faces = store.font_set(&wanted)?;
    let base = TextStyle {
        size: style_runs[0].size.unwrap_or(12.0),
        ..Default::default()
    };
    let style = text_style_of(&style_runs[0], &base);
    let primary = style_runs[0]
        .font
        .as_deref()
        .and_then(|n| faces.get(n))
        .unwrap_or_else(|| faces.default_face());
    let size = style.size.max(1.0);
    // Measured on `10_all_features.psd` ("11 vertical text", NotoSans 24 pt):
    // first baseline 20.57 pt below the top of the em column = (size + cap)/2;
    // each following upright glyph advances exactly one em. The glyph itself is
    // centred in the column: pen x = origin.x - advance/2 (NotoSans half
    // advances V 7.2, e 6.768, r 4.956, t 4.332, i 3.096, c 5.76 ...).
    let first_off = (size + cap_height_ratio(primary) * size) * 0.5;
    let shaped = runs::shape_runs_attrs(&faces, text, &style_runs, &style);
    let mut glyphs = Vec::new();
    let mut attrs = Vec::new();
    let mut placed = 0usize;
    for (mut g, a) in shaped {
        if matches!(g.ch, '\r' | '\n' | '\0') {
            continue;
        }
        g.x = -g.advance * 0.5;
        g.y = -(first_off + placed as f32 * size);
        glyphs.push(g);
        attrs.push(a);
        placed += 1;
    }
    let widest = glyphs.iter().map(|g| g.advance).fold(size, f32::max);
    let total = (placed as f32 * size).max(size);
    let spec = LayerTextStyle {
        style,
        paragraph: ParagraphStyle::default(),
        box_size: (widest, total),
        origin,
        color: style_runs[0].color.unwrap_or([0, 0, 0]),
    };
    let line = crate::area::Line {
        glyphs,
        baseline: 0.0,
        width: widest,
        attrs,
    };
    paint(
        &faces,
        vec![line],
        &style_runs,
        &spec,
        aa,
        store.atlases(),
        target,
        None,
    )
}

/// Lays out a vertical (`Ornt = Vrtc`) point-text layer from its `EngineData`.
///
/// The returned surface is ink-cropped with `origin` being the top of the em
/// column (`TySh` translation), the same placement contract as
/// [`render_from_engine_data`].
pub fn render_vertical_from_engine_data(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
) -> Result<LayerPixels, Error> {
    vertical_impl(store, ed, text, origin, None)
}

/// As [`render_vertical_from_engine_data`] but rasterised straight into
/// `target`, with `target.anchor` = the `TySh` translation.
pub fn render_vertical_from_engine_data_in_rect(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
    target: RectTarget,
) -> Result<LayerPixels, Error> {
    vertical_impl(store, ed, text, origin, Some(target))
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

/// Integer document rectangle a layer is rasterised into.
///
/// Photoshop stores each type layer as a raster cropped from the document at
/// `layer.rect`; painting straight into that rectangle keeps the glyph phase
/// relative to the document pixel grid, which an ink-cropped surface would lose.
#[derive(Clone, Copy, Debug)]
pub struct RectTarget {
    /// Top-left of the canvas in document pixels.
    pub origin: (i32, i32),
    /// Canvas size in pixels.
    pub size: (u32, u32),
    /// Document-space position of the layout origin handed to the layout
    /// functions: the `TySh` translation for point text, the text box origin for
    /// area text.
    pub anchor: (f32, f32),
}

/// Character features that live only in `Txt2`'s character sheet and therefore
/// cannot be read from `EngineData`. Indexed by the layer's style run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunFeatures {
    /// `Txt2` `/6/30` old-style figures (Photoshop's "on" value is 2).
    pub old_style: bool,
    /// Photoshop's optical kerning (`AutoKernType.OPTICAL`).
    ///
    /// The PSD does not record which kerning mode was used — `EngineData`
    /// carries `AutoKerning: True, Kerning: 0` for a metrics layer and for the
    /// layer authored with optical kerning alike — so a caller that knows the
    /// setting (its `Txt2`/layer model) asks for it per layer. When set, the
    /// engine spaces every pair by the shape-based [`crate::optical::optical_pair`]
    /// model instead of the font's pair kerning.
    pub optical_kerning: bool,
    /// Per-glyph advance corrections in points, one per shaped glyph in text
    /// order (spaces included, dictionary hyphens excluded; positive widens).
    ///
    /// This is where Txt2's composed cache belongs: the every-line composer's
    /// per-run tracking deltas (`/3` inside the cache run records) and optical
    /// kerning (the difference between the cached glyph origin and the metric
    /// advance of the same glyph). When non-empty the layout uses these
    /// advances instead of its uniform every-line nudge, so line breaking sees
    /// the same widths Photoshop did.
    pub advance_deltas: Vec<f32>,
}

/// Rasterises laid-out lines into a canvas cropped to the ink, or into a fixed
/// document rectangle when `target` is set.
/// Look up a glyph cell in an atlas: a `gid=` row covers substituted glyphs,
/// otherwise the codepoint-keyed cell is only safe for the nominal glyph.
fn atlas_cell<'a>(
    atlas: &'a crate::atlas::Atlas,
    gid: u16,
    ch: char,
    nominal: bool,
) -> Option<crate::atlas::GlyphRef<'a>> {
    atlas
        .glyph_gid(gid)
        .or_else(|| if nominal { atlas.glyph(ch) } else { None })
}

/// Where a warped draw anchors its canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WarpAnchor {
    /// The caller already placed the map (its values are canvas coordinates).
    Caller,
    /// Translate the warped ink's own bounding box minimum to the origin.
    Ink,
    /// Translate the ink minimum to the origin and stretch its extent onto the
    /// canvas.
    Stretch,
}

/// A warped draw: the flat-canvas -> warped-plane map plus its anchoring rule.
struct WarpDraw<'a> {
    map: &'a dyn Fn(f32, f32) -> (f32, f32),
    anchor: WarpAnchor,
}

/// One warped glyph collected by [`paint`] before the canvas is anchored.
struct WarpedGlyph {
    ch: char,
    gid: u16,
    contours: crate::outline::Contours,
    sx: f32,
    sy: f32,
    skew: f32,
    x: f32,
    baseline: f32,
    embolden: f32,
    color: [u8; 3],
}

/// One warped decoration rule (underline/strikeout) collected by [`paint`].
struct WarpedRule {
    x0: f32,
    x1: f32,
    y: f32,
    thickness: f32,
    offset: f32,
    color: [u8; 3],
}

/// Draws one decoration rule through the warp map.
///
/// A rule is a filled rectangle in flat space (`x0..x1` at `baseline + offset`,
/// `thickness` tall); the warp maps its outline like any other device point, so
/// the quad is subdivided every ~2 px before the map is applied.
fn warped_bar(
    canvas: &mut crate::raster::Canvas,
    x0: f32,
    x1: f32,
    baseline: f32,
    thickness: f32,
    offset: f32,
    map: &dyn Fn(f32, f32) -> (f32, f32),
    color: [u8; 3],
) {
    if thickness <= 0.0 || (x1 - x0).abs() <= 0.0 {
        return;
    }
    // The glyph loop leaves `embolden` at the last run's faux-bold stroke; a
    // rule never thickens with it.
    canvas.embolden = 0.0;
    let top = baseline + offset;
    let bottom = top + thickness;
    let steps = (((x1 - x0).abs() / 2.0).ceil() as usize).max(1);
    let mut polygon = Vec::with_capacity(2 * (steps + 1));
    let mut push = |x: f32, y: f32| {
        // `mask_contours_warped` maps `(px, py)` to `map(px, -py)` here.
        polygon.push(crate::outline::Point { x, y: -y, on: true });
    };
    for i in 0..=steps {
        push(x0 + (x1 - x0) * i as f32 / steps as f32, top);
    }
    for i in (0..=steps).rev() {
        push(x0 + (x1 - x0) * i as f32 / steps as f32, bottom);
    }
    let contours = vec![polygon];
    canvas.mask_contours_warped(&contours, 1.0, 1.0, 0.0, 0.0, 0.0, map, color, 1.0);
}

fn paint(
    faces: &FontSet<'_>,
    lines: Vec<crate::area::Line>,
    style_runs: &[RunStyle],
    spec: &LayerTextStyle,
    aa: crate::raster::AntiAlias,
    atlases: &[crate::atlas::Atlas],
    target: Option<RectTarget>,
    // Warped layers draw analytical outlines through this map instead of
    // blitting Atlas cells; `None` keeps the byte-exact atlas path.
    warp: Option<&WarpDraw<'_>>,
) -> Result<LayerPixels, Error> {
    let box_w = spec.box_size.0.max(1.0).ceil();
    let box_h = spec.box_size.1.max(1.0).ceil();
    // The canvas must clear the tallest run, a per-character vertical stretch
    // and a per-character baseline shift: any ink past the edge is dropped by
    // the rasteriser, not clipped into place afterwards.
    let size = style_runs
        .iter()
        .map(|r| r.size.unwrap_or(spec.style.size))
        .fold(spec.style.size, f32::max)
        .max(1.0);
    let vscale = style_runs
        .iter()
        .map(|r| r.vertical_scale.unwrap_or(1.0))
        .fold(1.0f32, f32::max)
        .max(1.0);
    let shift = style_runs
        .iter()
        .map(|r| r.baseline_shift.unwrap_or(0.0).abs())
        .fold(0.0f32, f32::max);
    let margin = (size * (2.0 + (vscale - 1.0)) + shift).ceil() as i32;
    // Point text aligns its lines against the type origin, so a centred or
    // right-aligned line starts at a negative pen position. Widen the canvas
    // (and move the pen origin) by the leftmost pen so no glyph is clipped.
    let (line_left, line_right) = lines.iter().fold((0.0f32, box_w), |(l, r), line| {
        line.glyphs.iter().fold((l, r), |(l, r), g| (l.min(g.x), r.max(g.x + g.advance)))
    });
    let pad_left = (-line_left).max(0.0).ceil();
    let pad_right = (line_right - box_w).max(0.0).ceil();
    let (width, height) = match target {
        Some(t) => (t.size.0.max(1), t.size.1.max(1)),
        None => (
            (((box_w + pad_left + pad_right) as i32).saturating_add(margin * 2)).max(1) as u32,
            (box_h as i32 + margin * 2).max(1) as u32,
        ),
    };
    let first_baseline = lines.first().map(|l| l.baseline).unwrap_or(0.0);
    // Device-space pen origin: `col_base + g.x` / `row_base - g.y` are the
    // canvas coordinates every glyph is drawn at.
    let (col_base, row_base) = match target {
        Some(t) => (
            t.anchor.0 - t.origin.0 as f32,
            t.anchor.1 - t.origin.1 as f32 + first_baseline,
        ),
        None => (margin as f32 + pad_left, margin as f32),
    };
    let mut canvas = Canvas::new(width, height);
    canvas.aa = std::env::var("PHOTOCRAFT_AA")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .and_then(crate::raster::AntiAlias::from_code)
        .unwrap_or(aa);
    // Temporary probe knobs (`cargo run` experiments), not part of the API.
    if let Ok(v) = std::env::var("PHOTOCRAFT_SNAP") {
        canvas.snap = v != "0";
    }
    if let Ok(v) = std::env::var("PHOTOCRAFT_SNAPX") {
        if let Ok(n) = v.parse::<f32>() {
            if n > 0.0 {
                canvas.snap_x = 1.0 / n;
            }
        }
    }
    if let Ok(v) = std::env::var("PHOTOCRAFT_COV") {
        canvas.coverage = match v.as_str() {
            "4" => crate::raster::Coverage::Scanlines(4),
            "8" => crate::raster::Coverage::Scanlines(8),
            "16" => crate::raster::Coverage::Scanlines(16),
            "area" => crate::raster::Coverage::Area,
            "g16" => crate::raster::Coverage::Grid(16),
            "g15" => crate::raster::Coverage::Grid(15),
            "g8" => crate::raster::Coverage::Grid(8),
            "g4" => crate::raster::Coverage::Grid(4),
            _ => canvas.coverage,
        };
    }
    if warp.is_some() {
        // A warped glyph's device position comes from the warp, not from a
        // quarter-pixel pen phase; snapping would quantise the outline twice.
        canvas.snap = false;
    }
    let aa_code = match canvas.aa {
        crate::raster::AntiAlias::None => 0,
        crate::raster::AntiAlias::Crisp => 1,
        crate::raster::AntiAlias::Strong => 2,
        crate::raster::AntiAlias::Smooth => 3,
        crate::raster::AntiAlias::Sharp => 4,
    };
    // Warped drawing is deferred: the canvas must be anchored on the warped
    // ink's own bounding box (Photoshop's layer rectangle *is* that box), so
    // every outline point has to be mapped once before the first fill.
    let mut warped_glyphs: Vec<WarpedGlyph> = Vec::new();
    let mut warped_decos: Vec<WarpedRule> = Vec::new();

    for line in &lines {
        // `layout_area` stores baselines *decreasing* down the box and already
        // folds each line's baseline into `GlyphPos::y`, so a glyph's screen
        // row is simply the flipped coordinate: `row_base - y`.
        let y_baseline = row_base - line.baseline;
        for (i, g) in line.glyphs.iter().enumerate() {
            let attrs = line.attrs.get(i);
            let run = attrs
                .map(|a| a.run)
                .and_then(|r| style_runs.get(r))
                .or_else(|| style_runs.first());
            let run_size = run.and_then(|r| r.size).unwrap_or(spec.style.size);
            let face = run
                .and_then(|r| r.font.as_deref())
                .and_then(|name| faces.get(name))
                .unwrap_or_else(|| faces.default_face());
            let upm = face.units_per_em() as f32;
            let scale = run_size / upm;
            // Attributes come from the shaping pass; when a layer is rendered
            // through the single-style path they are absent, so fall back to
            // the run's own values instead of silently dropping them.
            let vscale = attrs
                .map(|a| a.vertical_scale)
                .unwrap_or_else(|| run.and_then(|r| r.vertical_scale).unwrap_or(1.0));
            let hscale = run.and_then(|r| r.horizontal_scale).unwrap_or(1.0);
            // Small caps shrink the whole glyph (width and height).
            let x_scale = attrs.map(|a| a.x_scale).unwrap_or(1.0);
            let faux_italic = attrs
                .map(|a| a.faux_italic)
                .unwrap_or_else(|| run.map(|r| r.faux_italic).unwrap_or(false));
            let skew = if faux_italic {
                typography::FAUX_ITALIC_SKEW
            } else {
                0.0
            };
            let color = attrs
                .map(|a| a.color)
                .or_else(|| run.and_then(|r| r.color))
                .unwrap_or(spec.color);
            // A captured atlas holds Photoshop's own hinted bitmaps for one
            // font, size and antialias mode; nothing else can be byte-exact.
            let faux_bold = run.map(|r| r.faux_bold).unwrap_or(false);
            if warp.is_some() {
                // Photoshop bends the glyph *outlines* and rasterises them in
                // the warped space, so the atlas bitmaps (flat hinted pixels)
                // are not used at all here: they would be resampled and every
                // sheared stem would double.
                let contours = glyph_contours(face, g.gid)?;
                if contours.is_empty() {
                    continue;
                }
                warped_glyphs.push(WarpedGlyph {
                    ch: g.ch,
                    gid: g.gid,
                    contours,
                    sx: scale * hscale * x_scale,
                    sy: scale * vscale,
                    skew,
                    x: col_base + g.x,
                    baseline: row_base - g.y,
                    embolden: if faux_bold {
                        typography::faux_bold_stroke_offset(run_size)
                    } else {
                        0.0
                    },
                    color,
                });
                continue;
            }
            // CoolType's Crisp AA places a glyph on a quarter-pixel grid: the
            // bitmap is captured per phase and placed at `floor(pen x) + dx`.
            let pen_x = col_base + g.x;
            let pen_floor = pen_x.floor();
            let mut phase_q = ((pen_x - pen_floor) * 4.0).round() as i32;
            let mut phase_shift = 0;
            if phase_q >= 4 {
                // `frac` rounds up to the next pixel: use the phase-0 bitmap one
                // pixel to the right instead.
                phase_q = 0;
                phase_shift = 1;
            }
            // Synthetic small caps scale the glyph uniformly (0.8x); Photoshop
            // renders that glyph at the scaled size, so look the bitmap up at
            // `size * x_scale` rather than scaling a captured bitmap.
            let effective_size = run_size * x_scale;
            // Synthetic small caps are drawn from the capital at `size * 0.8`,
            // but Photoshop hints that capital for the *run's* size; a plain
            // capture at 20.8 pt is lighter than the stored raster. A capture
            // made with `TextCase.SMALLCAPS` at the run size carries exactly
            // those bitmaps, so it is accepted for the scaled glyphs too.
            let synthetic_scaled = x_scale < 1.0;
            let matches_meta = |a: &crate::atlas::Atlas| {
                a.font == run.and_then(|r| r.font.as_deref()).unwrap_or("")
                    && ((a.size - effective_size).abs() < 0.01
                        || (synthetic_scaled && a.small_caps && (a.size - run_size).abs() < 0.01))
                    && a.aa == aa_code
                    && a.bold == faux_bold
                    && a.italic == (skew != 0.0)
                    && (a.hscale - hscale).abs() < 0.001
                    && (vscale - x_scale).abs() < 0.001
            };
            // Character-keyed cells can only stand in for the font's nominal
            // glyph: ligatures (`fi`, `ffl`), small-caps and `onum` old-style
            // figures are different glyphs with their own outlines and must stay
            // analytic.  An atlas may instead carry `gid=` rows captured from
            // composed text, which cover exactly those substituted glyphs.
            let nominal = face.glyph_index(g.ch) == Some(g.gid);
            let has_cell = atlases
                .iter()
                .any(|a| matches_meta(a) && atlas_cell(a, g.gid, g.ch, nominal).is_some());
            // CFF (and any font whose bitmaps do not move with the fraction)
            // renders phase-independent bitmaps at an integer pen; TrueType
            // Crisp uses a quarter-pixel grid.  Detect it per glyph from the
            // captured phases themselves: identical bitmaps mean the phase
            // only moved the pen, so place at `round(pen x)` with the phase-0
            // bitmap, exactly like a legacy single atlas.
            let candidates: Vec<(&crate::atlas::Atlas, crate::atlas::GlyphRef<'_>)> = atlases
                .iter()
                .filter(|a| matches_meta(a))
                .filter_map(|a| atlas_cell(a, g.gid, g.ch, nominal).map(|e| (a, e)))
                .collect();
            let invariant = candidates.len() > 1
                && candidates.first().is_some_and(|(_, e)| {
                    candidates.iter().all(|(_, other)| {
                        other.glyph.w == e.glyph.w
                            && other.glyph.h == e.glyph.h
                            && other.alpha == e.alpha
                    })
                });
            let mut phased_entry = has_cell
                .then(|| {
                    if invariant {
                        candidates
                            .iter()
                            .filter(|(a, _)| (a.phase * 4.0).round() == 0.0)
                            .map(|(_, e)| (*e, false))
                            .next()
                    } else {
                        // The atlas whose phase matches need not carry this
                        // cell (a gid atlas is captured once, at phase 0), so
                        // look for the first one that does.
                        atlases
                            .iter()
                            .filter(|a| {
                                matches_meta(a) && ((a.phase * 4.0).round() as i32 & 3) == phase_q
                            })
                            .find_map(|a| atlas_cell(a, g.gid, g.ch, nominal).map(|e| (e, true)))
                    }
                })
                .flatten();
            if phased_entry.is_none() && candidates.len() == 1 {
                // A single atlas covering the glyph has no phase to select:
                // place its bitmap on the integer pen (legacy atlas, or a CFF
                // capture whose bitmaps are phase-independent).
                phased_entry = candidates.first().map(|(_, e)| (*e, false));
            }
            if let Some((entry, phased)) = phased_entry {
                let row = row_base - g.y;
                let x = if phased {
                    pen_floor as i32 + phase_shift + entry.glyph.dx
                } else {
                    // Legacy single atlas: Photoshop's Sharp bitmaps are
                    // position independent, so integer placement is exact.
                    (col_base + g.x).round() as i32 + entry.glyph.dx
                };
                canvas.blit_alpha(
                    x,
                    row.round() as i32 + entry.glyph.dy,
                    entry.glyph.w,
                    entry.glyph.h,
                    entry.alpha,
                    color,
                );
                continue;
            }
            let contours = glyph_contours(face, g.gid)?;
            if contours.is_empty() {
                continue;
            }
            // Faux bold thickens the mask; the atlas path above already has
            // Photoshop's own stroke baked in.
            canvas.embolden = if faux_bold {
                crate::typography::faux_bold_stroke_offset(run_size)
            } else {
                0.0
            };
            canvas.mask_contours_transformed(
                &contours,
                scale * hscale * x_scale,
                scale * vscale,
                skew,
                col_base + g.x,
                // `GlyphPos::y` carries the run's baseline shift; positive
                // values raise the glyph, so it subtracts from the screen row.
                row_base - g.y,
                color,
                1.0,
            );
        }
        // Decorations are drawn per contiguous run span on the line.
        let mut start = 0usize;
        while start < line.glyphs.len() {
            let run_index = line.attrs.get(start).map(|a| a.run).unwrap_or(0);
            let mut end = start + 1;
            while end < line.glyphs.len()
                && line.attrs.get(end).map(|a| a.run).unwrap_or(0) == run_index
            {
                end += 1;
            }
            let Some(run) = style_runs.get(run_index) else {
                start = end;
                continue;
            };
            let face = run
                .font
                .as_deref()
                .and_then(|name| faces.get(name))
                .unwrap_or_else(|| faces.default_face());
            let _upm = face.units_per_em() as f32;
            let run_size = run.size.unwrap_or(spec.style.size);
            // Decorations take the run's fill colour, like the glyphs do.
            let color = run.color.unwrap_or(spec.color);
            // Photoshop does not use the font's `post`/`OS/2` decoration
            // metrics; the measured quarter-pixel band geometry lives in
            // `typography::underline_band`/`strikeout_band` (rows 28.5..30.0
            // for layer 01, 23.25..24.5 for 07, 6.5..7.75 for 02).
            let underline = crate::typography::underline_band(run_size);
            let strikeout = crate::typography::strikeout_band(run_size);
            let first = &line.glyphs[start];
            let last = &line.glyphs[end - 1];
            let x0 = first.x + col_base;
            let x1 = last.x + last.advance + col_base;
            let y = y_baseline;
            if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "9") {
                eprintln!(
                    "deco: run={run_index} font={:?} size={run_size} underline={} strike={} x0={x0:.2} x1={x1:.2} y={y:.2} baseline_shift={:.3}",
                    run.font, run.underline, run.strikethrough, line.baseline
                );
            }
            match warp {
                // Photoshop bends a rule with the same patch as the glyphs, so
                // a warped layer's underline is a warped quad, not a bar.
                Some(_) => {
                    if run.underline {
                        warped_decos.push(WarpedRule {
                            x0,
                            x1,
                            y,
                            thickness: underline.thickness,
                            offset: underline.offset,
                            color,
                        });
                    }
                    if run.strikethrough {
                        warped_decos.push(WarpedRule {
                            x0,
                            x1,
                            y,
                            thickness: strikeout.thickness,
                            offset: strikeout.offset,
                            color,
                        });
                    }
                }
                None => {
                    if run.underline {
                        canvas.underline(
                            x0,
                            x1,
                            y,
                            underline.thickness,
                            underline.offset,
                            color,
                            1.0,
                        );
                    }
                    if run.strikethrough {
                        canvas.strikeout(
                            x0,
                            x1,
                            y,
                            strikeout.thickness,
                            strikeout.offset,
                            color,
                            1.0,
                        );
                    }
                }
            }
            start = end;
        }
    }

    if let Some(draw) = warp {
        let map = draw.map;
        // Measure the warped ink's bounding box: the anchoring rule below uses
        // it, and Photoshop's stored layer rectangle is exactly this box.
        let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
        let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
        for g in &warped_glyphs {
            for contour in &g.contours {
                for p in contour {
                    let (dx, dy) = map(
                        g.x + p.x * g.sx + g.skew * p.y * g.sy,
                        g.baseline - p.y * g.sy,
                    );
                    min_x = min_x.min(dx);
                    min_y = min_y.min(dy);
                    max_x = max_x.max(dx);
                    max_y = max_y.max(dy);
                }
            }
        }
        for r in &warped_decos {
            let top = r.y + r.offset;
            let bottom = top + r.thickness;
            for (px, py) in [(r.x0, top), (r.x1, top), (r.x0, bottom), (r.x1, bottom)] {
                let (dx, dy) = map(px, py);
                min_x = min_x.min(dx);
                min_y = min_y.min(dy);
                max_x = max_x.max(dx);
                max_y = max_y.max(dy);
            }
        }
        let (min_x, min_y, max_x, max_y) = if min_x.is_finite() && max_x.is_finite() {
            (min_x, min_y, max_x, max_y)
        } else {
            (0.0, 0.0, 1.0, 1.0)
        };
        // Photoshop's layer rectangle *is* the warped ink's bounding box, so the
        // analytical ink is fitted onto the target rectangle exactly: translate
        // the measured minimum to the canvas origin and scale the measured
        // extent to the canvas size. Without the scale a patch that bends the
        // ink 1-2 % tighter than Photoshop's would leave a blank border, and the
        // exporter (which derives the layer record from the surface's content
        // bounds) would crop the layer and shift it.
        let (fit_x, fit_y) = (
            canvas.width as f32 / (max_x - min_x).max(1.0e-3),
            canvas.height as f32 / (max_y - min_y).max(1.0e-3),
        );
        let (scale_x, scale_y, off_x, off_y) = match draw.anchor {
            WarpAnchor::Caller => (1.0, 1.0, 0.0, 0.0),
            WarpAnchor::Ink => {
                let f: f32 = std::env::var("PHOTOCRAFT_WARP_STRETCH")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0);
                (
                    1.0 + f * (fit_x - 1.0),
                    1.0 + f * (fit_y - 1.0),
                    min_x,
                    min_y,
                )
            }
            WarpAnchor::Stretch => (fit_x, fit_y, min_x, min_y),
        };
        let (min_x, min_y) = (off_x, off_y);
        if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "15") {
            eprintln!(
                "warpink: min=({min_x:.2},{min_y:.2}) max=({max_x:.2},{max_y:.2}) scale=({scale_x:.5},{scale_y:.5}) canvas={}x{}",
                canvas.width, canvas.height
            );
        }
        let shifted = |x: f32, y: f32| {
            let (dx, dy) = map(x, y);
            ((dx - min_x) * scale_x, (dy - min_y) * scale_y)
        };
        for g in &warped_glyphs {
            if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "13") {
                // Warped device bbox per glyph (after the canvas anchor): the
                // per-glyph alignment check needs the window each outline
                // occupies.
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for contour in &g.contours {
                    for p in contour {
                        let (dx, dy) = shifted(
                            g.x + p.x * g.sx + g.skew * p.y * g.sy,
                            g.baseline - p.y * g.sy,
                        );
                        x0 = x0.min(dx);
                        x1 = x1.max(dx);
                        y0 = y0.min(dy);
                        y1 = y1.max(dy);
                    }
                }
                eprintln!(
                    "warpglyph ch={:?} gid={} pen=({:.2},{:.2}) bbox=({x0:.2},{y0:.2})-({x1:.2},{y1:.2})",
                    g.ch, g.gid, g.x, g.baseline,
                );
            }
            canvas.embolden = g.embolden;
            canvas.mask_contours_warped(
                &g.contours,
                g.sx,
                g.sy,
                g.skew,
                g.x,
                g.baseline,
                &shifted,
                g.color,
                1.0,
            );
        }
        for r in &warped_decos {
            warped_bar(
                &mut canvas,
                r.x0,
                r.x1,
                r.y,
                r.thickness,
                r.offset,
                &shifted,
                r.color,
            );
        }
    }

    if let Some(t) = target {
        if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "2") {
            eprintln!(
                "paint(rect): origin={:?} size={:?} anchor=({:.3},{:.3}) first_baseline={first_baseline:.3}",
                t.origin, t.size, t.anchor.0, t.anchor.1
            );
        }
        return Ok(LayerPixels {
            width: t.size.0,
            height: t.size.1,
            rgba: canvas.pixels,
            offset: (0.0, 0.0),
        });
    }
    let (min_x, min_y, max_x, max_y) = canvas.ink_bounds();
    if max_x <= min_x || max_y <= min_y {
        return Ok(LayerPixels {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            offset: (0.0, 0.0),
        });
    }
    let out_w = (max_x - min_x) as u32;
    let out_h = (max_y - min_y) as u32;
    let mut rgba = Vec::with_capacity((out_w * out_h * 4) as usize);
    for y in min_y..max_y {
        let start = ((y * width as i32 + min_x) * 4) as usize;
        rgba.extend_from_slice(&canvas.pixels[start..start + (out_w * 4) as usize]);
    }
    // Both offsets are measured from the first baseline (x from the text
    // origin). The first baseline sits on canvas row `margin - baseline`, so
    // the ink offset adds that row back.
    let offset = (
        min_x as f32 - col_base,
        min_y as f32 - margin as f32 + first_baseline,
    );
    if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "2") {
        eprintln!(
            "paint: margin={margin} ink=({min_x},{min_y},{max_x},{max_y}) first_baseline={first_baseline:.3} offset=({:.3},{:.3})",
            offset.0, offset.1
        );
    }
    Ok(LayerPixels {
        width: out_w,
        height: out_h,
        rgba,
        offset,
    })
}

/// Separator-insensitive font-name key (`CCGrimNGritty-Regular` ==
/// `CCGrimNGritty Regular`), lowercased, alphanumerics only.
fn font_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}
