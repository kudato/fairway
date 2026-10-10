//! Builtin tables of several crates fill their namespaces without copying.

use std::{borrow::Cow, ptr};

use fairway_catalog::{__private, initialize};

fairway_catalog::namespace!(SKILLS: Cow<'static, [u8]>, "skill", builtin);
fairway_catalog::namespace!(GLOSSARY: Cow<'static, [u8]>, "glossary", builtin);

static COMMIT: [u8; 6] = *b"commit";

// Two crates embed into one namespace; the second one also into another.
#[__private::linkme::distributed_slice(__private::FAIRWAY_CATALOG_BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code, reason = "linkme places the table in a linker section")]
static FIRST: __private::Builtin = __private::Builtin::new(
    "skill",
    &[
        ("git/commit", &COMMIT),
        ("git/commit/convention", b"convention"),
    ],
    "first crate",
);

#[__private::linkme::distributed_slice(__private::FAIRWAY_CATALOG_BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code, reason = "linkme places the table in a linker section")]
static SECOND_SKILLS: __private::Builtin =
    __private::Builtin::new("skill", &[("rust/check", b"check")], "second crate");

#[__private::linkme::distributed_slice(__private::FAIRWAY_CATALOG_BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code, reason = "linkme places the table in a linker section")]
static SECOND_GLOSSARY: __private::Builtin =
    __private::Builtin::new("glossary", &[("git", b"version control")], "second crate");

#[test]
fn builtin_tables_are_the_initial_values() {
    initialize().unwrap();
    assert_eq!(
        SKILLS.list("*"),
        ["git/commit", "git/commit/convention", "rust/check"]
    );
    assert_eq!(GLOSSARY.list("*"), ["git"]);
    assert_eq!(
        GLOSSARY.get("git").as_deref(),
        Some(&b"version control"[..])
    );

    let Some(Cow::Borrowed(commit)) = SKILLS.get("git/commit") else {
        panic!("a builtin value borrows the embedded bytes");
    };
    assert!(ptr::eq(commit, &COMMIT[..]));

    let previous = SKILLS.put("git/commit", Cow::Owned(b"replacement".to_vec()));
    assert!(matches!(
        previous,
        Ok(Some(Cow::Borrowed(bytes))) if ptr::eq(bytes, &COMMIT[..])
    ));
    assert!(matches!(
        SKILLS.get("git/commit"),
        Some(Cow::Owned(bytes)) if bytes == b"replacement"
    ));
    let removed = SKILLS.delete("rust/check").unwrap();
    assert_eq!(removed.as_deref(), Some(&b"check"[..]));
    assert_eq!(SKILLS.list("*"), ["git/commit", "git/commit/convention"]);

    initialize().unwrap();
    assert_eq!(SKILLS.get("rust/check"), None);
    assert_eq!(
        SKILLS.get("git/commit").as_deref(),
        Some(&b"replacement"[..])
    );
}
