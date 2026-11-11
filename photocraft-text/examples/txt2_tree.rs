//! Prints the `Txt2` token tree, depth-limited, for structural inspection.
//!
//! `cargo run --example txt2_tree -- corpus/plain_latin.psd [depth]`

use photocraft_psd::PsdFile;
use photocraft_text::txt2::{self, Kind};

fn walk(block: &[u8], toks: &[txt2::Token], i: &mut usize, depth: usize, max: usize, out: &mut String) {
    let t = &toks[*i];
    let text = String::from_utf8_lossy(&block[t.start..t.end]);
    let short: String = text.chars().take(60).collect();
    match &t.kind {
        Kind::DictOpen | Kind::ArrayOpen => {
            out.push_str(&format!("{:indent$}{}\n", "", short, indent = depth * 2));
            *i += 1;
            while *i < toks.len() && depth < max {
                if matches!(toks[*i].kind, Kind::DictClose | Kind::ArrayClose) {
                    break;
                }
                walk(block, toks, i, depth + 1, max, out);
            }
            if *i < toks.len() {
                let c = &toks[*i];
                out.push_str(&format!("{:indent$}{}\n", "", String::from_utf8_lossy(&block[c.start..c.end]), indent = depth * 2));
            }
        }
        _ => {
            out.push_str(&format!("{:indent$}{}\n", "", short, indent = depth * 2));
        }
    }
    *i += 1;
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let file = PsdFile::from_bytes(&std::fs::read(&a[1])?)?;
    let block = file.global_block(b"Txt2").ok_or("no Txt2")?.data.clone();
    let depth: usize = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(4);
    let toks = txt2::tokens(&block)?;
    let mut i = 0;
    let mut out = String::new();
    while i < toks.len() {
        walk(&block, &toks, &mut i, 0, depth, &mut out);
    }
    println!("{out}");
    Ok(())
}
