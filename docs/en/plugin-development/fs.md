# fairway-fs

Asynchronous file operations for Fairway plugins.

A plugin reads and writes whole files and gets their contents straight as a
value of the type it needs: text, JSON, TOML, or a custom format. Writes are
atomic: other programs see either the old contents of the file or the new
contents in full. Changes made through `edit` are not lost, even if other
Fairway commands change the same file at the same time. The crate also works
with directories, temporary files, and the Fairway directory.

Plugin dependencies (`fairway-codec` is needed for formats such as `Json`):

```toml
[dependencies]
fairway-fs.workspace = true
fairway-codec.workspace = true
```

## Quick start

```rust
use std::path::Path;

use fairway_fs as fs;

async fn uppercase_copy(source: &Path, target: &Path) -> anyhow::Result<()> {
    let text: String = fs::read(source).await?;
    fs::write(target, text.to_uppercase()).await?;
    Ok(())
}
```

`read`, `write`, and `edit` work with the whole file, so the contents must fit
in memory. The value's type determines the format: `String` is UTF-8 text, and
`Vec<u8>` is raw bytes.

`write` takes ownership of the value, so pass a string literal as
`"text".to_owned()` and a byte slice as `bytes.to_vec()`.

All functions are asynchronous and work only inside a Tokio runtime. A command
handler already runs in one, and in tests `#[tokio::test]` provides it. File
I/O runs on a separate thread for blocking operations, where `fairway-codec`
also decodes and encodes the contents, so other tasks do not wait.

## Formats

Any type from [fairway-codec](codec.md) works: `Json<T>`, `Toml<T>`,
`Markdown`, or a custom format.

```rust
use std::path::Path;

use fairway_codec::Json;
use fairway_fs as fs;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Settings {
    theme: String,
}

async fn load(path: &Path) -> Result<Settings, fs::Error> {
    let Json(settings): Json<Settings> = fs::read(path).await?;
    Ok(settings)
}

async fn save(path: &Path, settings: Settings) -> Result<(), fs::Error> {
    fs::write(path, Json(settings)).await
}
```

- If the contents cannot be decoded, for example because a file read as
  a `String` is not UTF-8, `read` returns an error of kind `InvalidData`. If
  the value cannot be encoded, `write` returns `InvalidInput`.
- `fairway-codec` converts the value on a separate thread for blocking
  operations, so the value must be `Send + 'static`: it must own its data
  rather than borrow it. Instead of `Json(&settings)`, pass the value
  itself.

## Changing a file

`edit` reads the file, passes the decoded contents to a handler, and replaces
the file with the value that the handler returns. The path stays locked the
whole time, so other Fairway operations cannot change the file between the
read and the write.

```rust
use std::path::Path;

use fairway_codec::Json;
use fairway_fs as fs;

async fn add_tag(path: &Path, tag: String) -> anyhow::Result<()> {
    fs::edit(path, |Json(mut tags): Json<Vec<String>>| async move {
        if tags.contains(&tag) {
            anyhow::bail!("tag {tag} already exists");
        }
        tags.push(tag);
        Ok(Json(tags))
    })
    .await
}
```

- The file must exist and be writable. Both are checked before the handler is
  called; a missing file gives `NotFound`.
- The handler is called at most once, and not at all if the file cannot be
  read or decoded.
- The handler runs in the same task as `edit`, so it can borrow local
  variables and await other asynchronous operations. The future it returns
  must be `Send`.
- If the handler returns an error, the file is not changed, and `edit`
  returns that error unchanged. If it returns a value, the file is rewritten,
  even if the value is the same as before.
- The handler's error can be of any type that `fs::Error` converts into:
  `fs::Error`, `anyhow::Error`, or the plugin's own error type with
  `From<fs::Error>`. Errors of `edit` itself arrive converted into that type.
- Inside the handler, do not call `edit` for the same file: the nested call
  would wait forever for the lock held by the outer one. A `write` to the same
  file returns `WouldBlock`.

Do not replace `edit` with a `read` followed by a `write`: another command can
change the file in between, and its changes will be lost.

## Directories and metadata

```rust
use std::path::Path;

use fairway_fs as fs;

async fn print_sizes(directory: &Path) -> anyhow::Result<()> {
    let mut entries = fs::ls(directory).await?;
    while let Some(entry) = entries.next().await? {
        if entry.file_type().await?.is_file() {
            let size = fs::metadata(entry.path()).await?.len();
            println!("{}: {size} bytes", entry.file_name().to_string_lossy());
        }
    }
    Ok(())
}
```

- `mkdir` creates a directory together with any missing parents. An existing
  directory is not an error, so `mkdir` can be called every time before the
  directory is used.
- `ls` iterates over the entries of one directory, including hidden ones, but
  not `.` and `..`. It does not descend into subdirectories, and the order of
  the entries is up to the operating system: sort them if the order matters.
- `file_type` describes the entry itself: for a symbolic link, it is the link.
  `metadata(entry.path())` returns the type of the file that the link points
  to.
- `exists` returns `false` only if the path does not exist, including when
  a symbolic link is broken. Other errors, such as missing permissions, are
  returned as errors.
- `metadata` and `canonicalize` follow symbolic links. `canonicalize` requires
  every component of the path to exist.

## Temporary files and directories

```rust
use fairway_fs as fs;

async fn scratch() -> anyhow::Result<()> {
    let directory = fs::temp_dir().await?;
    let input = directory.path().join("input.txt");
    fs::write(&input, "data".to_owned()).await?;
    let text: String = fs::read(&input).await?;
    println!("{text}");
    directory.close().await?;
    Ok(())
}
```

`temp_file` and `temp_dir` create an empty file or directory with a unique
name `fairway-…` in the system temporary directory. On Unix, only its owner
has access to it.

`TempFile` and `TempDir` hold a path, not an open file: work with it through
`path()` by any means. They can be passed to `fs` functions in place of
a path.

`close` deletes the file, or the directory with all its contents, and reports
if the deletion fails. Without `close`, the object is deleted when it is
dropped: the deletion starts without waiting, and its errors are ignored.

## Fairway directory

`home()` returns the absolute path to the Fairway directory, which holds
configuration and service files. Fairway determines this path at startup:
from `FAIRWAY_HOME`, or as `~/.fairway` if the variable is not set.

If `FAIRWAY_HOME` is a relative path, the directory it selects depends on
where Fairway is started. For example, `.fairway` selects
`/work/project/.fairway` when started from `/work/project`. The path determined
at startup stays the same until the process exits.

`home()` does not create the directory. `write` and `edit` create its
`locks` subdirectory for their locks.

If the Fairway directory cannot be determined, for example because
`FAIRWAY_HOME` is empty, Fairway reports it at startup and does not call the
plugin's handler.

## Errors

`fs` functions return `fs::Error`. In a command handler, pass it on with `?`
like any other error.

The message of an `fs::Error` names the step that failed and the path, if known.
The cause is described in the message itself or in the underlying error,
available through `source()`. Fairway prints the whole error chain:

```text
error: cannot open /home/user/.fairway/example/state.json: No such file or directory (os error 2)
```

If you print the error yourself, use the `{:#}` format of `anyhow::Error`,
which prints the whole chain as well. The path in the message is absolute
when it can be determined; otherwise, it is the path passed to the function.

To react to a particular error, check `kind()`:

```rust
use std::{io, path::Path};

use fairway_codec::Json;
use fairway_fs as fs;
use serde::Deserialize;

#[derive(Default, Deserialize)]
struct State {
    runs: u32,
}

async fn load_state(path: &Path) -> Result<State, fs::Error> {
    match fs::read::<Json<State>>(path).await {
        Ok(Json(state)) => Ok(state),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error),
    }
}
```

The most common kinds of errors are:

- `NotFound`: the file or its directory does not exist, or the path is empty;
- `PermissionDenied`: the process lacks permissions on the file or the
  directory;
- `WouldBlock`: for `write`, another Fairway operation is changing the file;
- `InvalidData`: the contents could not be decoded;
- `InvalidInput`: the value could not be encoded, or the path is not a regular
  file: a directory, a special file, or, when writing, a symbolic link.

Other kinds come from the operating system's error. The
[API documentation](#api-documentation) lists the kinds that each function
returns.

`operation()` reports the step that failed, such as `Operation::Decode`, and
`path()` reports the path. The format error, with its line and column, is
available through `source()` and `downcast_ref::<fairway_codec::Error>()`. If
the `fs::Error` has already become an `anyhow::Error`, get it back with
`downcast_ref::<fs::Error>()`.

## Things to know

### How a file is saved

`write` and `edit` never change a file in place. The new contents are written
to a temporary `.fairway-…` file in the same directory, which then takes the
place of the old file in a single operation. So `read` and other programs see
either the complete old contents or the complete new contents. On error, the
temporary file is removed, and the old file is left untouched.

- `write` creates the file if it does not exist, but the parent directory must
  exist: create it with `mkdir`.
- A new file gets the same permissions as any newly created file; on Unix, the
  umask applies.
- Replacing an existing file requires write permission on both the file and
  its directory. A file that the process may not write to is not replaced,
  even if the directory would allow it.
- The replacement keeps the file's permissions, owner, group, extended
  attributes, and other platform metadata, such as ACLs, and on Windows its
  alternate data streams and attributes such as hidden. If they cannot be
  carried over, for example because the file belongs to another user, the
  file is not replaced. The modification time is updated.
- The path must hold a regular file or nothing. A symbolic link at the end of
  the path does not work: the replacement would replace the link itself
  rather than the file it points to. To write that file, pass the path from
  `canonicalize`. Symbolic links in the parent directories are resolved
  automatically.
- The file is replaced by a different file, so descriptors opened earlier and
  hard links to the old file keep seeing the old contents.
- Fairway does not wait for the data to reach the disk (`fsync`). The
  replacement is atomic, but after a power failure or an operating system
  crash, the new contents may be lost. If the process terminates in the middle
  of a replacement, a temporary `.fairway-…` file may remain next to the file;
  it can be deleted.

### Concurrent access

`write` and `edit` lock the path for the whole operation. So two Fairway
operations never replace the same file at once, and changes made through
`edit` are applied one after another without being lost.

- `read` does not use the lock and does not wait for writes: thanks to the
  atomic replacement, it always sees one whole version of the file.
- `write` does not wait: if the file is already being changed, it returns
  `WouldBlock` immediately. To wait for your turn, use `edit`.
- `edit` waits for its turn without a time limit. Limit the wait with
  `tokio::time::timeout`. Within a process, waiting `edit` calls for one file
  proceed in the order in which they started waiting; between processes, the
  order is not guaranteed.
- When `write` or `edit` returns, successfully or with an error, the lock has
  already been released. The exceptions are cancellation and panics: then the
  lock is released once the remaining work has finished.
- Only Fairway operations respect the lock: other programs can change the
  file at any time. Fairway processes share locks only if they use the same
  [Fairway directory](#fairway-directory).
- Different spellings of one path share a lock. Symbolic links in the parent
  directories are resolved, and on macOS and Windows, paths that the file
  system treats as the same, such as paths that differ only in letter case,
  are locked together too. Hard links to one file are locked independently.

### Cancellation

An operation is cancelled when its future is dropped before it completes: for
example, when another branch of `tokio::select!`, such as
`shutdown.requested()`, completes first, or when `tokio::time::timeout`
expires.

- A cancelled `read` changes nothing.
- An `edit` cancelled before the handler has returned a value leaves the file
  unchanged. This covers waiting for the lock, reading and decoding the file,
  and running the handler itself.
- Once `write` has acquired the lock, and once the `edit` handler has returned
  a value, they ignore cancellation: encoding and replacement run to
  completion on the thread for blocking operations. The file ends up either
  unchanged, if something fails, or with the complete new contents. The
  result of the operation, including any error, is lost.
- Until this work has finished, the path stays locked: a `write` right after
  the cancellation may return `WouldBlock`, and an `edit` waits.
- Returning from the command handler does not interrupt this work: Fairway
  waits for it before the process exits (after a shutdown signal, for
  at most [15 seconds](cli.md)).

### Panics

A panic in `Decode::decode` or `Encode::encode` is not turned into an error:
it resumes in the task that awaits `read`, `write`, or `edit`, as if the
conversion had run in that task. The `edit` handler runs in that task anyway.
After a panic during `write` or `edit`, the file is left unchanged.

### Paths

- A path is any `impl AsRef<Path>`: `&Path`, `PathBuf`, `&str`, `String`, and
  also `TempFile` and `TempDir`.
- A relative path is resolved against the process's current directory when
  the operation starts.
- An empty path gives `NotFound`.

## API documentation

The full description of the functions, types, and errors, including
platform-specific behavior, is in the crate documentation. To open it locally,
run

```sh
cargo doc -p fairway-fs --open
```

Published versions are available on [docs.rs](https://docs.rs/fairway-fs).
