//! Every `Txt2` block in the corpus must tokenise and rebuild byte for byte.

use photocraft_psd::PsdFile;

fn txt2_blocks(psd: &[u8]) -> Vec<Vec<u8>> {
    let Ok(file) = PsdFile::from_bytes(psd) else { return Vec::new() };
    file.global_blocks
        .iter()
        .filter(|b| &b.key == b"Txt2")
        .map(|b| b.data.clone())
        .collect()
}

#[test]
fn corpus_txt2_round_trips() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("corpus dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("psd") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read psd");
        for block in txt2_blocks(&bytes) {
            let tokens = match photocraft_text::txt2::tokens(&block) {
                Ok(t) => t,
                Err(e) => {
                    failures.push(format!("{}: tokenise failed: {e}", path.display()));
                    continue;
                }
            };
            let rebuilt = photocraft_text::txt2::untokenize(&block, &tokens);
            checked += 1;
            if rebuilt != block {
                failures.push(format!(
                    "{}: rebuilt {} vs {} bytes",
                    path.display(),
                    rebuilt.len(),
                    block.len()
                ));
            }
        }
    }
    assert!(checked > 0, "no Txt2 blocks in corpus");
    assert!(failures.is_empty(), "{failures:#?}");
}
