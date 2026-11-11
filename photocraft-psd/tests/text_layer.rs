use photocraft_psd::builder::{PixelData, PsdBuilder, TextLayerSpec};
use photocraft_psd::text_layer::TySh;
use photocraft_psd::{PsdFile, Rect};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn corpus(name: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus").join(name))
}

#[test]
fn tysh_round_trips_byte_exact_from_corpus() -> TestResult {
    let bytes = corpus("plain_latin.psd")?;
    let file = PsdFile::from_bytes(&bytes)?;
    let mut found = 0;
    for record in file.layers() {
        if let Some(block) = record.block(b"TySh") {
            let parsed = TySh::from_bytes(&block.data)?;
            assert_eq!(parsed.to_bytes(), block.data, "TySh must re-serialize to the original bytes");
            found += 1;
        }
    }
    assert!(found > 0, "corpus file has no TySh block");
    Ok(())
}

#[test]
fn tysh_rejects_unknown_version() {
    let ty = TySh::new_text("Hi", b"eng", [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [0, 0, 10, 10]);
    let mut bytes = ty.to_bytes();
    bytes[1] = 2;
    assert!(TySh::from_bytes(&bytes).is_err());
}

#[test]
fn builder_text_layer_and_txt2_re_read() -> TestResult {
    let ty = TySh::new_text("Hi\r", b"engine-data", [1.0, 0.0, 0.0, 1.0, 1.0, 2.0], [1, 2, 3, 3]).to_bytes();
    let txt2 = b"txt2-body".to_vec();
    let pixels: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255];
    let mut b = PsdBuilder::new(4, 4);
    b.push_text_layer(TextLayerSpec {
        name: "Text".into(),
        rect: Rect::from_xywh(1, 2, 2, 1),
        pixels: PixelData::Rgba8(pixels.clone()),
        ty_sh: ty.clone(),
        txt2: Some(txt2.clone()),
    })?;
    let file = PsdFile::from_bytes(&b.to_bytes()?)?;

    let layers = file.layers();
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].name(), "Text");
    assert_eq!(layers[0].rect, Rect::from_xywh(1, 2, 2, 1));
    assert_eq!(layers[0].block(b"TySh").map(|x| x.data.as_slice()), Some(ty.as_slice()));
    assert_eq!(file.global_block(b"Txt2").map(|x| x.data.as_slice()), Some(txt2.as_slice()));

    let layer = file.layer(0).ok_or("missing layer")?;
    assert_eq!(layer.rgba8()?.data, pixels);

    let composite = file.composite_rgba8()?;
    assert_eq!((composite.width, composite.height), (4, 4));
    Ok(())
}

#[test]
fn push_text_layer_rejects_invalid_tysh() {
    let mut b = PsdBuilder::new(4, 4);
    let r = b.push_text_layer(TextLayerSpec {
        name: "Bad".into(),
        rect: Rect::from_xywh(0, 0, 1, 1),
        pixels: PixelData::Rgba8(vec![0, 0, 0, 255]),
        ty_sh: vec![0, 9],
        txt2: None,
    });
    assert!(r.is_err());
}
