//! Renders one text layer of a PSD from its stored model (`TySh` `EngineData`
//! plus the `Txt2` frame) with the crate's own layout/rasteriser and writes a
//! PPM plus a one-line JSON description of the rectangle.  The surface is
//! painted straight into the layer record's rectangle, so it can be diffed
//! against Photoshop's stored layer pixels pixel for pixel.
//!
//! `cargo run --example render_psd -- file.psd <text-layer-ordinal> out.ppm [frame l,t,r,b]`

use std::io::Write;
use std::path::PathBuf;

use photocraft_psd::PsdFile;
use photocraft_psd::text_layer::Orientation;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        return Err("usage: render_psd <psd> <text-ordinal> <out.ppm> [l,t,r,b]".into());
    }
    let file = PsdFile::from_bytes(&std::fs::read(&args[1])?)?;
    let ordinal: usize = args[2].parse()?;
    let (layer_index, _) = file
        .text_layers()
        .nth(ordinal)
        .ok_or("text layer ordinal out of range")?;
    let record = file.layers().get(layer_index).ok_or("layer index")?;
    let rect = record.rect;
    let block = record.block(b"TySh").ok_or("no TySh")?.data.clone();
    let tysh = photocraft_psd::text_layer::TySh::from_bytes(&block)?;
    let text = tysh.text().ok_or("no TySh text")?;
    let ed_bytes = tysh.engine_data().ok_or("no EngineData")?;
    let ed = photocraft_text::engine_data::parse(ed_bytes)?;
    let orientation = tysh.orientation();
    let warp = tysh.warp_settings();

    let mut store = photocraft_text::layer_render::FaceStore::new();
    if let Some(home) = std::env::var_os("HOME") {
        store.load_dir(
            &PathBuf::from(home).join(".local/share/photocraft/wine/drive_c/windows/Fonts"),
        );
    }
    if let Some(dirs) = std::env::var_os("PHOTOCRAFT_FONT_DIRS") {
        for dir in std::env::split_paths(&dirs) {
            store.load_dir(&dir);
        }
    }

    // A Photoshop-captured glyph atlas (`tools/make_glyph_atlas.py`) makes the
    // hinted TrueType/CJK glyph bitmaps byte-identical to Photoshop's; CoolType's
    // hinting is proprietary (FreeType's interpreter produces different pixels),
    // so the atlas is the exact path.  One layer per process, so one atlas.
    if let Some(dir) = std::env::var_os("PHOTOCRAFT_ATLAS_DIR") {
        let runs = photocraft_text::runs::from_engine_data(&ed);
        let aa = photocraft_text::runs::antialias(&ed);
        let font = runs.first().and_then(|r| r.font.clone());
        let size = runs.first().and_then(|r| r.size).unwrap_or(0.0);
        let bold = runs.first().is_some_and(|r| r.faux_bold);
        let hscale = runs.first().and_then(|r| r.horizontal_scale).unwrap_or(1.0);
        // Synthetic small caps draw the capital at `size * 0.8`, so a capture at
        // that size belongs to the layer too (the engine's own lookup already
        // matches it by `effective_size`; this pre-filter must not drop it).
        let sizes = [size, size * photocraft_text::runs::SMALL_CAPS_SCALE];
        if let Some(font) = font {
            let mut found = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let Ok(atlas) = photocraft_text::atlas::Atlas::load(&path) else {
                        continue;
                    };
                    if atlas.font == font
                        && sizes.iter().any(|s| (atlas.size - s).abs() < 0.01)
                        && atlas.aa == aa.code()
                        && atlas.bold == bold
                        && !atlas.italic
                        && (atlas.hscale - hscale).abs() < 0.001
                    {
                        found.push(atlas);
                    }
                }
            }
            if !found.is_empty() {
                if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v != "0") {
                    eprintln!("atlas: {} size {size} phases {}", font, found.len());
                }
                store.set_atlases(found);
            }
        }
    }

    let frame: Option<[f32; 4]> = if args.len() >= 5 {
        let v: Vec<f32> = args[4]
            .split(',')
            .map(|s| s.parse())
            .collect::<Result<_, _>>()?;
        if v.len() != 4 {
            return Err("frame must be l,t,r,b".into());
        }
        Some([v[0], v[1], v[2], v[3]])
    } else {
        None
    };
    let paras = photocraft_text::runs::paragraphs_from_engine_data(&ed);
    // Old-style figures are a character-sheet feature: Photoshop writes it only
    // to `Txt2` `/6/30`, never to `EngineData`, so a renderer needs both blocks.
    // The app reads it per style run from its `Txt2` model; this example can only
    // switch it on for the whole layer (`PHOTOCRAFT_OLDSTYLE=1`).
    let old_style = std::env::var("PHOTOCRAFT_OLDSTYLE").is_ok_and(|v| v != "0");
    let run_count = photocraft_text::runs::from_engine_data(&ed).len();
    // `PHOTOCRAFT_ADVANCES_JSON` holds per-object per-glyph advance deltas in
    // points, extracted from `Txt2`'s composed cache
    // (`tools/txt2_advances.py`). They are keyed by the text-object index, the
    // same order the layers appear in the document.
    let advances: Vec<f32> = std::env::var("PHOTOCRAFT_ADVANCES_JSON")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| {
            let key = format!("\"{}\"", ordinal);
            let at = text.find(&key)?;
            let start = text[at + key.len()..].find('[')? + at + key.len();
            let end = start + text[start..].find(']')?;
            let body = &text[start + 1..end];
            let mut out = Vec::new();
            for piece in body.split(',') {
                if let Ok(v) = piece.trim().parse::<f32>() {
                    out.push(v);
                }
            }
            (!out.is_empty()).then_some(out)
        })
        .unwrap_or_default();
    // The PSD does not store Photoshop's kerning mode; a caller that knows the
    // layer was authored with optical kerning asks for it explicitly.
    let optical = std::env::var("PHOTOCRAFT_OPTICAL")
        .ok()
        .is_some_and(|v| v.split(',').any(|p| p.trim() == ordinal.to_string()))
        || std::env::var("PHOTOCRAFT_OPTICAL").is_ok_and(|v| v == "all");
    let features: Vec<photocraft_text::layer_render::RunFeatures> = vec![
            photocraft_text::layer_render::RunFeatures {
                old_style,
                optical_kerning: optical,
                advance_deltas: advances,
                word_spacing_deltas: Vec::new(),
            };
            run_count
        ];
    let trace = std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v != "0");
    if trace {
        eprintln!("aa={:?}", photocraft_text::runs::antialias(&ed));
        for (i, p) in paras.iter().enumerate() {
            eprintln!(
                "para {i}: just={:?} fli={} si={} ei={} sb={} sa={} lead={} evl={} hyph={} hang={}",
                p.justification,
                p.first_line_indent,
                p.start_indent,
                p.end_indent,
                p.space_before,
                p.space_after,
                p.auto_leading,
                p.every_line_composer,
                p.hyphenate,
                p.hanging
            );
        }
    }
    if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v.parse::<u32>().is_ok_and(|n| n >= 5)) {
        for (i, r) in photocraft_text::runs::from_engine_data(&ed)
            .iter()
            .enumerate()
        {
            eprintln!(
                "run {i}: font={:?} size={:?} tracking={:?} hscale={:?} kerning={:?}",
                r.font, r.size, r.tracking, r.horizontal_scale, r.kerning
            );
        }
    }
    let box_size = frame
        .map(|f| (f[2] - f[0], f[3] - f[1]))
        .unwrap_or((0.0, 0.0));
    let first_off =
        photocraft_text::layer_render::first_baseline_offset_of(&store, &ed).unwrap_or(0.0);
    let tx = tysh.transform[4] as f32;
    let ty = tysh.transform[5] as f32;
    let (origin, anchor) = match (frame, orientation) {
        (Some(f), Orientation::Horizontal) => ((0.0, 0.0), (tx + f[0], ty + f[1] + first_off)),
        (Some(f), Orientation::Vertical) => ((0.0, 0.0), (tx + f[0], ty + f[1])),
        (None, Orientation::Vertical) => ((0.0, 0.0), (tx, ty)),
        (None, Orientation::Horizontal) => ((0.0, 0.0), (tx, ty)),
    };
    let warped = warp.is_warped() && std::env::var("PHOTOCRAFT_WARP").map_or(true, |v| v != "0");
    // A warped layer goes through the crate's one-shot adapter: it renders the
    // flat text on a padded canvas and bends it into the stored rectangle, so
    // the flat and warped geometries cannot be mixed up here. `PHOTOCRAFT_PAD`
    // still pads the non-warped comparison path.
    let pad: i32 = std::env::var("PHOTOCRAFT_PAD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let target = photocraft_text::layer_render::RectTarget {
        origin: (rect.left - pad, rect.top - pad),
        size: (
            (rect.width() + 2 * i64::from(pad)).max(1) as u32,
            (rect.height() + 2 * i64::from(pad)).max(1) as u32,
        ),
        anchor,
    };
    let mut pixels = if warped && orientation == Orientation::Horizontal {
        let (bounds, baseline) = match tysh.layout_bounds() {
            Some([l, t, r, b]) => (
                (tx + l as f32, ty + t as f32, tx + r as f32, ty + b as f32),
                ty,
            ),
            None => ((0.0, 0.0, 0.0, 0.0), ty),
        };
        let params = photocraft_text::warp::Warp {
            style: photocraft_text::warp::Style::from_name(&warp.style),
            value: warp.value as f32,
            perspective: warp.perspective as f32,
            other: warp.perspective_other as f32,
        };
        let warped_target = photocraft_text::layer_render::RectTarget {
            origin: (rect.left, rect.top),
            size: (rect.width().max(1) as u32, rect.height().max(1) as u32),
            anchor,
        };
        photocraft_text::layer_render::render_from_engine_data_warped(
            &store,
            &ed,
            &text,
            &paras,
            &features,
            box_size,
            &params,
            photocraft_text::warp::FlatGeometry { bounds, baseline },
            warped_target,
        )?
        .pixels
    } else {
        match orientation {
            Orientation::Vertical => {
                photocraft_text::layer_render::render_vertical_from_engine_data_in_rect(
                    &store, &ed, &text, origin, frame.map(|f| (f[2] - f[0], f[3] - f[1])), target,
                )?
            }
            Orientation::Horizontal => {
                photocraft_text::layer_render::render_from_engine_data_in_rect_with_features(
                    &store, &ed, &text, &paras, &features, box_size, origin, target,
                )?
            }
        }
    };

    let mut out = std::fs::File::create(&args[3])?;
    write!(out, "P6\n{} {}\n255\n", pixels.width, pixels.height)?;
    for px in pixels.rgba.chunks_exact(4) {
        let a = u32::from(px[3]);
        let blend = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
        out.write_all(&[blend(px[0]), blend(px[1]), blend(px[2])])?;
    }
    let alpha_path = format!("{}.alpha.pgm", args[3]);
    let mut alpha = std::fs::File::create(alpha_path)?;
    write!(alpha, "P5\n{} {}\n255\n", pixels.width, pixels.height)?;
    for px in pixels.rgba.chunks_exact(4) {
        alpha.write_all(&[px[3]])?;
    }
    let head: String = text
        .chars()
        .take(32)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    println!(
        r#"{{"left": {}, "top": {}, "width": {}, "height": {}, "rect": [{}, {}, {}, {}], "first_baseline": {:.3}, "orientation": "{:?}", "warp": "{}/{:.0}", "text": "{}"}}"#,
        rect.left,
        rect.top,
        pixels.width,
        pixels.height,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        first_off,
        orientation,
        warp.style,
        warp.value,
        head.replace('\\', "\\\\").replace('"', "\\\"")
    );
    Ok(())
}
