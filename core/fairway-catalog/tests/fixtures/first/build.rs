fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=contents.txt");
    println!("cargo:rerun-if-changed=glossary.txt");
    owner::build_skills([
        ("git/commit".into(), std::fs::read("contents.txt")?),
        ("empty".into(), Vec::new()),
        ("literal/\"quote\"\\path\n\0/✓".into(), vec![0, 255, 128, 0]),
        ("../../escape.rs".into(), b"opaque address".to_vec()),
    ])?;
    let glossary = std::fs::read_to_string("glossary.txt")?;
    owner::build_glossary(glossary.lines().map(|line| {
        let (term, definition) = line.split_once(": ").expect("term: definition");
        (term.to_owned(), definition.as_bytes().to_vec())
    }))?;
    Ok(())
}
