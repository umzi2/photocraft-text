//! Dump the `Txt2` model of every text object in a PSD.
//!
//! Photoshop reads a text layer's contents and per-run styles from the `Txt2`
//! block, so its numeric keys are the authoritative encoding of every text
//! property. This tool prints, per text object, the character sheet of every
//! style run and the paragraph sheet of every paragraph run as `key = value`,
//! inside the exact path a writer has to address.
//!
//! `cargo run --example txt2_objects -- out/probe5/pA.psd [object]`
//!
//! Paths printed by the tool are the ones `photocraft_text::txt2` uses:
//!
//! ```text
//! /1/<engine>/<object>                 one text object (one text layer)
//!   /0/0                               the layer text (UTF-16BE, `\u000d` breaks)
//!   /0/6/0[run]/0/0/6/<key>            character sheet of style run `run`
//!   /0/5/0[run]/0/0/5/<key>            paragraph sheet of paragraph run `run`
//!   /0/10/0                            layer antialias code
//!   /1/2                               composed-layout cache (`[ ]` = not laid out)
//! ```
//!
//! The same block also carries the shared default character sheet (`/1/2`) and
//! the shared default paragraph sheet (`/1/3`); `--defaults` prints those, which
//! is how the full key space (89 character keys, 41 paragraph keys) becomes
//! visible even though a run only stores the keys it changed.
//!
//! Not every text property is in `Txt2`: the text object's *direction*
//! (horizontal/vertical) and the layer's *warp* settings live in the `TySh`
//! layer block instead, in the text descriptor (`Ornt`, `AntA`, `bounds`, …)
//! and the warp descriptor. `--tysh` prints those per layer, paired with the
//! `Txt2` object whose text it carries.

use photocraft_psd::descriptor::Value;
use photocraft_psd::{PsdFile, TySh};
use photocraft_text::txt2::{self, Kind, Token};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: txt2_objects <file.psd> [object-index] [--defaults] [--tysh]");
        std::process::exit(2);
    }
    let object_filter: Option<usize> = args.iter().skip(2).find_map(|a| a.parse().ok());
    let show_defaults = args.iter().any(|a| a == "--defaults");
    let show_tysh = args.iter().any(|a| a == "--tysh");

    let file = PsdFile::from_bytes(&std::fs::read(&args[1])?)?;
    let block = file
        .global_block(b"Txt2")
        .ok_or("no Txt2 block")?
        .data
        .clone();
    let toks = txt2::tokens(&block)?;

    let engine = top_value(&block, &toks, "1").ok_or("Txt2 has no /1 text engine")?;
    let objects = dict_value(&block, &toks, engine, "1").ok_or("Txt2 has no /1/1 object array")?;

    println!(
        "# {}: Txt2 {} bytes, {} token(s), {} text object(s)",
        args[1],
        block.len(),
        toks.len(),
        array_len(&toks, objects)
    );

    let mut texts = Vec::new();
    for i in 0..array_len(&toks, objects) {
        let obj = array_item(&toks, objects, i).ok_or("short object array")?;
        texts.push(object_text(&block, &toks, obj));
        if object_filter.is_some_and(|want| want != i) {
            continue;
        }
        dump_object(&block, &toks, i, obj)?;
    }

    if show_tysh {
        dump_tysh(&file, &texts)?;
    }

    if show_defaults {
        if let Some(node) = dict_value(&block, &toks, engine, "2") {
            println!(
                "\n# shared default character sheet /1/2 ({} keys)",
                dict_len(&block, &toks, node)
            );
            dump_dict(&block, &toks, node, "  /1/2");
        }
        if let Some(node) = dict_value(&block, &toks, engine, "3") {
            println!(
                "\n# shared default paragraph sheet /1/3 ({} keys)",
                dict_len(&block, &toks, node)
            );
            dump_dict(&block, &toks, node, "  /1/3");
        }
    }
    Ok(())
}

/// Text of text object `obj` (empty when the object has no `/0/0` string).
fn object_text(block: &[u8], toks: &[Token], obj: usize) -> String {
    dict_value(block, toks, obj, "0")
        .and_then(|view| dict_value(block, toks, view, "0"))
        .map(|n| decode_str(block, toks, n))
        .unwrap_or_default()
}

/// Layer `TySh` blocks: direction and warp, which `Txt2` does not store.
fn dump_tysh(file: &PsdFile, texts: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n# TySh descriptors (direction and warp; not in Txt2)");
    // `decode_str` escapes U+000D as the literal text `\u000d`; normalize both
    // sides before pairing a layer with its `Txt2` object. The two arrays are
    // not in the same order (Photoshop may sort text objects by internal id),
    // so the pairing is by text.
    let norm = |s: &str| {
        s.replace("\\u000d", "\r")
            .trim_end_matches(['\r', '\n'])
            .to_string()
    };
    for (li, layer) in file.layers().iter().enumerate() {
        let Some(block) = layer.block(b"TySh") else {
            continue;
        };
        let tysh = TySh::from_bytes(&block.data)?;
        let d = &tysh.descriptor.descriptor;
        let text = d.get("Txt ").and_then(value_text).unwrap_or_default();
        let trimmed = text.trim_end_matches('\r');
        let wanted = norm(trimmed);
        let tag = match texts.iter().position(|t| norm(t) == wanted) {
            Some(i) => format!("text object {i}"),
            None => "no matching Txt2 object".to_string(),
        };
        println!("\nlayer {li} {:?} -> {tag}", layer.name());
        println!("  text={trimmed:?}");
        println!("  text_version {} (TySh v1, warp v1)", tysh.text_version);
        println!(
            "  transform [{}] bounds [{} {} {} {}]",
            tysh.transform
                .iter()
                .map(|v| fmt_num(*v))
                .collect::<Vec<_>>()
                .join(" "),
            tysh.bounds[0],
            tysh.bounds[1],
            tysh.bounds[2],
            tysh.bounds[3]
        );
        for key in [
            "Ornt",
            "AntA",
            "textGridding",
            "TextIndex",
            "bounds",
            "boundingBox",
        ] {
            if let Some(v) = d.get(key) {
                println!("  {key} = {}", describe_value(v));
            }
        }
        let w = &tysh.warp.descriptor;
        let mut bits = Vec::new();
        for key in [
            "warpStyle",
            "warpValue",
            "warpPerspective",
            "warpPerspectiveOther",
            "warpRotate",
        ] {
            if let Some(v) = w.get(key) {
                bits.push(format!("{key}={}", describe_value(v)));
            }
        }
        if !bits.is_empty() {
            println!("  warp: {}", bits.join(" "));
        }
    }
    Ok(())
}

/// Everything a one-line descriptor print needs: enums as `value`, unit
/// floats as `value unit`, nested descriptors as their `Left/Top/Rght/Btom`.
fn describe_value(v: &Value) -> String {
    match v {
        Value::Enumerated { value, .. } => id_text(value),
        Value::Text(s) => format!("{:?}", s.to_string_lossy()),
        Value::Integer(i) => i.to_string(),
        Value::Double(d) => fmt_num(*d),
        Value::UnitFloat { unit, value } => format!("{} {}", fmt_num(*value), unit_text(unit)),
        Value::Descriptor(d) => d
            .items
            .iter()
            .map(|(k, v)| format!("{}={}", id_text(k), describe_value(v)))
            .collect::<Vec<_>>()
            .join(" "),
        Value::List(items) => items
            .iter()
            .map(describe_value)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Boolean(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

fn value_text(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s.to_string_lossy()),
        _ => None,
    }
}

fn id_text(id: &photocraft_psd::descriptor::Id) -> String {
    String::from_utf8_lossy(id.as_bytes()).into_owned()
}

fn unit_text(unit: &[u8; 4]) -> String {
    String::from_utf8_lossy(unit).into_owned()
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() { "0".to_string() } else { s }
}

fn dump_object(
    block: &[u8],
    toks: &[Token],
    index: usize,
    obj: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let view = dict_value(block, toks, obj, "0").ok_or("object has no /0 view")?;
    let text = dict_value(block, toks, view, "0")
        .map(|n| decode_str(block, toks, n))
        .unwrap_or_default();
    let aa = dict_value(block, toks, view, "10")
        .and_then(|n| dict_value(block, toks, n, "0"))
        .map(|n| scalar_text(block, toks, n));
    let frame = find_frame(block, toks, obj, &format!("/1/1[{index}]"));
    let kind = if frame.is_some() { "area" } else { "point" };

    println!("\nobject {index}: kind={kind}");
    println!("  text={text:?}");
    println!("  path /1/1[{index}]");
    println!(
        "  antialias={} path /1/1[{index}]/0/10/0",
        aa.as_deref().unwrap_or("(omitted = 3 smooth)")
    );
    if let Some((path, f)) = &frame {
        println!("  frame=[{}] path {path}", f.join(" "));
    }
    if let Some(cache) = dict_value(block, toks, obj, "1") {
        let laid_out = dict_value(block, toks, cache, "2")
            .map(|n| array_len(toks, n))
            .unwrap_or(0);
        println!(
            "  composed layout: cache /2 has {laid_out} entr{}",
            if laid_out == 1 { "y" } else { "ies" }
        );
    }

    if let Some(table) = dict_value(block, toks, view, "6") {
        let runs = dict_value(block, toks, table, "0").ok_or("style table has no /0 array")?;
        for r in 0..array_len(toks, runs) {
            let run = array_item(toks, runs, r).ok_or("short style table")?;
            let inner = dict_value(block, toks, run, "0")
                .and_then(|n| dict_value(block, toks, n, "0"))
                .ok_or("style run has no /0/0")?;
            let len = dict_value(block, toks, run, "1").map(|n| scalar_text(block, toks, n));
            println!(
                "\n  style run {r}: len={} path /1/1[{index}]/0/6/0[{r}]/0/0/6",
                len.as_deref().unwrap_or("?")
            );
            if let Some(sheet) = dict_value(block, toks, inner, "6") {
                dump_dict(block, toks, sheet, "    ");
            }
        }
    }

    if let Some(table) = dict_value(block, toks, view, "5") {
        let runs = dict_value(block, toks, table, "0").ok_or("paragraph table has no /0 array")?;
        for r in 0..array_len(toks, runs) {
            let run = array_item(toks, runs, r).ok_or("short paragraph table")?;
            let inner = dict_value(block, toks, run, "0")
                .and_then(|n| dict_value(block, toks, n, "0"))
                .ok_or("paragraph run has no /0/0")?;
            let len = dict_value(block, toks, run, "1").map(|n| scalar_text(block, toks, n));
            println!(
                "\n  paragraph run {r}: len={} path /1/1[{index}]/0/5/0[{r}]/0/0/5",
                len.as_deref().unwrap_or("?")
            );
            if let Some(sheet) = dict_value(block, toks, inner, "5") {
                dump_dict(block, toks, sheet, "    ");
            }
        }
    }
    Ok(())
}

/// One key per line, numeric keys in numeric order.
fn dump_dict(block: &[u8], toks: &[Token], node: usize, indent: &str) {
    let Some(entries) = txt2::dict_entries(toks, node) else {
        return;
    };
    let mut keys: Vec<(String, usize)> = entries;
    keys.sort_by_key(|(k, _)| k.parse::<i64>().unwrap_or(i64::MAX));
    for (key, value) in keys {
        println!("{indent}/{key:<3} {}", render(block, toks, value));
    }
}

/// Rebuilds a value the way it is stored, strings decoded to readable text.
fn render(block: &[u8], toks: &[Token], node: usize) -> String {
    match &toks[node].kind {
        Kind::DictOpen => {
            let entries = txt2::dict_entries(toks, node).unwrap_or_default();
            let body: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("/{k} {}", render(block, toks, *v)))
                .collect();
            format!("<< {} >>", body.join(" "))
        }
        Kind::ArrayOpen => {
            let mut items = Vec::new();
            let mut j = node + 1;
            while let Some(t) = toks.get(j) {
                if matches!(t.kind, Kind::ArrayClose) {
                    break;
                }
                items.push(render(block, toks, j));
                match txt2::skip_value(toks, j) {
                    Some(next) if next > j => j = next,
                    _ => break,
                }
            }
            format!("[ {} ]", items.join(" "))
        }
        Kind::Str(_) => format!("{:?}", decode_str(block, toks, node)),
        Kind::Name(n) => format!("/{n}"),
        Kind::Nil => "/nil".into(),
        Kind::Bool(b) => b.to_string(),
        Kind::Int(i) => i.to_string(),
        Kind::Real(r) => trim_real(*r),
        Kind::DictClose => ">>".into(),
        Kind::ArrayClose => "]".into(),
    }
}

fn trim_real(v: f64) -> String {
    let s = format!("{v:.5}");
    let s = s.trim_end_matches('0');
    if s.ends_with('.') {
        format!("{s}0")
    } else {
        s.to_string()
    }
}

fn scalar_text(block: &[u8], toks: &[Token], node: usize) -> String {
    match &toks[node].kind {
        Kind::Str(_) => decode_str(block, toks, node),
        _ => render(block, toks, node),
    }
}

/// UTF-16BE body of a string token; `\u000d` units stay visible as escapes.
fn decode_str(block: &[u8], toks: &[Token], node: usize) -> String {
    let Kind::Str(body) = &toks[node].kind else {
        return String::new();
    };
    let body: &[u8] = body;
    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    let mut out = String::new();
    for u in units {
        match u {
            0x000d => out.push_str("\\u000d"),
            0xfeff => {}
            u if u < 0x20 => out.push_str(&format!("\\u{u:04x}")),
            u => out.push(char::from_u32(u as u32).unwrap_or('?')),
        }
    }
    let _ = block;
    out
}

/// Token index of the value under a top-level name.
fn top_value(_block: &[u8], toks: &[Token], key: &str) -> Option<usize> {
    let mut i = 0;
    while i < toks.len() {
        if let Kind::Name(name) = &toks[i].kind {
            let value = i + 1;
            if name == key {
                return Some(value);
            }
            i = txt2::skip_value(toks, value)?;
        } else {
            i += 1;
        }
    }
    None
}

fn dict_value(_block: &[u8], toks: &[Token], node: usize, key: &str) -> Option<usize> {
    txt2::dict_entries(toks, node)?
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

fn dict_len(_block: &[u8], toks: &[Token], node: usize) -> usize {
    if !matches!(toks.get(node).map(|t| &t.kind), Some(Kind::DictOpen)) {
        return 0;
    }
    txt2::dict_entries(toks, node).map(|e| e.len()).unwrap_or(0)
}

fn array_len(toks: &[Token], node: usize) -> usize {
    if !matches!(toks.get(node).map(|t| &t.kind), Some(Kind::ArrayOpen)) {
        return 0;
    }
    let mut j = node + 1;
    let mut n = 0;
    while let Some(t) = toks.get(j) {
        if matches!(t.kind, Kind::ArrayClose) {
            break;
        }
        match txt2::skip_value(toks, j) {
            Some(next) if next > j => {
                j = next;
                n += 1;
            }
            _ => break,
        }
    }
    n
}

fn array_item(toks: &[Token], node: usize, index: usize) -> Option<usize> {
    let mut j = node + 1;
    for _ in 0..index {
        j = txt2::skip_value(toks, j)?;
    }
    if matches!(toks.get(j)?.kind, Kind::ArrayClose) {
        None
    } else {
        Some(j)
    }
}

/// The `[l t r b]` of an area-text frame and the path that addresses it.
///
/// The rectangle lives in the `/99 /F` dictionary, inside the object's
/// composed-layout container. A point-text layer carries a `/99 /F` node too
/// but no `/1` rectangle, so the search requires both.
fn find_frame(
    block: &[u8],
    toks: &[Token],
    node: usize,
    prefix: &str,
) -> Option<(String, Vec<String>)> {
    fn rect(block: &[u8], toks: &[Token], arr: usize) -> Option<Vec<String>> {
        if !matches!(toks.get(arr).map(|t| &t.kind), Some(Kind::ArrayOpen))
            || array_len(toks, arr) != 4
        {
            return None;
        }
        let mut out = Vec::new();
        let mut j = arr + 1;
        while let Some(t) = toks.get(j) {
            if matches!(t.kind, Kind::ArrayClose) {
                break;
            }
            out.push(scalar_text(block, toks, j));
            j = txt2::skip_value(toks, j)?;
        }
        Some(out)
    }

    fn walk(
        block: &[u8],
        toks: &[Token],
        node: usize,
        path: &mut String,
    ) -> Option<(String, Vec<String>)> {
        match &toks.get(node)?.kind {
            Kind::DictOpen => {
                let entries = txt2::dict_entries(toks, node)?;
                let is_frame = entries.iter().any(|(k, v)| {
                    k == "99" && matches!(&toks[*v].kind, Kind::Name(name) if name == "F")
                });
                if is_frame {
                    if let Some((_, arr)) = entries.iter().find(|(k, _)| k == "1") {
                        if let Some(r) = rect(block, toks, *arr) {
                            let saved = path.clone();
                            path.push_str("/1");
                            let hit = (path.clone(), r);
                            *path = saved;
                            return Some(hit);
                        }
                    }
                }
                for (k, v) in entries {
                    let saved = path.clone();
                    path.push('/');
                    path.push_str(&k);
                    if let Some(hit) = walk(block, toks, v, path) {
                        return Some(hit);
                    }
                    *path = saved;
                }
            }
            Kind::ArrayOpen => {
                let mut j = node + 1;
                let mut i = 0;
                while let Some(t) = toks.get(j) {
                    if matches!(t.kind, Kind::ArrayClose) {
                        break;
                    }
                    let saved = path.clone();
                    path.push_str(&format!("[{i}]"));
                    if let Some(hit) = walk(block, toks, j, path) {
                        return Some(hit);
                    }
                    *path = saved;
                    j = txt2::skip_value(toks, j)?;
                    i += 1;
                }
            }
            _ => {}
        }
        None
    }

    let mut path = prefix.to_string();
    walk(block, toks, node, &mut path)
}
