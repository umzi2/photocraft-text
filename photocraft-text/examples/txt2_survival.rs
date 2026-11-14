//! Writes CJK survival probes for the unknown paragraph keys: one candidate
//! value per file, plus a default control and an out-of-range control. The
//! caller opens each file in Photoshop, forces a recompose and saves; the
//! read-back shows whether Photoshop preserved the value (pass-through), reset
//! it (the key is modelled) or dropped it.
//!
//! Usage: `cargo run --example txt2_survival -- <base.psd> <out_dir>`

use photocraft_text::txt2::{Step, Value, replace_value};

const KEYS: [&str; 8] = ["8", "24", "25", "27", "28", "33", "35", "40"];

fn path<'a>(key: &'a str) -> Vec<Step<'a>> {
    vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(0),
        Step::Key("0"),
        Step::Key("5"),
        Step::Key("0"),
        Step::Index(0),
        Step::Key("0"),
        Step::Key("0"),
        Step::Key("5"),
        Step::Key(key),
    ]
}

fn write_variant(base: &str, out_dir: &str, name: &str, key: Option<&str>, value: Option<Value>) {
    let data = std::fs::read(base).expect("read base");
    let mut file = photocraft_psd::PsdFile::from_bytes(&data).expect("parse base");
    let block = file.txt2().expect("Txt2 block").to_vec();
    let next = match (key, value) {
        (Some(key), Some(value)) => replace_value(&block, &path(key), &value).expect("replace"),
        _ => block,
    };
    file.set_txt2(Some(next));
    let out = std::path::Path::new(out_dir).join(format!("{name}.psd"));
    std::fs::write(&out, file.to_bytes().expect("serialize")).expect("write");
    println!("{}", out.display());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (base, out_dir) = (&args[1], &args[2]);
    write_variant(base, out_dir, "surv_default", None, None);
    for key in KEYS {
        let value = match key {
            "27" | "28" => Value::Name("1".into()),
            _ => Value::Int(1),
        };
        write_variant(base, out_dir, &format!("surv_k{key}"), Some(key), Some(value));
    }
    write_variant(base, out_dir, "surv_k8_range", Some("8"), Some(Value::Int(9)));
}
