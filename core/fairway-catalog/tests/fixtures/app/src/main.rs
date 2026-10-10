use std::borrow::Cow;

use catalog_fixture_first as _;
use catalog_fixture_second as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Fairway's part: the catalog is initialized at startup.
    catalog::initialize()?;

    let Some(Cow::Borrowed(commit)) = owner::skill("git/commit") else {
        panic!("a builtin value borrows the embedded bytes");
    };
    assert_eq!(owner::skill("empty").as_deref(), Some(&b""[..]));
    assert_eq!(
        owner::skill("literal/\"quote\"\\path\n\0/✓").as_deref(),
        Some(&[0, 255, 128, 0][..])
    );
    assert_eq!(
        owner::skill("../../escape.rs").as_deref(),
        Some(&b"opaque address"[..])
    );
    assert_eq!(
        owner::skills("git/*"),
        ["git/commit", "git/hooks/pre-commit"]
    );
    assert_eq!(owner::term("git").as_deref(), Some(&b"version control"[..]));
    print!("{}", std::str::from_utf8(commit)?);
    println!("{}", owner::terms().join(","));

    owner::put_skill("git/commit", b"replacement".to_vec())?;
    catalog::initialize()?;
    assert_eq!(
        owner::skill("git/commit").as_deref(),
        Some(&b"replacement"[..])
    );
    Ok(())
}
