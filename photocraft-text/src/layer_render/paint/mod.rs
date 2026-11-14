//! Rasterisation and ink cropping shared by all layout directions.

mod atlas;

use atlas::GlyphQuery;

use crate::error::Error;
use crate::outline::glyph_contours;
use crate::raster::Canvas;
use crate::runs::{FontSet, RunStyle};
use crate::typography;

use super::{LayerPixels, LayerTextStyle, RectTarget};

/// Where a warped draw anchors its canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WarpAnchor {
    /// The caller already placed the map (its values are canvas coordinates).
    Caller,
    /// Translate the warped ink's own bounding box minimum to the origin.
    Ink,
    /// Translate the ink minimum to the origin and stretch its extent onto the
    /// canvas.
    Stretch,
}

/// A warped draw: the flat-canvas -> warped-plane map plus its anchoring rule.
pub(super) struct WarpDraw<'a> {
    pub(super) map: &'a dyn Fn(f32, f32) -> (f32, f32),
    pub(super) anchor: WarpAnchor,
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

/// A vertical roman glyph drawn turned 90° clockwise in its em column
/// (`BaselineDirection 2`), with the contours already turned so the plain
/// contour rasteriser can draw them.
struct RotatedGlyph {
    contours: crate::outline::Contours,
    x: f32,
    baseline: f32,
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

pub(super) fn paint(
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
        line.glyphs
            .iter()
            .fold((l, r), |(l, r), g| (l.min(g.x), r.max(g.x + g.advance)))
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
    let mut rotated_glyphs: Vec<RotatedGlyph> = Vec::new();
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
            if attrs.map(|a| a.rotate).unwrap_or(false) && warp.is_none() {
                let contours = glyph_contours(face, g.gid)?;
                if contours.is_empty() {
                    continue;
                }
                // Turn the outline 90° clockwise about the pen: a font-unit
                // point (p.x, p.y) becomes (p.y * sy, -p.x * sx), which the
                // identity-scaled contour rasteriser then places at the pen.
                let (sx, sy) = (scale * hscale * x_scale, scale * vscale);
                let turned = contours
                    .iter()
                    .map(|contour| {
                        contour
                            .iter()
                            .map(|p| crate::outline::Point {
                                x: p.x * 0.0 + p.y * sy,
                                y: -p.x * sx + p.y * 0.0,
                                on: p.on,
                            })
                            .collect()
                    })
                    .collect::<Vec<Vec<crate::outline::Point>>>();
                rotated_glyphs.push(RotatedGlyph {
                    contours: turned,
                    x: col_base + g.x,
                    baseline: row_base - g.y,
                    color,
                });
                continue;
            }
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
            let query = GlyphQuery {
                font: run.and_then(|r| r.font.as_deref()).unwrap_or(""),
                size: run_size,
                aa: aa_code,
                bold: faux_bold,
                italic: skew != 0.0,
                horizontal_scale: hscale,
                vertical_scale: vscale,
                cap_scale: x_scale,
                phase: phase_q,
                gid: g.gid,
                ch: g.ch,
                nominal: !atlases.is_empty() && face.glyph_index(g.ch) == Some(g.gid),
            };
            let phased_entry = query.select(atlases);
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

    for g in &rotated_glyphs {
        // The contours are already turned and scaled into font units; the
        // identity map keeps the flat placement.
        let identity = |x: f32, y: f32| (x, y);
        canvas.mask_contours_warped(
            &g.contours,
            1.0,
            1.0,
            0.0,
            g.x,
            g.baseline,
            &identity,
            g.color,
            1.0,
        );
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
