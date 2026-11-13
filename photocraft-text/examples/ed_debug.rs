use photocraft_text::engine_data::{Value, parse};
fn main() {
    let data = std::fs::read("tests/data/plain_latin_engine_data.txt").unwrap();
    let v = parse(&data).unwrap();
    if let Value::Dict(d) = &v {
        println!(
            "top keys: {:?}",
            d.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()
        );
        if let Some(Value::Dict(e)) = v.get("EngineDict") {
            println!(
                "engine keys: {:?}",
                e.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()
            );
        }
    }
}
