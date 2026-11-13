//! Every EngineData blob in the corpus must survive `parse` + `write` byte for byte.

use photocraft_psd::PsdFile;
use photocraft_psd::descriptor::{Value as DescValue, VersionedDescriptor};

fn engine_data_blobs(psd: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let Ok(file) = PsdFile::from_bytes(psd) else {
        return out;
    };
    for layer in file.layers() {
        let Some(block) = layer.blocks.iter().find(|b| &b.key == b"TySh") else {
            continue;
        };
        let data = &block.data;
        if data.len() < 52 {
            continue;
        }
        let Ok((desc, _)) = VersionedDescriptor::parse_prefix(&data[52..]) else {
            continue;
        };
        if let Some(DescValue::RawData(raw)) = desc.descriptor.get("EngineData") {
            out.push(raw.clone());
        }
    }
    out
}

#[test]
fn corpus_engine_data_round_trips_byte_identically() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("corpus dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("psd") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read psd");
        for raw in engine_data_blobs(&bytes) {
            let value = photocraft_text::engine_data::parse(&raw).expect("parse");
            let out = photocraft_text::engine_data::write(&value);
            checked += 1;
            if out != raw {
                let at = out
                    .iter()
                    .zip(raw.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or(out.len().min(raw.len()));
                failures.push(format!(
                    "{}: {} vs {} bytes, first diff at {}",
                    path.file_name().unwrap().to_string_lossy(),
                    out.len(),
                    raw.len(),
                    at
                ));
            }
        }
    }
    assert!(checked > 0, "no EngineData blobs found in corpus");
    assert!(failures.is_empty(), "round-trip mismatches: {failures:#?}");
}
