//! Typed plugin settings loaded once from `config.toml` and recursive `conf.d` files.
//!
//! ```
//! use serde::Deserialize;
//!
//! #[derive(Default, Deserialize)]
//! #[serde(default, deny_unknown_fields)]
//! struct Settings {
//!     enabled: bool,
//! }
//!
//! fairway_config::namespace!(EXAMPLE: Settings, "example");
//! ```
//!
//! Fairway prepares every namespace before running a command. Handlers read the
//! prepared settings synchronously through [`Namespace::get`].

mod error;
mod load;
mod registry;

use std::{any::Any, collections::BTreeMap, marker::PhantomData};

use tokio::sync::OnceCell;

pub use error::Error;

type Value = Box<dyn Any + Send + Sync>;
type Values = BTreeMap<&'static str, Value>;

// One cell publishes the complete set. Cancellation or failure cannot expose
// individually prepared namespaces to readers.
static CONFIG: OnceCell<Result<Values, Error>> = OnceCell::const_new();

/// A named configuration table and its prepared settings of type `T`.
///
/// Declared by [`namespace!`]. Two namespaces may use the same settings type
/// while holding independent values.
pub struct Namespace<T> {
    name: &'static str,
    settings: PhantomData<fn() -> T>,
}

impl<T> Namespace<T> {
    #[doc(hidden)]
    pub const fn new(name: &'static str) -> Self {
        registry::valid_name(name);
        Self {
            name,
            settings: PhantomData,
        }
    }
}

impl<T: Send + Sync + 'static> Namespace<T> {
    /// Returns the settings prepared at application startup without copying them.
    ///
    /// # Panics
    ///
    /// Panics if initialization has not succeeded or the namespace was not
    /// registered by [`namespace!`]. It must not be called from settings'
    /// `Default` or `Deserialize` implementations.
    pub fn get(&'static self) -> &'static T {
        let values = match CONFIG.get() {
            Some(Ok(values)) => values,
            _ => panic!("configuration is not initialized successfully"),
        };
        values
            .get(self.name)
            .and_then(|value| value.downcast_ref())
            .expect("namespace! registers this namespace with its settings type")
    }
}

/// Declares `pub(crate) static NAME: Namespace<T>` for a top-level TOML table.
///
/// `T` implements `Default + DeserializeOwned + Send + Sync + 'static`.
/// The application's registry test checks namespace name uniqueness across
/// all linked plugins. Reusing `T` under different names is allowed.
#[macro_export]
macro_rules! namespace {
    ($id:ident: $ty:ty, $name:literal $(,)?) => {
        pub(crate) static $id: $crate::Namespace<$ty> = $crate::Namespace::new($name);

        const _: () = {
            #[$crate::__private::linkme::distributed_slice($crate::__private::NAMESPACES)]
            #[linkme(crate = $crate::__private::linkme)]
            #[allow(
                unsafe_code,
                reason = "linkme places the declaration in a linker section"
            )]
            static REGISTRATION: $crate::__private::Registration =
                $crate::__private::Registration::new::<$ty>(
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

/// Application startup and macro support, not part of the plugin API.
#[doc(hidden)]
pub mod __private {
    pub use crate::registry::{NAMESPACES, Registration};
    pub use linkme;

    /// Validates all linked declarations without reading configuration or constructing settings.
    ///
    /// # Panics
    ///
    /// Panics on duplicate names, identifying both declarations.
    pub fn assert_valid() {
        if let Err(error) = crate::registry::checked() {
            panic!("{error}");
        }
    }

    /// Loads and publishes all settings once. Completed failures are cached as well.
    /// Cancellation before publication permits a later attempt to retry.
    pub async fn initialize() -> Result<(), crate::Error> {
        crate::CONFIG
            .get_or_init(crate::load::registered)
            .await
            .as_ref()
            .map(|_| ())
            .map_err(Clone::clone)
    }
}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/ru/plugin-development/config.md")]
mod guide_ru {}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/en/plugin-development/config.md")]
mod guide_en {}
