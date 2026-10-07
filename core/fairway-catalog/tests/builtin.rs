//! Lazy builtin decoding and replacement of an unreadable builtin entry.

use fairway_catalog::{__private, get, initialize, list, put};
use std::error::Error as _;

#[__private::linkme::distributed_slice(__private::BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code)]
static BUILTIN: __private::Builtin = __private::Builtin {
    entries: &[("corrupt", b"invalid compressed bytes")],
    source: "corrupt-entry fixture",
};

#[test]
fn corrupt_contents_do_not_prevent_initialization_listing_or_replacement() {
    initialize().unwrap();
    assert_eq!(list("*").unwrap(), ["corrupt"]);
    assert_eq!(get("absent").unwrap(), None);

    let error = get("corrupt").unwrap_err();
    assert!(error.to_string().contains("corrupt"));
    assert!(error.source().is_some());

    put("other", b"independent entry").unwrap();
    assert_eq!(get("other").unwrap().unwrap(), b"independent entry");
    put("corrupt", b"replacement").unwrap();
    assert_eq!(get("corrupt").unwrap().unwrap(), b"replacement");
    initialize().unwrap();
    assert_eq!(get("corrupt").unwrap().unwrap(), b"replacement");
}
