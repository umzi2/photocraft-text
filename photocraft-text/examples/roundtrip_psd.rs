use photocraft_psd::PsdFile;
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let o = std::env::args().nth(2).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    let file = PsdFile::from_bytes(&bytes).unwrap();
    let out = file.to_bytes().unwrap();
    println!("in {} out {} equal {}", bytes.len(), out.len(), bytes == out);
    std::fs::write(&o, &out).unwrap();
}
