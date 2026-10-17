use std::borrow::Cow;

use catalog_fixture_first as _;
use catalog_fixture_second as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Fairway's part: the catalog is initialized at startup.
    catalog::initialize()?;

    let Some(Cow::Borrowed(ipsum)) = owner::item("lorem/ipsum") else {
        panic!("a builtin value borrows the embedded bytes");
    };
    assert_eq!(owner::item("empty").as_deref(), Some(&b""[..]));
    assert_eq!(
        owner::item("literal/\"quote\"\\path\n\0/✓").as_deref(),
        Some(&[0, 255, 128, 0][..])
    );
    assert_eq!(
        owner::item("../../escape.rs").as_deref(),
        Some(&b"opaque address"[..])
    );
    assert_eq!(
        owner::items("lorem/*"),
        ["lorem/ipsum", "lorem/sit/dolorem-ipsum"]
    );
    assert_eq!(
        owner::beta_item("lorem").as_deref(),
        Some(&b"ipsum dolor"[..])
    );
    print!("{}", std::str::from_utf8(ipsum)?);
    println!("{}", owner::beta_items().join(","));

    owner::put_item("lorem/ipsum", b"replacement".to_vec())?;
    catalog::initialize()?;
    assert_eq!(
        owner::item("lorem/ipsum").as_deref(),
        Some(&b"replacement"[..])
    );
    Ok(())
}
