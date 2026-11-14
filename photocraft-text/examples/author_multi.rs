//! Rewrites a text layer as several style runs.
//!
//! `cargo run --example author_multi -- in.psd out.psd runs.json`
//!
//! `runs.json`: `[{"len": 14, "size": 96}, {"len": 15, "size": 48}]`. Lengths are
//! UTF-16 units and must cover the layer text including its trailing U+000D.
//!
//! The `Txt2` run table carries the styles Photoshop reads, its composed-layout
//! cache is emptied so Photoshop recomposes from those runs, the same split is
//! mirrored into `TySh` `EngineData` for every other reader, and the layer's
//! prerendered pixels are re-rasterised from that split so the stored copy
//! matches what Photoshop draws.

#[path = "common/fonts.rs"]
mod fonts;

use photocraft_psd::PsdFile;
use photocraft_text::psd::{LayerSpec, RunSpec, author};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let mut file = PsdFile::from_bytes(&std::fs::read(&a[1])?)?;
    let spec: Vec<RunSpec> = serde_json::from_str(&std::fs::read_to_string(&a[3])?)?;
    let text = photocraft_text::txt2::read_text(
        &file
            .global_blocks
            .iter()
            .find(|b| &b.key == b"Txt2")
            .ok_or("no Txt2 block")?
            .data,
    )
    .ok_or("no text")?;
    let store = fonts::load()?;
    author(
        &mut file,
        &LayerSpec {
            text: text.clone(),
            runs: spec.clone(),
            paragraph: Default::default(),
            aa: None,
            frame: None,
            shape: None,
            orientation: None,
            transform: None,
            atlas: None,
            fractional: None,
            ..LayerSpec::default()
        },
        &store,
    )?;
    std::fs::write(&a[2], file.to_bytes()?)?;
    println!(
        "wrote {} runs {:?}",
        &a[2],
        spec.iter().map(|r| r.len).collect::<Vec<_>>()
    );
    Ok(())
}
