//! Authors one probe PSD per text-setting job with this crate's writer, for the
//! writer-parity differential (`out/verify/sync/writer_parity.py`).
//!
//! `cargo run --example writer_probe -- template.psd jobs.json out_dir`
//!
//! `jobs.json` is a JSON array of `{"name": "...", "spec": <LayerSpec>}`. The
//! template's first text layer is rewritten by `psd::author_layer`; every probe
//! is written to `out_dir/<name>.psd`.

use std::path::PathBuf;

use photocraft_psd::PsdFile;
use photocraft_text::psd::{LayerSpec, author_layer};

#[path = "common/fonts.rs"]
mod fonts;

#[derive(serde::Deserialize)]
struct Job {
    name: String,
    spec: LayerSpec,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: writer_probe <template.psd> <jobs.json> <out_dir>");
        return Ok(());
    }
    let template = std::fs::read(&args[1])?;
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&args[2])?)?;
    let out_dir = PathBuf::from(&args[3]);
    std::fs::create_dir_all(&out_dir)?;
    let store = fonts::load()?;
    for job in jobs {
        let mut file = PsdFile::from_bytes(&template)?;
        let index = file
            .layers()
            .iter()
            .position(|l| l.block(b"TySh").is_some())
            .ok_or("template has no text layer")?;
        author_layer(&mut file, index, &job.spec, &store)?;
        let bytes = file.to_bytes()?;
        let path = out_dir.join(format!("{}.psd", job.name));
        std::fs::write(&path, &bytes)?;
        println!("{} {} B", path.display(), bytes.len());
    }
    Ok(())
}
