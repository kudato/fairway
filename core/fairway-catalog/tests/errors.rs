//! Initialization reports every problem at once and fills no namespace.

use std::borrow::Cow;

use fairway_catalog::{__private, initialize};

fairway_catalog::namespace!(VALID: String, "valid");
fairway_catalog::namespace!(TWICE: String, "twice");
fairway_catalog::namespace!(EMPTY: String, "alpha-empty");
fairway_catalog::namespace!(ITEMS: Cow<'static, [u8]>, "alpha-items", builtin);
fairway_catalog::namespace!(DEFAULTS: String, "alpha-defaults", initial = defaults);

mod other {
    fairway_catalog::namespace!(TWICE: String, "twice");
}

fn defaults() -> Vec<(String, String)> {
    vec![
        ("lorem/one".to_owned(), "ipsum".to_owned()),
        ("lorem//two".to_owned(), "dolor".to_owned()),
        ("lorem/one".to_owned(), "sit".to_owned()),
    ]
}

#[__private::linkme::distributed_slice(__private::FAIRWAY_CATALOG_BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code, reason = "linkme places the table in a linker section")]
static FIRST: __private::Builtin = __private::Builtin::new(
    "alpha-items",
    &[("lorem/*", b"mask"), ("lorem/ipsum", b"first")],
    "first crate",
);

#[__private::linkme::distributed_slice(__private::FAIRWAY_CATALOG_BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code, reason = "linkme places the table in a linker section")]
static SECOND: __private::Builtin =
    __private::Builtin::new("alpha-items", &[("lorem/ipsum", b"second")], "second crate");

#[__private::linkme::distributed_slice(__private::FAIRWAY_CATALOG_BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code, reason = "linkme places the table in a linker section")]
static MISSPELLED: __private::Builtin = __private::Builtin::new("alpha-itms", &[], "third crate");

#[__private::linkme::distributed_slice(__private::FAIRWAY_CATALOG_BUILTINS)]
#[linkme(crate = __private::linkme)]
#[allow(unsafe_code, reason = "linkme places the table in a linker section")]
static NOT_BUILTIN: __private::Builtin =
    __private::Builtin::new("alpha-empty", &[], "fourth crate");

#[test]
fn every_problem_is_reported_at_once() {
    let report = initialize().unwrap_err().to_string();
    let lines: Vec<_> = report.lines().collect();
    // Declaration lines move with this file, so only their start is compared.
    let declared = concat!("errors (", file!(), ":");
    let declared_in_other = concat!("errors::other (", file!(), ":");

    assert_eq!(lines.len(), 8, "{report}");
    assert_eq!(lines[0], "cannot initialize catalog:");
    assert!(
        lines[1].starts_with(&format!("  duplicate namespace \"twice\": {declared}"))
            && lines[1].contains(&format!(") and {declared_in_other}")),
        "{report}"
    );
    assert_eq!(
        lines[2],
        "  builtin data for namespace \"alpha-empty\" declared without builtin: fourth crate"
    );
    assert_eq!(
        lines[3],
        "  builtin data for unknown namespace \"alpha-itms\": third crate"
    );
    assert!(
        lines[4].starts_with(&format!(
            "  initial address \"alpha-defaults/lorem//two\" has an empty part: {declared}"
        )),
        "{report}"
    );
    assert!(
        lines[5].starts_with(&format!(
            "  duplicate initial address \"alpha-defaults/lorem/one\": {declared}"
        )) && !lines[5].contains(" and "),
        "{report}"
    );
    assert_eq!(
        lines[6],
        "  builtin address \"alpha-items/lorem/*\" contains `*`: first crate"
    );
    assert_eq!(
        lines[7],
        "  duplicate builtin address \"alpha-items/lorem/ipsum\": first crate and second crate"
    );
}

#[test]
#[should_panic(expected = "catalog is not initialized successfully")]
fn valid_namespaces_are_not_filled_either() {
    initialize().unwrap_err();
    VALID.list("*");
}
