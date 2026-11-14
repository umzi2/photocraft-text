//! Dumps the `EngineData` of every text layer in a PSD twice: as Photoshop
//! wrote it, and as this crate's writer produces it for the parsed layer spec.
//! The two files are diffed by `out/verify/sync/ed_parity_diff.py`.
use photocraft_text::engine_data::{self, Value};

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
}

fn dump(v: &Value, path: &str, out: &mut String) {
    match v {
        Value::Dict(d) => {
            for (k, v) in d {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                dump(v, &p, out);
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                dump(v, &format!("{path}[{i}]"), out);
            }
        }
        Value::Bool(b) => out.push_str(&format!("{path}\tB\t{b}\n")),
        Value::Int(i) => out.push_str(&format!("{path}\tI\t{i}\n")),
        Value::Float(f) => out.push_str(&format!("{path}\tR\t{f:.6}\n")),
        Value::Str(s) | Value::Name(s) => {
            out.push_str(&format!("{path}\tS\t{}\n", esc(s)));
        }
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: ed_parity <file.psd> <out-prefix>");
    let prefix = std::env::args().nth(2).expect("out prefix");
    let bytes = std::fs::read(&path).unwrap();
    let file = photocraft_psd::PsdFile::from_bytes(&bytes).unwrap();
    let mut ps = String::new();
    let mut ours = String::new();
    for i in 0..file.layers().len() {
        let Some(Ok(tysh)) = file.text_layer_tysh(i) else {
            continue;
        };
        let Some(raw) = tysh.engine_data() else {
            continue;
        };
        let Ok(tree) = engine_data::parse(raw) else {
            continue;
        };
        let name = file.layers()[i].name().to_string();
        dump(&tree, &format!("{i}:{name}"), &mut ps);
        let Ok(spec) = photocraft_text::psd::read_layer(&file, i) else {
            continue;
        };
        let built = photocraft_text::psd::build_engine_data(&spec);
        dump(&built, &format!("{i}:{name}"), &mut ours);
    }
    std::fs::write(format!("{prefix}-ps.txt"), ps).unwrap();
    std::fs::write(format!("{prefix}-ours.txt"), ours).unwrap();
    println!("wrote {prefix}-ps.txt/{prefix}-ours.txt");
}
