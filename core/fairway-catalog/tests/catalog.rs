//! Runtime access, literal addresses, masks, and concurrent replacement.

use fairway_catalog::{get, initialize, list, put};
use std::{sync::Barrier, thread};

#[test]
fn runtime_catalog_lifecycle() {
    assert!(get("before-initialization").is_err());
    assert!(list("*").is_err());
    assert!(put("before-initialization", b"must not be stored").is_err());

    thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| initialize().unwrap());
        }
    });
    assert!(list("*").unwrap().is_empty());
    assert_eq!(get("missing").unwrap(), None);

    for address in [
        "skill/git/commit",
        "skill/git/hooks/pre-commit",
        "resource/git/convention",
        "skill/git",
        "skill/rust/check",
        "Skill/git/commit",
    ] {
        put(address, address.as_bytes()).unwrap();
    }

    assert_eq!(
        list("skill/git/*").unwrap(),
        ["skill/git/commit", "skill/git/hooks/pre-commit"]
    );
    assert_eq!(
        list("skill/*").unwrap(),
        [
            "skill/git",
            "skill/git/commit",
            "skill/git/hooks/pre-commit",
            "skill/rust/check"
        ]
    );
    assert_eq!(list("*/git").unwrap(), ["skill/git"]);
    assert_eq!(
        list("*/git/*").unwrap(),
        [
            "Skill/git/commit",
            "resource/git/convention",
            "skill/git/commit",
            "skill/git/hooks/pre-commit"
        ]
    );
    assert!(list("missing/*").unwrap().is_empty());
    assert_eq!(list("skill/git").unwrap(), ["skill/git"]);

    let mut previously_read = get("skill/git/commit").unwrap().unwrap();
    let previous_list = list("skill/git/*").unwrap();
    put("skill/git/commit", b"replacement").unwrap();
    put("skill/git/rebase", b"new entry").unwrap();
    assert_eq!(previously_read, b"skill/git/commit");
    assert_eq!(previous_list.len(), 2);
    previously_read.fill(0);
    assert_eq!(get("skill/git/commit").unwrap().unwrap(), b"replacement");
    initialize().unwrap();
    assert_eq!(get("skill/git/commit").unwrap().unwrap(), b"replacement");

    let binary: Vec<u8> = (0..=255).cycle().take(100_000).collect();
    put("binary", &binary).unwrap();
    assert_eq!(get("binary").unwrap(), Some(binary));
    put("empty", b"").unwrap();
    assert_eq!(get("empty").unwrap(), Some(Vec::new()));

    for address in ["", "путь/задача", "a//b", "a/./b", "a/b", "literal/?[x]\\"] {
        put(address, address.as_bytes()).unwrap();
        assert_eq!(get(address).unwrap(), Some(address.as_bytes().to_vec()));
        assert_eq!(list(address).unwrap(), [address]);
    }

    concurrent_replacements();
}

fn concurrent_replacements() {
    let first = vec![0x11; 32 * 1024];
    let second = vec![0xee; 48 * 1024];
    put("concurrent/value", &first).unwrap();
    let barrier = Barrier::new(4);

    thread::scope(|scope| {
        for writer in [&first, &second] {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..64 {
                    put("concurrent/value", writer).unwrap();
                }
            });
        }
        for _ in 0..2 {
            let (barrier, first, second) = (&barrier, &first, &second);
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..128 {
                    let bytes = get("concurrent/value").unwrap().unwrap();
                    assert!(bytes == *first || bytes == *second);
                    assert_eq!(list("concurrent/*").unwrap(), ["concurrent/value"]);
                }
            });
        }
    });
}
