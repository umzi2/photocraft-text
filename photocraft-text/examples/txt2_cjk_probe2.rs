//! Follow-up CJK survival probes: the 0/1-valued key `8` on punctuation-heavy
//! text, and a dict-vs-scalar test for `27`/`28`.
//!
//! `cargo run --example txt2_cjk_probe2 -- <base.psd> <out_dir>`

use photocraft_text::txt2::{Step, Value, ensure_container, replace_value};

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

fn write(base: &str, out_dir: &str, name: &str, key: &str, scalar: Option<Value>, dict: Option<&[u8]>) {
    let data = std::fs::read(base).expect("read base");
    let mut file = photocraft_psd::PsdFile::from_bytes(&data).expect("parse base");
    let block = file.txt2().expect("Txt2").to_vec();
    let next = match (scalar, dict) {
        (Some(value), _) => replace_value(&block, &path(key), &value).expect("replace"),
        (None, Some(dict)) => ensure_container(&block, &path(key), dict).expect("container"),
        (None, None) => block,
    };
    file.set_txt2(Some(next));
    let out = std::path::Path::new(out_dir).join(format!("{name}.psd"));
    std::fs::write(&out, file.to_bytes().expect("serialize")).expect("write");
    println!("{}", out.display());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (base, out_dir) = (&args[1], &args[2]);
    write(base, out_dir, "p2_default", "", None, None);
    for (name, value) in [("p2_k8_0", 0), ("p2_k8_1", 1), ("p2_k8_9", 9)] {
        write(base, out_dir, name, "8", Some(Value::Int(value)), None);
    }
    for (name, value, key) in [
        ("p2_k28_7", 7, "28"),
        ("p2_k28_0", 0, "28"),
        ("p2_k28_1", 1, "28"),
        ("p2_k27_7", 7, "27"),
        ("p2_k27_1", 1, "27"),
        ("p2_k27_3", 3, "27"),
    ] {
        write(base, out_dir, name, key, Some(Value::Int(value)), None);
    }
    for key in ["27", "28"] {
        write(base, out_dir, &format!("p2_k{key}_dict"), key, None, Some(b"<< /0 1 >>"));
        write(base, out_dir, &format!("p2_k{key}_empty"), key, None, Some(b"<< >>"));
        write(base, out_dir, &format!("p2_k{key}_int2"), key, Some(Value::Int(2)), None);
    }
}
