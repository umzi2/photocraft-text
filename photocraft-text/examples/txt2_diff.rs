//! Prints the paths whose values differ between two PSDs' `Txt2` blocks.
//!
//! `cargo run --example txt2_diff -- base.psd variant.psd`
//!
//! Both blocks are parsed into nodes; every leaf (or container that shrank or
//! grew) whose value changed is printed as `/0/5/1: 0 -> 2`. That is how the
//! slots for a Photoshop control are identified: vary one control, diff.

use photocraft_psd::PsdFile;
use photocraft_text::txt2::{self, Kind};

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Str(String),
    Num(String),
    Name(String),
    Dict(Vec<(String, Node)>),
    Array(Vec<Node>),
    Other(String),
}

impl Node {
    fn show(&self) -> String {
        match self {
            Node::Str(s) => format!("({})", s.chars().take(24).collect::<String>()),
            Node::Num(s) | Node::Name(s) => s.clone(),
            Node::Other(s) => s.clone(),
            Node::Dict(_) => "{..}".into(),
            Node::Array(items) => format!("[{}]", items.len()),
        }
    }
}

fn txt2_block(file: &PsdFile) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(file
        .global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .ok_or("no Txt2 block")?
        .data
        .clone())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let left = txt2_block(&PsdFile::from_bytes(&std::fs::read(&a[1])?)?)?;
    let right = txt2_block(&PsdFile::from_bytes(&std::fs::read(&a[2])?)?)?;
    let (ln, used_l) = parse(&left)?;
    let (rn, used_r) = parse(&right)?;
    eprintln!(
        "parsed left {used_l}/{} bytes as {}, right {used_r}/{} as {}",
        left.len(),
        ln.show(),
        right.len(),
        rn.show()
    );
    if std::env::var_os("TXT2_PATHS").is_some() {
        let mut out = Vec::new();
        leaves("", &ln, &mut out);
        for line in out {
            println!("{line}");
        }
        return Ok(());
    }
    let mut out = Vec::new();
    diff("", &ln, &rn, &left, &right, &mut out);
    for line in out {
        println!("{line}");
    }
    Ok(())
}

/// The block holds a dictionary *body*: `/key value` pairs with the surrounding
/// `<<`/`>>` stripped, which is how `txt2::resolve` sees it too.
fn parse(block: &[u8]) -> Result<(Node, usize), Box<dyn std::error::Error>> {
    let toks = txt2::tokens(block)?;
    let mut i = 0;
    let mut entries: Vec<(String, Node)> = Vec::new();
    while i < toks.len() {
        let key_tok = &toks[i];
        if !matches!(key_tok.kind, Kind::Name(_)) {
            break;
        }
        let key = String::from_utf8_lossy(&block[key_tok.start..key_tok.end])
            .trim_start_matches('/')
            .to_string();
        i += 1;
        let v = value(block, &toks, &mut i)?;
        entries.push((key, v));
    }
    Ok((Node::Dict(entries), i))
}

fn value(
    block: &[u8],
    toks: &[txt2::Token],
    i: &mut usize,
) -> Result<Node, Box<dyn std::error::Error>> {
    let t = toks.get(*i).ok_or("unexpected end of Txt2")?;
    let text = String::from_utf8_lossy(&block[t.start..t.end]).to_string();
    match &t.kind {
        Kind::DictOpen | Kind::ArrayOpen => {
            let dict = matches!(t.kind, Kind::DictOpen);
            *i += 1;
            let mut entries: Vec<(String, Node)> = Vec::new();
            let mut items: Vec<Node> = Vec::new();
            loop {
                let Some(tok) = toks.get(*i) else { break };
                match tok.kind {
                    Kind::DictClose | Kind::ArrayClose => {
                        *i += 1;
                        break;
                    }
                    _ => {}
                }
                if dict {
                    let key_tok = &toks[*i];
                    let key =
                        String::from_utf8_lossy(&block[key_tok.start..key_tok.end]).to_string();
                    *i += 1;
                    let v = value(block, toks, i)?;
                    entries.push((key.trim_start_matches('/').to_string(), v));
                } else {
                    items.push(value(block, toks, i)?);
                }
            }
            Ok(if dict {
                Node::Dict(entries)
            } else {
                Node::Array(items)
            })
        }
        Kind::Str(_) => {
            *i += 1;
            Ok(Node::Str(text))
        }
        Kind::Name(_) => {
            *i += 1;
            Ok(Node::Name(text))
        }
        _ => {
            *i += 1;
            if text.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') {
                Ok(Node::Num(text))
            } else {
                Ok(Node::Other(text))
            }
        }
    }
}

fn leaves(path: &str, node: &Node, out: &mut Vec<String>) {
    match node {
        Node::Dict(entries) => {
            for (k, v) in entries {
                leaves(&format!("{path}/{k}"), v, out);
            }
        }
        Node::Array(items) => {
            for (i, v) in items.iter().enumerate().take(12) {
                leaves(&format!("{path}/{i}"), v, out);
            }
        }
        other => out.push(format!("{path}: {}", other.show())),
    }
}

fn diff(path: &str, left: &Node, right: &Node, _lb: &[u8], _rb: &[u8], out: &mut Vec<String>) {
    match (left, right) {
        (Node::Dict(a), Node::Dict(b)) => {
            for (key, lv) in a {
                match b.iter().find(|(k, _)| k == key) {
                    Some((_, rv)) => diff(&format!("{path}/{key}"), lv, rv, _lb, _rb, out),
                    None => out.push(format!("{path}/{key}: {} -> (missing)", lv.show())),
                }
            }
            for (key, rv) in b {
                if !a.iter().any(|(k, _)| k == key) {
                    out.push(format!("{path}/{key}: (missing) -> {}", rv.show()));
                }
            }
        }
        (Node::Array(a), Node::Array(b)) => {
            if a.len() != b.len() {
                out.push(format!("{path}: [{}] -> [{}]", a.len(), b.len()));
            }
            for (i, (lv, rv)) in a.iter().zip(b).enumerate() {
                diff(&format!("{path}/{i}"), lv, rv, _lb, _rb, out);
            }
        }
        (l, r) if l != r => out.push(format!("{path}: {} -> {}", l.show(), r.show())),
        _ => {}
    }
}
