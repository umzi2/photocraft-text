use crate::error::Error;

use super::path::{Step, Value, replace_value, resolve, value_span};
use super::syntax::{Kind, Token, skip_value, tokens};

// Run table. The layer text's style runs are `1 / 1[0] / 0 / 0 / 6 / 0[i]`:
// one dict per run. Each run dict holds its length in UTF-16 units at `/1`
// and its font size at `/0 /0 /6 /1`. Per-glyph advance and bounds tables
// (`1.0.9`, `1.1.0.1`) are not rebuilt by this module.

fn run_array() -> Vec<Step<'static>> {
    vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(0),
        Step::Key("0"),
        Step::Key("6"),
        Step::Key("0"),
    ]
}

/// Path to style run `run` (plus `tail`), for callers outside this module.
pub fn run_path_for(run: usize) -> Vec<Step<'static>> {
    run_path(run, &[])
}

pub(super) fn run_path<'a>(run: usize, tail: &[Step<'a>]) -> Vec<Step<'a>> {
    let mut p = run_array();
    p.push(Step::Index(run));
    p.extend_from_slice(tail);
    p
}

fn run_count_in(toks: &[Token]) -> Result<usize, Error> {
    let open =
        resolve(toks, &run_array()).ok_or_else(|| Error::Txt2("run table not found".into()))?;
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

fn run_lengths_in(toks: &[Token]) -> Result<Vec<i64>, Error> {
    let n = run_count_in(toks)?;
    (0..n)
        .map(|i| {
            let v = resolve(toks, &run_path(i, &[Step::Key("1")]))
                .ok_or_else(|| Error::Txt2(format!("run {i} has no length")))?;
            toks[v]
                .kind
                .as_int()
                .ok_or_else(|| Error::Txt2(format!("run {i} length is not an integer")))
        })
        .collect()
}

/// Number of style runs in the layer text.
pub fn run_count(block: &[u8]) -> Result<usize, Error> {
    run_count_in(&tokens(block)?)
}

/// Length of every style run in UTF-16 units, in text order.
pub fn run_lengths(block: &[u8]) -> Result<Vec<i64>, Error> {
    run_lengths_in(&tokens(block)?)
}

/// Writes new run lengths. The count must match the run table and the sum must
/// cover the whole text. \[INFERENCE\]: run lengths include the U+000D terminator
/// (plain_latin: text of 29 units, run length 30).
pub fn set_run_lengths(block: &[u8], lengths: &[i64]) -> Result<Vec<u8>, Error> {
    let n = run_count(block)?;
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
            Step::Index(0),
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
        out = replace_value(&out, &run_path(i, &[Step::Key("1")]), &Value::Int(*len))?;
    }
    Ok(out)
}

/// Splits the single style run of a freshly written block into runs of the
/// given lengths, in text order. The last length takes the U+000D terminator.
pub fn set_run_layout(block: &[u8], lengths: &[i64]) -> Result<Vec<u8>, Error> {
    let n = run_count(block)?;
    if n != 1 {
        return Err(Error::Txt2(format!("expected one run, found {n}")));
    }
    if lengths.is_empty() {
        return Err(Error::Txt2("expected at least one run length".into()));
    }
    let mut out = block.to_vec();
    for _ in 1..lengths.len() {
        out = duplicate_run(&out, 0)?;
    }
    let total = run_lengths(&out)?.iter().sum::<i64>();
    let mut wanted = lengths.to_vec();
    let sum: i64 = wanted.iter().sum();
    if sum + 1 == total {
        // The layer string carries a terminating U+000D that the last run owns.
        let last = wanted.len() - 1;
        wanted[last] += 1;
    }
    set_run_lengths(&out, &wanted)
}

/// Appends a copy of style run `from` to the run table. The copy takes one unit
/// from the last run, so the lengths still cover the text exactly. Fails when
/// the last run has only one unit. \[INFERENCE\]: per-glyph tables are not updated.
pub fn duplicate_run(block: &[u8], from: usize) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let n = run_count_in(&toks)?;
    if from >= n {
        return Err(Error::Txt2(format!("run {from} out of {n}")));
    }
    let mut lengths = run_lengths_in(&toks)?;
    // The new run takes one unit from the longest run, so repeated splits
    // always have a donor.
    let donor = (0..n)
        .max_by_key(|&i| lengths[i])
        .filter(|&i| lengths[i] >= 2)
        .ok_or_else(|| Error::Txt2("no run has a spare unit to split off".into()))?;
    let (rs, re) = value_span(&toks, &run_path(from, &[]))
        .ok_or_else(|| Error::Txt2("source run not found".into()))?;
    let open =
        resolve(&toks, &run_array()).ok_or_else(|| Error::Txt2("run table not found".into()))?;
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
    set_run_lengths(&out, &lengths)
}
