//! Text-layer read/write: replay of the Photoshop corpus, synthetic builder
//! files, and round trips of every text-layer setter.
//!
//! `corpus/*.psd` are files saved by Photoshop CC 2019 (20.0.2). The replay
//! asserts that parsing and writing them changes nothing — not the file bytes,
//! not the `TySh` blocks, not the document-level `Txt2` block, not one stored
//! pixel — and that the three copies of a layer's text (`TySh`, `Txt2`, XMP
//! `photoshop:TextLayers`) are all readable and consistent.
//!
//! Everything is written with [`Compression::Rle`]: Photoshop 20.0.2 rejects
//! ZIP-compressed files ("not compatible with this version of Photoshop").

use std::path::{Path, PathBuf};

use photocraft_psd::builder::{LayerSpec, PixelData, PsdBuilder, TextLayerSpec};
use photocraft_psd::descriptor::{Id, UnicodeString, Value};
use photocraft_psd::testgen;
use photocraft_psd::text_layer::{
    Orientation, TextRaster, TySh, Warp, XmpTextLayer, is_text_layer, parse_xmp_text_layers, write_xmp_text_layers,
};
use photocraft_psd::{ColorMode, Compression, PsdFile, Rect, TaggedBlock, Version};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus")
}

fn corpus_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(corpus_dir()).expect("corpus directory").flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|ext| ext == "psd")).collect();
    files.sort();
    files
}

fn name_of(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Text with its paragraph breaks folded to spaces (Photoshop's XMP form).
fn paragraphs_as_spaces(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

fn parse(bytes: &[u8]) -> PsdFile {
    PsdFile::from_bytes(bytes).unwrap_or_else(|error| panic!("parse: {error}"))
}

/// RGBA8 pattern for a `width × height` layer: opaque, with a moving red ramp.
fn rgba8(width: u32, height: u32) -> Vec<u8> {
    (0..width * height).flat_map(|i| [(i % 256) as u8, 0x40, 0x80, 0xff]).collect()
}

/// RGBA16 pattern for a `width × height` layer.
fn rgba16(width: u32, height: u32) -> Vec<u16> {
    (0..width * height).flat_map(|i| [(i % 65536) as u16, 0x4000, 0x8000, 0xffff]).collect()
}

/// A two-layer document: a background raster plus a text layer named `Text`
/// whose `TySh` says `Hello\r` and whose document-level `Txt2` is `TXT2-BODY`.
fn synthetic_text_file(depth: u16, version: Version) -> Vec<u8> {
    let (width, height) = (32u32, 24u32);
    let pixels = (0..width * height).flat_map(|_| [0x20u8, 0x60, 0xa0, 0xff]).collect::<Vec<u8>>();
    let mut builder = PsdBuilder::new(width, height).version(version).compression(Compression::Rle);
    if depth == 16 {
        builder = builder.depth(16);
        let background = (0..width * height).flat_map(|_| [0x2000u16, 0x6000, 0xa000, 0xffff]).collect::<Vec<u16>>();
        builder.push_layer(LayerSpec::new("Background", 0, 0, width, height, PixelData::Rgba16(background)));
        builder.composite(PixelData::Rgba16((0..width * height).flat_map(|_| [0x2000u16, 0x6000, 0xa000, 0xffff]).collect()));
    } else {
        assert_eq!(depth, 8);
        builder.push_layer(LayerSpec::new("Background", 0, 0, width, height, PixelData::Rgba8(pixels)));
        builder.composite(PixelData::Rgba8((0..width * height).flat_map(|_| [0x20u8, 0x60, 0xa0, 0xff]).collect()));
    }
    let text_pixels = if depth == 16 { PixelData::Rgba16(rgba16(4, 2)) } else { PixelData::Rgba8(rgba8(4, 2)) };
    let tysh = TySh::new_text("Hello\r", b"engine-data", [1.0, 0.0, 0.0, 1.0, 5.0, 7.0], [0, 0, 10, 5]);
    builder
        .push_text_layer(TextLayerSpec { name: "Text".into(), rect: Rect::from_xywh(5, 7, 4, 2), pixels: text_pixels, ty_sh: tysh.to_bytes(), txt2: Some(b"TXT2-BODY".to_vec()) })
        .expect("push text layer");
    builder.to_bytes().expect("build file")
}

/// Index of the text layer named `name`.
fn text_index(file: &PsdFile, name: &str) -> usize {
    file.text_layers().find(|(_, record)| record.name() == name).unwrap_or_else(|| panic!("no text layer {name}")).0
}

// -------------------------------------------------------------------------
// (b) Replay every corpus PSD
// -------------------------------------------------------------------------

/// Checks one text layer of a Photoshop document: `TySh`, `Txt2`, XMP and pixels.
fn check_corpus_text_layer(file: &PsdFile, index: usize, name: &str, xmp: &[XmpTextLayer]) -> TestResult {
    let record = &file.layers()[index];
    let block = record.block(b"TySh").unwrap_or_else(|| panic!("{name}: text layer {index} has no TySh"));
    let tysh = TySh::from_bytes(&block.data)?;
    assert_eq!(tysh.to_bytes(), block.data, "{name}: TySh must re-serialize byte for byte");
    assert!(tysh.text().is_some_and(|text| !text.is_empty()), "{name}: layer {index} has no text");
    assert_eq!(tysh.text_version, 50, "{name}: Photoshop 20 writes text version 50");
    let info = file.text_layer_info(index).expect("text layer info").unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(info.name, record.name());
    assert_eq!(info.text, tysh.text().unwrap_or_default());
    assert_eq!(info.layout_bounds.is_some(), tysh.layout_bounds().is_some());
    assert_eq!(info.engine_data_len, tysh.engine_data().map(<[u8]>::len).unwrap_or(0));
    assert!(info.has_txt2);
    if let Some(entry) = xmp.iter().find(|entry| entry.name == info.name) {
        // The XMP copy is a display copy: Photoshop writes paragraph breaks as
        // spaces there (lead_0.psd: TySh `line one\rline two`, XMP `line one line two`).
        assert_eq!(paragraphs_as_spaces(&entry.text), paragraphs_as_spaces(&info.text), "{name}: XMP LayerText differs from TySh Txt");
    }
    let raster = file.text_layer_raster(index).expect("text layer raster").unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(raster.rect, record.rect, "{name}: raster rect");
    assert_eq!(raster.raster.depth(), file.header.depth);
    let (width, height) = record.rect.size()?;
    assert_eq!(raster.raster.len(), width * height * 4, "{name}: raster size");
    Ok(())
}

/// Parses, writes and re-parses one Photoshop file and compares everything.
fn check_corpus_file(path: &Path) -> TestResult {
    let name = name_of(path);
    let bytes = read(path);
    let file = parse(&bytes);
    assert!(file.text_layer_count() >= 1, "{name}: no text layers");
    let txt2 = file.txt2().unwrap_or_else(|| panic!("{name}: no Txt2 block")).to_vec();
    let xmp = file.xmp_text_layers();
    assert_eq!(xmp.len(), file.text_layer_count(), "{name}: XMP lists {} text layers, the file has {}", xmp.len(), file.text_layer_count());
    let layer_names: Vec<String> = file.text_layers().map(|(_, record)| record.name()).collect();
    assert_eq!(xmp.iter().map(|entry| entry.name.clone()).collect::<Vec<_>>(), layer_names, "{name}: XMP order/names");
    for (index, _) in file.text_layers() {
        check_corpus_text_layer(&file, index, &name, &xmp)?;
    }
    // Writing every text structure back through the setters does not touch the file.
    let mut rewritten = file.clone();
    for (index, record) in file.text_layers() {
        let tysh = TySh::from_bytes(&record.block(b"TySh").expect("TySh").data)?;
        rewritten.set_text_layer_tysh(index, &tysh)?;
        rewritten.set_text_layer_orientation(index, tysh.orientation())?;
        rewritten.set_text_layer_warp(index, &tysh.warp_settings())?;
        rewritten.set_text_layer_transform(index, tysh.transform)?;
    }
    assert_eq!(rewritten.to_bytes()?, bytes, "{name}: rewriting the stored TySh values must not change the file");

    // Unmodified files write back byte for byte.
    let written = file.to_bytes()?;
    assert_eq!(written, bytes, "{name}: unmodified file must write back identically");
    let reparsed = parse(&written);
    assert_eq!(reparsed, file, "{name}: model round trip");
    assert_eq!(reparsed.txt2(), Some(&txt2[..]), "{name}: Txt2 bytes");
    for (index, record) in file.text_layers() {
        let tysh = record.block(b"TySh").expect("TySh");
        let again = reparsed.text_layer_tysh(index).expect("text layer").unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(again.to_bytes(), tysh.data, "{name}: TySh bytes after a write");
        let before = file.text_layer_raster(index).expect("raster").unwrap_or_else(|error| panic!("{name}: {error}"));
        let after = reparsed.text_layer_raster(index).expect("raster").unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(after, before, "{name}: pixels");
    }
    Ok(())
}

#[test]
fn corpus_text_layers_round_trip_unchanged() -> TestResult {
    let files = corpus_files();
    assert!(files.len() >= 40, "expected the Photoshop corpus in corpus/, found {} PSD files", files.len());
    for path in &files {
        check_corpus_file(path)?;
    }
    Ok(())
}

// -------------------------------------------------------------------------
// TySh shape, against what Photoshop itself writes
// -------------------------------------------------------------------------

#[test]
fn tysh_new_text_matches_the_photoshop_descriptor_shape() -> TestResult {
    let bytes = read(&corpus_dir().join("plain_latin.psd"));
    let file = parse(&bytes);
    let (_, record) = file.text_layers().next().expect("text layer");
    let saved = TySh::from_bytes(&record.block(b"TySh").expect("TySh").data)?;

    assert_eq!(saved.text_version, 50);
    let keys = |items: &[(Id, Value)]| items.iter().map(|(key, _)| String::from_utf8_lossy(key.as_bytes()).into_owned()).collect::<Vec<_>>();
    let photoshop_keys = keys(&saved.text_descriptor().items);
    assert_eq!(photoshop_keys, ["Txt ", "textGridding", "Ornt", "AntA", "bounds", "boundingBox", "TextIndex", "EngineData"]);
    assert_eq!(keys(&saved.warp_descriptor().items), ["warpStyle", "warpValue", "warpPerspective", "warpPerspectiveOther", "warpRotate"]);
    assert_eq!(*saved.text_descriptor().class_id.as_bytes(), *b"TxLr");
    assert_eq!(saved.text_descriptor().name, UnicodeString(vec![0]), "Photoshop writes a one-NUL descriptor name");
    assert_eq!(saved.text_units().and_then(|units| units.last()).copied(), Some(0), "Txt is NUL-terminated");
    assert_eq!(saved.orientation(), Orientation::Horizontal);
    assert_eq!(saved.warp_settings(), Warp::default());
    // The two rectangles are points, not pixels.
    for key in ["bounds", "boundingBox"] {
        match saved.text_descriptor().get(key) {
            Some(Value::Descriptor(rectangle)) => {
                assert!(Id::new(key) == rectangle.class_id, "{key} class");
                match rectangle.get("Left") {
                    Some(Value::UnitFloat { unit, .. }) => assert_eq!(unit, b"#Pnt", "{key} unit"),
                    other => panic!("{key} Left is {other:?}"),
                }
            }
            other => panic!("{key} is {other:?}"),
        }
    }
    assert!(saved.layout_bounds().is_some_and(|bounds| bounds[0] < bounds[2] && bounds[1] < bounds[3]));

    // The builder writes the same shape.
    let built = TySh::new_text("Hello", b"engine", [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [0, 0, 10, 5]);
    assert_eq!(built.text_version, 50);
    assert_eq!(keys(&built.text_descriptor().items), photoshop_keys);
    assert_eq!(keys(&built.warp_descriptor().items), keys(&saved.warp_descriptor().items));
    assert_eq!(built.text_descriptor().name, saved.text_descriptor().name);
    assert_eq!(built.warp_descriptor().name, saved.warp_descriptor().name);
    assert_eq!(built.text_units().and_then(|units| units.last()).copied(), Some(0));
    assert_eq!(built.layout_bounds(), Some([0.0, 0.0, 10.0, 5.0]));
    assert_eq!(built.bounds, [0; 4], "Photoshop leaves the 16 bound bytes zero unless warped");
    Ok(())
}

// -------------------------------------------------------------------------
// (a) Synthetic builder files, PSD and PSB
// -------------------------------------------------------------------------

#[test]
fn synthetic_text_layers_round_trip_as_psd_and_psb() -> TestResult {
    for version in [Version::Psd, Version::Psb] {
        for depth in [8u16, 16] {
            let bytes = synthetic_text_file(depth, version);
            let file = parse(&bytes);
            assert_eq!(file.header.version, version);
            assert_eq!(file.header.depth, depth);
            assert_eq!(file.text_layer_count(), 1);
            let index = text_index(&file, "Text");
            assert!(file.is_text_layer(index));
            assert_eq!(file.layer(index).expect("layer").rgba8().expect("rgba8").width, 4);
            assert_eq!(file.txt2(), Some(&b"TXT2-BODY"[..]));
            let tysh = file.text_layer_tysh(index).expect("tysh").unwrap_or_else(|error| panic!("{version:?}/{depth}: {error}"));
            assert_eq!(tysh.text().as_deref(), Some("Hello\r"));
            assert_eq!(tysh.engine_data(), Some(&b"engine-data"[..]));
            assert_eq!(tysh.transform, [1.0, 0.0, 0.0, 1.0, 5.0, 7.0]);
            let raster = file.text_layer_raster(index).expect("raster").unwrap_or_else(|error| panic!("{version:?}/{depth}: {error}"));
            assert_eq!(raster.raster.depth(), depth);
            assert_eq!(raster.rect, Rect::from_xywh(5, 7, 4, 2));
            // Nothing is ZIP-compressed: Photoshop 20 rejects such files.
            for (_, record) in file.text_layers() {
                for channel in &record.channels {
                    assert_ne!(channel.compression, Some(Compression::Zip), "builder wrote ZIP");
                    assert_ne!(channel.compression, Some(Compression::ZipPrediction), "builder wrote ZIP prediction");
                }
            }
            // Byte stability.
            let written = file.to_bytes()?;
            assert_eq!(written, bytes, "{version:?}/{depth}: unmodified synthetic file");
            let reparsed = parse(&written);
            assert_eq!(reparsed, file);
            assert_eq!(reparsed.text_layer_tysh(index).expect("tysh").unwrap_or_else(|error| panic!("{error}")).to_bytes(), tysh.to_bytes());
            assert_eq!(reparsed.txt2(), Some(&b"TXT2-BODY"[..]));
        }
    }
    Ok(())
}

#[test]
fn promoting_a_text_psd_to_psb_keeps_the_text_blocks() -> TestResult {
    let bytes = synthetic_text_file(8, Version::Psd);
    let file = parse(&bytes);
    let text = text_index(&file, "Text");
    let tysh = file.text_layer_tysh(text).expect("tysh").unwrap_or_else(|error| panic!("{error}")).to_bytes();
    let raster = file.text_layer_raster(text).expect("raster").unwrap_or_else(|error| panic!("{error}"));
    let mut promoted = file.clone();
    promoted.promote_to_psb()?;
    let psb = promoted.to_bytes()?;
    let back = parse(&psb);
    assert_eq!(back.header.version, Version::Psb);
    assert_eq!(back.text_layer_count(), 1);
    assert_eq!(back.text_layer_tysh(text).expect("tysh").unwrap_or_else(|error| panic!("{error}")).to_bytes(), tysh);
    assert_eq!(back.txt2(), Some(&b"TXT2-BODY"[..]));
    assert_eq!(back.text_layer_raster(text).expect("raster").unwrap_or_else(|error| panic!("{error}")), raster);
    assert_eq!(back.to_bytes()?, psb);
    // A PSB model re-encoding the raster stays RLE.
    let mut psb_model = back.clone();
    psb_model.set_text_layer_raster(text, &raster)?;
    let written = psb_model.to_bytes()?;
    for channel in &PsdFile::from_bytes(&written)?.layers()[0].channels {
        assert_ne!(channel.compression, Some(Compression::Zip));
    }
    Ok(())
}

// -------------------------------------------------------------------------
// TySh setters
// -------------------------------------------------------------------------

#[test]
fn tysh_setters_round_trip_through_the_file() -> TestResult {
    let bytes = synthetic_text_file(8, Version::Psd);
    let mut file = parse(&bytes);
    let text = text_index(&file, "Text");
    let background = file.layers().iter().position(|record| !is_text_layer(record)).expect("background layer");
    // Left behind by every setter: an unknown block and the document Txt2.
    let unknown = TaggedBlock { signature: *b"8B64", key: *b"zz01", data: vec![1, 2, 3], padding: Some(vec![0; 3]) };
    file.layers_mut()[text].blocks.push(unknown.clone());
    let txt2 = file.txt2().expect("Txt2").to_vec();
    let pixels = file.text_layer_raster(text).expect("raster").unwrap_or_else(|error| panic!("{error}"));

    let engine = vec![0x5a; 311];
    file.set_text_layer_name(text, "Vеrtical \u{1F600}")?;
    file.set_text_layer_engine_data(text, &engine)?;
    file.set_text_layer_text(text, "Goodbye")?;
    file.set_text_layer_transform(text, [1.0, 0.0, 0.0, 1.0, 33.5, 44.25])?;
    let warp = Warp { style: "warpArc".into(), value: 50.0, perspective: 12.5, perspective_other: -7.0, rotate: Orientation::Vertical };
    file.set_text_layer_warp(text, &warp)?;
    file.set_text_layer_orientation(text, Orientation::Vertical)?;

    let written = file.to_bytes()?;
    let back = parse(&written);
    assert_eq!(back.text_layer_name(text).as_deref(), Some("Vеrtical \u{1F600}"));
    assert_eq!(back.text_layer_engine_data(text).expect("engine data").unwrap_or_else(|error| panic!("{error}")), engine);
    assert_eq!(back.text_layer_text(text).expect("text").unwrap_or_else(|error| panic!("{error}")), "Goodbye");
    assert_eq!(back.text_layer_transform(text).expect("transform").unwrap_or_else(|error| panic!("{error}")), [1.0, 0.0, 0.0, 1.0, 33.5, 44.25]);
    assert_eq!(back.text_layer_warp(text).expect("warp").unwrap_or_else(|error| panic!("{error}")), warp);
    assert_eq!(back.text_layer_orientation(text).expect("orientation").unwrap_or_else(|error| panic!("{error}")), Orientation::Vertical);
    // The unknown block, the Txt2 block and the stored raster are untouched.
    assert_eq!(back.layers()[text].blocks.last(), Some(&unknown));
    assert_eq!(back.txt2(), Some(&txt2[..]));
    assert_eq!(back.text_layer_raster(text).expect("raster").unwrap_or_else(|error| panic!("{error}")), pixels);
    // Setters refuse layers that are not text layers.
    assert!(file.set_text_layer_engine_data(background, b"x").is_err());
    assert!(file.set_text_layer_transform(background, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).is_err());
    assert!(file.set_text_layer_text(file.layers().len(), "x").is_err());
    assert!(file.text_layer_tysh(background).is_none());
    assert!(file.text_layer_info(background).is_none());
    assert!(file.text_layer_raster(background).is_none());
    Ok(())
}

#[test]
fn vertical_text_orientation_and_warp_survive_a_round_trip() -> TestResult {
    // Reference: out/probe5/cjkvert.psd saves vertical text as `Ornt=Vrtc`, and
    // its descriptor rectangle flips to a tall, narrow line (12 × 137 points);
    // `out/probe5/pE.psd` saves text warps only in `TySh` (`warpArc`, value 50).
    let bytes = synthetic_text_file(8, Version::Psd);
    let mut file = parse(&bytes);
    let text = text_index(&file, "Text");
    let txt2 = file.txt2().expect("Txt2").to_vec();
    file.set_text_layer_orientation(text, Orientation::Vertical)?;
    let mut tysh = file.text_layer_tysh(text).expect("tysh").unwrap_or_else(|error| panic!("{error}"));
    tysh.set_layout_bounds([0.0, 0.0, 12.0, 144.0]);
    tysh.set_ink_bounds([1.0, 2.0, 11.0, 140.0]);
    file.set_text_layer_tysh(text, &tysh)?;
    let warp = Warp { style: "warpArch".into(), value: 50.0, perspective: 0.0, perspective_other: 0.0, rotate: Orientation::Horizontal };
    file.set_text_layer_warp(text, &warp)?;

    let written = file.to_bytes()?;
    // The orientation and the warp live in `TySh` only.
    assert!(written.windows(4).any(|window| window == b"Vrtc"), "Vrtc is not in the file");
    assert!(written.windows(8).any(|window| window == b"warpArch"), "warpArch is not in the file");
    let back = parse(&written);
    let tysh = back.text_layer_tysh(text).expect("tysh").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(tysh.orientation(), Orientation::Vertical);
    assert_eq!(tysh.warp_settings(), warp);
    assert_eq!(tysh.layout_bounds(), Some([0.0, 0.0, 12.0, 144.0]));
    assert_eq!(tysh.ink_bounds(), Some([1.0, 2.0, 11.0, 140.0]));
    let bounds = tysh.layout_bounds().expect("layout bounds");
    assert!(bounds[2] - bounds[0] < bounds[3] - bounds[1], "vertical text bounds are tall and narrow");
    assert_eq!(back.txt2(), Some(&txt2[..]), "Txt2 carries neither orientation nor warp");
    let info = back.text_layer_info(text).expect("info").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(info.orientation, Orientation::Vertical);
    assert_eq!(info.warp, warp);
    assert_eq!(info.layout_bounds, Some([0.0, 0.0, 12.0, 144.0]));
    Ok(())
}

// -------------------------------------------------------------------------
// (c) Unknown tagged blocks survive
// -------------------------------------------------------------------------

/// Unknown layer and document blocks with awkward lengths, signatures and
/// explicit (non-canonical) padding.
fn awkward_blocks() -> Vec<TaggedBlock> {
    vec![
        TaggedBlock::new(*b"zz01", vec![1, 2, 3]),
        TaggedBlock { signature: *b"8B64", key: *b"zz02", data: vec![9; 4], padding: Some(vec![0; 3]) },
        TaggedBlock { signature: *b"8BIM", key: *b"zz03", data: b"\x08BIM!".to_vec(), padding: Some(vec![]) },
        TaggedBlock::new(*b"fxrp", vec![0; 16]),
    ]
}

#[test]
fn unknown_tagged_blocks_survive_text_layer_writes() -> TestResult {
    let bytes = synthetic_text_file(8, Version::Psd);
    let mut file = parse(&bytes);
    let text = text_index(&file, "Text");
    for block in awkward_blocks() {
        file.layers_mut()[text].blocks.push(block);
    }
    file.layers_mut()[text].blocks.push(TaggedBlock::new(*b"zz99", vec![7]));
    file.global_blocks.push(TaggedBlock::new(*b"zz01", vec![4, 5, 6]));
    file.global_blocks.push(TaggedBlock { signature: *b"8B64", key: *b"zz02", data: vec![1; 3], padding: Some(vec![0; 2]) });

    let written = file.to_bytes()?;
    let back = parse(&written);
    let layer_blocks = back.layers()[text].blocks.clone();
    for block in awkward_blocks() {
        assert!(layer_blocks.contains(&block), "layer block {} was not preserved: {block:?}", block.key_str());
    }
    assert!(layer_blocks.iter().any(|block| &block.key == b"zz99" && block.data == [7]));
    let global_blocks = back.global_blocks.clone();
    assert!(global_blocks.contains(&TaggedBlock::new(*b"zz01", vec![4, 5, 6])));
    assert!(global_blocks.contains(&TaggedBlock { signature: *b"8B64", key: *b"zz02", data: vec![1; 3], padding: Some(vec![0; 2]) }));

    // Editing the text block (different length, so the block padding changes)
    // leaves every unknown block alone.
    let engine = vec![0x11; file.text_layer_engine_data(text).expect("engine").unwrap_or_else(|error| panic!("{error}")).len() + 3];
    let mut edited = back.clone();
    edited.set_text_layer_engine_data(text, &engine)?;
    edited.set_text_layer_text(text, "A longer replacement text")?;
    let written = edited.to_bytes()?;
    let third = parse(&written);
    let without_tysh = |file: &PsdFile| file.layers()[text].blocks.iter().filter(|block| &block.key != b"TySh").cloned().collect::<Vec<_>>();
    assert_eq!(without_tysh(&third), without_tysh(&back), "every non-TySh layer block survives the edit");
    assert_eq!(third.global_blocks, back.global_blocks);
    assert_eq!(third.text_layer_engine_data(text).expect("engine").unwrap_or_else(|error| panic!("{error}")), engine);
    assert_eq!(third.text_layer_text(text).expect("text").unwrap_or_else(|error| panic!("{error}")), "A longer replacement text");
    Ok(())
}

#[test]
fn txt2_setter_keeps_one_document_block_and_leaves_neighbours() -> TestResult {
    let bytes = synthetic_text_file(8, Version::Psd);
    let mut file = parse(&bytes);
    // Photoshop writes one Txt2 per document; a duplicated block is collapsed.
    file.global_blocks.push(TaggedBlock::new(*b"Txt2", b"SECOND".to_vec()));
    file.global_blocks.push(TaggedBlock::new(*b"zz01", vec![1]));
    assert_eq!(file.global_blocks.iter().filter(|block| &block.key == b"Txt2").count(), 2);
    file.set_txt2(Some(b"NEW BODY".to_vec()));
    assert_eq!(file.txt2(), Some(&b"NEW BODY"[..]));
    assert_eq!(file.global_blocks.iter().filter(|block| &block.key == b"Txt2").count(), 1);
    assert!(file.global_blocks.iter().any(|block| &block.key == b"zz01"));
    let back = parse(&file.to_bytes()?);
    assert_eq!(back.txt2(), Some(&b"NEW BODY"[..]));
    assert!(back.global_blocks.iter().any(|block| &block.key == b"zz01"));
    // Removing it drops every Txt2 block but nothing else.
    let mut without = back.clone();
    without.set_txt2(None);
    assert!(without.txt2().is_none());
    let back = parse(&without.to_bytes()?);
    assert!(back.txt2().is_none());
    assert!(back.global_blocks.iter().any(|block| &block.key == b"zz01"));
    // A file that has no Txt2 gets one.
    let mut none = testgen::merged_only(Version::Psd, ColorMode::Rgb, 8, Compression::Rle, 3, 2);
    assert!(none.txt2().is_none());
    none.set_txt2(Some(b"BODY".to_vec()));
    assert_eq!(parse(&none.to_bytes()?).txt2(), Some(&b"BODY"[..]));
    Ok(())
}

// -------------------------------------------------------------------------
// Stored raster, at every depth a text layer can use
// -------------------------------------------------------------------------

#[test]
fn stored_raster_reads_and_writes_at_8_16_and_32_bits() -> TestResult {
    // 8 and 16 bits: a builder document.
    for depth in [8u16, 16] {
        let bytes = synthetic_text_file(depth, Version::Psd);
        let mut file = parse(&bytes);
        let text = text_index(&file, "Text");
        let original = file.text_layer_raster(text).expect("raster").unwrap_or_else(|error| panic!("{depth}: {error}"));
        assert_eq!(original.raster.depth(), depth);
        let mut edited = original.clone();
        edited.rect = Rect::from_xywh(6, 8, 4, 2);
        match &mut edited.raster {
            TextRaster::Rgba8(samples) => {
                samples[0] = 1;
                samples[3] = 128;
            }
            TextRaster::Rgba16(samples) => {
                samples[0] = 0x0102;
                samples[3] = 0x8000;
            }
            TextRaster::Rgba32(_) => panic!("wrong depth"),
        }
        file.set_text_layer_raster(text, &edited)?;
        let back = parse(&file.to_bytes()?);
        assert_eq!(back.text_layer_raster(text).expect("raster").unwrap_or_else(|error| panic!("{depth}: {error}")), edited);
        assert_eq!(back.layers()[text].rect, edited.rect);
        for channel in &back.layers()[text].channels {
            assert_eq!(channel.compression, Some(Compression::Rle), "{depth}-bit raster channel was not RLE");
        }
        // A raster of the wrong depth is refused.
        let mut wrong = edited.clone();
        wrong.raster = match depth {
            8 => TextRaster::Rgba16(vec![0; wrong.raster.len()]),
            _ => TextRaster::Rgba8(vec![0; wrong.raster.len()]),
        };
        assert!(file.set_text_layer_raster(text, &wrong).is_err());
        // So is a raster whose size does not match its rect.
        let mut mismatched = edited.clone();
        mismatched.rect = Rect::from_xywh(0, 0, 1, 1);
        assert!(file.set_text_layer_raster(text, &mismatched).is_err());
    }

    // 32 bits: a generated document with a `TySh` block on one layer.
    let mut file = testgen::layered(Version::Psd, ColorMode::Rgb, 32, Compression::Rle);
    let tysh = TySh::new_text("Float", b"engine", [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [0, 0, 10, 10]).to_bytes();
    file.layers_mut()[0].blocks.push(TaggedBlock::new(*b"TySh", tysh));
    let original = file.text_layer_raster(0).expect("raster").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(original.raster.depth(), 32);
    let mut edited = original.clone();
    let TextRaster::Rgba32(samples) = &mut edited.raster else { panic!("not a float raster") };
    samples[0] = 0.25;
    samples[3] = 0.5;
    file.set_text_layer_raster(0, &edited)?;
    let back = parse(&file.to_bytes()?);
    assert_eq!(back.text_layer_raster(0).expect("raster").unwrap_or_else(|error| panic!("{error}")), edited);
    assert_eq!(back.to_bytes()?, file.to_bytes()?);
    Ok(())
}

// -------------------------------------------------------------------------
// XMP `photoshop:TextLayers` (resource 1060)
// -------------------------------------------------------------------------

const PACKET: &str = concat!(
    "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n",
    "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n",
    " <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n",
    "  <rdf:Description rdf:about=\"\" xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n",
    "   <dc:format>application/vnd.adobe.photoshop</dc:format>\n",
    "   <photoshop:ColorMode>3</photoshop:ColorMode>\n",
    "  </rdf:Description>\n",
    " </rdf:RDF>\n",
    "</x:xmpmeta>\n",
    "<?xpacket end=\"w\"?>"
);

#[test]
fn xmp_text_layers_parse_write_and_stay_consistent() -> TestResult {
    // Parsing: escaped and raw control characters, unicode, any prefix.
    let packet = PACKET.replace("<photoshop:ColorMode>", "<ps:ColorMode>").replace("</photoshop:ColorMode>", "</ps:ColorMode>").replace(
        "xmlns:photoshop=",
        "xmlns:ps=",
    );
    assert!(parse_xmp_text_layers(&packet).is_empty(), "no TextLayers element yet");
    let layers = vec![
        XmpTextLayer { name: "Grüße & <co>".into(), text: "two\rparagraphs".into() },
        XmpTextLayer { name: "emoji".into(), text: "a < b & c > d\nx".into() },
    ];
    let written = write_xmp_text_layers(&packet, &layers)?;
    assert!(written.contains("application/vnd.adobe.photoshop"), "the rest of the packet must survive");
    assert!(written.contains("Gr\u{fc}\u{df}e &amp; &lt;co&gt;"), "text is escaped: {written}");
    assert!(written.contains("&#xD;") && written.contains("&#xA;"), "control characters use references");
    assert_eq!(parse_xmp_text_layers(&written), layers);
    // Writing the parsed list again changes nothing.
    assert_eq!(write_xmp_text_layers(&written, &parse_xmp_text_layers(&written))?, written);
    // A raw carriage return (Photoshop writes one in out/para.psd) is read.
    let raw = written.replacen("two&#xD;paragraphs", "two\rparagraphs", 1);
    assert_eq!(parse_xmp_text_layers(&raw).first().map(|entry| entry.text.as_str()), Some("two\rparagraphs"));
    // Removing the list leaves the rest of the packet.
    let removed = write_xmp_text_layers(&written, &[])?;
    assert!(parse_xmp_text_layers(&removed).is_empty());
    assert!(removed.contains("application/vnd.adobe.photoshop"));
    assert!(!removed.contains("TextLayers"));

    // Document level: no XMP resource → a packet with the list is created.
    let mut file = parse(&synthetic_text_file(8, Version::Psd));
    assert!(file.xmp_packet().is_none());
    file.set_xmp_text_layers(&layers)?;
    assert!(file.resource(1060).is_some(), "resource 1060 was not added");
    assert_eq!(file.xmp_text_layers(), layers);
    let mut back = parse(&file.to_bytes()?);
    assert_eq!(back.xmp_text_layers(), layers);
    // Replacing the packet (resource 1060) is byte-exact through the file.
    back.set_xmp_packet(PACKET);
    assert_eq!(back.xmp_packet(), Some(PACKET));
    assert_eq!(parse(&back.to_bytes()?).xmp_packet(), Some(PACKET));
    // A packet with no rdf:Description cannot hold the list.
    let mut minimal = parse(&synthetic_text_file(8, Version::Psd));
    minimal.set_xmp_packet("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>");
    assert!(minimal.set_xmp_text_layers(&layers).is_err());

    // Inserting into a packet that does not bind the Photoshop namespace adds it.
    let plain = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\"><dc:creator xmlns:dc=\"http://purl.org/dc/elements/1.1/\">x</dc:creator></rdf:Description></rdf:RDF></x:xmpmeta>";
    let inserted = write_xmp_text_layers(plain, &layers)?;
    assert!(inserted.contains("xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\""), "missing namespace declaration: {inserted}");
    assert_eq!(parse_xmp_text_layers(&inserted), layers);
    assert!(inserted.contains("<dc:creator xmlns:dc=\"http://purl.org/dc/elements/1.1/\">x</dc:creator>"), "the description's content survives");
    // A self-closing rdf:Description is expanded.
    let self_closing = "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\"/></rdf:RDF>";
    let inserted = write_xmp_text_layers(self_closing, &layers)?;
    assert_eq!(parse_xmp_text_layers(&inserted), layers);
    assert!(inserted.contains("</rdf:Description>"), "the self-closing description is expanded: {inserted}");
    Ok(())
}

#[test]
fn sync_xmp_text_layers_follows_the_tysh_text() -> TestResult {
    let mut file = parse(&synthetic_text_file(8, Version::Psd));
    let text = text_index(&file, "Text");
    file.set_xmp_packet(PACKET);
    file.sync_xmp_text_layers()?;
    assert_eq!(file.xmp_text_layers(), vec![XmpTextLayer { name: "Text".into(), text: "Hello\r".into() }]);
    // A new TySh text reaches the XMP list on the next sync, and nowhere else.
    file.set_text_layer_text(text, "Two\rlines")?;
    file.sync_xmp_text_layers()?;
    assert_eq!(file.xmp_text_layers(), vec![XmpTextLayer { name: "Text".into(), text: "Two\rlines".into() }]);
    let back = parse(&file.to_bytes()?);
    assert_eq!(back.text_layer_text(text).expect("text").unwrap_or_else(|error| panic!("{error}")), "Two\rlines");
    assert_eq!(back.xmp_text_layers(), vec![XmpTextLayer { name: "Text".into(), text: "Two\rlines".into() }]);
    assert!(back.xmp_packet().is_some_and(|packet| packet.contains("application/vnd.adobe.photoshop")), "rest of the packet survives");
    Ok(())
}

// -------------------------------------------------------------------------
// Evidence files produced by Photoshop, when this workspace still has them
// -------------------------------------------------------------------------

#[test]
fn photoshop_probe_files_round_trip_when_present() -> TestResult {
    let probe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../out/probe5");
    let vertical = probe.join("cjkvert.psd");
    if vertical.exists() {
        let bytes = read(&vertical);
        let file = parse(&bytes);
        assert_eq!(file.to_bytes()?, bytes, "cjkvert.psd must write back byte for byte");
        let verticals: Vec<usize> = file.text_layers().filter(|(_, record)| TySh::from_bytes(&record.block(b"TySh").expect("TySh").data).unwrap_or_else(|error| panic!("TySh: {error}")).orientation() == Orientation::Vertical).map(|(index, _)| index).collect();
        assert_eq!(verticals.len(), 1, "one vertical text layer in cjkvert.psd");
        let bounds = file.text_layer_tysh(verticals[0]).expect("tysh").unwrap_or_else(|error| panic!("{error}")).layout_bounds().expect("layout bounds");
        assert!(bounds[2] - bounds[0] < bounds[3] - bounds[1], "vertical text: tall and narrow, {bounds:?}");
        let mut tysh = file.text_layer_tysh(verticals[0]).expect("tysh").unwrap_or_else(|error| panic!("{error}"));
        tysh.set_orientation(Orientation::Vertical);
        tysh.set_warp_settings(&tysh.warp_settings());
        let mut rewritten = file.clone();
        rewritten.set_text_layer_tysh(verticals[0], &tysh)?;
        assert_eq!(rewritten.to_bytes()?, bytes, "rewriting the same orientation/warp must not change the file");
    }
    let warped = probe.join("pE.psd");
    if warped.exists() {
        let bytes = read(&warped);
        let file = parse(&bytes);
        assert_eq!(file.to_bytes()?, bytes, "pE.psd must write back byte for byte");
        let arcs: Vec<usize> = file
            .text_layers()
            .filter(|(_, record)| TySh::from_bytes(&record.block(b"TySh").expect("TySh").data).unwrap_or_else(|error| panic!("TySh: {error}")).warp_settings().is_warped())
            .map(|(index, _)| index)
            .collect();
        assert_eq!(arcs.len(), 2, "pE.psd has the warpArc and warpArch probes");
        for index in arcs {
            let tysh = file.text_layer_tysh(index).expect("tysh").unwrap_or_else(|error| panic!("{error}"));
            let warp = tysh.warp_settings();
            assert_eq!(warp.value, 50.0, "Photoshop saves warp value 50");
            let points = tysh.bounds_points();
            let layout = tysh.layout_bounds().expect("layout bounds");
            for (point, value) in points.into_iter().zip(layout) {
                assert!((f64::from(point) - value).abs() < 1e-3, "the 16 bound bytes are the descriptor bounds as f32: {points:?} vs {layout:?}");
            }
            let mut rewritten = tysh.clone();
            rewritten.set_warp_settings(&warp);
            let mut file = file.clone();
            file.set_text_layer_tysh(index, &rewritten)?;
            assert_eq!(file.to_bytes()?, bytes, "rewriting the same warp must not change the file");
        }
    }
    Ok(())
}
