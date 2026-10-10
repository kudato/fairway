//! An owner of two builtin namespaces: other crates use
//! its functions and macros, not the catalog.

use std::borrow::Cow;

catalog::namespace!(SKILLS: Cow<'static, [u8]>, "skill", builtin);
catalog::namespace!(GLOSSARY: Cow<'static, [u8]>, "glossary", builtin);

pub fn skill(address: &str) -> Option<Cow<'static, [u8]>> {
    SKILLS.get(address)
}

pub fn skills(pattern: &str) -> Vec<String> {
    SKILLS.list(pattern)
}

pub fn put_skill(
    address: &str,
    contents: Vec<u8>,
) -> Result<Option<Cow<'static, [u8]>>, catalog::Error> {
    SKILLS.put(address, Cow::Owned(contents))
}

pub fn term(term: &str) -> Option<Cow<'static, [u8]>> {
    GLOSSARY.get(term)
}

pub fn terms() -> Vec<String> {
    GLOSSARY.list("*")
}

#[cfg(feature = "build")]
pub fn build_skills(
    skills: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Result<(), catalog::BuildError> {
    catalog::build("skill", skills)
}

#[cfg(feature = "build")]
pub fn build_glossary(
    terms: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Result<(), catalog::BuildError> {
    catalog::build("glossary", terms)
}

#[macro_export]
macro_rules! embed_skills {
    () => {
        $crate::__private::catalog::embed!("skill");
    };
}

#[macro_export]
macro_rules! embed_glossary {
    () => {
        $crate::__private::catalog::embed!("glossary");
    };
}

#[doc(hidden)]
pub mod __private {
    pub use catalog;
}
