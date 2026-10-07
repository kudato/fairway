use catalog_fixture_first as _;
use catalog_fixture_second as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let expected = std::env::args().nth(1).expect("expected builtin contents");
    catalog::initialize()?;
    assert_eq!(
        catalog::get("skill/git/commit")?,
        Some(expected.into_bytes())
    );
    assert_eq!(
        catalog::get("resource/git/convention")?,
        Some(b"convention".to_vec())
    );
    assert_eq!(catalog::get("empty")?, Some(Vec::new()));
    assert_eq!(
        catalog::get("literal/\"quote\"\\path\n\0/✓")?,
        Some(vec![0, 255, 128, 0])
    );
    assert_eq!(
        catalog::get("../../escape.rs")?,
        Some(b"opaque address".to_vec())
    );
    assert_eq!(
        catalog::list("skill/git/*")?,
        ["skill/git/commit", "skill/git/hooks/pre-commit"]
    );

    catalog::put("skill/git/commit", b"replacement")?;
    catalog::put("runtime", b"added")?;
    catalog::initialize()?;
    assert_eq!(
        catalog::get("skill/git/commit")?,
        Some(b"replacement".to_vec())
    );
    assert_eq!(catalog::get("runtime")?, Some(b"added".to_vec()));
    Ok(())
}
