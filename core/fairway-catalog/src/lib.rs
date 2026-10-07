//! A mutable catalog of opaque bytes, addressed by strings.
//!
//! Consumers supply builtin entries through `build` in their build scripts
//! (with the `build` feature) and include them with [`embed!`] in their crate.
//! The application calls [`initialize`] before accessing the catalog.
//!
//! Initialization borrows the embedded compressed blocks. [`put`] adds or
//! replaces an entry in the same index; [`get`] returns its original bytes.
//! Runtime changes live in memory for the remainder of the process.
//!
//! ```
//! fairway_catalog::initialize()?;
//! fairway_catalog::put("example/message", b"hello")?;
//! assert_eq!(fairway_catalog::get("example/message")?, Some(b"hello".to_vec()));
//! assert_eq!(fairway_catalog::list("example/*")?, ["example/message"]);
//! # Ok::<(), fairway_catalog::Error>(())
//! ```

#[cfg(feature = "build")]
mod build;
mod compression;
mod error;
mod pattern;
mod registry;

use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{Arc, OnceLock, RwLock},
};

#[cfg(feature = "build")]
pub use build::build;
#[cfg(feature = "build")]
pub use error::BuildError;
pub use error::Error;

type Entries = BTreeMap<Cow<'static, str>, Content>;

static CATALOG: OnceLock<Result<RwLock<Entries>, Error>> = OnceLock::new();

#[derive(Clone)]
enum Content {
    Builtin(&'static [u8]),
    Owned(Arc<Vec<u8>>),
}

impl Content {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Builtin(bytes) => bytes,
            Self::Owned(bytes) => bytes,
        }
    }
}

/// Initializes the catalog from all linked builtin tables.
///
/// The index borrows builtin addresses and compressed bytes. Initialization
/// neither copies nor decompresses their contents. Repeated and concurrent
/// calls preserve the initialized catalog, including subsequent runtime changes.
///
/// # Errors
///
/// Duplicate builtin addresses fail initialization, with both registration
/// locations in the diagnostic. No partial catalog is published. The failure
/// is retained for subsequent calls.
pub fn initialize() -> Result<(), Error> {
    CATALOG
        .get_or_init(|| registry::collect(&registry::BUILTINS).map(RwLock::new))
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}

/// Returns an entry's decompressed bytes, or `None` if its address is absent.
///
/// Addresses are matched literally, including case. The returned buffer belongs
/// to the caller and remains valid after the entry changes. A concurrent update
/// yields either the entire old contents or the entire new contents.
///
/// # Errors
///
/// Returns an error if initialization has not succeeded, the index lock is
/// poisoned, or the selected entry cannot be decompressed.
pub fn get(address: &str) -> Result<Option<Vec<u8>>, Error> {
    let content = catalog()?
        .read()
        .map_err(|_| Error::message("catalog index lock is poisoned"))?
        .get(address)
        .cloned();

    // Own the selected block while decoding, so writes can proceed meanwhile.
    content
        .map(|content| {
            compression::decompress(content.bytes()).map_err(|source| {
                Error::with_source(
                    format!("cannot decompress catalog entry {address:?}"),
                    source,
                )
            })
        })
        .transpose()
}

/// Adds an entry or replaces all of its contents.
///
/// The catalog compresses the supplied bytes before replacing the entry.
/// Replacement is atomic and a failed operation leaves the previous entry
/// intact. Replacing a builtin entry does not create a fallback to that entry.
///
/// # Errors
///
/// Returns an error if initialization has not succeeded, compression fails,
/// or the index lock is poisoned.
pub fn put(address: &str, content: &[u8]) -> Result<(), Error> {
    let catalog = catalog()?;
    let compressed = compression::compress(content).map_err(|source| {
        Error::with_source(format!("cannot compress catalog entry {address:?}"), source)
    })?;
    let content = Content::Owned(Arc::new(compressed));

    catalog
        .write()
        .map_err(|_| Error::message("catalog index lock is poisoned"))?
        .insert(Cow::Owned(address.to_owned()), content);
    Ok(())
}

/// Returns a lexicographically sorted snapshot of addresses matching `pattern`.
///
/// `*` matches zero or more characters, including `/`. All other characters
/// are literal. Matching covers the entire address and is case-sensitive.
/// `"*"` selects all entries; `"skill/git/*"` selects every address beginning
/// with `"skill/git/"`, regardless of the number of further slashes.
///
/// No matches produce an empty vector. Listing does not decompress contents.
///
/// # Errors
///
/// Returns an error if initialization has not succeeded or the index lock
/// is poisoned.
pub fn list(pattern: &str) -> Result<Vec<String>, Error> {
    let pattern = pattern::Pattern::new(pattern);
    Ok(catalog()?
        .read()
        .map_err(|_| Error::message("catalog index lock is poisoned"))?
        .keys()
        .filter(|address| pattern.matches(address))
        .map(|address| address.to_string())
        .collect())
}

fn catalog() -> Result<&'static RwLock<Entries>, Error> {
    CATALOG
        .get()
        .ok_or_else(|| Error::message("catalog is not initialized"))?
        .as_ref()
        .map_err(Clone::clone)
}

/// Embeds the builtin table prepared by this crate's build script.
///
/// Call once in the consumer's `src/lib.rs` or `src/main.rs`, after using
/// `build` from its `build.rs`. The macro registers the table through
/// `linkme`; the consumer needs no direct dependency on `linkme`.
/// The application must link the consumer crate for its entries to be present.
#[macro_export]
macro_rules! embed {
    () => {
        const _: () = {
            #[$crate::__private::linkme::distributed_slice($crate::__private::BUILTINS)]
            #[linkme(crate = $crate::__private::linkme)]
            #[allow(
                unsafe_code,
                reason = "linkme places the builtin table in a linker section"
            )]
            static REGISTRATION: $crate::__private::Builtin = $crate::__private::Builtin {
                entries: ::core::include!(::core::concat!(
                    ::core::env!("OUT_DIR"),
                    "/fairway-catalog/entries.rs"
                )),
                source: ::core::concat!(
                    ::core::module_path!(),
                    " (",
                    ::core::file!(),
                    ":",
                    ::core::line!(),
                    ")"
                ),
            };
        };
    };
}

/// Implementation support for [`embed!`].
#[doc(hidden)]
pub mod __private {
    pub use crate::registry::{BUILTINS, Builtin};
    pub use linkme;
}
