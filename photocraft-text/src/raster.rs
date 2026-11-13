//! Glyph rasteriser: outline -> per-pixel coverage -> alpha.
//!
//! Three independent knobs, measured against Photoshop's own type-layer alpha
//! in `tests/raster.rs`:
//!
//! * [`Coverage`] — how a pixel's coverage is integrated (`Scanlines(n)`: n
//!   sub-scanlines with exact horizontal fractions; `Area`: exact signed-area
//!   accumulation).
//! * [`AlphaCurve`] — the transfer from coverage to stored alpha.
//! * `snap` — whether the glyph origin is rounded to whole device pixels.
//!
//! The canvas stores straight alpha: colour stays unpremultiplied and the
//! transferred coverage becomes the alpha channel.

use crate::outline::{Contours, Point};

/// How per-pixel coverage is integrated from the glyph outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// `n` sub-scanlines per pixel row, each contributing its exact horizontal
    /// span fraction; non-zero winding.
    Scanlines(u32),
    /// Exact signed-area accumulation per cell (font-rs style); non-zero
    /// winding is `min(|accumulated area|, 1)`.
    Area,
    /// `n x n` point sampling per pixel (CoolType's coverage grid).
    Grid(u32),
}

/// Transfer from integrated coverage to alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaCurve {
    /// `alpha = coverage`.
    Linear,
    /// `alpha = coverage^g`. Values below 1 darken partial pixels (stem
    /// darkening); values above 1 thin them.
    Power(f32),
    /// `alpha = sRGB_encode(coverage)`: coverage treated as linear light and
    /// encoded for storage.
    Srgb,
}

impl AlphaCurve {
    /// Maps coverage in `[0, 1]` to alpha in `[0, 1]`.
    pub fn apply(self, c: f32) -> f32 {
        let c = c.clamp(0.0, 1.0);
        match self {
            AlphaCurve::Linear => c,
            AlphaCurve::Power(g) => c.powf(g),
            AlphaCurve::Srgb => {
                if c <= 0.0031308 {
                    12.92 * c
                } else {
                    1.055 * c.powf(1.0 / 2.4) - 0.055
                }
            }
        }
    }
}

/// Photoshop's text antialiasing modes (`EngineData` `/AntiAlias`).
///
/// The stored code is `0` none, `1` crisp, `2` strong, `3` smooth, `4` sharp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AntiAlias {
    None,
    Crisp,
    Strong,
    Smooth,
    #[default]
    Sharp,
}

impl AntiAlias {
    /// Photoshop's stored code, or `None` for an unknown value.
    pub fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::None),
            1 => Some(Self::Crisp),
            2 => Some(Self::Strong),
            3 => Some(Self::Smooth),
            4 => Some(Self::Sharp),
            _ => None,
        }
    }

    /// Photoshop's stored code (`EngineDict/AntiAlias`).
    pub fn code(self) -> i64 {
        match self {
            Self::None => 0,
            Self::Crisp => 1,
            Self::Strong => 2,
            Self::Smooth => 3,
            Self::Sharp => 4,
        }
    }

    /// Number of coverage steps the mode quantises to (`None` = fully on/off).
    pub fn levels(self) -> Option<u32> {
        match self {
            Self::None => None,
            Self::Crisp | Self::Sharp => Some(15),
            Self::Strong | Self::Smooth => Some(16),
        }
    }
}

/// RGBA8 canvas drawn with straight alpha.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// Rounds each glyph origin (pen x, baseline y) to whole device pixels.
    pub snap: bool,
    /// Horizontal grid for the pen x when `snap` is on (1.0 = whole pixels).
    /// Photoshop's Crisp mode positions glyphs on a finer subpixel grid than
    /// the baseline grid; the exact step is measured per layer.
    pub snap_x: f32,
    /// Coverage integrator used by the `mask_contours` / `fill_contours` family.
    pub coverage: Coverage,
    /// Faux-bold stroke radius in pixels, applied to the next masked glyph.
    pub embolden: f32,
    /// Coverage-to-alpha transfer.
    pub curve: AlphaCurve,
    /// Photoshop's antialiasing mode; quantises the stored alpha.
    pub aa: AntiAlias,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
            snap: true,
            snap_x: 1.0,
            coverage: Coverage::Scanlines(4),
            curve: AlphaCurve::Linear,
            aa: AntiAlias::Sharp,
            embolden: 0.0,
        }
    }

    /// Fills the canvas from a function of the pixel position.
    /// Bounding box of pixels with non-zero alpha, as `(left, top, right, bottom)`
    /// with an exclusive right and bottom edge. Returns `(0, 0, 0, 0)` when the
    /// canvas is empty.
    pub fn ink_bounds(&self) -> (i32, i32, i32, i32) {
        let mut left = i32::MAX;
        let mut top = i32::MAX;
        let mut right = i32::MIN;
        let mut bottom = i32::MIN;
        for y in 0..self.height {
            for x in 0..self.width {
                let a = self.pixels[((y * self.width + x) * 4 + 3) as usize];
                if a != 0 {
                    left = left.min(x as i32);
                    right = right.max(x as i32 + 1);
                    top = top.min(y as i32);
                    bottom = bottom.max(y as i32 + 1);
                }
            }
        }
        if right <= left || bottom <= top {
            (0, 0, 0, 0)
        } else {
            (left, top, right, bottom)
        }
    }

    pub fn fill_with(&mut self, mut f: impl FnMut(u32, u32) -> [u8; 3]) {
        for y in 0..self.height {
            for x in 0..self.width {
                let rgb = f(x, y);
                let i = ((y * self.width + x) * 4) as usize;
                self.pixels[i] = rgb[0];
                self.pixels[i + 1] = rgb[1];
                self.pixels[i + 2] = rgb[2];
                self.pixels[i + 3] = 255;
            }
        }
    }

    /// Fills one glyph, `scale` device pixels per font unit, with the glyph
    /// origin at `(x, y)` (baseline, y down). Composites opaquely over the
    /// existing colour.
    pub fn fill_contours(
        &mut self,
        contours: &Contours,
        scale: f32,
        x: f32,
        y: f32,
        color: [u8; 3],
        alpha: f32,
    ) {
        let Some(mask) = self.glyph_mask(contours, scale, scale, 0.0, x, y) else {
            return;
        };
        mask.for_each_pixel(self.width, self.height, |p, cov| {
            let a = (self.curve.apply(cov) * alpha).clamp(0.0, 1.0);
            for c in 0..3 {
                let dst = self.pixels[p + c] as f32;
                self.pixels[p + c] = (dst * (1.0 - a) + color[c] as f32 * a).round() as u8;
            }
            self.pixels[p + 3] = 255;
        });
    }

    /// Draws one glyph as a straight-alpha mask: the colour stays unpremultiplied
    /// and the transferred coverage becomes the alpha, so a file with this layer
    /// carries the same antialiasing we computed.
    pub fn mask_contours(
        &mut self,
        contours: &Contours,
        scale: f32,
        x: f32,
        y: f32,
        color: [u8; 3],
        alpha: f32,
    ) {
        self.mask_contours_transformed(contours, scale, scale, 0.0, x, y, color, alpha);
    }

    /// As [`Canvas::mask_contours`] with independent horizontal and vertical
    /// scales: `sx` and `sy` device pixels per font unit. The origin
    /// `(x, baseline)` follows the same snap rule.
    pub fn mask_contours_scaled(
        &mut self,
        contours: &Contours,
        sx: f32,
        sy: f32,
        x: f32,
        baseline: f32,
        color: [u8; 3],
        alpha: f32,
    ) {
        self.mask_contours_transformed(contours, sx, sy, 0.0, x, baseline, color, alpha);
    }

    /// Affine glyph placement: a font-unit point `(px, py)` lands at
    /// `(x + px*sx + skew*py*sy, baseline - py*sy)`. `skew` is `tan(angle)`
    /// (faux italic shear about the baseline). `faux_italic_point` is not
    /// reused: it bakes in `FAUX_ITALIC_SKEW` and has no scale parameters.
    pub fn mask_contours_transformed(
        &mut self,
        contours: &Contours,
        sx: f32,
        sy: f32,
        skew: f32,
        x: f32,
        baseline: f32,
        color: [u8; 3],
        alpha: f32,
    ) {
        let Some(mut mask) = self.glyph_mask(contours, sx, sy, skew, x, baseline) else {
            return;
        };
        if self.embolden > 0.0 {
            mask.dilate(self.embolden);
        }
        mask.for_each_pixel(self.width, self.height, |p, cov| {
            // Photoshop quantises glyph coverage per antialias mode; rules and
            // other solid bars take the unquantised transfer.
            let a = self.quantised(self.curve.apply(cov)) * alpha;
            self.over(p, color, a.clamp(0.0, 1.0));
        });
    }

    /// Glyph placement through an arbitrary point map, used for warped type.
    ///
    /// The affine part is the same as [`Canvas::mask_contours_transformed`]
    /// (`(x + px*sx + skew*py*sy, baseline - py*sy)`); `warp` then maps that
    /// flat device point to the output canvas. Photoshop warps the glyph
    /// *outlines* and rasterises them in the warped space, so a warped layer
    /// must go through this path rather than resampling a flat bitmap (which
    /// doubles edges wherever the warp has a shear). No pen snapping is
    /// applied: the affine placement is the flat layout's own position.
    pub fn mask_contours_warped(
        &mut self,
        contours: &Contours,
        sx: f32,
        sy: f32,
        skew: f32,
        x: f32,
        baseline: f32,
        warp: &dyn Fn(f32, f32) -> (f32, f32),
        color: [u8; 3],
        alpha: f32,
    ) {
        let map = |px: f32, py: f32| warp(x + px * sx + skew * py * sy, baseline - py * sy);
        let edges = flatten(contours, &map);
        let Some(mut mask) = glyph_mask(&edges, self.coverage) else {
            return;
        };
        if self.embolden > 0.0 {
            mask.dilate(self.embolden);
        }
        mask.for_each_pixel(self.width, self.height, |p, cov| {
            let a = self.quantised(self.curve.apply(cov)) * alpha;
            self.over(p, color, a.clamp(0.0, 1.0));
        });
    }

    /// Solid horizontal bar for an underline. `offset` is the distance from
    /// the baseline to the bar's top edge, positive downward in device space.
    /// Columns span `round(x0)..round(x1)`; top and bottom rows carry partial
    /// coverage, full rows are solid. The bar is not passed through `curve`.
    pub fn underline(
        &mut self,
        x0: f32,
        x1: f32,
        baseline: f32,
        thickness: f32,
        offset: f32,
        color: [u8; 3],
        alpha: f32,
    ) {
        self.bar(x0, x1, baseline, thickness, offset, color, alpha);
    }

    /// Solid horizontal bar for a strikeout; same conventions as [`Canvas::underline`].
    /// Pass a negative `offset` to sit above the baseline.
    pub fn strikeout(
        &mut self,
        x0: f32,
        x1: f32,
        baseline: f32,
        thickness: f32,
        offset: f32,
        color: [u8; 3],
        alpha: f32,
    ) {
        self.bar(x0, x1, baseline, thickness, offset, color, alpha);
    }

    fn bar(
        &mut self,
        x0: f32,
        x1: f32,
        baseline: f32,
        thickness: f32,
        offset: f32,
        color: [u8; 3],
        alpha: f32,
    ) {
        if thickness <= 0.0 || alpha <= 0.0 {
            return;
        }
        let baseline = if self.snap {
            baseline.round()
        } else {
            baseline
        };
        // Both axes use exact coverage: Photoshop draws the rule from a
        // fractional x range and leaves a partial last column (layer `01`'s
        // 573.51 pt rule stores 128 on its last column, layer `07`'s 433.03 pt
        // rule 48/32), so rounding the x edges to whole pixels would paint a
        // full column where Photoshop paints half, and drop the 1/4 one.
        let lo = x0.min(x1).max(0.0);
        let hi = x0.max(x1).min(self.width as f32);
        if hi <= lo {
            return;
        }
        let top = baseline + offset;
        let bottom = top + thickness;
        let y_from = top.floor().max(0.0) as i64;
        let y_to = (bottom.ceil() as i64).min(self.height as i64);
        let xs = lo.floor() as i64;
        let xe = hi.ceil() as i64;
        for yy in y_from..y_to {
            let cov_y = ((bottom.min((yy + 1) as f32)) - top.max(yy as f32)).clamp(0.0, 1.0);
            if cov_y <= 0.0 {
                continue;
            }
            for xx in xs..xe {
                let cov_x = ((hi.min((xx + 1) as f32)) - lo.max(xx as f32)).clamp(0.0, 1.0);
                if cov_x <= 0.0 {
                    continue;
                }
                let a = (cov_y * cov_x * alpha).clamp(0.0, 1.0);
                let p = (yy as usize * self.width as usize + xx as usize) * 4;
                self.over(p, color, a);
            }
        }
    }

    /// Straight-alpha "over" of `color` at alpha `a` onto the pixel at byte
    /// index `p`. Colour is unpremultiplied; the existing alpha is `prev`.
    /// Quantises a coverage-implied alpha to the antialias mode's grid.
    ///
    /// Photoshop stores crisp/sharp as `15` coverage steps mapped onto
    /// `17 * k` (255/15), strong as `16 * k`, smooth as `round(k/16 * 255)`,
    /// and no AA as fully on or off.
    fn quantised(&self, a: f32) -> f32 {
        let Some(levels) = self.aa.levels() else {
            return if a >= 0.5 { 1.0 } else { 0.0 };
        };
        let k = (a.clamp(0.0, 1.0) * levels as f32).round();
        match self.aa {
            AntiAlias::Smooth => (k / levels as f32 * 255.0).round() / 255.0,
            AntiAlias::Strong => (k * 16.0) / 255.0,
            _ => (k * 17.0) / 255.0,
        }
    }

    /// Composites a ready-made alpha bitmap (from an [`crate::atlas::Atlas`])
    /// with its top-left corner at `(x, y)`. The atlas alpha is already
    /// quantised by Photoshop, so it is not quantised again.
    pub fn blit_alpha(&mut self, x: i32, y: i32, w: u32, h: u32, alpha: &[u8], color: [u8; 3]) {
        for row in 0..h as i32 {
            let py = y + row;
            if py < 0 || py as u32 >= self.height {
                continue;
            }
            for col in 0..w as i32 {
                let px = x + col;
                if px < 0 || px as u32 >= self.width {
                    continue;
                }
                let a = alpha[(row as usize) * (w as usize) + col as usize] as f32 / 255.0;
                if a > 0.0 {
                    let p = ((py as u32 * self.width + px as u32) * 4) as usize;
                    self.over(p, color, a);
                }
            }
        }
    }

    fn over(&mut self, p: usize, color: [u8; 3], a: f32) {
        let prev = self.pixels[p + 3] as f32 / 255.0;
        let out = (a + prev * (1.0 - a)).clamp(0.0, 1.0);
        if out > 0.0 {
            for c in 0..3 {
                let mixed =
                    (color[c] as f32 * a + self.pixels[p + c] as f32 * prev * (1.0 - a)) / out;
                self.pixels[p + c] = mixed.round().clamp(0.0, 255.0) as u8;
            }
            self.pixels[p + 3] = (out * 255.0).round() as u8;
        }
    }

    fn glyph_mask(
        &self,
        contours: &Contours,
        sx: f32,
        sy: f32,
        skew: f32,
        x: f32,
        baseline: f32,
    ) -> Option<Mask> {
        let (x, y) = if self.snap {
            let grid = self.snap_x.max(1.0e-3);
            ((x / grid).round() * grid, baseline.round())
        } else {
            (x, baseline)
        };
        let map = |px: f32, py: f32| (x + px * sx + skew * py * sy, y - py * sy);
        let edges = flatten(contours, &map);
        glyph_mask(&edges, self.coverage)
    }
}

/// Coverage of one glyph over its own bounding box, in device pixels.
struct Mask {
    x0: i32,
    y0: i32,
    w: usize,
    h: usize,
    cov: Vec<f32>,
}

impl Mask {
    /// Dilates coverage by `radius` pixels (a square kernel clipped to a disc).
    ///
    /// Photoshop's faux bold is a synthetic stroke; this reproduces its weight
    /// closely but not exactly. Use a captured atlas (`--faux-bold`) when the
    /// pixels must match.
    fn dilate(&mut self, radius: f32) {
        let r = radius.round() as i32;
        if r <= 0 {
            return;
        }
        let (w, h) = (self.w as i32, self.h as i32);
        let nw = (w + 2 * r) as usize;
        let nh = (h + 2 * r) as usize;
        let mut cov = vec![0f32; nw * nh];
        for y in 0..h {
            for x in 0..w {
                let v = self.cov[(y * w + x) as usize];
                if v <= 0.0 {
                    continue;
                }
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dy * dy > r * r {
                            continue;
                        }
                        let (nx, ny) = (x + dx + r, y + dy + r);
                        cov[(ny * nw as i32 + nx) as usize] =
                            cov[(ny * nw as i32 + nx) as usize].max(v);
                    }
                }
            }
        }
        self.cov = cov;
        self.x0 -= r;
        self.y0 -= r;
        self.w = nw;
        self.h = nh;
    }

    /// Calls `f(byte_index_of_pixel, coverage)` for every mask pixel that lies
    /// on the canvas and has non-zero coverage.
    fn for_each_pixel(&self, cw: u32, ch: u32, mut f: impl FnMut(usize, f32)) {
        for my in 0..self.h {
            let cy = self.y0 + my as i32;
            if cy < 0 || cy >= ch as i32 {
                continue;
            }
            for mx in 0..self.w {
                let cov = self.cov[my * self.w + mx];
                if cov <= 0.0 {
                    continue;
                }
                let cx = self.x0 + mx as i32;
                if cx < 0 || cx >= cw as i32 {
                    continue;
                }
                let p = (cy as usize * cw as usize + cx as usize) * 4;
                f(p, cov.min(1.0));
            }
        }
    }
}

/// A straight device-space segment, y down.
#[derive(Clone, Copy)]
struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// Converts quadratic contours to device-space line segments through `map`,
/// which takes font-unit `(x, y)` (y up) to device `(x, y)` (y down).
fn flatten(contours: &Contours, map: &dyn Fn(f32, f32) -> (f32, f32)) -> Vec<Edge> {
    let mut edges = Vec::new();
    let to_device = |p: Point| map(p.x, p.y);
    for contour in contours {
        if contour.len() < 2 {
            continue;
        }
        let mut points: Vec<(f32, f32, bool)> = Vec::with_capacity(contour.len() + 2);
        // Start where the contour does; an off-curve start implies a midpoint
        // between the last and the first point.
        for p in contour {
            let (x, y) = (p.x, p.y);
            points.push((x, y, p.on));
        }
        let first = points[0];
        let start = if first.2 {
            first
        } else {
            let last = *points.last().unwrap();
            if last.2 {
                last
            } else {
                ((last.0 + first.0) / 2.0, (last.1 + first.1) / 2.0, true)
            }
        };
        let mut cur = to_device(Point {
            x: start.0,
            y: start.1,
            on: true,
        });
        let mut ctrl: Option<(f32, f32)> = None;
        for &(x, y, on) in points.iter().chain(std::iter::once(&start)) {
            if on {
                let end = to_device(Point { x, y, on: true });
                match ctrl.take() {
                    Some((cx, cy)) => {
                        let c = to_device(Point {
                            x: cx,
                            y: cy,
                            on: false,
                        });
                        emit_quad(&mut edges, cur, c, end);
                    }
                    None => edges.push(Edge {
                        x0: cur.0,
                        y0: cur.1,
                        x1: end.0,
                        y1: end.1,
                    }),
                }
                cur = end;
            } else {
                if let Some((px, py)) = ctrl {
                    let mid = ((px + x) / 2.0, (py + y) / 2.0);
                    let end = to_device(Point {
                        x: mid.0,
                        y: mid.1,
                        on: true,
                    });
                    let c = to_device(Point {
                        x: px,
                        y: py,
                        on: false,
                    });
                    emit_quad(&mut edges, cur, c, end);
                    cur = end;
                }
                ctrl = Some((x, y));
            }
        }
    }
    edges
}

fn emit_quad(edges: &mut Vec<Edge>, from: (f32, f32), ctrl: (f32, f32), to: (f32, f32)) {
    const STEPS: usize = 16;
    let mut prev = from;
    for i in 1..=STEPS {
        let next = quad_at(from, ctrl, to, i as f32 / STEPS as f32);
        edges.push(Edge {
            x0: prev.0,
            y0: prev.1,
            x1: next.0,
            y1: next.1,
        });
        prev = next;
    }
}

fn quad_at(a: (f32, f32), c: (f32, f32), b: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    (
        u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0,
        u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1,
    )
}

/// Computes the coverage of `edges` over their padded bounding box.
fn glyph_mask(edges: &[Edge], mode: Coverage) -> Option<Mask> {
    if edges.is_empty() {
        return None;
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for e in edges {
        minx = minx.min(e.x0.min(e.x1));
        maxx = maxx.max(e.x0.max(e.x1));
        miny = miny.min(e.y0.min(e.y1));
        maxy = maxy.max(e.y0.max(e.y1));
    }
    let x0 = minx.floor() as i32;
    let y0 = miny.floor() as i32;
    // Two columns of slack: the area integrator writes one cell past the right
    // edge of a span.
    let w = (maxx.ceil() as i32 - x0 + 2).max(1) as usize;
    let h = (maxy.ceil() as i32 - y0 + 1).max(1) as usize;
    let local: Vec<Edge> = edges
        .iter()
        .map(|e| Edge {
            x0: e.x0 - x0 as f32,
            y0: e.y0 - y0 as f32,
            x1: e.x1 - x0 as f32,
            y1: e.y1 - y0 as f32,
        })
        .collect();
    let cov = match mode {
        Coverage::Scanlines(n) => scanlines(&local, w, h, n.max(1) as usize),
        Coverage::Area => area(&local, w, h),
        Coverage::Grid(n) => grid(&local, w, h, n.max(1) as usize),
    };
    Some(Mask { x0, y0, w, h, cov })
}

/// Non-zero winding per sub-scanline, exact horizontal fractions.
fn scanlines(edges: &[Edge], w: usize, h: usize, sub: usize) -> Vec<f32> {
    let mut cov = vec![0f32; w * h];
    let weight = 1.0 / sub as f32;
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for row in 0..h {
        for s in 0..sub {
            let y = row as f32 + (s as f32 + 0.5) / sub as f32;
            xs.clear();
            for e in edges {
                let (y0, y1) = (e.y0, e.y1);
                if (y0 <= y && y1 > y) || (y1 <= y && y0 > y) {
                    let t = (y - y0) / (y1 - y0);
                    xs.push((e.x0 + t * (e.x1 - e.x0), if y1 > y0 { 1 } else { -1 }));
                }
            }
            if xs.len() < 2 {
                continue;
            }
            xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut winding = 0;
            let mut span_start = 0f32;
            for &(x, dir) in xs.iter() {
                let was = winding;
                winding += dir;
                if was == 0 && winding != 0 {
                    span_start = x;
                } else if was != 0 && winding == 0 {
                    span(span_start, x, row * w, w, &mut cov, weight);
                }
            }
        }
    }
    cov
}

fn span(xa: f32, xb: f32, base: usize, width: usize, coverage: &mut [f32], weight: f32) {
    let xa = xa.max(0.0);
    let xb = xb.min(width as f32);
    if xb <= xa {
        return;
    }
    let first = xa.floor() as usize;
    let last = (xb.ceil() as usize).min(width);
    for px in first..last {
        let l = (px as f32).max(xa);
        let r = ((px + 1) as f32).min(xb);
        coverage[base + px] += (r - l).max(0.0) * weight;
    }
}

/// Exact signed-area accumulation (font-rs). Each cell receives the area the
/// edges cover inside it; a running sum along the row gives the coverage.
/// `n x n` supersampling: each sample counts as one `1 / n^2` of the pixel.
fn grid(edges: &[Edge], w: usize, h: usize, n: usize) -> Vec<f32> {
    let mut cov = vec![0f32; w * h];
    let weight = 1.0 / (n * n) as f32;
    let rows = h * n;
    let mut xs: Vec<f32> = Vec::with_capacity(edges.len());
    for row in 0..rows {
        let y = (row as f32 + 0.5) / n as f32;
        xs.clear();
        for e in edges {
            if (e.y0 > y) != (e.y1 > y) {
                let t = (y - e.y0) / (e.y1 - e.y0);
                xs.push(e.x0 + t * (e.x1 - e.x0));
            }
        }
        if xs.is_empty() {
            continue;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let base = (row / n) * w;
        for pair in xs.chunks_exact(2) {
            // Sample points strictly inside the span accumulate.
            let (a, b) = (pair[0], pair[1]);
            // Sample centres `(k + 0.5) / n` in `[a, b)`.
            let first = (a * n as f32 - 0.5).ceil() as i64;
            let last = (b * n as f32 - 0.5).ceil() as i64 - 1;
            for k in first..=last {
                let col = (k as f32 / n as f32).floor() as i64;
                if (0..w as i64).contains(&col) {
                    cov[base + col as usize] += weight;
                }
            }
        }
    }
    cov
}

fn area(edges: &[Edge], w: usize, h: usize) -> Vec<f32> {
    let mut acc = vec![0f32; w * h];
    for e in edges {
        accumulate_edge(&mut acc, w, h, e);
    }
    let mut cov = vec![0f32; w * h];
    for row in 0..h {
        let mut sum = 0f32;
        for x in 0..w {
            sum += acc[row * w + x];
            cov[row * w + x] = sum.abs().min(1.0);
        }
    }
    cov
}

fn accumulate_edge(acc: &mut [f32], w: usize, h: usize, e: &Edge) {
    if e.y0 == e.y1 {
        return;
    }
    let (dir, (px0, py0), (px1, py1)) = if e.y0 < e.y1 {
        (1.0f32, (e.x0, e.y0), (e.x1, e.y1))
    } else {
        (-1.0f32, (e.x1, e.y1), (e.x0, e.y0))
    };
    let dxdy = (px1 - px0) / (py1 - py0);
    let mut x = px0;
    let y_first = py0.max(0.0) as usize;
    let y_last = (py1.ceil().max(0.0) as usize).min(h);
    for y in y_first..y_last {
        let yf = y as f32;
        let line = y * w;
        let dy = (yf + 1.0).min(py1) - yf.max(py0);
        let xnext = x + dxdy * dy;
        let d = dy * dir;
        let (xa, xb) = if x < xnext { (x, xnext) } else { (xnext, x) };
        let x0floor = xa.floor();
        let x0i = x0floor as i32;
        let x1ceil = xb.ceil();
        let x1i = x1ceil as i32;
        let put = |acc: &mut [f32], xi: i32, v: f32| {
            if xi >= 0 && (xi as usize) < w {
                acc[line + xi as usize] += v;
            }
        };
        if x1i <= x0i + 1 {
            let xmf = 0.5 * (x + xnext) - x0floor;
            put(acc, x0i, d - d * xmf);
            put(acc, x0i + 1, d * xmf);
        } else {
            let s = 1.0 / (xb - xa);
            let x0f = xa - x0floor;
            let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
            let x1f = xb - x1ceil + 1.0;
            let am = 0.5 * s * x1f * x1f;
            put(acc, x0i, d * a0);
            if x1i == x0i + 2 {
                put(acc, x0i + 1, d * (1.0 - a0 - am));
            } else {
                let a1 = s * (1.5 - x0f);
                put(acc, x0i + 1, d * (a1 - a0));
                for xi in (x0i + 2)..(x1i - 1) {
                    put(acc, xi, d * s);
                }
                let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                put(acc, x1i - 1, d * (1.0 - a2 - am));
            }
            put(acc, x1i, d * am);
        }
        x = xnext;
    }
}
