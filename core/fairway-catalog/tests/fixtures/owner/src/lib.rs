//! An owner of two builtin namespaces: other crates use
//! its functions and macros, not the catalog.

use std::borrow::Cow;

catalog::namespace!(ALPHA: Cow<'static, [u8]>, "alpha-items", builtin);
catalog::namespace!(BETA: Cow<'static, [u8]>, "beta-items", builtin);

pub fn item(address: &str) -> Option<Cow<'static, [u8]>> {
    ALPHA.get(address)
}

pub fn items(pattern: &str) -> Vec<String> {
    ALPHA.list(pattern)
}

pub fn put_item(
    address: &str,
    contents: Vec<u8>,
) -> Result<Option<Cow<'static, [u8]>>, catalog::Error> {
    ALPHA.put(address, Cow::Owned(contents))
}

pub fn beta_item(key: &str) -> Option<Cow<'static, [u8]>> {
    BETA.get(key)
}

pub fn beta_items() -> Vec<String> {
    BETA.list("*")
}

#[cfg(feature = "build")]
pub fn build_items(
    items: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Result<(), catalog::BuildError> {
    catalog::build("alpha-items", items)
}

#[cfg(feature = "build")]
pub fn build_beta_items(
    items: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Result<(), catalog::BuildError> {
    catalog::build("beta-items", items)
}

#[macro_export]
macro_rules! embed_items {
    () => {
        $crate::__private::catalog::embed!("alpha-items");
    };
}

#[macro_export]
macro_rules! embed_beta_items {
    () => {
        $crate::__private::catalog::embed!("beta-items");
    };
}

#[doc(hidden)]
pub mod __private {
    pub use catalog;
}
