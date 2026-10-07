# fairway-catalog

The catalog stores data at string addresses in memory for the lifetime of
Fairway. Each entry contains arbitrary bytes. The consumer defines
their format and the meaning of the address; the catalog handles
compression, storage, and decompression.

Initial contents can be prepared at build time and embedded in the program —
these are builtin entries. Entries can be added and replaced at runtime.
All operations are synchronous.

Plugin dependencies:

```toml
[dependencies]
fairway-catalog.workspace = true
```

## Quick start

```rust
use fairway_catalog::{Error, get, initialize, put};

fn main() -> Result<(), Error> {
    initialize()?;
    put("skill/git/commit", b"Commit instructions")?;

    assert_eq!(
        get("skill/git/commit")?,
        Some(b"Commit instructions".to_vec()),
    );
    Ok(())
}
```

Fairway calls `initialize()` before first accessing the catalog.
The function collects builtin entries from linked crates into a single
mutable catalog. If there are no builtin entries, the catalog starts empty.

Initialization retains references to the embedded compressed data.
Calling `initialize()` again after successful initialization preserves
the current state, including changes made through `put`.

## Builtin entries

In a crate that supplies builtin entries, add a build dependency:

```toml
[build-dependencies]
fairway-catalog = { workspace = true, features = ["build"] }
```

The `build` feature enables the `build` function and the `BuildError` type.
In `build.rs`, the consumer reads source files and assigns their full addresses:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=documents/commit.md");
    println!("cargo:rerun-if-changed=documents/convention.md");

    fairway_catalog::build([
        (
            "skill/git/commit".to_owned(),
            std::fs::read("documents/commit.md")?,
        ),
        (
            "resource/git/convention".to_owned(),
            std::fs::read("documents/convention.md")?,
        ),
    ])?;
    Ok(())
}
```

In the same crate's `src/lib.rs`, call once:

```rust
fairway_catalog::embed!();
```

## Addresses and contents

Addresses are compared literally, with case sensitivity and no
normalization. The consumer defines the meaning of the parts in
`skill/git/commit` and `resource/git/convention`. To the catalog,
these are string keys.

The consumer converts text, structs, and other values to and from bytes,
for example using [fairway-codec](codec.md).

## Reading and updating

`get(address)` returns `Result<Option<Vec<u8>>, Error>`: the original
decompressed bytes, or `None` if the address is absent. The returned
buffer belongs to the caller. Subsequent changes to the entry do not
change data already retrieved.

`put(address, content)` accepts an address as `&str` and contents as
`&[u8]`. It compresses the data and adds an entry or replaces the
entire contents of an existing entry, including a builtin entry.

The catalog supports access from multiple threads. Replacement is atomic:
a concurrent `get` receives the entire old or new contents. A failed
`put` preserves the previous entry.

Changes are kept only in memory. On the next launch, the catalog starts
from builtin entries again; consumers supply any additional contents.

## Selecting addresses

`list(pattern)` returns `Result<Vec<String>, Error>` — a snapshot of
matching addresses in lexicographic order.
No matches produce an empty list.

The pattern matches the entire address and is case-sensitive.
`*` matches any sequence of characters, including an empty sequence
and sequences containing `/`. All other characters are literal.

| Call | Selected addresses |
|---|---|
| `list("*")` | All addresses |
| `list("skill/*")` | Starting with `skill/`, such as `skill/git/commit` |
| `list("skill/git/*")` | Starting with `skill/git/`, including `skill/git/hooks/pre-commit` |
| `list("*/git")` | Ending in `/git`, such as `skill/git` |
| `list("*/git/*")` | Containing `/git/`, such as `skill/git/commit` and `resource/git/convention` |

`list("*/git")` does not select `skill/git/commit`: that address
does not end in `/git`.

## Errors

`build` returns `fairway_catalog::BuildError` if `OUT_DIR` is absent,
compression fails, or artifacts cannot be written.

`initialize`, `get`, `put`, and `list` return
`fairway_catalog::Error`. `get`, `put`, and `list` require
successful initialization.

Duplicate builtin addresses cause `initialize` to fail.
The message identifies the address and both registration locations.
The catalog remains unavailable after such a conflict. Subsequent calls to
`initialize` return the same error.

Compression and decompression errors identify the entry's address;
artifact write errors identify the affected path. If there is an
underlying error, it is available through `std::error::Error::source()`.
Both error types implement `std::error::Error + Send + Sync` and own
their diagnostic information.
