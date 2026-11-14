//! Repro: a fresh `Txt2` block for a box-text spec loses its object/frame.
use photocraft_text::psd::{self, LayerSpec, RunSpec};

fn main() {
    let spec = LayerSpec {
        text: "Hello".into(),
        runs: vec![RunSpec { len: 6, ..RunSpec::default() }],
        frame: Some([10.0, 20.0, 210.0, 120.0]),
        ..LayerSpec::default()
    };
    match psd::build_txt2(&[(0, &spec)], None) {
        Ok(bytes) => {
            std::fs::write("/tmp/txt2_box.ps", &bytes).unwrap();
            println!("OK {} bytes -> /tmp/txt2_box.ps", bytes.len());
            for probe in [0usize] {
                let _ = probe;
            }
        }
        Err(e) => println!("ERR {e:?}"),
    }
}
