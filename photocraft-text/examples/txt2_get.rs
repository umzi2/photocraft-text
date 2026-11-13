//! Prints the value at a `Txt2` path, e.g. `-- file.psd k1 k1 k0 k0 k5 k0 0 k0 k0 k5 k0`.
//! `k<n>` is a dictionary key, a bare number is an array index.
use photocraft_psd::PsdFile;
use photocraft_text::txt2::{self, Step};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let file = PsdFile::from_bytes(&std::fs::read(&a[1])?)?;
    let block = file
        .global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .ok_or("no Txt2 block")?
        .data
        .clone();
    if a.get(2).map(String::as_str) == Some("run0") {
        let mut path = txt2::run_path_for(0);
        path.extend([
            Step::Key("0"),
            Step::Key("0"),
            Step::Key("6"),
            Step::Key("5"),
        ]);
        println!("path {path:?}");
        println!(
            "{:?}",
            txt2::read_path(&block, &path)?.map(|v| String::from_utf8_lossy(&v).to_string())
        );
        let path2 = vec![
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(0),
            Step::Key("0"),
            Step::Key("5"),
            Step::Key("0"),
            Step::Index(0),
            Step::Key("0"),
            Step::Key("0"),
            Step::Key("5"),
            Step::Key("0"),
        ];
        println!(
            "para {:?}",
            txt2::read_path(&block, &path2)?.map(|v| String::from_utf8_lossy(&v).to_string())
        );
        let spec = [
            "k1", "k1", "k0", "k0", "k5", "k0", "0", "k0", "k0", "k5", "k0",
        ];
        let path3: Vec<Step> = spec
            .iter()
            .map(|s| match s.strip_prefix('k') {
                Some(key) => Step::Key(key),
                None => Step::Index(s.parse::<usize>().unwrap()),
            })
            .collect();
        println!("path3 {path3:?}");
        println!(
            "para3 {:?}",
            txt2::read_path(&block, &path3)?.map(|v| String::from_utf8_lossy(&v).to_string())
        );
        let path4: Vec<Step> = vec![
            Step::Key("1"),
            Step::Key("1"),
            Step::Key("0"),
            Step::Key("0"),
            Step::Key("5"),
            Step::Key("0"),
            Step::Index(0),
            Step::Key("0"),
            Step::Key("0"),
            Step::Key("5"),
            Step::Key("0"),
        ];
        println!("path2 {path2:?}");
        println!("path4 {path4:?}");
        println!("path4 == path2: {}", path4 == path2);
        println!("path4 == path3: {}", path4 == path3);
        return Ok(());
    }
    let path: Vec<Step> = a[2..]
        .iter()
        .map(|s| match s.strip_prefix('k') {
            Some(key) => Step::Key(key),
            None => Step::Index(s.parse::<usize>().expect("number or k<key>")),
        })
        .collect();
    if std::env::var_os("TXT2_DEBUG").is_some() {
        eprintln!("path {path:?}");
    }
    match txt2::read_path(&block, &path)? {
        Some(v) => println!("{}", String::from_utf8_lossy(&v)),
        None => println!("(missing)"),
    }
    Ok(())
}
