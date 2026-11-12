//! Line breaking for area text: greedy first-fit and an every-line
//! (Knuth-Plass style) optimiser over the same piece model.
//!
//! A line is the piece range `[start, end)`. Its measured content excludes one
//! trailing space, a hanging left prefix and a hanging right suffix (see
//! [`crate::typography`] for the measured character tables).
//! Adjustment ratio `r` is the slack divided by the stretch (or shrink) the
//! line has. Badness `B = 100|r|^3`, line cost `B^3`. A line that is the last of
//! its paragraph and fits costs 0. A line that ends inside a word adds
//! `FLAG_COST`, the cost of a ratio-1 line.

use crate::typography::{is_left_hang, is_right_hang, left_hang_fit, right_hang_fit, kinsoku};

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

/// Content range of a line and its measured width.
///
/// `start..end` is the content that takes part in justification and alignment;
/// `start` is after a hanging left prefix and `end` is before a hanging right
/// suffix. `hang_end` is the end of that suffix (exclusive). `width` is the
/// width the line-fit test compares with the available measure: hanging
/// characters contribute only their paid share (`typography::left_hang_fit` /
/// `right_hang_fit`; 0 = hangs entirely in the margin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub start: usize,
    pub end: usize,
    pub hang_end: usize,
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
pub struct Breaker {
    /// Available width of the paragraph's first line and of the following lines.
    pub avail: [f32; 2],
    /// Justified paragraphs stretch and shrink lines; otherwise lines are ragged.
    pub justify: bool,
    /// Hanging punctuation (measured tables in `typography`).
    pub hang: bool,
    /// Kinsoku line-breaking rules. The rules themselves are the same tables
    /// the hanging code uses.
    pub kinsoku: bool,
}

fn ratio_badness(slack: f64, room: f64) -> f64 {
    if room <= 1e-9 {
        return if slack <= FIT_EPS { 0.0 } else { 100.0 * MAX_RATIO.powi(3) };
    }
    100.0 * (slack / room).min(MAX_RATIO).powi(3)
}

impl Breaker {
    /// Available width for line `line` of the paragraph (0 = first).
    pub fn avail_for(&self, line: usize) -> f64 {
        f64::from(self.avail[usize::from(line > 0)])
    }

    /// Width a piece adds to the fit test, and the part of its advance that
    /// hangs when it is the first (`left`) or last (`right`) piece of a line.
    fn paid(piece: &Piece, left: bool) -> f64 {
        let fit = if left {
            left_hang_fit(piece.ch).unwrap_or(1.0)
        } else {
            right_hang_fit(piece.ch).unwrap_or(1.0)
        };
        f64::from(piece.advance) * f64::from(fit)
    }

    /// Content range and fitted width of line `[start, end)` at line index `line`.
    pub fn extent(&self, p: &Pieces, start: usize, end: usize) -> Extent {
        let mut content_end = end;
        if content_end > start && p.items[content_end - 1].ch == ' ' {
            content_end -= 1;
        }
        let mut cstart = start;
        let mut hang_start = content_end;
        if self.hang {
            while cstart < content_end && is_left_hang(p.items[cstart].ch) {
                cstart += 1;
            }
            while hang_start > cstart && is_right_hang(p.items[hang_start - 1].ch) {
                hang_start -= 1;
            }
            // A line made only of hanging punctuation cannot hang; keep it in
            // the measured content instead.
            if cstart >= hang_start {
                cstart = start;
                hang_start = content_end;
            }
        }
        let mut width = 0.0;
        for piece in &p.items[start..cstart] {
            width += Self::paid(piece, true);
        }
        width += p.natural(cstart, hang_start);
        for piece in &p.items[hang_start..content_end] {
            width += Self::paid(piece, false);
        }
        Extent { start: cstart, end: hang_start, hang_end: content_end, width }
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
        let ext = self.extent(p, start, end);
        let slack = a - ext.width;
        let badness = if slack >= 0.0 {
            if last {
                0.0
            } else {
                let room = if self.justify { p.stretch_room(ext.start, ext.end) } else { a };
                ratio_badness(slack, room)
            }
        } else {
            let shrink = p.shrink_room(ext.start, ext.end);
            if !self.justify || last || shrink <= 0.0 || -slack > shrink + FIT_EPS {
                return None;
            }
            100.0 * (slack / shrink).abs().powi(3)
        };
        Some(badness.powi(3) + flag)
    }

    /// Whether a line may start with `ch`. Hanging punctuation is allowed to
    /// start a line (that is the whole point of the feature); everything else
    /// follows the kinsoku table.
    fn may_start(&self, ch: char) -> bool {
        if !self.kinsoku {
            return true;
        }
        if self.hang && is_left_hang(ch) {
            return true;
        }
        kinsoku::may_start_line(ch)
    }

    /// Whether a line may end with `ch` at a soft break.
    fn may_end(&self, ch: char) -> bool {
        if !self.kinsoku {
            return true;
        }
        if self.hang && is_right_hang(ch) {
            return true;
        }
        kinsoku::may_end_line(ch)
    }

    /// Pulls punctuation off forbidden break points (kinsoku). A line that
    /// would start with closing punctuation takes that character from the next
    /// line; a line that would end with opening punctuation gives it back.
    /// [INFERENCE] the exact push-in/push-out choice is not measured; this is
    /// the standard JIS X 4051 "push in" adjustment and never produces an
    /// empty line.
    fn repair_kinsoku(&self, p: &Pieces, ends: Vec<usize>) -> Vec<usize> {
        if !self.kinsoku {
            return ends;
        }
        let n = p.len();
        let mut out = ends;
        for _ in 0..4 {
            let mut changed = false;
            let mut start = 0;
            for slot in &mut out {
                let mut e = (*slot).max(start + 1).min(n);
                if e < n {
                    // Pull a closing character off the next line's start.
                    if e > start + 1 && !self.may_start(p.items[e].ch) {
                        while e < n && !self.may_start(p.items[e].ch) {
                            e += 1;
                        }
                    }
                    // Push an opening character to the next line.
                    while e > start + 1 && !self.may_end(p.items[e - 1].ch) {
                        e -= 1;
                    }
                }
                if e != *slot {
                    *slot = e;
                    changed = true;
                }
                start = e;
            }
            if !changed {
                break;
            }
        }
        out
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
            let fits = |e: usize| self.extent(p, start, e).width <= self.avail_for(line) + FIT_EPS;
            let word_end = |e: usize| e == n || p.items[e - 1].ch == ' ';
            let pick = (start + 1..=n)
                .rev()
                .find(|&e| word_end(e) && fits(e))
                .or_else(|| (start + 1..=n).rev().find(|&e| fits(e)))
                .unwrap_or(start + 1);
            ends.push(pick);
            start = pick;
        }
        self.repair_kinsoku(p, ends)
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
        // Content can be several pieces shorter than its span (trailing space,
        // hanging prefix and suffix runs), so the window is generous.
        let window = max_a + 8.0 * max_nat;
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
        self.repair_kinsoku(p, ends)
    }

    /// Per-piece advance deltas that justify line `[start, end)` to its
    /// available width, spread over the stretch (or shrink) of each piece.
    ///
    /// Photoshop's paragraph sheet gives word, letter and glyph spacing as
    /// `[minimum, desired, maximum]`; the ranges set each piece's capacity, so
    /// the slack reaches spaces, letters and glyphs in proportion to how much
    /// each is allowed to move. With the corpus default
    /// (`WordSpacing [0.8 1.0 1.33]`, `LetterSpacing [0 0 0]`,
    /// `GlyphSpacing [1 1 1]`) only spaces move and the whole slack goes there,
    /// reproducing `corpus/box_justify.pdf` (see
    /// `tests/paragraph_layout.rs`). Hanging prefix/suffix glyphs never move.
    pub fn distribute(&self, p: &Pieces, start: usize, end: usize, line: usize) -> Vec<f32> {
        let ext = self.extent(p, start, end);
        let slack = self.avail_for(line) - ext.width;
        let mut out = vec![0f32; end - start];
        let total = if slack >= 0.0 {
            p.stretch_room(ext.start, ext.end)
        } else {
            p.shrink_room(ext.start, ext.end)
        };
        if total <= 1e-9 {
            return out;
        }
        for i in ext.start..ext.end {
            let piece = p.items[i];
            let w = if slack >= 0.0 { piece.stretch } else { piece.shrink };
            out[i - start] = (slack * f64::from(w) / total) as f32;
        }
        out
    }
}

impl Breaker {
    /// Constructor used by `area`: hanging and kinsoku are both on.
    pub fn new(avail: [f32; 2], justify: bool, hang: bool) -> Self {
        Self { avail, justify, hang, kinsoku: true }
    }
}
