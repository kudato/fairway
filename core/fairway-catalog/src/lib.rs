//! Global in-memory namespaces of values, with builtin initial data.
//!
//! A crate that owns a namespace declares it with [`namespace!`] and keeps the
//! static private: other crates use the owner's API. A namespace holds values
//! of the owner's type by address, such as `git/commit`; together with the
//! namespace name this is the full address `skill/git/commit`. Fairway calls
//! [`initialize`] at startup, which checks every linked namespace and fills
//! all of them or none.
//!
//! A namespace starts empty, receives the builtin tables that crates prepare
//! with `build` in their build scripts (the `build` feature) and include with
//! [`embed!`], or receives the values of the owner's function. Values are
//! stored as they are: encoding, if any, is up to the owner.
//!
//! ```
//! fairway_catalog::namespace!(NOTES: String, "doc-notes");
//!
//! fairway_catalog::initialize()?;
//! NOTES.put("todo/first", "write tests".to_owned())?;
//! assert_eq!(NOTES.get("todo/first").as_deref(), Some("write tests"));
//! assert_eq!(NOTES.list("todo/*"), ["todo/first"]);
//! # Ok::<(), fairway_catalog::Error>(())
//! ```

mod address;
#[cfg(feature = "build")]
mod build;
mod error;
mod namespace;
mod pattern;
mod registry;

use std::sync::OnceLock;

#[cfg(feature = "build")]
pub use build::build;
#[cfg(feature = "build")]
pub use error::BuildError;
pub use error::Error;
pub use namespace::Namespace;

// The first result is kept: a failure stays, and filled namespaces keep their
// later changes.
static INITIALIZED: OnceLock<Result<(), Error>> = OnceLock::new();

/// Checks every linked namespace and builtin table, then fills all namespaces.
///
/// Fairway calls this at startup, before any namespace is used; owners' tests
/// call it themselves. Repeated and concurrent calls return the result of the
/// first one and keep the changes made since.
///
/// # Errors
///
/// Returns every problem at once: a repeated namespace name, builtin data for
/// an unknown namespace or for one declared without `builtin`, an invalid
/// address, or an address repeated in a namespace, with the declarations and
/// [`embed!`] calls involved. No namespace is filled then.
pub fn initialize() -> Result<(), Error> {
    INITIALIZED
        .get_or_init(|| {
            registry::initialize(
                &registry::FAIRWAY_CATALOG_NAMESPACES,
                &registry::FAIRWAY_CATALOG_BUILTINS,
            )
        })
        .clone()
}

/// Declares `pub(crate) static NAME: Namespace<V>` and registers it by name.
///
/// The name is one part of a full address: it is not empty and contains
/// neither `/` nor `*`, otherwise the declaration does not compile. A name
/// declared twice in the linked crates is reported by [`initialize`]. The
/// parameter after the name chooses how `initialize` fills the namespace:
///
/// - none: the namespace starts empty;
/// - `builtin`: it receives the tables included with [`embed!`] under its
///   name. `V` implements `From<&'static [u8]>`; `Cow<'static, [u8]>` borrows
///   the embedded bytes without copying them;
/// - `initial = function`: it receives the values returned by a
///   `fn() -> Vec<(String, V)>`.
///
/// ```
/// use std::borrow::Cow;
///
/// fn defaults() -> Vec<(String, u16)> {
///     vec![("http/port".to_owned(), 8080)]
/// }
///
/// fairway_catalog::namespace!(STATE: String, "doc-state");
/// fairway_catalog::namespace!(SKILLS: Cow<'static, [u8]>, "doc-skill", builtin);
/// fairway_catalog::namespace!(CONFIG: u16, "doc-config", initial = defaults);
/// ```
#[macro_export]
macro_rules! namespace {
    ($id:ident: $ty:ty, $name:literal $(,)?) => {
        $crate::__namespace!($id, $ty, $crate::Namespace::new($name));
    };
    ($id:ident: $ty:ty, $name:literal, builtin $(,)?) => {
        $crate::__namespace!($id, $ty, $crate::Namespace::builtin($name));
    };
    ($id:ident: $ty:ty, $name:literal, initial = $initial:expr $(,)?) => {
        $crate::__namespace!($id, $ty, $crate::Namespace::initial($name, $initial));
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __namespace {
    ($id:ident, $ty:ty, $namespace:expr) => {
        pub(crate) static $id: $crate::Namespace<$ty> = $namespace;

        const _: () = {
            #[$crate::__private::linkme::distributed_slice(
                $crate::__private::FAIRWAY_CATALOG_NAMESPACES
            )]
            #[linkme(crate = $crate::__private::linkme)]
            #[allow(
                unsafe_code,
                reason = "linkme places the declaration in a linker section"
            )]
            static REGISTRATION: $crate::__private::Registration =
                $crate::__private::Registration::new(
                    &$id,
                    ::core::concat!(
                        ::core::module_path!(),
                        " (",
                        ::core::file!(),
                        ":",
                        ::core::line!(),
                        ")"
                    ),
                );
        };
    };
}

/// Includes the builtin table that this crate's build script prepared for a
/// namespace.
///
/// The build script calls `build` with the same namespace name. A crate can
/// embed tables into several namespaces, one call per namespace. The name is a
/// literal, because the macro builds the path of the table from it; an invalid
/// name does not compile. The namespace must be declared with `builtin`, which
/// [`initialize`] checks. Owners wrap this macro and `build` for the crates
/// that embed into their namespaces.
#[macro_export]
macro_rules! embed {
    ($namespace:literal $(,)?) => {
        const _: () = {
            #[$crate::__private::linkme::distributed_slice(
                $crate::__private::FAIRWAY_CATALOG_BUILTINS
            )]
            #[linkme(crate = $crate::__private::linkme)]
            #[allow(
                unsafe_code,
                reason = "linkme places the builtin table in a linker section"
            )]
            static REGISTRATION: $crate::__private::Builtin = $crate::__private::Builtin::new(
                $namespace,
                ::core::include!(::core::concat!(
                    ::core::env!("OUT_DIR"),
                    "/fairway-catalog/",
                    $namespace,
                    "/entries.rs"
                )),
                ::core::concat!(
                    ::core::module_path!(),
                    " (",
                    ::core::file!(),
                    ":",
                    ::core::line!(),
                    ")"
                ),
            );
        };
    };
}

/// Macro support, not part of the owner API.
#[doc(hidden)]
pub mod __private {
    pub use crate::registry::{
        Builtin, FAIRWAY_CATALOG_BUILTINS, FAIRWAY_CATALOG_NAMESPACES, Registration,
    };
    pub use linkme;
}
