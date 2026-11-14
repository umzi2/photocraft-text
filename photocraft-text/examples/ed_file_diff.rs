//! Diffs the leaves of two raw `EngineData` files (`path\tvalue` lines).
use photocraft_text::engine_data::{self, Value};
use std::collections::BTreeMap;

fn walk(v: &Value, path: &str, out: &mut BTreeMap<String, String>) {
    match v {
        Value::Dict(d) => {
            for (k, v) in d {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(v, &p, out);
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                walk(v, &format!("{path}[{i}]"), out);
            }
        }
        Value::Bool(b) => drop(out.insert(path.into(), format!("B {b}"))),
        Value::Int(i) => drop(out.insert(path.into(), format!("I {i}"))),
        Value::Float(f) => drop(out.insert(path.into(), format!("R {f}"))),
        Value::Str(s) | Value::Name(s) => drop(out.insert(path.into(), format!("S {s}"))),
    }
}

fn main() {
    let a = std::env::args().nth(1).unwrap();
    let b = std::env::args().nth(2).unwrap();
    let mut left = BTreeMap::new();
    let mut right = BTreeMap::new();
    walk(
        &engine_data::parse(&std::fs::read(&a).unwrap()).unwrap(),
        "",
        &mut left,
    );
    walk(
        &engine_data::parse(&std::fs::read(&b).unwrap()).unwrap(),
        "",
        &mut right,
    );
    for (k, v) in &left {
        match right.get(k) {
            None => println!("- {k}\t{v}"),
            Some(w) if w != v => println!("~ {k}\t{v} -> {w}"),
            _ => {}
        }
    }
    for (k, v) in &right {
        if !left.contains_key(k) {
            println!("+ {k}\t{v}");
        }
    }
}
