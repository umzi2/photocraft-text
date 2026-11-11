use photocraft_psd::PsdFile;
use photocraft_psd::descriptor::VersionedDescriptor;
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    let file = PsdFile::from_bytes(&bytes).unwrap();
    let layer = file.layers().iter().find(|l| l.blocks.iter().any(|b| &b.key == b"TySh")).unwrap();
    let block = layer.blocks.iter().find(|b| &b.key == b"TySh").unwrap();
    let data = &block.data;
    println!("len {}", data.len());
    let ver = u16::from_be_bytes([data[0], data[1]]);
    let text_version = u16::from_be_bytes([data[50], data[51]]);
    let (desc, used) = VersionedDescriptor::parse_prefix(&data[52..]).unwrap();
    let warp_at = 52 + used;
    let warp_version = u16::from_be_bytes([data[warp_at], data[warp_at + 1]]);
    let (warp, wused) = VersionedDescriptor::parse_prefix(&data[warp_at + 2..]).unwrap();
    let tail_at = warp_at + 2 + wused;
    println!("ver {ver} tver {text_version} used {used} warp_ver {warp_version} wused {wused} tail {}", data.len() - tail_at);
    let mut out = Vec::new();
    out.extend_from_slice(&ver.to_be_bytes());
    out.extend_from_slice(&data[2..50]);
    out.extend_from_slice(&text_version.to_be_bytes());
    out.extend_from_slice(&desc.to_bytes());
    out.extend_from_slice(&warp_version.to_be_bytes());
    out.extend_from_slice(&warp.to_bytes());
    out.extend_from_slice(&data[tail_at..]);
    println!("rebuild {} original {} equal {}", out.len(), data.len(), out == *data);
    if out != *data {
        for (i, (a, b)) in out.iter().zip(data.iter()).enumerate() {
            if a != b { println!("first diff at {i}: {a:02x} vs {b:02x}"); break; }
        }
    }
    let _ = tail_at;
}
