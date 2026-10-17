fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=contents.txt");
    println!("cargo:rerun-if-changed=beta.txt");
    owner::build_items([
        ("lorem/ipsum".into(), std::fs::read("contents.txt")?),
        ("empty".into(), Vec::new()),
        ("literal/\"quote\"\\path\n\0/✓".into(), vec![0, 255, 128, 0]),
        ("../../escape.rs".into(), b"opaque address".to_vec()),
    ])?;
    let beta = std::fs::read_to_string("beta.txt")?;
    owner::build_beta_items(beta.lines().map(|line| {
        let (key, value) = line.split_once(": ").expect("key: value");
        (key.to_owned(), value.as_bytes().to_vec())
    }))?;
    Ok(())
}
