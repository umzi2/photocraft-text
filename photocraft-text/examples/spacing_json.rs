//! Dumps Photoshop's composed layout as JSON for the spacing analysis tools.

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
    print!("{{\"objects\":[");
    let mut first = true;
    for object in 0..256usize {
        let Ok(layout) =
            txt2::composed_layout_oriented(&block, object, txt2::Orientation::Horizontal)
        else {
            break;
        };
        if layout.lines.is_empty() {
            continue;
        }
        let text = txt2::object_text(&block, object).unwrap_or_default();
        if !first {
            print!(",");
        }
        first = false;
        print!("{{\"object\":{object},\"text\":");
        print!("{}", json_str(&text));
        let font = txt2::run_property_at(&block, object, 0, txt2::RunProperty::FontIndex)
            .and_then(|v| v.as_i64())
            .and_then(|i| txt2::font_name(&block, i as usize));
        let size = txt2::run_property_at(&block, object, 0, txt2::RunProperty::FontSize)
            .and_then(|v| v.as_f64());
        print!(",\"font\":{}", json_str(font.as_deref().unwrap_or("")));
        print!(
            ",\"size\":{}",
            size.map(|s| format!("{s:.6}"))
                .unwrap_or_else(|| "null".into())
        );

        print!(",\"lines\":[");
        for (i, line) in layout.lines.iter().enumerate() {
            if i > 0 {
                print!(",");
            }
            print!(
                "{{\"origin\":[{:.6},{:.6}],\"baseline\":{:.6},\"ascent\":{:.6},\"descent\":{:.6},\"runs\":[",
                line.origin.0, line.origin.1, line.baseline, line.ascent, line.descent
            );
            for (j, run) in line.runs.iter().enumerate() {
                if j > 0 {
                    print!(",");
                }
                print!(
                    "{{\"origin\":[{:.6},{:.6}],\"bounds\":[{:.6},{:.6},{:.6},{:.6}],\"gids\":{:?},\"adj\":[",
                    run.origin.0,
                    run.origin.1,
                    run.bounds[0],
                    run.bounds[1],
                    run.bounds[2],
                    run.bounds[3],
                    run.gids
                );
                for (k, adj) in run.adjustments.iter().enumerate() {
                    if k > 0 {
                        print!(",");
                    }
                    match adj {
                        Some(v) => print!("{v:.6}"),
                        None => print!("null"),
                    }
                }
                print!("]}}");
            }
            print!("]}}");
        }
        print!("]}}");
    }
    println!("]}}");
    Ok(())
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
