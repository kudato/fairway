//! The lifecycle of namespaces that start empty or with the owner's values.

use std::{panic, sync::Barrier, thread};

use fairway_catalog::initialize;

fairway_catalog::namespace!(STATE: String, "state");
fairway_catalog::namespace!(CONFIG: u16, "config", initial = defaults);

fn defaults() -> Vec<(String, u16)> {
    vec![
        ("http/workers".to_owned(), 4),
        ("http/port".to_owned(), 8080),
    ]
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
    assert_eq!(CONFIG.list("*"), ["http/port"]);
    assert_eq!(CONFIG.get("http/port"), Some(80));
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
        "git/commit",
        "git/hooks/pre-commit",
        "git",
        "rust/check",
        "Git/commit",
        "путь/задача ✓",
    ] {
        assert_eq!(STATE.put(address, address.to_owned()).unwrap(), None);
    }
    assert_eq!(STATE.list("git/*"), ["git/commit", "git/hooks/pre-commit"]);
    assert_eq!(STATE.list("*/commit"), ["Git/commit", "git/commit"]);
    assert_eq!(STATE.list("git"), ["git"]);
    assert_eq!(STATE.list("*задача*"), ["путь/задача ✓"]);
    assert!(STATE.list("missing/*").is_empty());
    assert_eq!(STATE.list("*").len(), 6);

    let previous = STATE.put("git/commit", "replacement".to_owned()).unwrap();
    assert_eq!(previous.as_deref(), Some("git/commit"));
    assert_eq!(STATE.get("git/commit").as_deref(), Some("replacement"));
    let removed = STATE.delete("git/commit").unwrap();
    assert_eq!(removed.as_deref(), Some("replacement"));
    assert_eq!(STATE.delete("git/commit").unwrap(), None);
    assert_eq!(STATE.get("git/commit"), None);
    assert_eq!(STATE.list("git/*"), ["git/hooks/pre-commit"]);
}

fn the_owners_values() {
    assert_eq!(CONFIG.list("*"), ["http/port", "http/workers"]);
    assert_eq!(CONFIG.get("http/port"), Some(8080));
    assert_eq!(CONFIG.put("http/port", 80).unwrap(), Some(8080));
    assert_eq!(CONFIG.delete("http/workers").unwrap(), Some(4));
    assert_eq!(CONFIG.list("*"), ["http/port"]);
}

fn invalid_addresses_change_nothing() {
    let before = STATE.list("*");
    for (address, error) in [
        ("", "address \"state/\" has an empty part"),
        ("/git", "address \"state//git\" has an empty part"),
        ("git/", "address \"state/git/\" has an empty part"),
        (
            "git//commit",
            "address \"state/git//commit\" has an empty part",
        ),
        ("git/*", "address \"state/git/*\" contains `*`"),
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
