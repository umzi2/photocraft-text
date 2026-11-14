use crate::error::Error;

use super::path::{Step, Value, ensure_value, replace_value, resolve, value_span};
use super::syntax::{Kind, Token, skip_value, tokens};

// Run table. The layer text's style runs are `1 / 1[object] / 0 / 6 / 0[i]`:
// one dict per run. Each run item is `<< /0 << /0 << /0 (name) /5 1 /6 <sheet>
// >> >> /1 len >>`: the length in UTF-16 units at `/1`, the character-run
// marker at `/0 /0 /5` and the character sheet at `/0 /0 /6`. Per-glyph advance
// and bounds tables (`1.0.9`, `1.1.0.1`) are not rebuilt by this module.
//
// Every function takes the text object's index in the block's `/1 /1` array;
// the object-0 forms are thin wrappers for the single-object callers.

fn run_array_at(object: usize) -> Vec<Step<'static>> {
    vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("0"),
        Step::Key("6"),
        Step::Key("0"),
    ]
}

/// Path to style run `run` of text object `object` (plus `tail`).
pub fn run_path_for_at(object: usize, run: usize) -> Vec<Step<'static>> {
    run_path_at(object, run, &[])
}

/// Path to style run `run` of text object `object` (plus `tail`).
pub(super) fn run_path_at<'a>(object: usize, run: usize, tail: &[Step<'a>]) -> Vec<Step<'a>> {
    let mut p = run_array_at(object);
    p.push(Step::Index(run));
    p.extend_from_slice(tail);
    p
}

/// Path to style run `run` of text object 0 (plus `tail`).
pub fn run_path_for(run: usize) -> Vec<Step<'static>> {
    run_path(run, &[])
}

pub(super) fn run_path<'a>(run: usize, tail: &[Step<'a>]) -> Vec<Step<'a>> {
    run_path_at(0, run, tail)
}

fn run_count_in(toks: &[Token], object: usize) -> Result<usize, Error> {
    let open = resolve(toks, &run_array_at(object))
        .ok_or_else(|| Error::Txt2("run table not found".into()))?;
    if !matches!(toks[open].kind, Kind::ArrayOpen) {
        return Err(Error::Txt2("run table is not an array".into()));
    }
    let mut j = open + 1;
    let mut n = 0;
    while !matches!(
        toks.get(j)
            .ok_or_else(|| Error::Txt2("unterminated run table".into()))?
            .kind,
        Kind::ArrayClose
    ) {
        j = skip_value(toks, j).ok_or_else(|| Error::Txt2("bad run element".into()))?;
        n += 1;
    }
    Ok(n)
}

fn run_lengths_in(toks: &[Token], object: usize) -> Result<Vec<i64>, Error> {
    let n = run_count_in(toks, object)?;
    (0..n)
        .map(|i| {
            let v = resolve(toks, &run_path_at(object, i, &[Step::Key("1")]))
                .ok_or_else(|| Error::Txt2(format!("run {i} has no length")))?;
            toks[v]
                .kind
                .as_int()
                .ok_or_else(|| Error::Txt2(format!("run {i} length is not an integer")))
        })
        .collect()
}

/// Number of style runs of text object `object`.
pub fn run_count_at(block: &[u8], object: usize) -> Result<usize, Error> {
    run_count_in(&tokens(block)?, object)
}

/// Length of every style run of text object `object` in UTF-16 units, in text order.
pub fn run_lengths_at(block: &[u8], object: usize) -> Result<Vec<i64>, Error> {
    run_lengths_in(&tokens(block)?, object)
}

/// Number of style runs in the layer text (text object 0).
pub fn run_count(block: &[u8]) -> Result<usize, Error> {
    run_count_at(block, 0)
}

/// Length of every style run in UTF-16 units, in text order (text object 0).
pub fn run_lengths(block: &[u8]) -> Result<Vec<i64>, Error> {
    run_lengths_at(block, 0)
}

/// Writes new run lengths. The count must match the run table and the sum must
/// cover the whole text. \[INFERENCE\]: run lengths include the U+000D terminator
/// (plain_latin: text of 29 units, run length 30).
pub fn set_run_lengths(block: &[u8], lengths: &[i64]) -> Result<Vec<u8>, Error> {
    set_run_lengths_at(block, 0, lengths)
}

/// Writes new run lengths for text object `object`.
pub fn set_run_lengths_at(block: &[u8], object: usize, lengths: &[i64]) -> Result<Vec<u8>, Error> {
    let n = run_count_at(block, object)?;
    if lengths.len() != n {
        return Err(Error::Txt2(format!(
            "{} lengths for {} runs",
            lengths.len(),
            n
        )));
    }
    // The layer string's raw body is BOM + units (CR terminator included), so
    // its unit count is (len - 2) / 2. read_text stops at the first CR and would
    // undercount multi-paragraph text.
    let toks = tokens(block)?;
    let text = resolve(
        &toks,
        &[
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(object),
            Step::Key("0"),
            Step::Key("0"),
        ],
    )
    .ok_or_else(|| Error::Txt2("layer text not found".into()))?;
    let units = match &toks[text].kind {
        Kind::Str(body) => {
            // `body` is the raw token: BOM, UTF-16BE units (possibly with
            // backslash escapes) and the 00 0d terminator.  Unescape before
            // dividing, otherwise one parenthesis inflates the count.
            let mut raw = Vec::with_capacity(body.len());
            let mut i = 0;
            while i < body.len() {
                if body[i] == b'\\' && i + 1 < body.len() {
                    i += 1;
                }
                raw.push(body[i]);
                i += 1;
            }
            (raw.len() as i64 - 2) / 2
        }
        _ => return Err(Error::Txt2("layer text is not a string".into())),
    };
    let total: i64 = lengths.iter().sum();
    if total != units {
        return Err(Error::Txt2(format!(
            "run lengths sum to {total}, text needs {units}"
        )));
    }
    let mut out = block.to_vec();
    for (i, len) in lengths.iter().enumerate() {
        out = replace_value(
            &out,
            &run_path_at(object, i, &[Step::Key("1")]),
            &Value::Int(*len),
        )?;
    }
    Ok(out)
}

/// Splits the single style run of a freshly written block into runs of the
/// given lengths, in text order. The last length takes the U+000D terminator.
pub fn set_run_layout(block: &[u8], lengths: &[i64]) -> Result<Vec<u8>, Error> {
    set_run_layout_at(block, 0, lengths)
}

/// [`set_run_layout`] for text object `object`.
pub fn set_run_layout_at(block: &[u8], object: usize, lengths: &[i64]) -> Result<Vec<u8>, Error> {
    let n = run_count_at(block, object)?;
    if n != 1 {
        return Err(Error::Txt2(format!("expected one run, found {n}")));
    }
    if lengths.is_empty() {
        return Err(Error::Txt2("expected at least one run length".into()));
    }
    let mut out = block.to_vec();
    for _ in 1..lengths.len() {
        out = duplicate_run_at(&out, object, 0)?;
    }
    let total = run_lengths_at(&out, object)?.iter().sum::<i64>();
    let mut wanted = lengths.to_vec();
    let sum: i64 = wanted.iter().sum();
    if sum + 1 == total {
        // The layer string carries a terminating U+000D that the last run owns.
        let last = wanted.len() - 1;
        wanted[last] += 1;
    }
    set_run_lengths_at(&out, object, &wanted)
}

/// Gives text object `object` Photoshop's one-run style table when it has none:
/// `<< /0 << /0 () /5 << >> /6 1 >> >> /1 1 >>`, which [`set_run_layout_at`]
/// then refits. A table with runs is left as it is.
pub fn ensure_style_run_at(block: &[u8], object: usize) -> Result<Vec<u8>, Error> {
    if run_count_at(block, object)? != 0 {
        return Ok(block.to_vec());
    }
    super::path::replace_container(
        block,
        &run_array_at(object),
        b"[ << /0 << /0 << /0 () /5 1 /6 << /0 0 /11 1 >> >> >> /1 1 >> ]",
    )
}

/// Appends a copy of style run `from` to the run table. The copy takes one unit
/// from the last run, so the lengths still cover the text exactly. Fails when
/// the last run has only one unit. \[INFERENCE\]: per-glyph tables are not updated.
pub fn duplicate_run(block: &[u8], from: usize) -> Result<Vec<u8>, Error> {
    duplicate_run_at(block, 0, from)
}

/// [`duplicate_run`] for text object `object`.
pub fn duplicate_run_at(block: &[u8], object: usize, from: usize) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let n = run_count_in(&toks, object)?;
    if from >= n {
        return Err(Error::Txt2(format!("run {from} out of {n}")));
    }
    let mut lengths = run_lengths_in(&toks, object)?;
    // The new run takes one unit from the longest run, so repeated splits
    // always have a donor.
    let donor = (0..n)
        .max_by_key(|&i| lengths[i])
        .filter(|&i| lengths[i] >= 2)
        .ok_or_else(|| Error::Txt2("no run has a spare unit to split off".into()))?;
    let (rs, re) = value_span(&toks, &run_path_at(object, from, &[]))
        .ok_or_else(|| Error::Txt2("source run not found".into()))?;
    let open = resolve(&toks, &run_array_at(object))
        .ok_or_else(|| Error::Txt2("run table not found".into()))?;
    let mut j = open + 1;
    while !matches!(toks[j].kind, Kind::ArrayClose) {
        j = skip_value(&toks, j).ok_or_else(|| Error::Txt2("bad run element".into()))?;
    }
    let close = toks[j].start;
    let clone = block[rs..re].to_vec();
    let mut out = block.to_vec();
    let mut insert = Vec::with_capacity(clone.len() + 2);
    insert.push(b' ');
    insert.extend_from_slice(&clone);
    insert.push(b' ');
    out.splice(close..close, insert);
    lengths[donor] -= 1;
    lengths.push(1);
    set_run_lengths_at(&out, object, &lengths)
}

/// Writes the character-run marker `/5 1` of style run `run` in text object
/// `object` (the wrapper `<< /0 << /0 << /0 () /5 1 /6 … >> >> /1 len >>`).
///
/// Photoshop's composer treats a style run without the marker as having no
/// style runs of its own and falls back to the block's shared default sheet,
/// so every run this crate writes carries it (`docs/text-engine-migration.md`).
pub fn set_character_run_marker(block: &[u8], object: usize, run: usize) -> Result<Vec<u8>, Error> {
    ensure_value(
        block,
        &run_path_at(
            object,
            run,
            &[Step::Key("0"), Step::Key("0"), Step::Key("5")],
        ),
        &Value::Int(1),
    )
}
