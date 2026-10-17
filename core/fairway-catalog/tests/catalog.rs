//! The lifecycle of namespaces that start empty or with the owner's values.

use std::{panic, sync::Barrier, thread};

use fairway_catalog::initialize;

fairway_catalog::namespace!(STATE: String, "state");
fairway_catalog::namespace!(DEFAULTS: u16, "defaults", initial = defaults);

fn defaults() -> Vec<(String, u16)> {
    vec![("lorem/two".to_owned(), 2), ("lorem/one".to_owned(), 1)]
}

// One test: the namespaces and their initialization are shared by the process.
#[test]
fn catalog_lifecycle() {
    operations_panic_before_initialization();

    thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| initialize().unwrap());
        }
    });

    an_empty_namespace();
    the_owners_values();
    invalid_addresses_change_nothing();
    concurrent_replacements();

    initialize().unwrap();
    assert_eq!(DEFAULTS.list("*"), ["lorem/one"]);
    assert_eq!(DEFAULTS.get("lorem/one"), Some(11));
}

fn operations_panic_before_initialization() {
    for operation in [
        (|| drop(STATE.get("a"))) as fn(),
        || drop(STATE.put("a", String::new())),
        || drop(STATE.delete("a")),
        || drop(STATE.list("*")),
    ] {
        assert!(panic::catch_unwind(operation).is_err());
    }
}

fn an_empty_namespace() {
    assert!(STATE.list("*").is_empty());
    assert_eq!(STATE.get("missing"), None);

    for address in [
        "lorem/ipsum",
        "lorem/sit/dolorem-ipsum",
        "lorem",
        "sed/tempor",
        "Lorem/ipsum",
        "лорем/рыба ✓",
        // Parts are literal: no path resolution and no escaping.
        "../escape.rs",
        "literal/\"quote\"\\path\n\0/✓",
    ] {
        assert_eq!(STATE.put(address, address.to_owned()).unwrap(), None);
        assert_eq!(STATE.get(address).as_deref(), Some(address));
    }
    assert_eq!(
        STATE.list("lorem/*"),
        ["lorem/ipsum", "lorem/sit/dolorem-ipsum"]
    );
    assert_eq!(STATE.list("*/ipsum"), ["Lorem/ipsum", "lorem/ipsum"]);
    assert_eq!(STATE.list("lorem"), ["lorem"]);
    assert_eq!(STATE.list("*рыба*"), ["лорем/рыба ✓"]);
    assert!(STATE.list("missing/*").is_empty());
    assert_eq!(STATE.list("*").len(), 8);

    let previous = STATE.put("lorem/ipsum", "replacement".to_owned()).unwrap();
    assert_eq!(previous.as_deref(), Some("lorem/ipsum"));
    assert_eq!(STATE.get("lorem/ipsum").as_deref(), Some("replacement"));
    let removed = STATE.delete("lorem/ipsum").unwrap();
    assert_eq!(removed.as_deref(), Some("replacement"));
    assert_eq!(STATE.delete("lorem/ipsum").unwrap(), None);
    assert_eq!(STATE.get("lorem/ipsum"), None);
    assert_eq!(STATE.list("lorem/*"), ["lorem/sit/dolorem-ipsum"]);
}

fn the_owners_values() {
    assert_eq!(DEFAULTS.list("*"), ["lorem/one", "lorem/two"]);
    assert_eq!(DEFAULTS.get("lorem/one"), Some(1));
    assert_eq!(DEFAULTS.put("lorem/one", 11).unwrap(), Some(1));
    assert_eq!(DEFAULTS.delete("lorem/two").unwrap(), Some(2));
    assert_eq!(DEFAULTS.list("*"), ["lorem/one"]);
}

fn invalid_addresses_change_nothing() {
    let before = STATE.list("*");
    for (address, error) in [
        ("", "address \"state/\" has an empty part"),
        ("/lorem", "address \"state//lorem\" has an empty part"),
        ("lorem/", "address \"state/lorem/\" has an empty part"),
        (
            "lorem//ipsum",
            "address \"state/lorem//ipsum\" has an empty part",
        ),
        ("lorem/*", "address \"state/lorem/*\" contains `*`"),
        ("lorem/ips*m", "address \"state/lorem/ips*m\" contains `*`"),
        ("*", "address \"state/*\" contains `*`"),
    ] {
        let put = STATE.put(address, String::new()).unwrap_err();
        assert_eq!(put.to_string(), error);
        assert_eq!(STATE.delete(address).unwrap_err().to_string(), error);
    }
    assert_eq!(STATE.list("*"), before);
}

fn concurrent_replacements() {
    let first = "first".repeat(1024);
    let second = "second".repeat(1024);
    STATE.put("concurrent/value", first.clone()).unwrap();
    let barrier = Barrier::new(4);

    thread::scope(|scope| {
        for value in [&first, &second] {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..64 {
                    STATE.put("concurrent/value", value.clone()).unwrap();
                }
            });
        }
        for _ in 0..2 {
            let (barrier, first, second) = (&barrier, &first, &second);
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..128 {
                    let value = STATE.get("concurrent/value").unwrap();
                    assert!(value == *first || value == *second);
                    assert_eq!(STATE.list("concurrent/*"), ["concurrent/value"]);
                }
            });
        }
    });
}
