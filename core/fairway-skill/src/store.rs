//! Skills and references in the catalog, each file's text compressed.
//!
//! The catalog keeps the text of the file as it was written, frontmatter
//! included, so that it can be given back as it is; each read splits it again.

use std::borrow::Cow;

use fairway_catalog::Namespace;
use fairway_codec::{Decode, Encode, Zstd};

use crate::address::Kind;

pub(crate) type Stored = Cow<'static, [u8]>;

fairway_catalog::namespace!(SKILLS: Stored, "skill", builtin);
fairway_catalog::namespace!(REFERENCES: Stored, "reference", builtin);

/// The catalog namespace of addresses of `kind`.
pub(crate) fn namespace(kind: Kind) -> &'static Namespace<Stored> {
    match kind {
        Kind::Skill => &SKILLS,
        Kind::Reference => &REFERENCES,
    }
}

/// Compresses the text of a file for the catalog.
pub(crate) fn encode(text: String) -> Result<Vec<u8>, fairway_codec::Error> {
    Zstd(text).encode()
}

/// Restores the text that [`encode`] compressed.
pub(crate) fn decode(stored: Stored) -> String {
    Zstd::<String>::decode(stored.into_owned())
        .expect("fairway-skill stores only the texts it compressed")
        .into_inner()
}
