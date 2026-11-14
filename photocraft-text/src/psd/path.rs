//! Area/text-frame paths: the `Txt2` frame outline into the model and back.
//!
//! Photoshop stores a text layer's frame in the document `Txt2` block. A box or
//! area frame is a flat list of cubic segments in frame space (`/1 /0`, eight
//! numbers per segment: `anchor, out, next-in, next-anchor`, the shared anchor
//! repeated between segments, a closed path ending where it started). `/2 /2`
//! maps that space into the `TySh` text space; `/2 /0` is the frame kind
//! (`1` box/area, `2` type on a path); `/2 /6` is the type's range in fractional
//! curve-segment units (`-1` is the stored default) and `/2 /11 /0` marks
//! flipped type.
//!
//! This is the upstream port of the fork's `psd_path` reader. The input is a
//! [`FrameSource`]: either a frame already decoded by
//! [`txt2::object_frame_path`] (its points are in text space) or an
//! `EngineData` frame dictionary (points in the frame's own space), so unit
//! tests can exercise both and [`super::read_layer`] uses the `Txt2` one.
//!
//! # Reading
//!
//! [`read_shape`] produces a [`ShapeSpec`]: the stored frame verbatim
//! ([`FrameSpec`], needed for a byte-identical re-emit) plus the decoded curve
//! ([`PathSpec`]) with the type's placement resolved exactly like the fork's
//! reader: a closed path with the default range starts on its closing segment
//! (from the last anchor), `SAME_ARC` collapses an empty closed range to zero
//! length, and an explicit end stays explicit even when it reaches the path's
//! end.
//!
//! # Writing
//!
//! [`to_frame_parts`] returns what [`txt2::set_object_frame_path`] writes. A
//! shape that carries a stored frame is re-emitted verbatim (the box path's 32
//! numbers, a Photoshop path frame's exact points/range/flip, so an unedited
//! layer keeps its bytes). A decoded-curve-only shape is baked into the fork's
//! canonical form: closed paths bake the start into the curve (`/6` keeps the
//! default `[-1, -1]`), flipped type is the path run backwards with
//! `/11 /0 false`, and a moved start or an end limit on an open path goes into
//! the `/6` range in fractional segment units.

use crate::engine_data::Value as E;
use crate::txt2;

use super::model::{FrameSpec, PathKnot, PathSpec, ShapeSpec};

/// The `/2 /0` frame kind of type on a path.
pub const PATH_FRAME: i64 = 2;
/// The `/2 /0` frame kind of a text box / rectangular area frame.
pub const BOX_FRAME: i64 = 1;
/// Most cubic segments read from a frame (a hostile file cannot make this large).
const MAX_SEGMENTS: usize = 4096;
/// How close (px) two points must be to count as the same: Photoshop writes a
/// segment's start as an exact copy of the previous segment's end.
const SAME: f64 = 1e-3;
/// How close (px) two arc lengths must be to count as the same placement.
const SAME_ARC: f64 = 1e-3;
/// Pieces each segment is measured in when a start is baked into the curve.
const STEPS: usize = 64;

type Pt = [f64; 2];
type Seg = [Pt; 4];

fn dist(a: Pt, b: Pt) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn same(a: Pt, b: Pt) -> bool {
    dist(a, b) <= SAME
}

fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// The point of cubic segment `s` at parameter `t`.
fn cubic(s: &Seg, t: f64) -> Pt {
    let (a, b, c) = (
        lerp(s[0], s[1], t),
        lerp(s[1], s[2], t),
        lerp(s[2], s[3], t),
    );
    lerp(lerp(a, b, t), lerp(b, c, t), t)
}

/// Maps a frame-space point through a `[a, b, c, d, e, f]` matrix.
fn apply(m: [f64; 6], p: Pt) -> Pt {
    [
        m[0] * p[0] + m[2] * p[1] + m[4],
        m[1] * p[0] + m[3] * p[1] + m[5],
    ]
}

fn matrix_ok(m: [f64; 6]) -> bool {
    let det = m[0] * m[3] - m[1] * m[2];
    det.is_finite() && det.abs() > 1e-12
}

fn ed_path<'a>(value: &'a E, keys: &[&str]) -> Option<&'a E> {
    let mut current = value;
    for key in keys {
        current = current.get(key)?;
    }
    Some(current)
}

/// Every entry of `v` as a finite-or-not number (`None` when some entry is not
/// a number).
fn numbers(v: &E) -> Option<Vec<f64>> {
    v.as_array()?
        .iter()
        .map(|v| match v {
            E::Float(f) => Some(*f),
            E::Int(i) => Some(*i as f64),
            _ => None,
        })
        .collect()
}

/// The `/2 /0` kind of a frame dictionary: 0 point text, 1 box/area, 2 path.
pub fn frame_kind(frame: &E) -> i64 {
    ed_path(frame, &["2", "0"]).and_then(E::as_i64).unwrap_or(0)
}

/// Where a frame outline comes from.
#[derive(Clone, Copy)]
pub enum FrameSource<'a> {
    /// A frame read by [`txt2::object_frame_path`]: its points are already in
    /// `TySh` text space and the stored matrix must not be applied again.
    Frame(&'a txt2::FramePath),
    /// A frame dictionary as `EngineData`
    /// (`<< /1 << /0 [ … ] >> /2 << … >> >>`); its points are in the frame's
    /// own space, mapped to text space through `/2 /2`.
    EngineData(&'a E),
}

impl FrameSource<'_> {
    fn numbers(&self) -> Option<Vec<f64>> {
        match self {
            Self::Frame(frame) => Some(frame.points.clone()),
            Self::EngineData(value) => ed_path(value, &["1", "0"]).and_then(numbers),
        }
    }

    fn matrix(&self) -> Result<Option<[f64; 6]>, String> {
        match self {
            Self::Frame(frame) => Ok(frame.matrix),
            Self::EngineData(value) => match ed_path(value, &["2", "2"]) {
                None => Ok(None),
                Some(m) => numbers(m)
                    .and_then(|m| <[f64; 6]>::try_from(m).ok())
                    .map(Some)
                    .ok_or_else(|| "its frame matrix is not six numbers".into()),
            },
        }
    }

    /// Whether the points still need the stored matrix applied.
    fn applies_matrix(&self) -> bool {
        matches!(self, Self::EngineData(_))
    }

    fn kind(&self) -> Option<i64> {
        match self {
            Self::Frame(frame) => Some(frame.kind),
            Self::EngineData(value) => Some(frame_kind(value)),
        }
    }

    fn range(&self) -> [f64; 2] {
        match self {
            Self::Frame(frame) => frame.range,
            Self::EngineData(value) => ed_path(value, &["2", "6"])
                .and_then(numbers)
                .and_then(|r| <[f64; 2]>::try_from(r).ok())
                .unwrap_or([-1.0, -1.0]),
        }
    }

    fn flip(&self) -> Option<bool> {
        match self {
            Self::Frame(frame) => frame.flip,
            Self::EngineData(value) => ed_path(value, &["2", "11", "0"]).and_then(E::as_bool),
        }
    }
}

/// The cubic segments of `source` in text space, in the file's order, and
/// whether they close. `Err` says why they cannot be read.
fn segments(source: FrameSource<'_>) -> Result<(Vec<Seg>, bool), String> {
    let raw = source.numbers().ok_or("the frame has no path")?;
    if raw.is_empty() || !raw.len().is_multiple_of(8) {
        return Err(format!(
            "its path has {} coordinates, not whole curve segments",
            raw.len()
        ));
    }
    if raw.len() / 8 > MAX_SEGMENTS {
        return Err(format!("its path has more than {MAX_SEGMENTS} segments"));
    }
    let matrix = match source.matrix()? {
        None => None,
        Some(m) if matrix_ok(m) => Some(m),
        Some(_) => return Err("its frame matrix is degenerate".into()),
    };
    let mut points: Vec<Pt> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[x, y]| [x, y])
        .collect();
    if source.applies_matrix()
        && let Some(m) = matrix
    {
        points = points.into_iter().map(|p| apply(m, p)).collect();
    }
    if points
        .iter()
        .any(|p| !p[0].is_finite() || !p[1].is_finite())
    {
        return Err("its path is out of range".into());
    }
    let segs: Vec<Seg> = points.as_chunks::<4>().0.to_vec();
    if segs.windows(2).any(|w| !same(w[0][3], w[1][0])) {
        return Err("its path has more than one piece".into());
    }
    let (Some(first), Some(last)) = (segs.first(), segs.last()) else {
        return Err("the frame has no path".into());
    };
    let (mut x0, mut x1, mut y0, mut y1) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for p in &points {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    if x1 - x0 <= SAME && y1 - y0 <= SAME {
        return Err("its path has no length".into());
    }
    let closed = same(last[3], first[0]);
    Ok((segs, closed))
}

/// One curve knot: an anchor with the handles of the incoming and outgoing
/// segments.
#[derive(Clone, Copy, Debug)]
struct Knot {
    anchor: Pt,
    in_ctrl: Pt,
    out_ctrl: Pt,
    smooth: bool,
}

/// Whether the handles `a` (in) and `b` (out) of `anchor` are collinear on
/// opposite sides.
fn smooth(anchor: Pt, a: Pt, b: Pt) -> bool {
    let (ax, ay) = (a[0] - anchor[0], a[1] - anchor[1]);
    let (bx, by) = (b[0] - anchor[0], b[1] - anchor[1]);
    let (la, lb) = (ax.hypot(ay), bx.hypot(by));
    la > SAME && lb > SAME && (ax * by - ay * bx).abs() <= 1e-3 * la * lb && ax * bx + ay * by < 0.0
}

fn knot(in_ctrl: Pt, anchor: Pt, out_ctrl: Pt) -> Knot {
    Knot {
        anchor,
        in_ctrl,
        out_ctrl,
        smooth: smooth(anchor, in_ctrl, out_ctrl),
    }
}

/// The knots of `segs` (closing when `closed`), the first knot first.
fn knots_of(segs: &[Seg], closed: bool) -> Vec<Knot> {
    let Some(last) = segs.last() else {
        return Vec::new();
    };
    let mut knots: Vec<Knot> = Vec::with_capacity(segs.len() + 1);
    for (i, s) in segs.iter().enumerate() {
        let prev = match i.checked_sub(1).and_then(|j| segs.get(j)) {
            Some(p) => p[2],
            None if closed => last[2],
            None => s[0],
        };
        knots.push(knot(prev, s[0], s[1]));
    }
    if !closed {
        knots.push(knot(last[2], last[3], last[3]));
    }
    knots
}

/// A path in the writer's working form, independent of the model's `f32`
/// coordinates.
#[derive(Clone, Debug)]
struct Sub {
    knots: Vec<Knot>,
    closed: bool,
}

impl Sub {
    fn from_path(path: &PathSpec) -> Sub {
        let point = |p: [f32; 2]| [f64::from(p[0]), f64::from(p[1])];
        Sub {
            knots: path
                .knots
                .iter()
                .map(|k| Knot {
                    anchor: point(k.anchor),
                    in_ctrl: point(k.in_ctrl),
                    out_ctrl: point(k.out_ctrl),
                    smooth: k.smooth.unwrap_or(false),
                })
                .collect(),
            closed: path.closed,
        }
    }

    fn segments(&self) -> Vec<Seg> {
        let n = self.knots.len();
        if n == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(if self.closed { n } else { n - 1 });
        for i in 0..n {
            if !self.closed && i + 1 == n {
                break;
            }
            let k = self.knots[i];
            let next = self.knots[(i + 1) % n];
            out.push([k.anchor, k.out_ctrl, next.in_ctrl, next.anchor]);
        }
        out
    }
}

/// The flattening type paths are measured with: the cumulative arc length at
/// each of the `n` equal parameter steps of `s`, starting with zero.
fn flattened(s: &Seg) -> Vec<f64> {
    let poly = dist(s[0], s[1]) + dist(s[1], s[2]) + dist(s[2], s[3]);
    let n = if poly.is_finite() {
        ((poly / 2.0).ceil() as usize).clamp(4, 128)
    } else {
        4
    };
    let mut cum = Vec::with_capacity(n + 1);
    let mut acc = 0.0;
    let mut prev = s[0];
    cum.push(0.0);
    for i in 1..=n {
        let p = cubic(s, i as f64 / n as f64);
        acc += dist(p, prev);
        prev = p;
        cum.push(acc);
    }
    cum
}

/// Arc lengths along a path's segments, to convert between arc length and the
/// `/6` range's segment units.
struct Measure {
    /// Per segment, the cumulative lengths of [`flattened`].
    segs: Vec<Vec<f64>>,
    /// Arc length at the start of each segment, then the total.
    at: Vec<f64>,
}

impl Measure {
    fn new(segs: &[Seg]) -> Measure {
        let segs: Vec<Vec<f64>> = segs.iter().map(flattened).collect();
        let mut at = Vec::with_capacity(segs.len() + 1);
        let mut acc = 0.0;
        at.push(0.0);
        for s in &segs {
            acc += s.last().copied().unwrap_or(0.0);
            at.push(acc);
        }
        Measure { segs, at }
    }

    fn count(&self) -> f64 {
        self.segs.len() as f64
    }

    fn length(&self) -> f64 {
        self.at.last().copied().unwrap_or(0.0)
    }

    /// The arc length at range position `p` (segment index plus parameter),
    /// clamped to the path.
    fn arc(&self, p: f64) -> f64 {
        let p = p.clamp(0.0, self.count());
        let i = (p.floor() as usize).min(self.segs.len().saturating_sub(1));
        let (Some(cum), Some(base)) = (self.segs.get(i), self.at.get(i)) else {
            return 0.0;
        };
        let steps = cum.len().saturating_sub(1).max(1) as f64;
        let u = ((p - i as f64) * steps).clamp(0.0, steps);
        let k = (u.floor() as usize).min(cum.len().saturating_sub(2));
        let (a, b) = (
            cum.get(k).copied().unwrap_or(0.0),
            cum.get(k + 1).copied().unwrap_or(0.0),
        );
        base + a + (b - a) * (u - k as f64)
    }

    /// The range position (segment index plus parameter) at arc length `s`,
    /// clamped to the path.
    fn position(&self, s: f64) -> f64 {
        let s = if s.is_finite() {
            s.clamp(0.0, self.length())
        } else {
            0.0
        };
        for (i, cum) in self.segs.iter().enumerate() {
            let (Some(base), Some(next)) = (self.at.get(i), self.at.get(i + 1)) else {
                break;
            };
            if s > *next && i + 1 < self.segs.len() {
                continue;
            }
            let left = s - base;
            let steps = cum.len().saturating_sub(1).max(1);
            for (k, w) in cum.windows(2).enumerate() {
                let (a, b) = (w[0], w[1]);
                if left <= b || k + 1 == steps {
                    let f = if b > a {
                        ((left - a) / (b - a)).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    return i as f64 + (k as f64 + f) / steps as f64;
                }
            }
            return i as f64 + 1.0;
        }
        self.count()
    }
}

/// The `/6` range of `source`: start and end in segment units, each `None` when
/// it is the stored default (`-1`) or not a number on the path.
fn range_of(source: FrameSource<'_>, count: f64) -> [Option<f64>; 2] {
    let v = source.range();
    let at = |i: usize| v.get(i).copied().filter(|p| (0.0..=count).contains(p));
    [at(0), at(1)]
}

fn kind_name(kind: Option<i64>) -> Option<String> {
    match kind {
        Some(0) => Some("point".into()),
        Some(BOX_FRAME) => Some("box".into()),
        Some(PATH_FRAME) => Some("path".into()),
        _ => None,
    }
}

fn path_knots(knots: &[Knot]) -> Vec<PathKnot> {
    knots
        .iter()
        .map(|k| PathKnot {
            anchor: [k.anchor[0] as f32, k.anchor[1] as f32],
            in_ctrl: [k.in_ctrl[0] as f32, k.in_ctrl[1] as f32],
            out_ctrl: [k.out_ctrl[0] as f32, k.out_ctrl[1] as f32],
            smooth: Some(k.smooth),
        })
        .collect()
}

fn text_space_points(segs: &[Seg]) -> Vec<f64> {
    segs.iter().flatten().flat_map(|p| [p[0], p[1]]).collect()
}

/// Reads a frame outline into [`ShapeSpec`]: the stored frame verbatim plus the
/// decoded curve and the type's resolved placement. `None` when the frame is
/// not a readable single-piece path.
pub fn read_shape(source: FrameSource<'_>) -> Option<ShapeSpec> {
    let (segs, closed) = segments(source).ok()?;
    let measure = Measure::new(&segs);
    let count = measure.count();
    let length = measure.length();
    let [a, b] = range_of(source, count);
    let flip = source.flip().unwrap_or(false);
    let (knots, start, end) = if closed && a.is_none() && b.is_none() {
        // The default: one lap from the last anchor (the closing segment).
        let mut knots = knots_of(&segs, closed);
        knots.rotate_right(1);
        (knots, 0.0, length)
    } else {
        let start_arc = measure.arc(a.unwrap_or(if closed { count - 1.0 } else { 0.0 }));
        let end_arc = match b.map(|b| measure.arc(b)) {
            // A closed range runs on from its start, past the seam if it must.
            // Equal ends are an empty range, not a lap: a full lap is written
            // with its end a lap on (see [`frame_parts`]).
            Some(e) if closed => {
                let d = e - start_arc;
                start_arc
                    + if d.abs() <= SAME_ARC {
                        0.0
                    } else if d > 0.0 {
                        d
                    } else {
                        d + length
                    }
            }
            Some(e) => e.max(start_arc),
            None if closed => start_arc + length,
            None => length,
        };
        (knots_of(&segs, closed), start_arc, end_arc)
    };
    // Flipped type is the same stretch of path measured from the other end.
    let (start, end) = if flip {
        let reversed_start = length - end;
        let reversed_start = if closed {
            reversed_start.rem_euclid(length)
        } else {
            reversed_start.max(0.0)
        };
        (reversed_start, reversed_start + (end - start))
    } else {
        (start, end)
    };
    let path = PathSpec {
        knots: path_knots(&knots),
        closed,
        start: start as f32,
        // An explicit end stays explicit even when it reaches the path's end.
        end: b.is_some().then_some(end as f32),
        flip,
        matrix: source.matrix().ok().flatten(),
        kind: kind_name(source.kind()),
    };
    let frame = FrameSpec {
        points: text_space_points(&segs),
        kind: source.kind().unwrap_or(0),
        range: source.range(),
        flip: source.flip(),
        matrix: source.matrix().ok().flatten(),
    };
    Some(ShapeSpec {
        frame: Some(frame),
        path: Some(path),
    })
}

/// `path` with its knots in the opposite order (the segments run backwards).
fn reversed(path: &Sub) -> Sub {
    let mut knots: Vec<Knot> = path
        .knots
        .iter()
        .rev()
        .map(|k| Knot {
            anchor: k.anchor,
            in_ctrl: k.out_ctrl,
            out_ctrl: k.in_ctrl,
            smooth: k.smooth,
        })
        .collect();
    if path.closed && !knots.is_empty() {
        // Keep the first knot first: k0, k(n-1), …, k1.
        knots.rotate_right(1);
    }
    Sub {
        knots,
        closed: path.closed,
    }
}

/// `path` starting `s` px along it: an open path loses the part before `s`, a
/// closed one starts at a new smooth knot there.
fn starting_at(path: &Sub, s: f64) -> Sub {
    let segs = path.segments();
    let lengths: Vec<f64> = segs
        .iter()
        .map(|seg| {
            (1..=STEPS)
                .map(|i| {
                    let (a, b) = (
                        cubic(seg, (i - 1) as f64 / STEPS as f64),
                        cubic(seg, i as f64 / STEPS as f64),
                    );
                    dist(a, b)
                })
                .sum()
        })
        .collect();
    let total: f64 = lengths.iter().sum();
    if !s.is_finite() || !total.is_finite() || total <= SAME {
        return path.clone();
    }
    let s = if path.closed { s.rem_euclid(total) } else { s };
    if s <= SAME || s >= total - SAME {
        return path.clone();
    }
    // The segment and parameter at arc length `s` (linear within a measured
    // piece).
    let mut left = s;
    let mut at = None;
    for (i, (seg, len)) in segs.iter().zip(&lengths).enumerate() {
        if left > *len {
            left -= len;
            continue;
        }
        let mut walked = 0.0;
        for k in 1..=STEPS {
            let (t0, t1) = ((k - 1) as f64 / STEPS as f64, k as f64 / STEPS as f64);
            let (a, b) = (cubic(seg, t0), cubic(seg, t1));
            let piece = dist(a, b);
            if walked + piece >= left || k == STEPS {
                let f = if piece > 0.0 {
                    ((left - walked) / piece).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                at = Some((i, t0 + (t1 - t0) * f, *seg));
                break;
            }
            walked += piece;
        }
        break;
    }
    let Some((i, t, seg)) = at else {
        return path.clone();
    };
    // de Casteljau: left half p0 a ab m, right half m bc c p3.
    let (a, b, c) = (
        lerp(seg[0], seg[1], t),
        lerp(seg[1], seg[2], t),
        lerp(seg[2], seg[3], t),
    );
    let (ab, bc) = (lerp(a, b, t), lerp(b, c, t));
    let m = lerp(ab, bc, t);
    let n = path.knots.len();
    if n == 0 {
        return path.clone();
    }
    let next = (i + 1) % n;
    let mut knots = Vec::with_capacity(n + 1);
    knots.push(Knot {
        anchor: m,
        in_ctrl: if path.closed { ab } else { m },
        out_ctrl: bc,
        smooth: path.closed,
    });
    let count = if path.closed { n } else { n - next };
    for r in 0..count {
        let j = (next + r) % n;
        let Some(mut k) = path.knots.get(j).copied() else {
            continue;
        };
        if j == next {
            k.in_ctrl = c;
        }
        if path.closed && j == i {
            k.out_ctrl = a;
        }
        knots.push(k);
    }
    Sub {
        knots,
        closed: path.closed,
    }
}

/// The path a frame is written with for a moved start / flipped type with the
/// range left at the default: flipped type is the path run backwards and a
/// moved start the path from there on.
fn frame_path(path: &Sub, start: f32, flip: bool) -> Sub {
    let path = if flip { reversed(path) } else { path.clone() };
    starting_at(&path, f64::from(start))
}

/// What a path frame is written with for a decoded curve: the path, whether its
/// segment list starts at the second knot, and the `/6` range.
fn frame_parts(path: &Sub, start: f32, end: Option<f32>, flip: bool) -> (Sub, bool, [f64; 2]) {
    const DEFAULT: [f64; 2] = [-1.0, -1.0];
    let start = if start.is_finite() {
        f64::from(start)
    } else {
        0.0
    };
    let end = end.map(f64::from).filter(|e| e.is_finite());
    if path.closed {
        let path = frame_path(path, start as f32, flip);
        let measure = Measure::new(&path.segments());
        let length = measure.length();
        return match end.map(|e| (e - start).max(0.0)) {
            // A lap or more is an explicit end at the seam, apart from the
            // default (no end).
            Some(span) if span >= length - SAME_ARC => (path, false, [0.0, measure.count()]),
            Some(span) => (path, false, [0.0, measure.position(span)]),
            None => (path, true, DEFAULT),
        };
    }
    let base = if flip { reversed(path) } else { path.clone() };
    let measure = Measure::new(&base.segments());
    let length = measure.length();
    let start = start.clamp(0.0, length);
    let end = end.map(|e| e.clamp(start, length));
    let range = match end {
        None if start <= SAME_ARC => DEFAULT,
        // A default end keeps legacy anchor alignment even when the start has
        // moved.
        None => [measure.position(start), -1.0],
        Some(e) => [measure.position(start), measure.position(e)],
    };
    (base, false, range)
}

/// The segment list (`/1 /0`) written for `path`: on a closed path with the
/// default range the type starts at the last anchor, so with `rotate` the first
/// knot goes last.
fn segment_list(path: &Sub, rotate: bool) -> Option<Vec<f64>> {
    let mut knots = path.knots.clone();
    if rotate && path.closed && !knots.is_empty() {
        knots.rotate_left(1);
    }
    let segs = Sub {
        knots,
        closed: path.closed,
    }
    .segments();
    if segs.is_empty()
        || segs
            .iter()
            .flatten()
            .any(|p| !p[0].is_finite() || !p[1].is_finite())
    {
        return None;
    }
    Some(text_space_points(&segs))
}

/// The frame [`txt2::set_object_frame_path`] writes for `shape`.
///
/// A stored frame ([`ShapeSpec::frame`]) is re-emitted verbatim, so an unedited
/// layer keeps Photoshop's bytes (a box path's 32 numbers, a path frame's exact
/// points/range/flip/matrix). A decoded-curve-only shape is baked into the
/// writer's canonical form: closed paths take the start into the curve, flipped
/// type becomes the path run backwards with `/11 /0 false`, and an open path's
/// moved start or end limit goes into `/6`.
pub fn to_frame_parts(shape: &ShapeSpec) -> Option<txt2::FramePath> {
    if let Some(frame) = &shape.frame {
        return Some(txt2::FramePath {
            points: frame.points.clone(),
            kind: frame.kind,
            range: frame.range,
            flip: frame.flip,
            matrix: frame.matrix,
            vertical: None,
        });
    }
    let path = shape.path.as_ref()?;
    if path.knots.is_empty() {
        return None;
    }
    let (sub, rotate, range) = frame_parts(&Sub::from_path(path), path.start, path.end, path.flip);
    let points = segment_list(&sub, rotate)?;
    Some(txt2::FramePath {
        points,
        kind: PATH_FRAME,
        range,
        flip: Some(false),
        matrix: None,
        vertical: None,
    })
}

#[cfg(test)]
mod tests;
