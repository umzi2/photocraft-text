//! Prints Photoshop's composed layout (per line: origin, baseline, per-run
//! bounds/gids/adjustments) for every text object of a PSD, read from the
//! `Txt2` cache Photoshop itself wrote. The oracle dump for spacing work.

use photocraft_psd::PsdFile;
use photocraft_text::txt2;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let file = PsdFile::from_bytes(&std::fs::read(&a[1])?)?;
    let block = file
        .global_block(b"Txt2")
        .ok_or("no Txt2 block")?
        .data
        .clone();
    for object in 0..64usize {
        let Ok(layout) =
            txt2::composed_layout_oriented(&block, object, txt2::Orientation::Horizontal)
        else {
            break;
        };
        if layout.lines.is_empty() {
            continue;
        }
        println!("object {object}: {} lines", layout.lines.len());
        for (i, line) in layout.lines.iter().enumerate() {
            print!(
                "  line {i}: origin=({:.4},{:.4}) baseline={:.4} asc={:.4} desc={:.4}",
                line.origin.0, line.origin.1, line.baseline, line.ascent, line.descent
            );
            for run in &line.runs {
                print!(
                    " | run origin=({:.4},{:.4}) bounds=[{:.4},{:.4},{:.4},{:.4}] gids={:?} adj={:?}",
                    run.origin.0,
                    run.origin.1,
                    run.bounds[0],
                    run.bounds[1],
                    run.bounds[2],
                    run.bounds[3],
                    run.gids,
                    run.adjustments
                );
            }
            println!();
        }
    }
    Ok(())
}
