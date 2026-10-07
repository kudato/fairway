//! Initialization rejects collisions without exposing part of the catalog.

use fairway_catalog::{__private, get, initialize, list, put};

#[__private::linkme::distributed_slice(__private::BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code)]
static FIRST: __private::Builtin = __private::Builtin {
    entries: &[("a-unique", b""), ("duplicate", b"")],
    source: "first consumer",
};

#[__private::linkme::distributed_slice(__private::BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code)]
static SECOND: __private::Builtin = __private::Builtin {
    entries: &[("duplicate", b"")],
    source: "second consumer",
};

#[test]
fn duplicate_addresses_fail_without_publishing_a_partial_catalog() {
    let error = initialize().unwrap_err().to_string();
    for detail in ["duplicate", "first consumer", "second consumer"] {
        assert!(error.contains(detail), "missing {detail:?} in {error:?}");
    }
    assert_eq!(initialize().unwrap_err().to_string(), error);
    assert!(get("a-unique").is_err());
    assert!(list("*").is_err());
    assert!(put("runtime", b"content").is_err());
}
