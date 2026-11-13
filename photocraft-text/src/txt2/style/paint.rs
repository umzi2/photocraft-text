use crate::error::Error;

use super::super::path::{Step, ensure_container, format_real, value_span};
use super::super::runs::run_path;
use super::super::syntax::tokens;
use super::RunProperty;

/// Writes a run's fill colour as Photoshop's `SimplePaint` dictionary:
/// `/53 << /99 /SimplePaint /0 << /0 1 /1 [ a r g b ] >> >>` (alpha first).
pub fn set_run_fill_color(block: &[u8], run: usize, rgba: [f32; 4]) -> Result<Vec<u8>, Error> {
    set_run_paint(block, run, RunProperty::FillPaint, rgba)
}

/// Reads a run's fill colour, if it carries one.
pub fn run_fill_color(block: &[u8], run: usize) -> Option<[f32; 4]> {
    run_paint(block, run, RunProperty::FillPaint)
}

/// Writes a run's stroke colour in the same `SimplePaint` shape at `/54`
/// (`out/probe2/base.psd`: `/54 << /99 /SimplePaint /0 << /0 1 /1 [ 1.0 0.0
/// 0.0 0.0 ] >> >>`).
pub fn set_run_stroke_color(block: &[u8], run: usize, rgba: [f32; 4]) -> Result<Vec<u8>, Error> {
    set_run_paint(block, run, RunProperty::StrokePaint, rgba)
}

/// Reads a run's stroke colour, if it carries one.
pub fn run_stroke_color(block: &[u8], run: usize) -> Option<[f32; 4]> {
    run_paint(block, run, RunProperty::StrokePaint)
}

fn paint_path(run: usize, property: RunProperty) -> Vec<Step<'static>> {
    run_path(
        run,
        &[
            Step::Key("0"),
            Step::Key("0"),
            Step::Key("6"),
            Step::Key(property.key()),
        ],
    )
}

fn set_run_paint(
    block: &[u8],
    run: usize,
    property: RunProperty,
    rgba: [f32; 4],
) -> Result<Vec<u8>, Error> {
    let value = format!(
        "<< /99 /SimplePaint /0 << /0 1 /1 [ {} {} {} {} ] >> >>",
        format_real(f64::from(rgba[0])),
        format_real(f64::from(rgba[1])),
        format_real(f64::from(rgba[2])),
        format_real(f64::from(rgba[3])),
    );
    ensure_container(block, &paint_path(run, property), value.as_bytes())
}

fn run_paint(block: &[u8], run: usize, property: RunProperty) -> Option<[f32; 4]> {
    let (s, e) = value_span(&tokens(block).ok()?, &paint_path(run, property))?;
    let text = std::str::from_utf8(&block[s..e]).ok()?;
    let at = text.find("/1 [")? + 4;
    let body = text[at..].split(']').next()?;
    let nums: Vec<f32> = body
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    match nums.as_slice() {
        [a, r, g, b] => Some([*a, *r, *g, *b]),
        _ => None,
    }
}
