//! Live text editor: a tiny HTTP server over the engine, for poking text and
//! styles interactively in a browser.
//!
//!     cargo run --example live -- [port] [font dir] [atlas dir] [template.psd]
//!
//! `GET /` serves the editor; `GET /render?text=..&size=..` returns a PNG of
//! the rendered layer. With `atlas=1` and a captured glyph atlas next to the
//! font directory, glyphs come from Photoshop's own bitmaps. `POST /export.psd`
//! takes the same JSON body as `POST /render` and returns a PSD whose text layer
//! carries that text and split.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use photocraft_text::layer_render::{FaceStore, LayerTextStyle, RenderOptions};
use photocraft_text::runs::RunStyle;

#[path = "common/author.rs"]
mod common_author;

const PAGE: &str = include_str!("live.html");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(7878);
    let fonts = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default_font_dir()));
    let atlas_dir = args.next().map(PathBuf::from);
    // The PSD the exported layer is authored into: a one-text-layer document
    // whose paragraph/style tables get rewritten.
    let template = args
        .next()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(default_template);

    let mut store = FaceStore::new();
    store.load_dir(&fonts);
    if let Some(dir) = &atlas_dir {
        match photocraft_text::atlas::Atlas::load(dir) {
            Ok(atlas) => {
                println!("atlas: {} @ {} pt, {} glyphs", atlas.font, atlas.size, atlas.len());
                store.set_atlas(atlas);
            }
            Err(e) => eprintln!("atlas: {e}"),
        }
    }
    if store.names().next().is_none() {
        return Err(format!("no fonts in {}", fonts.display()).into());
    }
    let store = Arc::new(store);
    let area_template = args
        .next()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(default_area_template);
    let templates = Templates {
        point: template,
        area: area_template,
    };
    match &templates.point {
        Some(path) => println!("psd template: {}", path.display()),
        None => println!("psd template: none (export disabled)"),
    }
    if let Some(path) = &templates.area {
        println!("psd area template: {}", path.display());
    }

    let listener = TcpListener::bind(("127.0.0.1", port))?;
    println!("live editor: http://127.0.0.1:{port}/  (fonts: {})", fonts.display());
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let store = store.clone();
        let template = templates.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(stream, &store, &template) {
                eprintln!("connection: {e}");
            }
        });
    }
    Ok(())
}

/// A text-layer template next to the crate: the corpus document the engine was
/// measured against.
fn default_template() -> Option<PathBuf> {
    ["../corpus/plain_latin.psd", "corpus/plain_latin.psd"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

/// Area-text template: a one-text-layer document whose `Txt2` carries the frame
/// rectangle, which a point-text document does not have.
fn default_area_template() -> Option<PathBuf> {
    ["../corpus/box_left.psd", "corpus/box_left.psd"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

/// The template for this export: area text needs the frame-carrying document.
fn export_template<'a>(templates: &'a Templates, body: &[u8]) -> Option<&'a Path> {
    let spec: common_author::LayerSpec = serde_json::from_slice(body).ok()?;
    if spec.frame.is_some() {
        templates.area.as_deref()
    } else {
        templates.point.as_deref()
    }
}

fn default_font_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.local/share/photocraft/wine/drive_c/windows/Fonts")
}

/// The template PSDs an exported layer is authored into: point text keeps the
/// plain document, area text needs one whose `Txt2` carries a frame rectangle.
#[derive(Clone, Debug, Default)]
struct Templates {
    point: Option<PathBuf>,
    area: Option<PathBuf>,
}

fn serve(
    mut stream: std::net::TcpStream,
    store: &FaceStore,
    templates: &Templates,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    let target = request.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    if path == "/" || path == "/index.html" {
        return respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes());
    }
    if (path == "/render" || path == "/render.png") && !body.is_empty() {
        return match render_json(store, &body) {
            Ok(png) => respond(&mut stream, "200 OK", "image/png", &png),
            Err(e) => respond(
                &mut stream,
                "500 Internal Server Error",
                "text/plain",
                e.to_string().as_bytes(),
            ),
        };
    }
    if path == "/render" || path == "/render.png" {
        let q = Query::parse(query);
        return match render(store, &q) {
            Ok(png) => respond(&mut stream, "200 OK", "image/png", &png),
            Err(e) => respond(
                &mut stream,
                "500 Internal Server Error",
                "text/plain",
                e.to_string().as_bytes(),
            ),
        };
    }
    if path == "/export.psd" || path == "/export" {
        let Some(template) = export_template(templates, &body) else {
            return respond(
                &mut stream,
                "503 Service Unavailable",
                "text/plain",
                b"no psd template for this text kind; pass one as the fourth argument",
            );
        };
        return match export_psd(template, &body) {
            Ok(psd) => respond_psd(&mut stream, "text.psd", &psd),
            Err(e) => respond(
                &mut stream,
                "500 Internal Server Error",
                "text/plain",
                e.to_string().as_bytes(),
            ),
        };
    }
    if path == "/config" {
        let names: Vec<&str> = store.names().collect();
        let atlas = store.atlas().map(|a| {
            serde_json::json!({
                "font": a.font,
                "size": a.size,
                "glyphs": a.len(),
            })
        });
        let body = serde_json::json!({
            "fonts": names,
            "atlas": atlas,
            "export": templates.point.is_some(),
            "export_area": templates.area.is_some(),
        })
        .to_string();
        return respond(&mut stream, "200 OK", "application/json", body.as_bytes());
    }
    respond(&mut stream, "404 Not Found", "text/plain", b"not found")
}

fn respond_psd(
    stream: &mut std::net::TcpStream,
    name: &str,
    body: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: image/vnd.adobe.photoshop\r\n\
         Content-Disposition: attachment; filename=\"{name}\"\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

fn respond(
    stream: &mut std::net::TcpStream,
    status: &str,
    ctype: &str,
    body: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

/// Minimal percent-decoded query string.
struct Query(Vec<(String, String)>);

impl Query {
    fn parse(raw: &str) -> Self {
        let mut out = Vec::new();
        for pair in raw.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            out.push((decode(k), decode(v)));
        }
        Self(out)
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn f32(&self, key: &str, default: f32) -> f32 {
        self.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }

    fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some("1") | Some("true") | Some("on"))
    }
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `POST /render` body: the whole layer, with per-range runs.
///
/// ```json
/// { "text": "Hi\nall", "atlas": true, "aa": 4,
///   "runs": [ { "len": 2, "size": 72, "color": [255,0,0] },
///             { "len": 5, "size": 48 } ] }
/// ```
fn render_json(store: &FaceStore, body: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let spec: common_author::LayerSpec = serde_json::from_slice(body)?;
    let text = spec.text.as_str();
    let total = text.encode_utf16().count();
    let mut runs: Vec<RunStyle> = spec
        .runs
        .iter()
        .filter(|r| r.len > 0)
        .map(run_style_of)
        .collect();
    // The editor sends the runs it has; anything uncovered continues the last
    // run's style, and an empty list means one default run.
    let covered: usize = runs.iter().map(|r| r.len).sum();
    if covered < total {
        let mut filler = runs.last().cloned().unwrap_or_else(|| RunStyle {
            len: 0,
            size: Some(48.0),
            ..RunStyle::default()
        });
        filler.len = total - covered;
        runs.push(filler);
    }
    if runs.is_empty() {
        runs.push(RunStyle { len: total.max(1), size: Some(48.0), ..RunStyle::default() });
    }
    let first = &runs[0];
    let paragraph = &spec.paragraph;
    let layer = LayerTextStyle {
        style: photocraft_text::layout::TextStyle {
            size: first.size.unwrap_or(48.0),
            tracking: first.tracking.unwrap_or(0),
            kerning: match (first.auto_kerning, first.kerning) {
                (_, Some(k)) if first.auto_kerning == Some(false) => {
                    photocraft_text::layout::Kerning::Manual(k)
                }
                _ => photocraft_text::layout::Kerning::Metrics,
            },
            horizontal_scale: first.horizontal_scale.unwrap_or(1.0),
            baseline_shift: first.baseline_shift.unwrap_or(0.0),
            leading: first.leading,
            auto_leading: paragraph.auto_leading,
        },
        paragraph: photocraft_text::area::ParagraphStyle {
            justification: photocraft_text::area::Justification::from_psd(paragraph.justification as i32),
            first_line_indent: paragraph.first_line_indent,
            start_indent: paragraph.start_indent,
            end_indent: paragraph.end_indent,
            space_before: paragraph.space_before,
            space_after: paragraph.space_after,
            word_spacing: paragraph.word_spacing,
            letter_spacing: paragraph.letter_spacing,
            glyph_spacing: paragraph.glyph_scaling,
            auto_leading: paragraph.auto_leading,
            every_line_composer: paragraph.every_line_composer,
            ..Default::default()
        },
        box_size: match spec.frame {
            Some([l, t, r, b]) => ((r - l).max(1.0), (b - t).max(1.0)),
            None => (0.0, 0.0),
        },
        origin: match spec.frame {
            Some([l, t, _, _]) => (l, t),
            None => (0.0, 0.0),
        },
        color: first.color.unwrap_or([0, 0, 0]),
    };
    let options = RenderOptions {
        aa: spec.aa.and_then(photocraft_text::raster::AntiAlias::from_code),
        use_atlas: spec.atlas.unwrap_or(true),
    };
    let pixels = store.render_spec_with(text, &runs, &layer, options)?;
    Ok(encode_png(&pixels.rgba, pixels.width, pixels.height))
}

fn run_style_of(run: &common_author::RunSpec) -> RunStyle {
    RunStyle {
        len: run.len,
        font: run.font.clone(),
        size: run.size,
        leading: run.leading,
        auto_leading: run.auto_leading,
        tracking: run.tracking,
        baseline_shift: run.baseline_shift,
        horizontal_scale: run.horizontal_scale,
        vertical_scale: run.vertical_scale,
        color: run.color,
        auto_kerning: run.auto_kerning,
        kerning: run.kerning,
        faux_bold: run.bold,
        faux_italic: run.italic,
        underline: run.underline,
        strikethrough: run.strikethrough,
        all_caps: run.all_caps,
        small_caps: run.small_caps,
        ..RunStyle::default()
    }
}

/// Authors `body`'s layer into the template PSD.
///
/// The style-run table Photoshop reads must cover the text *plus* its trailing
/// U+000D terminator, which the editor's lengths (plain text units) do not, so
/// the terminator is added to the last run here.
fn export_psd(
    template: &Path,
    body: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut spec: common_author::LayerSpec = serde_json::from_slice(body)?;
    let total = spec
        .text
        .replace("\r\n", "\r")
        .replace('\n', "\r")
        .trim_end_matches(['\r', '\n'])
        .encode_utf16()
        .count();
    if total == 0 {
        return Err("empty text".into());
    }
    let covered: usize = spec.runs.iter().map(|r| r.len).sum();
    if covered < total {
        let mut filler = spec.runs.last().cloned().unwrap_or_default();
        filler.len = total - covered;
        spec.runs.push(filler);
    }
    match spec.runs.last_mut() {
        Some(last) => last.len += 1,
        None => return Err("no runs".into()),
    }
    let bytes = std::fs::read(template)?;
    let mut file = photocraft_psd::PsdFile::from_bytes(&bytes)?;
    common_author::author(&mut file, &spec)?;
    Ok(file.to_bytes()?)
}

fn render(store: &FaceStore, q: &Query) -> Result<Vec<u8>, photocraft_text::error::Error> {
    let text = q.get("text").unwrap_or("Hamburgefonstiv");
    let size = q.f32("size", 48.0);
    let color = q.get("color").unwrap_or("000000");
    let color = parse_color(color);
    let run = RunStyle {
        len: text.encode_utf16().count().max(1),
        font: Some(q.get("font").unwrap_or("NotoSans-Regular").to_owned()),
        size: Some(size),
        tracking: Some(q.f32("tracking", 0.0) as i16),
        horizontal_scale: Some(q.f32("hscale", 100.0) / 100.0),
        vertical_scale: Some(q.f32("vscale", 100.0) / 100.0),
        baseline_shift: Some(q.f32("baseline", 0.0)),
        color: Some(color),
        faux_bold: q.flag("bold"),
        faux_italic: q.flag("italic"),
        underline: q.flag("underline"),
        strikethrough: q.flag("strike"),
        all_caps: q.flag("caps"),
        ..RunStyle::default()
    };
    let style = photocraft_text::layout::TextStyle {
        size,
        ..Default::default()
    };
    let spec = LayerTextStyle {
        style,
        paragraph: Default::default(),
        box_size: (0.0, 0.0),
        origin: (0.0, 0.0),
        color,
    };
    let options = RenderOptions {
        aa: q.get("aa").and_then(|v| v.parse().ok()).and_then(photocraft_text::raster::AntiAlias::from_code),
        use_atlas: q.flag("atlas"),
    };
    let pixels = store.render_spec_with(text, &[run], &spec, options)?;
    Ok(encode_png(&pixels.rgba, pixels.width, pixels.height))
}

fn parse_color(s: &str) -> [u8; 3] {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() >= 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        [r, g, b]
    } else {
        [0, 0, 0]
    }
}

fn encode_png(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in 0..h as usize {
        raw.push(0); // filter: none
        let start = row * w as usize * 4;
        raw.extend_from_slice(&rgba[start..start + w as usize * 4]);
    }
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_store(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// zlib stream with stored (uncompressed) deflate blocks.
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut offset = 0;
    while offset < data.len() {
        let len = (data.len() - offset).min(65_535);
        out.push(if offset + len >= data.len() { 1 } else { 0 });
        out.extend_from_slice(&(len as u16).to_le_bytes());
        out.extend_from_slice(&(!(len as u16)).to_le_bytes());
        out.extend_from_slice(&data[offset..offset + len]);
        offset += len;
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[allow(dead_code)]
fn unused(_: &Path) {}

/// Reads the whole body of a request; kept for future POST endpoints.
#[allow(dead_code)]
fn read_body(stream: &mut std::net::TcpStream) -> std::io::Result<String> {
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    Ok(buf)
}
