# Settings

`fairway-config` provides plugins with typed settings from `config.toml`
and additional TOML files in the `conf.d` directory.

## Quick start

`Namespace<T>` binds a table name to a settings type.
`namespace!(EXAMPLE: Settings, "example")` declares a namespace:
`EXAMPLE` is the object name in Rust code, `Settings` is the settings type,
and `"example"` is the table name in the configuration.

Fairway loads settings at startup, before calling the command handler.
`get()` synchronously returns the prepared value.

```rust
// src/settings.rs

use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) enabled: bool,
}

fairway_config::namespace!(EXAMPLE: Settings, "example");

pub(crate) fn is_enabled() -> bool {
    EXAMPLE.get().enabled
}
```

`config.toml`:

```toml
[example]
enabled = true
```

## Default values

If a table is absent from all files, `Settings::default()` is used.
For a table that is present, `#[serde(default)]` on the struct fills
missing fields with values from `Default`.
`#[serde(deny_unknown_fields)]` rejects unknown fields.

```rust
// src/settings.rs

use std::net::{Ipv4Addr, SocketAddr};

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) listen: SocketAddr,
    pub(crate) idle_ttl: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, 8787)),
            idle_ttl: 300,
        }
    }
}

fairway_config::namespace!(HARNESS: Settings, "harness");

pub(crate) fn print_settings() {
    let settings = HARNESS.get();
    println!("{} ({} s)", settings.listen, settings.idle_ttl);
}
```

`config.toml`:

```toml
[harness]
listen = "127.0.0.1:9000"
```

`HARNESS.get()` returns `listen = 127.0.0.1:9000` and `idle_ttl = 300`.

## Additional files

`conf.d` resides alongside `config.toml`. TOML files are also read
from nested directories. Each file contains ordinary configuration tables;
file and directory names do not determine the settings namespace.

```text
~/.fairway/
├── config.toml
└── conf.d/
    ├── 10-server.toml
    └── 90-local/
        └── harness.toml
```

`config.toml` is applied first, followed by files from `conf.d`
in relative path order. Tables are merged by key;
later values replace earlier ones.

`config.toml`:

```toml
[harness]
listen = "127.0.0.1:8787"
idle_ttl = 300
```

`conf.d/10-server.toml`:

```toml
[harness]
idle_ttl = 60
```

`conf.d/90-local/harness.toml`:

```toml
[harness]
listen = "127.0.0.1:9000"
```

For `HARNESS` declared above, the resulting values are
`listen = 127.0.0.1:9000` and `idle_ttl = 60`.
The plugin reads them with the same `HARNESS.get()` call.

## Independent settings namespaces

Multiple `namespace!` declarations with the same type and different table names
create independent settings.

```rust
// src/settings.rs

use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) enabled: bool,
}

fairway_config::namespace!(PRIMARY: Settings, "primary");
fairway_config::namespace!(SECONDARY: Settings, "secondary");

pub(crate) fn print_settings() {
    println!("primary: {}", PRIMARY.get().enabled);
    println!("secondary: {}", SECONDARY.get().enabled);
}
```

`config.toml`:

```toml
[primary]
enabled = true

[secondary]
enabled = false
```

## API

### Registration

- `namespace!(NAME: T, "table")` declares
  `pub(crate) static NAME: Namespace<T>` and registers a settings namespace.

`T` implements `Default + serde::de::DeserializeOwned + Send + Sync + 'static`.
Neither `Clone` nor `Serialize` is required. The visibility of the type and its fields
depends on which plugin modules need access to the settings;
Fairway does not require public fields to load them.

A namespace name is a nonempty string without whitespace or control characters.
It identifies one top-level TOML key and is case-sensitive.
A name containing a dot must be quoted in TOML: `"foo.bar"` corresponds to
the `["foo.bar"]` table. The `[foo.bar]` table belongs to the `"foo"` namespace.

Names are unique among configuration namespaces from linked crates.
One crate may declare multiple namespaces, including namespaces of the same type.
CLI and configuration namespaces are registered independently;
their names need not match crate names or each other.

The shared application test checks for duplicate names: `cargo test -p fairway`.
The check covers the plugins and Cargo features included in that build;
`cargo build` does not run it. A conflict diagnostic identifies
both declaration locations. The check does not read user configuration.

### Access

A `Namespace<T>` object declared through `namespace!` provides this method:

- `get(&'static self) -> &'static T` returns the prepared settings.

`get()` is available in the command handler and tasks started by it.
The method performs no I/O or deserialization and does not copy `T`.
Repeated calls for one namespace return the same reference until the process exits.
Different namespaces of the same type hold separate values.

Plugins do not initiate loading themselves. Calling `get()` before loading
has completed successfully panics. This also applies to calls from other
settings' `Default` and `Deserialize` implementations: preparation order
does not establish dependencies between namespaces.

### Sources

Paths are relative to the Fairway directory returned by
[`fairway_fs::home`](fs.md#fairway-directory):

- `config.toml` is the main file.
- `conf.d` holds additional files, with recursive traversal of subdirectories.

The default root is `~/.fairway`. The `FAIRWAY_HOME` environment variable
changes it according to the rules of `fs::home`; configuration uses
the same fixed path. Loading does not create directories or files
and does not write default values to disk.

`config.toml` and `conf.d` may be absent independently.
`NotFound` when opening the main file or the `conf.d` root means
that the corresponding source is absent. If both are absent,
every namespace receives its default values.

Files in `conf.d` are selected by the case-sensitive `.toml` extension.
Hidden files and directories follow the same traversal rules.
Files with other extensions are skipped. An empty TOML file is valid.

Symbolic links to files and directories are followed during reading and traversal.
A directory link cycle is a loading error. Application order is determined
by the path within `conf.d`, rather than the location of the link target.
If a file is reachable through multiple selected paths,
each path participates in loading separately.

The main file and selected additional files must be regular files.
`conf.d` must be a directory. A traversal or read error, including
the disappearance of an already discovered file or subdirectory, aborts loading.

### Application order

1. Read `config.toml` if it exists.
2. Collect files from `conf.d` and its subdirectories.
3. Apply additional files in lexicographic order of their path components
   relative to `conf.d`, with case-sensitive comparison.
4. Deserialize each registered namespace's resulting table into `T`.

The order is independent of the directory entry order returned by the filesystem.
Numbers in names are compared as text: `10.toml` precedes `2.toml`.
Parent path components are compared first, followed by filenames within them;
a number in a filename does not set a global priority outside its directory.

### Merging

Each file is parsed as a separate TOML document.
A registered namespace must be a table in every file where it appears.
Multiple files may contribute to one table.

Matching keys follow these rules:

- Two tables are merged recursively by key, including nested tables.
- In every other case, the later value completely replaces the earlier one.
  This includes replacing a value with a different TOML type.
- Arrays, including arrays of tables, are replaced in full.
  Elements are neither appended nor merged by index or name.
- Keys absent from a later file keep their previous values.
  An empty table does not clear an earlier table; an empty array replaces an earlier array.

The type `T`, required fields, and deserialization rules are checked
after merging. An individual file may contain only part of the settings.
A repeated key within one file follows TOML rules and is an error;
matching keys across files follow the merging rules.

Unregistered top-level keys are ignored. This allows the same files
to be used with different sets of plugins. A misspelled namespace name
is also ignored. Syntax and UTF-8 are checked in every selected file,
including the contents of unregistered namespaces.

Strings are passed to settings without expanding environment variables or `~`.
Relative paths in fields are not automatically resolved against the file's directory.
Their meaning is determined by the plugin.

### Defaults and validation

- A namespace is absent from every file: `T::default()` is called.
- A table is present, even if empty: it is deserialized into `T`.
- With `#[serde(default)]` on the struct, missing fields are filled from `T::default()`.
  Field attributes may specify their own default values.
- A missing required field without a default rule is an error.
  Requiring `T: Default` does not itself make fields optional during deserialization.

Defaults are applied after files are merged.
Nested types define their own rules for missing and unknown fields.
Attributes follow [Serde's rules](https://serde.rs/container-attrs.html).

Ranges and relationships between fields are checked during deserialization,
for example through `#[serde(try_from = "RawSettings")]`.
When a table is absent, deserialization is not called:
`Default` must produce valid settings on its own.

### Execution

Fairway loads configuration once at startup, after CLI parsing
and before calling the command handler. Help, version output, and CLI argument errors
do not require loading settings. Argument parsers and `workers` selectors
run before loading and cannot call `get()`.

Filesystem operations run asynchronously through `fairway-fs`.
TOML parsing, merging, and value preparation run in the shared
[fairway-compute](compute.md) pool, leaving Tokio worker threads available.
Subsequent access through `get()` is synchronous because values are already in memory.

Every registered namespace is loaded. An error in any namespace prevents
the command from starting. Values become available only after the entire
configuration has been prepared successfully; partially prepared settings
are not used. The merged configuration and the values being prepared
must fit in memory.

File changes after loading take effect on the next process start.
Reading multiple files does not create a shared filesystem snapshot.
The [`fs` reading rules](fs.md#reading) apply to each individual file.

### Errors

- An incompatible type `T` or an invalid namespace name is a declaration compilation error.
- A duplicate namespace name fails the application's shared registration test.
- Failure to locate the Fairway directory, traverse `conf.d`, or read a file is a loading error.
  Missing main sources follow the [source rules](#sources).
- Invalid UTF-8 or TOML in any selected file is a loading error,
  even if later files override its values.
- A registered namespace value that is not a table is a loading error.
- An incompatible value type, a missing required field, or a validation failure
  when deserializing the resulting settings is a namespace loading error.
- An unknown field with `#[serde(deny_unknown_fields)]` is a namespace loading error.
- Calling `get()` before loading succeeds panics because initialization order was violated.

Traversal and read error diagnostics include the path and cause.
A TOML error includes the file path and position, if known.
An error in the resulting settings includes the namespace name, cause, and list
of files that contributed to its table; the field path is added if known.
The original error is retained in the `std::error::Error::source()` chain.

On a loading error, Fairway prints the diagnostic to stderr and exits
with code `1` without calling the command handler. Defaults do not replace errors.
