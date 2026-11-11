//! Rendering a text layer's pixels.
//!
//! A Photoshop type layer stores both the editable text (`TySh`/`Txt2`) and a
//! rasterised copy of the glyphs in the layer's pixel channels. Apps without a
//! text engine show that copy, and Photoshop uses it until it re-renders the
//! layer. When we rewrite the text we must rewrite the pixels too, otherwise the
//! file shows the old words in every viewer that does not re-lay-out text.

use std::path::{Path, PathBuf};

use crate::area::{first_baseline_ratio, layout_area, layout_area_runs, ParagraphStyle};
use crate::engine_data::Value;
use crate::error::Error;
use crate::font::Face;
use crate::layout::{Kerning, TextStyle};
use crate::outline::glyph_contours;
use crate::raster::Canvas;
use crate::runs::{self, FontSet, RunStyle};
use crate::typography;

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
    /// Photoshop-captured glyph bitmaps; when set, glyphs it covers skip the
    /// outline rasteriser and come out byte-identical to Photoshop's.
    atlas: Option<crate::atlas::Atlas>,
}

impl FaceStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches a glyph atlas (see [`crate::atlas`]).
    pub fn set_atlas(&mut self, atlas: crate::atlas::Atlas) {
        self.atlas = Some(atlas);
    }

    /// The attached glyph atlas, if any.
    pub fn atlas(&self) -> Option<&crate::atlas::Atlas> {
        self.atlas.as_ref()
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
                .atlas
                .as_ref()
                .map(|a| a.font.clone())
                .filter(|n| self.bytes(n).is_some())
                .or_else(|| self.first_usable_name())
                .unwrap_or_default();
            self.font_set(&[name])?
        } else {
            self.font_set(&wanted)?
        };
        let atlas = if options.use_atlas { self.atlas() } else { None };
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
        render_text_layer_with_atlas(&faces, text, runs, &spec, atlas, options.aa)
    }

    /// Loads every `.ttf`/`.otf` in `dir`, registering the file stem as the name.
    pub fn load_dir(&mut self, dir: &Path) -> usize {
        let Ok(read) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut loaded = 0;
        for entry in read.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
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
        Face::parse(&bytes)?;
        self.entries.push((name.to_string(), bytes, path.to_path_buf()));
        Ok(())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.iter().any(|(n, _, _)| n == name)
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
            let Some(bytes) = self.bytes(name) else { return false };
            let Ok(face) = Face::parse(bytes) else { return false };
            let Some(gid) = face.glyph_index('A') else { return false };
            crate::outline::glyph_contours(&face, gid)
                .map(|c| !c.is_empty())
                .unwrap_or(false)
        })
    }

    fn bytes(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, b, _)| b.as_slice())
    }

    /// Builds a `FontSet` whose default face is the first name that loads.
    /// Panics never: an empty store is an error.
    fn font_set(&self, wanted: &[String]) -> Result<FontSet<'_>, Error> {
        let first = wanted.iter().find(|n| self.bytes(n).is_some()).ok_or(Error::BadFont(
            "no run font available in the store",
        ))?;
        let mut set = FontSet::new(first.as_str(), Face::parse(self.bytes(first).ok_or(
            Error::BadFont("font vanished from the store"),
        )?)?);
        for name in wanted {
            if name == first {
                continue;
            }
            if let Some(bytes) = self.bytes(name) {
                set.insert(name.as_str(), Face::parse(bytes)?);
            }
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

fn paragraph_style_of(runs: &[runs::ParagraphRun]) -> ParagraphStyle {
    let Some(first) = runs.first() else {
        return ParagraphStyle::default();
    };
    ParagraphStyle {
        justification: first.justification,
        first_line_indent: first.first_line_indent,
        start_indent: first.start_indent,
        end_indent: first.end_indent,
        space_before: first.space_before,
        space_after: first.space_after,
        word_spacing: first.word_spacing,
        letter_spacing: first.letter_spacing,
        glyph_spacing: first.glyph_scaling,
        auto_leading: first.auto_leading,
        every_line_composer: first.every_line_composer,
        ..Default::default()
    }
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

/// As [`render_text_layer`], with an optional captured glyph atlas and
/// antialias override.
pub fn render_text_layer_with_atlas(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    spec: &LayerTextStyle,
    atlas: Option<&crate::atlas::Atlas>,
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
        atlas,
    )
}

/// Lays out a layer straight from its `EngineData` and rasterises it.
///
/// This is the path used when rewriting a PSD: the run fonts are resolved
/// through the layer's own `FontSet` names.
pub fn render_from_engine_data(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    box_size: (f32, f32),
    origin: (f32, f32),
) -> Result<LayerPixels, Error> {
    let style_runs = runs::from_engine_data(ed);
    let aa = runs::antialias(ed);
    let paragraphs = runs::paragraphs_from_engine_data(ed);
    if style_runs.is_empty() {
        return Err(Error::EngineData("no style runs"));
    }
    let mut wanted: Vec<String> = style_runs
        .iter()
        .filter_map(|r| r.font.clone())
        .collect();
    wanted.sort();
    wanted.dedup();
    let faces = store.font_set(&wanted)?;

    let base = TextStyle {
        size: style_runs[0].size.unwrap_or(12.0),
        ..Default::default()
    };
    let style = text_style_of(&style_runs[0], &base);
    let para = paragraph_style_of(&paragraphs);
    let primary = style_runs[0]
        .font
        .as_deref()
        .and_then(|n| faces.get(n))
        .unwrap_or_else(|| faces.default_face());
    let ascender = first_baseline_ratio(primary);

    // Point text has no frame: measure it unwrapped and size the canvas to the
    // laid-out lines instead of to the layer's old ink rectangle.
    let box_size = if box_size.0 > 0.0 && box_size.1 > 0.0 {
        box_size
    } else {
        // Measure flush left: alignment against a 1e7 pt frame would push a
        // centred line millions of points right and the canvas could not hold it.
        let mut probe_para = para.clone();
        probe_para.justification = crate::area::Justification::Left;
        probe_para.first_line_indent = 0.0;
        probe_para.start_indent = 0.0;
        probe_para.end_indent = 0.0;
        let probe = layout_area_runs(
            &faces, text, &style_runs, &style, &probe_para, 1.0e7, 1.0e7, origin, ascender,
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

    let layout = layout_area_runs(
        &faces, text, &style_runs, &style, &para, box_size.0, box_size.1, origin, ascender,
    );
    let spec = LayerTextStyle {
        style,
        paragraph: para,
        box_size,
        origin,
        color: style_runs[0].color.unwrap_or([0, 0, 0]),
    };
    paint(&faces, layout.lines, &style_runs, &spec, aa, store.atlas())
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
    Some(crate::area::first_baseline_offset(first_baseline_ratio(&face), size))
}

/// Rasterises laid-out lines into a canvas cropped to the ink.
fn paint(
    faces: &FontSet<'_>,
    lines: Vec<crate::area::Line>,
    style_runs: &[RunStyle],
    spec: &LayerTextStyle,
    aa: crate::raster::AntiAlias,
    atlas: Option<&crate::atlas::Atlas>,
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
    let width = (box_w as i32 + margin * 2).max(1) as u32;
    let height = (box_h as i32 + margin * 2).max(1) as u32;
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
    let aa_code = match canvas.aa {
        crate::raster::AntiAlias::None => 0,
        crate::raster::AntiAlias::Crisp => 1,
        crate::raster::AntiAlias::Strong => 2,
        crate::raster::AntiAlias::Smooth => 3,
        crate::raster::AntiAlias::Sharp => 4,
    };

    for line in &lines {
        // `layout_area` stores baselines *decreasing* down the box and already
        // folds each line's baseline into `GlyphPos::y`, so a glyph's screen
        // row is simply the flipped coordinate: `margin - y`.
        let y_baseline = margin as f32 - line.baseline;
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
            let vscale = attrs.map(|a| a.vertical_scale).unwrap_or_else(|| {
                run.and_then(|r| r.vertical_scale).unwrap_or(1.0)
            });
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
            let atlas_glyph = atlas.filter(|a| {
                a.font == run.and_then(|r| r.font.as_deref()).unwrap_or("")
                    && (a.size - run_size).abs() < 0.01
                    && a.aa == aa_code
                    && a.bold == faux_bold
                    && a.italic == (skew != 0.0)
                    && (a.hscale - hscale).abs() < 0.001
                    && vscale == 1.0
                    && x_scale == 1.0
            });
            if let Some(entry) = atlas_glyph.and_then(|a| a.glyph(g.ch)) {
                let row = margin as f32 - g.y;
                canvas.blit_alpha(
                    margin + g.x.round() as i32 + entry.glyph.dx,
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
                g.x + margin as f32,
                // `GlyphPos::y` carries the run's baseline shift; positive
                // values raise the glyph, so it subtracts from the screen row.
                margin as f32 - g.y,
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
            let upm = face.units_per_em() as f32;
            let run_size = run.size.unwrap_or(spec.style.size);
            // Decorations take the run's fill colour, like the glyphs do.
            let color = run.color.unwrap_or(spec.color);
            let first = &line.glyphs[start];
            let last = &line.glyphs[end - 1];
            let x0 = first.x + margin as f32;
            let x1 = last.x + last.advance + margin as f32;
            let y = y_baseline;
            if run.underline {
                if let Some(d) = face
                    .table(*b"post")
                    .and_then(|post| typography::underline_geometry(post, run_size, upm as u16))
                {
                    // `post.underlinePosition` is negative below the baseline;
                    // the bar takes the opposite sign in screen rows.
                    canvas.underline(x0, x1, y, d.thickness, -d.offset, color, 1.0);
                }
            }
            if run.strikethrough {
                if let Some(d) = face
                    .table(*b"OS/2")
                    .and_then(|os2| typography::strikeout_geometry(os2, run_size, upm as u16))
                {
                    let offset = -(d.offset + d.thickness);
                    canvas.strikeout(x0, x1, y, d.thickness, offset, color, 1.0);
                }
            }
            start = end;
        }
    }

    let (min_x, min_y, max_x, max_y) = canvas.ink_bounds();
    if max_x <= min_x || max_y <= min_y {
        return Ok(LayerPixels { width: 0, height: 0, rgba: Vec::new(), offset: (0.0, 0.0) });
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
    let first_baseline = lines.first().map(|l| l.baseline).unwrap_or(0.0);
    let offset = (
        min_x as f32 - margin as f32,
        min_y as f32 - margin as f32 + first_baseline,
    );
    Ok(LayerPixels { width: out_w, height: out_h, rgba, offset })
}
