# Files

`fairway-fs` lets plugins work with the user's filesystem asynchronously.

## Quick start

`read` reads the whole file. The result type determines the representation:
`String` for UTF-8 text, `Vec<u8>` for bytes, or `Json<T>`, `Toml<T>`, and `Markdown`
from [fairway-codec](codec.md) for decoded data.

`write` accepts text, bytes, or a value of one of these codec types.
Writing is atomic: a new file appears with its complete contents,
and an existing file is replaced only after all new data has been written.

```rust
use std::{io, path::Path};

use fairway_fs as fs;

async fn uppercase_copy(source: &Path, target: &Path) -> io::Result<()> {
    let text: String = fs::read(source).await?;
    fs::write(target, text.to_uppercase()).await
}
```

## Reading and writing in chunks

`reader` opens a file and returns a `Reader` that reads bytes or lines
as requested. `writer` returns a `Writer` that accepts data in chunks.
This lets you process large files without loading them entirely into memory.

```rust
use std::{io, path::Path};

use fairway_fs as fs;

async fn copy_file(source: &Path, target: &Path) -> io::Result<()> {
    let mut reader = fs::reader(source).await?;
    let mut writer = fs::writer(target).await?;
    let mut buffer = vec![0_u8; 64 * 1024];

    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        writer.write(buffer[..count].to_vec()).await?;
    }

    writer.finish().await
}

async fn print_lines(path: &Path) -> io::Result<()> {
    let mut reader = fs::reader(path).await?;
    while let Some(line) = reader.next_line().await? {
        println!("{line}");
    }
    Ok(())
}
```

`Reader::read` fills some or all of the buffer and returns the number of bytes read.
`next_line` returns a line without its trailing newline, or `None` at the end of the file.

## Finishing a write

`Writer::write` accepts a chunk of data, which may remain in the buffer.
`flush` waits until all accepted data has been written to the temporary file.
You can continue writing after `flush`.

`finish` performs `flush`, then atomically renames the temporary file
to the target path, replacing any existing file. A separate `flush`
before `finish` is optional. Until the rename, an existing file retains
its previous contents. A missing file appears only after the rename.

```rust
use std::{io, path::Path};

use fairway_fs as fs;

async fn write_result(target: &Path) -> io::Result<()> {
    let mut output = fs::writer(target).await?;

    output.write("First part\n".to_owned()).await?;
    output.flush().await?;

    output.write("Second part\n".to_owned()).await?;
    output.finish().await
}
```

## Editing

`edit` reads the specified file in full and passes its contents to an asynchronous function.
The function returns new contents, which `edit` saves atomically.
From the initial read until saving completes, other `fs` operations cannot change this file.

Use `editor` to edit in chunks. It returns an `Editor`:
its read methods access the original file, and its write methods write to a temporary file.
`finish` atomically replaces the original file with the written result.

Each `edit` or `editor` call acquires its own lock for the specified path.
If another `fs` operation is changing that file, the call waits for it to finish
before reading the file.

```rust
use std::{io, path::Path};

use fairway_fs as fs;

async fn uppercase(path: &Path) -> io::Result<()> {
    fs::edit(path, |text: String| async move { Ok(text.to_uppercase()) }).await
}

async fn first_hundred_lines(path: &Path) -> io::Result<()> {
    let mut editor = fs::editor(path).await?;

    for _ in 0..100 {
        let Some(line) = editor.next_line().await? else {
            break;
        };
        editor.write(line).await?;
        editor.write("\n".to_owned()).await?;
    }

    editor.finish().await
}
```

## Data formats

The result type of `read` selects the reading format, and the value passed to `write` selects the writing format.
Reading requires `Decode`, and writing requires `Encode`.
Supported types include `Json<T>`, `Toml<T>`, and `Markdown` from [fairway-codec](codec.md)
and custom implementations of these traits.

For JSONL, read lines through `Reader::next_line` and decode each through `codec::decode`.
Passing `Json(value)` to `Writer::write` writes one JSON record with a trailing `\n`.
Conversions run through `codec::decode` and `codec::encode`.

```rust
use std::{io, path::Path};

use fairway_codec::{self as codec, Json};
use fairway_fs as fs;

async fn copy_json(source: &Path, target: &Path) -> io::Result<()> {
    let words: Json<Vec<String>> = fs::read(source).await?;
    fs::write(target, words).await
}

async fn copy_jsonl(source: &Path, target: &Path) -> anyhow::Result<()> {
    let mut input = fs::reader(source).await?;
    let mut output = fs::writer(target).await?;
    while let Some(line) = input.next_line().await? {
        let word: Json<String> = codec::decode(line.into_bytes()).await?;
        output.write(word).await?;
    }

    output.finish().await?;
    Ok(())
}
```

## Directories and metadata

`ls` traverses the files, subdirectories, and symbolic links in a directory.
Each entry provides its name, path, and type. Use `metadata` to obtain
size and other properties of a file or directory.

`exists` checks whether a file or directory exists at a path.
`canonicalize` returns an absolute path with symbolic links resolved.
`home` returns the path to Fairway's application directory.

```rust
use std::io;

use fairway_fs as fs;

async fn list_fairway_files() -> io::Result<()> {
    let directory = fs::home().await?;
    if !fs::exists(&directory).await? {
        println!("Directory does not exist: {}", directory.display());
        return Ok(());
    }
    let mut entries: fs::DirEntries = fs::ls(&directory).await?;

    while let Some(entry) = entries.next().await? {
        if entry.file_type().await?.is_file() {
            let path = entry.path();
            let metadata = fs::metadata(&path).await?;
            let resolved = fs::canonicalize(&path).await?;
            println!(
                "{}: {} bytes ({})",
                entry.file_name().to_string_lossy(),
                metadata.len(),
                resolved.display()
            );
        }
    }
    Ok(())
}
```

## Temporary files and directories

`temp_file` creates a temporary file and returns a `TempFile`.
`temp_dir` creates a directory and returns a `TempDir`.
`path()` returns a path that can be passed to `fs` functions.
`close().await` removes the file or directory and waits for removal to complete.

```rust
use std::io;

use fairway_fs as fs;

async fn temporary_data() -> io::Result<()> {
    let file: fs::TempFile = fs::temp_file().await?;
    fs::write(file.path(), "Temporary data".to_owned()).await?;
    let bytes: Vec<u8> = fs::read(file.path()).await?;
    println!("Read {} bytes", bytes.len());

    let directory: fs::TempDir = fs::temp_dir().await?;
    let data = directory.path().join("data");
    fs::mkdir(&data).await?;
    fs::write(data.join("input.txt"), "Data to process".to_owned()).await?;

    file.close().await?;
    directory.close().await
}
```

## API

Path arguments accept `impl AsRef<Path>`.
Relative paths are resolved against the working directory at the start of an operation.
Later changes to the working directory do not affect an operation already in progress.
`read`, `reader`, `exists`, `metadata`, and `canonicalize` follow symbolic links.
`Reader`, `Writer`, `Editor`, `DirEntries`, `TempFile`, and `TempDir` implement `Send`.

### Fairway directory

- `home().await -> io::Result<PathBuf>` returns the path to Fairway's application directory.

The `FAIRWAY_HOME` environment variable sets this path. The default is `~/.fairway`.
`fairway-fs` resolves the path at Fairway startup, before configuration loads,
and retains it for the lifetime of the process.
If the path cannot be determined, the function returns an error.

### Reading

- `read::<T>(path).await -> io::Result<T>` reads a whole regular file and decodes it as `T`.
- `reader(path).await -> io::Result<Reader>` opens a file for sequential reading.

`read` checks the opened object's type before reading. A detected directory,
named pipe, device, or other special file produces `InvalidInput`.
Errors from opening the path or reading its metadata retain their original `io::ErrorKind`.
A symbolic link to a regular file is accepted.
On Unix, opening a named pipe does not wait for a writer.

`read` supports these result types:

- `String` for the whole file as UTF-8 text.
- `Vec<u8>` for all bytes in the file.
- `Json<T>`, `Toml<T>`, and `Markdown` from [fairway-codec](codec.md).
- Custom types implementing `Decode`.

In all cases, `read` loads the whole file into memory.

The `Reader` returned by `reader` provides these methods:

- `read(&mut [u8]).await -> io::Result<usize>` fills part or all of the supplied buffer
  and returns the number of bytes read. `0` means EOF or an empty buffer.
- `next_line().await -> io::Result<Option<String>>` returns the next line
  without its trailing `\n` or `\r\n`. Returns `None` at EOF.

`Reader` starts at the beginning of the file. Both methods share a position:
each call continues where the previous one stopped.
`next_line` also returns a final line without a newline.
An empty line produces `Some(String::new())`.
Cancelling `next_line` retains any partially read line for the next call.

`read` and `reader` do not wait for writes to finish. If a file is replaced after opening,
reading continues from the previous file. The next open reads the version
available at the path at that time.

### Writing

- `write(path, contents).await -> io::Result<()>` atomically writes contents
  to a new file or replaces an existing file.
- `writer(path).await -> io::Result<Writer>` prepares a temporary file for writing in chunks.

The `Writer` returned by `writer` provides these methods:

- `write(contents).await -> io::Result<()>` accepts the entire supplied chunk.
  Some data may remain buffered until the next write, `flush`, or `finish`.
- `flush().await -> io::Result<()>` writes buffered data to the temporary file
  and waits for the OS write to complete.
- `finish(self).await -> io::Result<()>` performs `flush`, then atomically
  renames the temporary file to the target path, replacing any existing file.

The internal buffer is bounded and does not grow with the file.
When space runs out, `write` flushes the buffer itself, waiting until it can continue.
Calling `flush` after each chunk is unnecessary.

The `contents: V` argument requires `V: Encode + Send + 'static` and is passed by value.
For text and bytes, pass an owned `String`, `Vec<u8>`, or byte array.
Convert a string literal explicitly, for example `"text".to_owned()`.
Copy a slice of a local buffer into an owned `Vec<u8>` before passing it.
`fs::write` encodes the entire contents before writing, and `Writer::write` encodes the supplied chunk.

The temporary file is created in the same directory as the target.
Until the rename, an existing file retains its previous contents,
and a missing file has not yet appeared. After a successful `fs::write` or `finish`,
the target path refers to a file containing all written data.
An error before the rename leaves the original file unchanged.
If atomic renaming is not possible, the operation returns an error.

The parent directory must exist. The target path must refer to a regular file
or no file. If its final component is a symbolic link, including a dangling link,
a directory, or a special file, the operation returns an error.
Symbolic links in parent directories are allowed.

Replacement creates a new file object at the target path.
Other hard links and previously opened descriptors still refer to the old object.
Permissions, owner, group, ACLs, and extended attributes are copied from the original file.
If copying them fails, the original file is not replaced.
No backup copies are created.

After a successful `fs::write`, `fs::edit`, `flush`, or `finish`, data has been handed to the OS.
The OS is responsible for subsequently saving it to the storage device.

After an error from `Writer::write` or `flush`, or cancellation of a started
`Writer::write`, `finish` returns an error and does not replace the target file.

Cancelling the wait for `Writer::flush` or `Editor::flush` does not discard data.
You can then continue writing, repeat `flush`, or call `finish`.
Completed writes are accounted for, and remaining data is written in its original order.

`Writer` and `Editor` do not implement `Clone`. Dropping them without `finish`
leaves the original file unchanged and starts removing the temporary file in the background.
The temporary file may remain after a crash.

Cancelling the wait for `fs::write`, `fs::edit`, or `finish` does not guarantee
that saving already in progress stops. The lock is held until background file operations finish.
Replacement may occur after cancellation, but only with a completely written file.

### Editing

- `edit(path, handler).await -> Result<(), E>` reads the whole file, calls
  an asynchronous handler, and saves its result while holding one lock.
- `editor(path).await -> io::Result<Editor>` opens the original file for reading
  and a temporary file for the new contents while holding one lock.

After successful reading and decoding, `edit` calls `handler` once.
The handler receives the file contents and asynchronously returns `Result<T, E>`.
`T` is the type of both the original and new contents, such as `String`.
`E` is the handler's error type.

Handler requirements:

- The contents type `T` implements `Decode` for reading and `Encode` for saving the result.
- `T: Send + 'static` allows the contents to be transferred to the compute pool.
  The value must not contain references borrowing the caller's local data.
- The future returned by the handler implements `Send` so it can be transferred between threads.
- The error type `E` implements `From<io::Error>` so `edit` can return filesystem
  errors through the same type. Both `io::Error` and `anyhow::Error` satisfy this requirement.

`Decode` and `Encode` errors follow the shared [data conversion requirements](#data-conversion).
Decoding and encoding errors are converted to `io::Error`.
A read, conversion, or handler error leaves the file unchanged.

`edit` and `editor` require an existing regular file. A missing file returns `NotFound`.
Checking and reading happen after the lock is acquired.
Atomic writing, link handling, and metadata preservation follow the same rules as `write`.

The `Editor` returned by `editor` provides these methods:

- `read(&mut [u8]).await -> io::Result<usize>` reads bytes from the original file.
- `next_line().await -> io::Result<Option<String>>` reads a line from the original file.
- `write(contents).await -> io::Result<()>` writes a chunk of the new contents.
- `flush().await -> io::Result<()>` flushes buffered data to the temporary file.
- `finish(self).await -> io::Result<()>` atomically replaces the original file with the result.

`Editor` read methods behave like `Reader` methods, and its write methods behave like
`Writer` methods, with the same `V: Encode + Send + 'static` requirement.
Read and write positions are independent. `finish` saves only explicitly written contents:
reading the entire original is optional, its remainder is not copied, and writing nothing produces an empty file.
A read, write, or `flush` error, or cancellation of a started `Editor::write`,
prevents saving: a subsequent `finish` returns an error.

### Coordination

Changes to one path are protected by an internal interprocess `FileLock`.
Before choosing a lock, the parent directory is converted to an absolute,
canonical path with symbolic links resolved. The same file name in that
directory uses one lock, including access through a symbolic link to the directory.

`write` and `edit` hold the lock throughout the operation.
`writer` and `editor` acquire it before opening files and transfer it to the returned
object. It remains held until writing finishes or the result is discarded.

If the lock is busy, `write` and `writer` return `WouldBlock`.
`edit` and `editor` wait asynchronously for it, with no built-in timeout.
Waiting can be cancelled. The file is not changed before the lock is acquired.
Within one process, waiting edits retain their queue order.
Order between processes is not guaranteed. Different target paths are processed independently.
Separate `read` and `write` calls do not form a protected edit.

`fs` creates a `locks` directory inside the Fairway directory and manages its lock files.
Processes sharing the same Fairway root use the same locks.
A lock file is removed after its operation ends. Unlocked files left after a crash
are cleaned up at the next startup. Busy files are retained.

Opening, acquiring, and deleting lock files are coordinated through a permanent
`locks/.coordination.lock`. It is held only during these housekeeping operations.
After an unsuccessful attempt, a waiter closes the lock file before releasing
`.coordination.lock` and reopens it on the next attempt.
`.coordination.lock` itself is never removed.

### Directories

- `mkdir(path).await -> io::Result<()>` creates a directory and its parents.
  An existing directory is accepted. An existing file is an error.
- `ls(path).await -> io::Result<DirEntries>` opens a directory traversal.

If `mkdir` fails, directories already created remain.
Traversal includes hidden entries, without recursion, sorting, or a snapshot.
Errors can occur while opening the directory or reading an entry.

The `DirEntries` returned by `ls` provides this method:

- `next().await -> io::Result<Option<DirEntry>>` returns the next entry
  or `None` at the end of traversal.

Each entry is a `DirEntry` with these methods:

- `path() -> PathBuf` returns the entry's path.
- `file_name() -> OsString` returns its name.
- `file_type().await -> io::Result<std::fs::FileType>` returns the type of the entry itself,
  without following a symbolic link.

### Existence and metadata

- `exists(path).await -> io::Result<bool>` checks whether a path exists, without creating anything.
- `metadata(path).await -> io::Result<std::fs::Metadata>` returns file or directory metadata.
- `canonicalize(path).await -> io::Result<PathBuf>` returns an absolute path
  with symbolic links resolved.

`exists` returns `false` for a missing path or dangling symbolic link.
An inability to check, including insufficient permissions, returns an error.
Metadata reflects the state at the time of the request.

`canonicalize` requires an existing path. A missing path or dangling symbolic link
returns `NotFound`; other filesystem errors are preserved.

### Temporary resources

- `temp_file().await -> io::Result<TempFile>` creates an empty temporary file.
- `temp_dir().await -> io::Result<TempDir>` creates an empty temporary directory.

Resources receive unique names in the system temporary directory.
`TempFile` and `TempDir` implement `AsRef<Path>` and provide these methods:

- `path() -> &Path` returns the resource's path. Copying it does not extend the resource's lifetime.
- `close(self).await -> io::Result<()>` removes the resource and waits for completion.
  A directory is removed with all its contents.

Release open readers, writers, and directory traversals before calling `close`.
Dropping a resource without `close` starts cleanup without waiting for confirmation.
A resource may remain after a crash.

### Data conversion

`Decode` and `Encode` are defined in [fairway-codec](codec.md#synchronous-traits).
For custom conversions, add `fairway-codec` to the plugin's dependencies
and import the traits from `fairway_codec`.

`read` requires `T: Decode + Send + 'static`, reads all file bytes,
and passes them to `T::decode(bytes)`.
`write`, `Writer::write`, and `Editor::write` accept `V: Encode + Send + 'static`.
`edit` uses `Decode` when reading and `Encode` when saving the handler's result.

Trait methods are synchronous. `fs` calls them through `codec::decode` and `codec::encode`
and follows the same [execution rules](codec.md#execution).

When used through `fs`, the error types of `Decode` and `Encode` must implement
`Send` and `Into<Box<dyn std::error::Error + Send + Sync + 'static>>`.
These are requirements of filesystem operations, not of the codec traits.

`Reader` reads bytes or lines without parsing a format.
Supported formats and custom conversions are described in [fairway-codec](codec.md).

### Errors

Filesystem operations return `io::Result`. `edit` returns the handler's error type,
which must implement `From<io::Error>`.

- Filesystem errors retain their original `io::ErrorKind`.
- Decoding errors, including invalid UTF-8, are returned as `InvalidData`.
- Encoding errors are returned as `InvalidInput`.

The underlying conversion error is available through `io::Error::get_ref`.
Errors for built-in formats are described in the [codec documentation](codec.md#errors).
