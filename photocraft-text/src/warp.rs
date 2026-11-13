//! Photoshop's text warp (`TySh` warp descriptor).
//!
//! Photoshop stores the warp only in `TySh`; the `Txt2` cache holds the flat
//! layout. `10_all_features.psd` has the two styles the catalogue measures
//! (`warpArc` 50 %, `warpArch` 50 %), and Photoshop bends both with a bicubic
//! (4x4 control grid) patch over the flat layout box — not with a circular arc.
//! A cylinder compresses the warped ink's width and rotates the glyphs by the
//! arc angle, which plateaued below the patch model. Each style therefore has
//! its own fitted control grid ([`ARC_PATCH`], [`ARCH_PATCH`]), fitted to that
//! layer's stored raster; other bend percentages interpolate every control
//! point from the identity grid (`(i/3, j/3)`) by `value / 50`.
//!
//! Two renderers take that grid:
//!
//! * [`apply`] resamples the flat *bitmap* through the inverse map. It is what
//!   the raster warp was originally fitted with, and it is still the fallback
//!   for styles without a fitted grid.
//! * The analytical path in [`crate::layer_render`] warps the glyph *outlines*
//!   through [`Placement::forward`] and rasterises them with the canvas AA.
//!   That is what Photoshop itself does, and it is the default for warped
//!   layers: a flat hinted bitmap carries the unbent stem pattern, which the
//!   resampler smears across two device columns wherever the warp shears the
//!   stem (visible as doubled/greyed arches). Outlines have no such pattern.
//!
//! [`placement`] computes the flat-ink-bbox → layer-rectangle anchor both
//! renderers share; it is derived from the flat raster's ink bounds pushed
//! through the forward patch, which is how Photoshop sizes the layer record.
//!
//! Measured on `10_all_features.psd` (`tools/alpha_iou.py`, alpha>127 IoU and
//! RGB-diff>8 pixels): layer `12` (arc) outlines 0.9323 / 1112 versus 0.8646 /
//! 2127 for the bitmap resampler; layer `13` (arch) 0.8040 / 1505 versus
//! 0.7760 / 2048. Layer `13`'s number includes [`fill_border`]: the outline
//! render itself is 1492, but its ink stops a few pixels short of the stored
//! rectangle, and the exporter derives the layer record from the surface's
//! content bounds, so the border continuation keeps the record equal to the
//! rectangle instead of cropping the layer. The residual is Photoshop hinting
//! the warped stems (its warped raster is ~11 % heavier), which an unhinted
//! outline renderer cannot reproduce without thickening every stroke.
//! [MEASURED; other bend values and warp styles are extrapolations of these
//! two grids.]
//!
//! `PHOTOCRAFT_WARP_PATCH` (32 floats: the row-major control grid) overrides
//! the fitted grids for refitting experiments; `PHOTOCRAFT_WARP_RESAMPLE`
//! restores the bitmap path for comparison. Both are probe hooks, not API.

/// Warp style, restricted to the styles the catalogue measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    None,
    Arc,
    Arch,
    Other,
}

impl Style {
    /// Parses the `TySh` style name (`warpArc`, `warpArch`, ...).
    pub fn from_name(name: &str) -> Self {
        match name {
            "warpNone" | "" => Style::None,
            "warpArc" => Style::Arc,
            "warpArch" => Style::Arch,
            _ => Style::Other,
        }
    }
}

/// The parameters of one layer's warp, in Photoshop's units.
#[derive(Clone, Copy, Debug)]
pub struct Warp {
    pub style: Style,
    /// Photoshop's "Bend" percentage (`warpValue`).
    pub value: f32,
    /// Horizontal distortion (`warpPerspective`), percent.
    pub perspective: f32,
    /// Vertical distortion (`warpPerspectiveOther`), percent.
    pub other: f32,
}

impl Default for Warp {
    fn default() -> Self {
        Warp {
            style: Style::None,
            value: 0.0,
            perspective: 0.0,
            other: 0.0,
        }
    }
}

impl Warp {
    /// `true` when a warp other than `warpNone` is applied.
    pub fn is_warped(&self) -> bool {
        self.style != Style::None
    }
}

/// A flat RGBA surface and the document rectangle it was painted into.
#[derive(Clone, Copy, Debug)]
pub struct FlatSurface {
    pub origin: (i32, i32),
    pub width: u32,
    pub height: u32,
}

/// Geometry of the flat text: the layout box and the baseline, in document
/// points (the same space as `FlatSurface::origin`).
#[derive(Clone, Copy, Debug)]
pub struct FlatGeometry {
    /// `(left, top, right, bottom)` of the flat layout box.
    pub bounds: (f32, f32, f32, f32),
    /// Document y of the flat first baseline.
    pub baseline: f32,
}

/// Flat-box-normalized 4x4 Bézier control grid for `warpArc` at 50 % bend.
///
/// Photoshop bends warped type with a bicubic patch rather than a circular
/// arc: the grid below was fitted to the stored raster of layer `12`
/// (`warpArc`, 50 %) by coordinate descent on the alpha L1 distance through
/// the same inverse-sampled rasteriser [`apply`] uses. Coordinates are
/// `(u, v)` of the flat layout box (`u = (x - left) / width`,
/// `v = (y - top) / height`, `v` increasing downward). The warped ink's
/// bounding box is translated onto the layer rectangle by [`apply`], so the
/// grid carries no absolute placement. Other bend percentages interpolate
/// each control point from the identity grid (`(i/3, j/3)`) by `value / 50`.
#[rustfmt::skip]
const ARC_PATCH: [[[f32; 2]; 4]; 4] = [
    [[-0.13648, 1.31338], [-0.06448, 1.69142], [-0.06452, 1.74467], [0.01816, 2.19446]],
    [[0.22029, -0.54010], [0.21285, -0.16439], [0.26875, 0.15676], [0.26980, 0.64172]],
    [[0.77315, -0.54635], [0.78965, -0.16283], [0.73125, 0.16113], [0.73145, 0.61797]],
    [[1.13898, 1.31963], [1.06698, 1.70267], [1.06452, 1.73342], [0.98153, 2.17508]],
];

/// Flat-box-normalized 4x4 Bézier control grid for `warpArch` at 50 % bend.
///
/// Fitted the same way to layer `13` of `10_all_features.psd`. The arch keeps
/// the flat box's width (its stored ink is exactly the flat width) while the
/// glyphs stay nearly upright, which a cylindrical bend cannot represent.
#[rustfmt::skip]
const ARCH_PATCH: [[[f32; 2]; 4]; 4] = [
    [[0.06143, 0.04592], [-0.01420, 0.82539], [-0.04604, 0.55520], [0.07901, 1.67470]],
    [[0.09797, -1.23404], [0.37947, -1.12739], [0.28446, -0.68183], [0.18704, -0.37000]],
    [[0.90328, -1.23404], [0.62053, -1.13989], [0.71242, -0.67995], [0.81296, -0.35625]],
    [[0.93013, 0.06842], [1.01545, 0.82476], [1.04604, 0.54145], [0.92099, 1.73220]],
];

/// The bend percentage both fitted grids were measured at.
const FITTED_BEND: f32 = 50.0;

/// Control grid for a warped layer: the style's fitted grid interpolated
/// from the identity grid by the layer's bend percentage. `None` for
/// unwarped layers and styles without a fitted grid.
/// The control grid for a warped layer: the style's fitted grid interpolated
/// from the identity grid by the layer's bend percentage. `None` for unwarped
/// layers and styles without a fitted grid.
pub fn patch_for(warp: &Warp) -> Option<[[[f32; 2]; 4]; 4]> {
    patch_grid(warp)
}

/// Forward map: a document point in the flat layout box -> its warped document
/// point. This is what an analytical outline renderer pushes every glyph point
/// through.
pub fn forward(
    patch: &[[[f32; 2]; 4]; 4],
    bounds: (f32, f32, f32, f32),
    x: f32,
    y: f32,
) -> (f32, f32) {
    let (left, top, right, bottom) = bounds;
    let u = (x - left) / (right - left).max(1.0);
    let v = (y - top) / (bottom - top).max(1.0);
    patch_point(patch, bounds, u, v)
}

fn patch_grid(warp: &Warp) -> Option<[[[f32; 2]; 4]; 4]> {
    if warp.value.abs() < 1e-3 {
        return None;
    }
    // Probe hook for re-fitting the control grid against a stored raster:
    // `PHOTOCRAFT_WARP_PATCH` names a text file with 32 floats (row-major
    // `(u, v)` pairs) used verbatim as the grid. Not part of the public API;
    // the checked-in constants are the measured grids.
    if let Some(path) = std::env::var_os("PHOTOCRAFT_WARP_PATCH") {
        if let Ok(text) = std::fs::read_to_string(path) {
            let values: Vec<f32> = text
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if values.len() == 32 {
                let mut grid = [[[0.0f32; 2]; 4]; 4];
                for i in 0..4 {
                    for j in 0..4 {
                        grid[i][j] = [values[(i * 4 + j) * 2], values[(i * 4 + j) * 2 + 1]];
                    }
                }
                return Some(grid);
            }
        }
    }
    let mut grid = match warp.style {
        Style::Arc => ARC_PATCH,
        Style::Arch => ARCH_PATCH,
        _ => return None,
    };
    let k = warp.value / FITTED_BEND;
    for (i, row) in grid.iter_mut().enumerate() {
        for (j, point) in row.iter_mut().enumerate() {
            let identity = [i as f32 / 3.0, j as f32 / 3.0];
            point[0] = identity[0] + (point[0] - identity[0]) * k;
            point[1] = identity[1] + (point[1] - identity[1]) * k;
        }
    }
    Some(grid)
}

fn bezier(t: f32) -> [f32; 4] {
    let u = 1.0 - t;
    [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t]
}

fn bezier_derivative(t: f32) -> [f32; 4] {
    let u = 1.0 - t;
    [
        -3.0 * u * u,
        3.0 * u * u - 6.0 * u * t,
        6.0 * u * t - 3.0 * t * t,
        3.0 * t * t,
    ]
}

/// Maps flat-box-normalized `(u, v)` through the patch to document points.
fn patch_point(
    grid: &[[[f32; 2]; 4]; 4],
    bounds: (f32, f32, f32, f32),
    u: f32,
    v: f32,
) -> (f32, f32) {
    let (left, top, right, bottom) = bounds;
    let width = right - left;
    let height = bottom - top;
    let bu = bezier(u);
    let bv = bezier(v);
    let mut x = 0.0;
    let mut y = 0.0;
    for i in 0..4 {
        for j in 0..4 {
            let w = bu[i] * bv[j];
            x += w * grid[i][j][0];
            y += w * grid[i][j][1];
        }
    }
    (left + width * x, top + height * y)
}

/// Inverse of [`patch_point`]: flat-box-normalized `(u, v)` for a document
/// point. Newton from the box mapping; warped text never folds, so a handful
/// of iterations is enough.
fn patch_uv(
    grid: &[[[f32; 2]; 4]; 4],
    bounds: (f32, f32, f32, f32),
    px: f32,
    py: f32,
) -> (f32, f32) {
    let (left, top, right, bottom) = bounds;
    let width = right - left;
    let height = bottom - top;
    let mut u = (px - left) / width;
    let mut v = (py - top) / height;
    for _ in 0..10 {
        let bu = bezier(u);
        let bv = bezier(v);
        let du = bezier_derivative(u);
        let dv = bezier_derivative(v);
        let (mut cx, mut cy, mut xu, mut yu, mut xv, mut yv) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, row) in grid.iter().enumerate() {
            for (j, point) in row.iter().enumerate() {
                let w = bu[i] * bv[j];
                cx += w * point[0];
                cy += w * point[1];
                xu += du[i] * bv[j] * point[0];
                yu += du[i] * bv[j] * point[1];
                xv += bu[i] * dv[j] * point[0];
                yv += bu[i] * dv[j] * point[1];
            }
        }
        let target_x = (px - left) / width;
        let target_y = (py - top) / height;
        let rx = cx - target_x;
        let ry = cy - target_y;
        let det = xu * yv - xv * yu;
        if det.abs() < 1e-8 {
            break;
        }
        let step_u = (rx * yv - ry * xv) / det;
        let step_v = (ry * xu - rx * yu) / det;
        u = (u - step_u).clamp(-0.5, 1.5);
        v = (v - step_v).clamp(-0.5, 1.5);
        if step_u.abs() < 1e-5 && step_v.abs() < 1e-5 {
            break;
        }
    }
    (u, v)
}

/// Jacobian determinant of [`patch_point`] at flat-box-normalized `(u, v)`.
/// A non-injective grid folds where the determinant hits zero; those regions
/// are outside the warped quad and must stay transparent.
fn patch_det(grid: &[[[f32; 2]; 4]; 4], u: f32, v: f32) -> f32 {
    let bu = bezier(u);
    let bv = bezier(v);
    let du = bezier_derivative(u);
    let dv = bezier_derivative(v);
    let (mut xu, mut yu, mut xv, mut yv) = (0.0, 0.0, 0.0, 0.0);
    for (i, row) in grid.iter().enumerate() {
        for (j, point) in row.iter().enumerate() {
            xu += du[i] * bv[j] * point[0];
            yu += du[i] * bv[j] * point[1];
            xv += bu[i] * dv[j] * point[0];
            yv += bu[i] * dv[j] * point[1];
        }
    }
    xu * yv - xv * yu
}

/// Where a warped layer lands: the fitted control grid (flat-box normalized)
/// and the document point that maps to the output surface's top-left corner.
///
/// [`placement`] derives it exactly the way [`apply`] does — from the flat
/// raster's ink bounds pushed through the forward patch — so an analytic
/// outline renderer and the bitmap resampler put the warped ink in the same
/// place and the stored layer rectangle stays flush.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    /// 4x4 Bézier control grid over the flat layout box.
    pub patch: [[[f32; 2]; 4]; 4],
    /// Document point `(x, y)` that maps to the output surface's origin:
    /// a warped document point `p` lands at colour-buffer coordinate
    /// `p - min` (pixel centres at `floor(i) + 0.5`).
    pub min: (f32, f32),
}

impl Placement {
    /// Forward map: document point in the flat layout box -> document point in
    /// the warped plane.
    pub fn forward(&self, geom: FlatGeometry, x: f32, y: f32) -> (f32, f32) {
        let (left, top, right, bottom) = geom.bounds;
        let width = (right - left).max(1.0);
        let height = (bottom - top).max(1.0);
        let u = (x - left) / width;
        let v = (y - top) / height;
        patch_point(&self.patch, geom.bounds, u, v)
    }
}

/// Computes the warped placement of a flat raster, or `None` when the style has
/// no fitted patch (nothing to bend).
pub fn placement(
    flat: &[u8],
    flat_geo: FlatSurface,
    geom: FlatGeometry,
    warp: &Warp,
) -> Option<Placement> {
    let patch = patch_grid(warp)?;
    let (bx0, bt, bx1, bb) = geom.bounds;
    let width = (bx1 - bx0).max(1.0);
    let height = (bb - bt).max(1.0);
    let (fx0, fy0, fx1, fy1) = ink_bounds(flat, flat_geo);
    if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "14") {
        eprintln!(
            "placement: ink=({fx0:.2},{fy0:.2})-({fx1:.2},{fy1:.2}) bounds={:?}",
            geom.bounds
        );
    }
    let forward = |x: f32, y: f32| -> (f32, f32) {
        let u = (x - bx0) / width;
        let v = (y - bt) / height;
        patch_point(&patch, geom.bounds, u, v)
    };
    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
    let mut sample = |x: f32, y: f32| {
        let (px, py) = forward(x, y);
        min_x = min_x.min(px);
        min_y = min_y.min(py);
    };
    const STEPS: i32 = 48;
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let x = fx0 + (fx1 - fx0) * t;
        sample(x, fy0);
        sample(x, fy1);
        let y = fy0 + (fy1 - fy0) * t;
        sample(fx0, y);
        sample(fx1, y);
    }
    if !(min_x.is_finite() && min_y.is_finite()) {
        return None;
    }
    Some(Placement {
        patch,
        min: (min_x, min_y),
    })
}

/// Warps `flat` into a new surface of `out_size` whose top-left is
/// `out_origin` (document points).  Returns straight-alpha RGBA.
pub fn apply(
    flat: &[u8],
    flat_geo: FlatSurface,
    geom: FlatGeometry,
    warp: &Warp,
    out_origin: (i32, i32),
    out_size: (u32, u32),
) -> Vec<u8> {
    let mut out = vec![0u8; (out_size.0 * out_size.1 * 4) as usize];
    let Some(patch) = patch_grid(warp) else {
        // No warp: copy the overlapping part.
        copy_overlap(&mut out, out_size, out_origin, flat, flat_geo);
        return out;
    };
    if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "8") {
        eprintln!(
            "warp: patch flat={:?} geom={:?}",
            (flat_geo.origin, flat_geo.width, flat_geo.height),
            geom
        );
    }
    let (bx0, bt, bx1, bb) = geom.bounds;
    let width = (bx1 - bx0).max(1.0);
    let height = (bb - bt).max(1.0);
    // Warped ink bounds from the flat ink bounds (alpha > 0), sampled on the
    // box border so the layer rectangle hugs the warped ink like Photoshop's.
    let (fx0, fy0, fx1, fy1) = ink_bounds(flat, flat_geo);
    if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "14") {
        eprintln!(
            "placement: ink=({fx0:.2},{fy0:.2})-({fx1:.2},{fy1:.2}) bounds={:?}",
            geom.bounds
        );
    }
    let forward = |x: f32, y: f32| -> (f32, f32) {
        let u = (x - bx0) / width;
        let v = (y - bt) / height;
        patch_point(&patch, geom.bounds, u, v)
    };
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut sample = |x: f32, y: f32| {
        let (px, py) = forward(x, y);
        min_x = min_x.min(px);
        max_x = max_x.max(px);
        min_y = min_y.min(py);
        max_y = max_y.max(py);
    };
    const STEPS: i32 = 48;
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let x = fx0 + (fx1 - fx0) * t;
        sample(x, fy0);
        sample(x, fy1);
        let y = fy0 + (fy1 - fy0) * t;
        sample(fx0, y);
        sample(fx1, y);
    }
    if !(min_x.is_finite() && min_y.is_finite()) {
        return out;
    }
    // Place the warped ink at the output rectangle, rounding like Photoshop's
    // stored layer bounds.
    let trans = (out_origin.0 as f32 - min_x, out_origin.1 as f32 - min_y);
    if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "8") {
        eprintln!(
            "warp: ink=({fx0:.1},{fy0:.1})-({fx1:.1},{fy1:.1}) mapped=({min_x:.1},{min_y:.1})-({max_x:.1},{max_y:.1}) trans=({:.1},{:.1})",
            trans.0, trans.1
        );
    }
    let fw = flat_geo.width as f32;
    let fh = flat_geo.height as f32;
    // `(offset x, offset y, source span x, source span y)`: the output pixel
    // centres walk this window of the warped ink plane. Photoshop stores the
    // layer rectangle as the warped ink's bounding box, so the window is
    // Pixel-centre sampling leaves the warped ink's outermost half pixel
    // outside the surface (alpha rounds it away), so the rendered content can
    // come out a pixel smaller than the analytic bounding box; `fill_to_rect`
    // extends the remaining border to the stored layer rectangle below.
    let bias = 0.5f32;
    for oy in 0..out_size.1 {
        for ox in 0..out_size.0 {
            let out_x = out_origin.0 as f32 - trans.0 + ox as f32 + bias;
            let out_y = out_origin.1 as f32 - trans.1 + oy as f32 + bias;
            let (px, py) = (out_x, out_y);
            let (u, v) = patch_uv(&patch, geom.bounds, px, py);
            // A cubic patch is only injective on its intended domain: outside
            // the warped quad the inverse can fold back to a valid `uv` and
            // sample glyph ink, which shows up as 1-3 px specks at the arc
            // ends. Require the inverse to land on the flat box, keep a
            // non-degenerate (non-folded) Jacobian, and check that mapping the
            // result forward returns the requested pixel.
            const DOMAIN_EPS: f32 = 1.0e-3;
            if !(-DOMAIN_EPS..=1.0 + DOMAIN_EPS).contains(&u)
                || !(-DOMAIN_EPS..=1.0 + DOMAIN_EPS).contains(&v)
            {
                continue;
            }
            if patch_det(&patch, u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)) <= 1.0e-6 {
                continue;
            }
            let (fx, fy) = patch_point(&patch, geom.bounds, u, v);
            if (fx - px).abs() > 0.75 || (fy - py).abs() > 0.75 {
                continue;
            }
            let (x, y) = (bx0 + u * width, bt + v * height);
            // The outermost output pixels map onto (or a fraction outside) the
            // flat surface's own edge. Clamp those samples to the border
            // instead of dropping them: Photoshop's stored raster has ink in
            // its edge row and columns.
            let raw_x = x - flat_geo.origin.0 as f32 - 0.5;
            let raw_y = y - flat_geo.origin.1 as f32 - 0.5;
            let clamped = raw_x < 0.0 || raw_y < 0.0 || raw_x > fw - 1.001 || raw_y > fh - 1.001;
            let sx = raw_x.clamp(0.0, fw - 1.001);
            let sy = raw_y.clamp(0.0, fh - 1.001);
            let ix = sx.floor() as usize;
            let iy = sy.floor() as usize;
            let (ax, ay) = if clamped {
                // There is no half-pixel neighbourhood past the flat surface's
                // edge; take the nearest pixel's own coverage instead of fading
                // the border to transparent.
                (0.0, 0.0)
            } else {
                (sx - ix as f32, sy - iy as f32)
            };
            let at = |i: usize, j: usize| -> [f32; 4] {
                let p = (j * flat_geo.width as usize + i) * 4;
                [
                    flat[p] as f32,
                    flat[p + 1] as f32,
                    flat[p + 2] as f32,
                    flat[p + 3] as f32,
                ]
            };
            let (a, b, c, d) = (
                at(ix, iy),
                at(ix + 1, iy),
                at(ix, iy + 1),
                at(ix + 1, iy + 1),
            );
            let mut px4 = [0f32; 4];
            for k in 0..4 {
                px4[k] = a[k] * (1.0 - ax) * (1.0 - ay)
                    + b[k] * ax * (1.0 - ay)
                    + c[k] * (1.0 - ax) * ay
                    + d[k] * ax * ay;
            }
            let p = ((oy as usize * out_size.0 as usize) + ox as usize) * 4;
            out[p] = px4[0].round().clamp(0.0, 255.0) as u8;
            out[p + 1] = px4[1].round().clamp(0.0, 255.0) as u8;
            out[p + 2] = px4[2].round().clamp(0.0, 255.0) as u8;
            out[p + 3] = px4[3].round().clamp(0.0, 255.0) as u8;
        }
    }
    fill_border(&mut out, out_size);
    out
}

/// Extends the warped ink to the surface edges by repeating the nearest painted
/// row/column.
///
/// The layer rectangle is Photoshop's warped-ink bounding box, and its stored
/// raster has ink in the edge row and columns; the fitted bicubic patch leaves
/// their pixel centres just outside the flat surface, so the render comes out a
/// row and a column or two short. Since the exporter derives the layer record
/// from the surface's content bounds, a blank border would crop the layer and
/// shift it by a pixel — the edge continuation keeps the record equal to the
/// stored rectangle. [MEASURED on `10_all_features.psd` layer `13`: Photoshop's
/// row 0 alpha sum 1263, column 0 sum 32, last column 1040; the patch render
/// starts at row 1, column 2.)
pub fn fill_border(out: &mut [u8], out_size: (u32, u32)) {
    let (w, h) = (out_size.0 as usize, out_size.1 as usize);
    if w == 0 || h == 0 {
        return;
    }
    let alpha = |x: usize, y: usize| out[(y * w + x) * 4 + 3];
    let first_row = (0..h).find(|y| (0..w).any(|x| alpha(x, *y) > 0));
    let last_row = (0..h).rev().find(|y| (0..w).any(|x| alpha(x, *y) > 0));
    let first_col = (0..w).find(|x| (0..h).any(|y| alpha(*x, y) > 0));
    let last_col = (0..w).rev().find(|x| (0..h).any(|y| alpha(*x, y) > 0));
    // The outermost row/column of a Photoshop layer rectangle carries the
    // raster's own antialiasing, so the continuation fades towards the border
    // instead of repeating the edge pixel at full weight.
    if let Some(first) = first_row {
        let k = first + 1;
        for y in (0..first).rev() {
            let weight = (first - y) as f32 / k as f32;
            let src = (y + 1) * w * 4;
            let dst = y * w * 4;
            out.copy_within(src..src + w * 4, dst);
            for x in 0..w {
                let i = dst + x * 4 + 3;
                let a = out[i];
                if a > 0 {
                    // Keep at least one level of coverage: the exporter derives
                    // the layer record from the surface's content bounds, so a
                    // fully faded border pixel would crop the layer.
                    out[i] = (((a as f32) * weight) as u8).max(1);
                }
            }
        }
    }
    if let Some(last) = last_row {
        let k = h - last;
        for y in last + 1..h {
            let weight = (last + k - y) as f32 / k as f32;
            let src = (y - 1) * w * 4;
            let dst = y * w * 4;
            out.copy_within(src..src + w * 4, dst);
            for x in 0..w {
                let i = dst + x * 4 + 3;
                let a = out[i];
                if a > 0 {
                    out[i] = (((a as f32) * weight) as u8).max(1);
                }
            }
        }
    }
    if let Some(first) = first_col {
        let k = first + 1;
        for x in (0..first).rev() {
            let weight = (first - x) as f32 / k as f32;
            for y in 0..h {
                let src = (y * w + x + 1) * 4;
                let dst = (y * w + x) * 4;
                out.copy_within(src..src + 4, dst);
                let a = out[dst + 3];
                if a > 0 {
                    out[dst + 3] = (((a as f32) * weight) as u8).max(1);
                }
            }
        }
    }
    if let Some(last) = last_col {
        let k = w - last;
        for x in last + 1..w {
            let weight = (last + k - x) as f32 / k as f32;
            for y in 0..h {
                let src = (y * w + x - 1) * 4;
                let dst = (y * w + x) * 4;
                out.copy_within(src..src + 4, dst);
                let a = out[dst + 3];
                if a > 0 {
                    out[dst + 3] = (((a as f32) * weight) as u8).max(1);
                }
            }
        }
    }
}

fn ink_bounds(flat: &[u8], geo: FlatSurface) -> (f32, f32, f32, f32) {
    let (w, h) = (geo.width as usize, geo.height as usize);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if flat[(y * w + x) * 4 + 3] > 0 {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x > max_x {
        return (0.0, 0.0, 1.0, 1.0);
    }
    (
        geo.origin.0 as f32 + min_x as f32,
        geo.origin.1 as f32 + min_y as f32,
        geo.origin.0 as f32 + max_x as f32 + 1.0,
        geo.origin.1 as f32 + max_y as f32 + 1.0,
    )
}

fn copy_overlap(
    out: &mut [u8],
    out_size: (u32, u32),
    out_origin: (i32, i32),
    flat: &[u8],
    geo: FlatSurface,
) {
    let dx = geo.origin.0 - out_origin.0;
    let dy = geo.origin.1 - out_origin.1;
    for y in 0..geo.height as i32 {
        let oy = y + dy;
        if oy < 0 || oy >= out_size.1 as i32 {
            continue;
        }
        for x in 0..geo.width as i32 {
            let ox = x + dx;
            if ox < 0 || ox >= out_size.0 as i32 {
                continue;
            }
            let src = ((y as usize * geo.width as usize) + x as usize) * 4;
            let dst = ((oy as usize * out_size.0 as usize) + ox as usize) * 4;
            out[dst..dst + 4].copy_from_slice(&flat[src..src + 4]);
        }
    }
}
