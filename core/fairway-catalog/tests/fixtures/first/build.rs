fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=contents.txt");
    catalog::build([
        ("skill/git/commit".into(), std::fs::read("contents.txt")?),
        ("empty".into(), Vec::new()),
        ("literal/\"quote\"\\path\n\0/✓".into(), vec![0, 255, 128, 0]),
        ("../../escape.rs".into(), b"opaque address".to_vec()),
    ])?;
    Ok(())
}
