//! Line breaking for area text: greedy first-fit and an every-line
//! (Knuth-Plass style) optimiser over the same piece model.
//!
//! A line is the piece range `[start, end)`. Its content excludes one trailing
//! space and, when hanging is on, one trailing closing punctuation glyph.
//! Adjustment ratio `r` is the slack divided by the stretch (or shrink) the
//! line has. Badness `B = 100|r|^3`, line cost `B^3`. A line that is the last of
//! its paragraph and fits costs 0. A line that ends inside a word adds
//! `FLAG_COST`, the cost of a ratio-1 line.

use crate::typography::{can_hang_trailing, Kinsoku};

/// Cost of a line at ratio 1 (`B = 100`).
const FLAG_COST: f64 = 1e6;
const FIT_EPS: f64 = 1e-4;
/// Caps the ratio so cubic costs stay finite for unstretchable lines.
const MAX_RATIO: f64 = 1e3;

/// One glyph as the breaker sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub ch: char,
    pub advance: f32,
    /// Width the piece may give up when its line is too long (justify only).
    pub shrink: f32,
    /// Width the piece may take when its line is too short (justify only).
    pub stretch: f32,
}

/// Content range of a line and its width. `end` excludes the trailing space
/// and any hung glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub end: usize,
    pub width: f64,
}

/// Pieces of one paragraph with prefix sums for O(1) line measures.
pub struct Pieces {
    pub items: Vec<Piece>,
    nat: Vec<f64>,
    shrink: Vec<f64>,
    stretch: Vec<f64>,
}

fn prefix(items: &[Piece], f: impl Fn(&Piece) -> f32) -> Vec<f64> {
    let mut sums = Vec::with_capacity(items.len() + 1);
    sums.push(0.0);
    for p in items {
        let last = sums[sums.len() - 1];
        sums.push(last + f64::from(f(p)));
    }
    sums
}

impl Pieces {
    pub fn new(items: Vec<Piece>) -> Self {
        Self {
            nat: prefix(&items, |p| p.advance),
            shrink: prefix(&items, |p| p.shrink),
            stretch: prefix(&items, |p| p.stretch),
            items,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn natural(&self, s: usize, e: usize) -> f64 {
        self.nat[e] - self.nat[s]
    }

    fn shrink_room(&self, s: usize, e: usize) -> f64 {
        self.shrink[e] - self.shrink[s]
    }

    fn stretch_room(&self, s: usize, e: usize) -> f64 {
        self.stretch[e] - self.stretch[s]
    }
}

/// Line-fitting rules for one paragraph.
pub struct Breaker<'a> {
    /// Available width of the paragraph's first line and of the following lines.
    pub avail: [f32; 2],
    /// Justified paragraphs stretch and shrink lines; otherwise lines are ragged.
    pub justify: bool,
    /// Hanging punctuation rules; `None` when hanging is off.
    pub hang: Option<&'a Kinsoku>,
}

fn ratio_badness(slack: f64, room: f64) -> f64 {
    if room <= 1e-9 {
        return if slack <= FIT_EPS { 0.0 } else { 100.0 * MAX_RATIO.powi(3) };
    }
    100.0 * (slack / room).min(MAX_RATIO).powi(3)
}

impl Breaker<'_> {
    /// Available width for line `line` of the paragraph (0 = first).
    pub fn avail_for(&self, line: usize) -> f64 {
        f64::from(self.avail[usize::from(line > 0)])
    }

    /// Content range and width of line `[start, end)` at line index `line`.
    pub fn extent(&self, p: &Pieces, start: usize, end: usize, line: usize) -> Extent {
        let a = self.avail_for(line);
        let mut content_end = end;
        if content_end > start && p.items[content_end - 1].ch == ' ' {
            content_end -= 1;
        }
        let width = p.natural(start, content_end);
        if width <= a + FIT_EPS {
            return Extent { end: content_end, width };
        }
        if let Some(kin) = self.hang
            && content_end > start
        {
            let last = p.items[content_end - 1];
            let before = p.natural(start, content_end - 1);
            if last.ch != ' ' && can_hang_trailing(before as f32, last.ch, last.advance, a as f32, kin) {
                return Extent { end: content_end - 1, width: before };
            }
        }
        Extent { end: content_end, width }
    }

    /// Cost of line `[start, end)` at line index `line`, or `None` if the line
    /// cannot be set (too long and not shrinkable).
    fn line_cost(&self, p: &Pieces, start: usize, end: usize, line: usize) -> Option<f64> {
        let n = p.len();
        let flag = if end < n && p.items[end - 1].ch != ' ' && p.items[end].ch != ' ' {
            FLAG_COST
        } else {
            0.0
        };
        if end < n && p.items[end].ch == ' ' {
            // A break here would start the next line with the space that ends
            // this one; the break belongs one piece later.
            return None;
        }
        if end - start <= 1 {
            return Some(flag);
        }
        let last = end == n;
        let a = self.avail_for(line);
        let ext = self.extent(p, start, end, line);
        let slack = a - ext.width;
        let badness = if slack >= 0.0 {
            if last {
                0.0
            } else {
                let room = if self.justify { p.stretch_room(start, ext.end) } else { a };
                ratio_badness(slack, room)
            }
        } else {
            let shrink = p.shrink_room(start, ext.end);
            if !self.justify || last || shrink <= 0.0 || -slack > shrink + FIT_EPS {
                return None;
            }
            100.0 * (slack / shrink).abs().powi(3)
        };
        Some(badness.powi(3) + flag)
    }

    /// First-fit line breaks: the furthest word boundary that fits, else the
    /// furthest character that fits (a word longer than the box is broken
    /// between characters).
    pub fn greedy(&self, p: &Pieces) -> Vec<usize> {
        let n = p.len();
        let mut ends = Vec::new();
        let mut start = 0;
        while start < n {
            let line = ends.len();
            let fits = |e: usize| self.extent(p, start, e, line).width <= self.avail_for(line) + FIT_EPS;
            let word_end = |e: usize| e == n || p.items[e - 1].ch == ' ';
            let pick = (start + 1..=n)
                .rev()
                .find(|&e| word_end(e) && fits(e))
                .or_else(|| (start + 1..=n).rev().find(|&e| fits(e)))
                .unwrap_or(start + 1);
            ends.push(pick);
            start = pick;
        }
        ends
    }

    /// Minimum-badness breaks into exactly `lines` lines. `lines` must be a
    /// count the greedy breaker can reach (its own count always is).
    pub fn optimal(&self, p: &Pieces, lines: usize) -> Vec<usize> {
        let n = p.len();
        if n == 0 || lines == 0 {
            return Vec::new();
        }
        let max_a = f64::from(self.avail[0].max(self.avail[1]));
        let max_nat = p.items.iter().map(|g| f64::from(g.advance)).fold(0.0, f64::max);
        // Content is at most two pieces shorter than its span (trailing space, hung glyph).
        let window = max_a + 2.0 * max_nat;
        let mut cost = vec![vec![f64::INFINITY; n + 1]; lines + 1];
        let mut from = vec![vec![0usize; n + 1]; lines + 1];
        cost[0][0] = 0.0;
        for k in 0..lines {
            for j in 1..=n {
                for i in (0..j).rev() {
                    if p.natural(i, j) - p.shrink_room(i, j) > window {
                        break;
                    }
                    if !cost[k][i].is_finite() {
                        continue;
                    }
                    let Some(c) = self.line_cost(p, i, j, k) else {
                        continue;
                    };
                    let total = cost[k][i] + c;
                    if total < cost[k + 1][j] {
                        cost[k + 1][j] = total;
                        from[k + 1][j] = i;
                    }
                }
            }
        }
        assert!(cost[lines][n].is_finite(), "line count not reachable by the greedy breaker");
        let mut ends = vec![0usize; lines];
        let mut j = n;
        for k in (1..=lines).rev() {
            ends[k - 1] = j;
            j = from[k][j];
        }
        ends
    }

    /// Per-piece advance deltas that justify line `[start, end)` to its
    /// available width, spread over the stretch (or shrink) of each piece.
    pub fn distribute(&self, p: &Pieces, start: usize, end: usize, line: usize) -> Vec<f32> {
        let ext = self.extent(p, start, end, line);
        let slack = self.avail_for(line) - ext.width;
        let mut out = vec![0f32; end - start];
        let total = if slack >= 0.0 {
            p.stretch_room(start, ext.end)
        } else {
            p.shrink_room(start, ext.end)
        };
        if total <= 1e-9 {
            return out;
        }
        for i in start..ext.end {
            let piece = p.items[i];
            let w = if slack >= 0.0 { piece.stretch } else { piece.shrink };
            out[i - start] = (slack * f64::from(w) / total) as f32;
        }
        out
    }
}
