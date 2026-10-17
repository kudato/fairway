//! Builtin tables of several crates fill their namespaces without copying.

use std::{borrow::Cow, ptr};

use fairway_catalog::{__private, initialize};

fairway_catalog::namespace!(ALPHA: Cow<'static, [u8]>, "alpha-items", builtin);
fairway_catalog::namespace!(BETA: Cow<'static, [u8]>, "beta-items", builtin);

static LOREM: [u8; 11] = *b"lorem ipsum";

// Two crates embed into one namespace; the second one also into another.
#[__private::linkme::distributed_slice(__private::FAIRWAY_CATALOG_BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code, reason = "linkme places the table in a linker section")]
static FIRST: __private::Builtin = __private::Builtin::new(
    "alpha-items",
    &[("lorem/ipsum", &LOREM), ("lorem/ipsum/elit", b"elit")],
    "first crate",
);

#[__private::linkme::distributed_slice(__private::FAIRWAY_CATALOG_BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code, reason = "linkme places the table in a linker section")]
static SECOND_ALPHA: __private::Builtin =
    __private::Builtin::new("alpha-items", &[("sed/tempor", b"tempor")], "second crate");

#[__private::linkme::distributed_slice(__private::FAIRWAY_CATALOG_BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code, reason = "linkme places the table in a linker section")]
static SECOND_BETA: __private::Builtin =
    __private::Builtin::new("beta-items", &[("lorem", b"ipsum dolor")], "second crate");

#[test]
fn builtin_tables_are_the_initial_values() {
    initialize().unwrap();
    assert_eq!(
        ALPHA.list("*"),
        ["lorem/ipsum", "lorem/ipsum/elit", "sed/tempor"]
    );
    assert_eq!(BETA.list("*"), ["lorem"]);
    assert_eq!(BETA.get("lorem").as_deref(), Some(&b"ipsum dolor"[..]));

    let Some(Cow::Borrowed(ipsum)) = ALPHA.get("lorem/ipsum") else {
        panic!("a builtin value borrows the embedded bytes");
    };
    assert!(ptr::eq(ipsum, &LOREM[..]));

    let previous = ALPHA.put("lorem/ipsum", Cow::Owned(b"replacement".to_vec()));
    assert!(matches!(
        previous,
        Ok(Some(Cow::Borrowed(bytes))) if ptr::eq(bytes, &LOREM[..])
    ));
    assert!(matches!(
        ALPHA.get("lorem/ipsum"),
        Some(Cow::Owned(bytes)) if bytes == b"replacement"
    ));
    let removed = ALPHA.delete("sed/tempor").unwrap();
    assert_eq!(removed.as_deref(), Some(&b"tempor"[..]));
    assert_eq!(ALPHA.list("*"), ["lorem/ipsum", "lorem/ipsum/elit"]);

    initialize().unwrap();
    assert_eq!(ALPHA.get("sed/tempor"), None);
    assert_eq!(
        ALPHA.get("lorem/ipsum").as_deref(),
        Some(&b"replacement"[..])
    );
}
