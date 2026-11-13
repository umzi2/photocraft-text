//! Hyphenation with Adobe's Hunspell-style dictionaries.
//!
//! Photoshop 20.0.2 (CC 2019) hyphenates through the Hunspell provider that
//! ships in its `Required/Linguistics/.../AdobeHunspellPlugin/Dictionaries`
//! tree.  The `hyph_en_US.dic` there is a Liang pattern dictionary: patterns
//! such as `a1bc` mark odd positions as legal breaks once the maximum digit of
//! every matching pattern is taken.  Applying it to `incomprehensible` yields
//! breaks after `in`, `incom`, … and Photoshop's own render of
//! `10_all_features.psd` layer `14 hyphenation composer` breaks the word as
//! `incom-`, which is the break this module reproduces.
//!
//! The dictionary is Adobe-proprietary and is *not* embedded in this crate.
//! The caller points the engine at a `.dic` file (the test harness uses the
//! copy under the Wine Photoshop install via `PHOTOCRAFT_HYPHEN_DIC`); without
//! one, [`default`] returns `None` and paragraphs are laid out unhyphenated.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// One parsed Liang pattern: literal characters plus digit marks.
#[derive(Clone, Debug, Default)]
struct Pattern {
    letters: Vec<char>,
    /// `(index into letters, digit)`; the digit sits *before* `letters[index]`.
    marks: Vec<(usize, u8)>,
}

impl Pattern {
    fn parse(text: &str) -> Option<Self> {
        let mut letters = Vec::new();
        let mut marks = Vec::new();
        for c in text.chars() {
            if let Some(d) = c.to_digit(10) {
                marks.push((letters.len(), d as u8));
            } else if c.is_whitespace() {
                return None;
            } else {
                letters.push(c.to_ascii_lowercase());
            }
        }
        if letters.is_empty() {
            return None;
        }
        Some(Self { letters, marks })
    }
}

/// A hyphenation dictionary: Liang patterns plus hyphenated exception words.
#[derive(Clone, Debug, Default)]
pub struct Hyphenator {
    patterns: Vec<Pattern>,
    /// First literal of a pattern -> pattern indices, to keep matching cheap.
    by_first: HashMap<char, Vec<usize>>,
    /// Word -> byte positions where a hyphen may be inserted.
    exceptions: HashMap<String, Vec<usize>>,
    /// The dictionary's own `LEFTHYPHENMIN` / `RIGHTHYPHENMIN`.
    left_min: usize,
    right_min: usize,
}

impl Hyphenator {
    /// Parses a Hunspell `.dic` file's text.
    pub fn from_hunspell(text: &str) -> Self {
        let mut out = Self {
            left_min: 2,
            right_min: 2,
            ..Self::default()
        };
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('%') || trimmed.starts_with('#') {
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("LEFTHYPHENMIN") {
                if let Ok(v) = rest.trim().parse() {
                    out.left_min = v;
                }
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("RIGHTHYPHENMIN") {
                if let Ok(v) = rest.trim().parse() {
                    out.right_min = v;
                }
                continue;
            }
            if trimmed.starts_with("NOHYPHEN")
                || trimmed.starts_with("COMPOUND")
                || trimmed.starts_with("NEXTLEVEL")
            {
                continue;
            }
            // `word/flags=hy-phen-at-ed` is an exception entry.
            if let Some((lhs, rhs)) = trimmed.split_once('=') {
                let word = lhs.split('/').next().unwrap_or(lhs).to_ascii_lowercase();
                if !word.is_empty() && rhs.contains('-') {
                    out.exceptions.insert(word, hyphen_positions(rhs));
                }
                continue;
            }
            // A plain word (no digits) is an exception written with hyphens.
            if trimmed.contains('/') {
                let word = trimmed.split('/').next().unwrap_or(trimmed);
                if !word.is_empty() {
                    out.exceptions.entry(word.to_ascii_lowercase()).or_default();
                }
                continue;
            }
            if !trimmed.contains(|c: char| c.is_ascii_digit()) {
                if trimmed
                    .chars()
                    .all(|c| c.is_alphabetic() || c == '.' || c == '\'' || c == '-')
                {
                    // The dictionary keys exceptions by the plain word; the
                    // hyphens in the line only mark the break positions.
                    let word: String = trimmed
                        .chars()
                        .filter(|&c| c != '-')
                        .collect::<String>()
                        .to_ascii_lowercase();
                    if !word.is_empty() {
                        out.exceptions.insert(word, hyphen_positions(trimmed));
                    }
                }
                continue;
            }
            if let Some(pattern) = Pattern::parse(trimmed) {
                let first = pattern.letters[0];
                out.by_first
                    .entry(first)
                    .or_default()
                    .push(out.patterns.len());
                out.patterns.push(pattern);
            }
        }
        out
    }

    /// Loads a `.dic` file.
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::from_hunspell(&text))
    }

    /// Byte offsets inside `word` (after ASCII/Unicode letters) where a hyphen
    /// may be inserted, using `left_min`/`right_min` as the minimum number of
    /// letters kept on either side.
    ///
    /// Only the first alphabetic run of `word` is hyphenated, which is what the
    /// layout needs: it passes single words.
    pub fn breaks(&self, word: &str, left_min: usize, right_min: usize) -> Vec<usize> {
        let left = left_min.max(1);
        let right = right_min.max(1);
        let lower = word.to_lowercase();
        if let Some(found) = self.exceptions.get(&lower) {
            return found
                .iter()
                .copied()
                .filter(|&b| b > 0 && b < word.len())
                .filter(|&b| lower[..b].chars().count() >= left)
                .filter(|&b| lower[b..].chars().count() >= right)
                .collect();
        }
        let chars: Vec<char> = lower.chars().collect();
        if chars.len() < left + right || chars.len() < 4 {
            return Vec::new();
        }
        // Liang: work on `.word.` and keep the highest mark per position.
        let mut wrapped = Vec::with_capacity(chars.len() + 2);
        wrapped.push('.');
        wrapped.extend_from_slice(&chars);
        wrapped.push('.');
        let mut score = vec![0u8; wrapped.len() + 1];
        for start in 0..wrapped.len() {
            let Some(list) = self.by_first.get(&wrapped[start]) else {
                continue;
            };
            for &pi in list {
                let pattern = &self.patterns[pi];
                if start + pattern.letters.len() > wrapped.len() {
                    continue;
                }
                if wrapped[start..start + pattern.letters.len()] != pattern.letters[..] {
                    continue;
                }
                for &(at, digit) in &pattern.marks {
                    let slot = start + at;
                    if slot < score.len() && digit > score[slot] {
                        score[slot] = digit;
                    }
                }
            }
        }
        let mut out = Vec::new();
        for i in 1..chars.len() {
            // `i` letters before the break; the wrapped string has one extra
            // leading '.', so the mark that decides the break sits at `i + 1`.
            if score[i + 1] % 2 == 1 && i >= left && chars.len() - i >= right {
                let byte = chars[..i].iter().map(|c| c.len_utf8()).sum();
                out.push(byte);
            }
        }
        out
    }
}

/// Byte positions of the hyphens in a hyphenated dictionary word.
fn hyphen_positions(hyphenated: &str) -> Vec<usize> {
    // Break offsets are the positions of the hyphens in the line itself
    // (`as-so-ciate` -> 2, 5), which `breaks` then applies to the plain word.
    let parts: Vec<&str> = hyphenated.split('-').collect();
    let mut out = Vec::new();
    let mut bytes = 0;
    for (i, part) in parts.iter().enumerate() {
        bytes += part.len();
        if i + 1 < parts.len() {
            out.push(bytes);
            bytes += 1;
        }
    }
    out
}

/// Path of the hyphenation dictionary to use, from `PHOTOCRAFT_HYPHEN_DIC`.
///
/// The harness points this at Photoshop's own `hyph_en_US.dic`, which keeps the
/// engine's hyphenation decisions identical to the application whose pixels we
/// compare against.  A default search under the Wine Photoshop install is
/// attempted as well so interactive use needs no configuration.
fn discover() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PHOTOCRAFT_HYPHEN_DIC") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let home = std::env::var_os("HOME")?;
    let base = PathBuf::from(home).join(".local/share/photocraft/wine/drive_c/Program Files/Adobe");
    let mut stack = vec![base];
    let mut seen = 0usize;
    // Bounded search: only the Photoshop tree, only `hyph_en_US.dic`.
    while let Some(dir) = stack.pop() {
        seen += 1;
        if seen > 4096 {
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some("hyph_en_US.dic") {
                return Some(path);
            }
        }
    }
    None
}

/// The process-wide dictionary, loaded on first use.
///
/// Returns `None` when no dictionary is configured or discoverable; layouts
/// then fall back to no hyphenation instead of failing.
pub fn default() -> Option<&'static Hyphenator> {
    static INSTANCE: OnceLock<Option<Hyphenator>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            let path = discover()?;
            Hyphenator::from_path(&path).ok()
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A handful of real patterns from Adobe's en-US dictionary, enough to pin
    // the Liang algorithm (the full file is proprietary and not embedded).
    const SAMPLE: &str = "\
% comment
LEFTHYPHENMIN 2
RIGHTHYPHENMIN 2
a1bc
in1com
com1pre
pre1hen
hen1si
si1ble
be1tween
a1bc
";

    #[test]
    fn liang_marks_and_minima_produce_adobe_breaks() {
        let h = Hyphenator::from_hunspell(SAMPLE);
        assert_eq!(h.breaks("incomprehensible", 2, 3), vec![2, 5, 8, 11, 13]);
        assert_eq!(h.breaks("between", 2, 2), vec![2]);
        // Nothing before left_min or after right_min.
        assert_eq!(h.breaks("a", 2, 2), Vec::<usize>::new());
    }

    #[test]
    fn exceptions_use_their_own_hyphenation() {
        let h = Hyphenator::from_hunspell("as-so-ciate\n");
        assert_eq!(h.breaks("associate", 2, 2), vec![2, 5]);
        // The paragraph's minima still apply: break 2 keeps only two letters,
        // so it is dropped; break 5 keeps five before and four after.
        assert_eq!(h.breaks("associate", 3, 3), vec![5]);
    }
}
