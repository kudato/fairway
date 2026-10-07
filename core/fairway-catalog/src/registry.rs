use std::borrow::Cow;

use crate::{Content, Entries, Error};

/// One consumer's embedded table and its registration location.
pub struct Builtin {
    /// Literal addresses and their independently compressed contents.
    pub entries: &'static [(&'static str, &'static [u8])],
    /// The crate, file, and line containing the embedding macro.
    pub source: &'static str,
}

/// Builtin tables contributed by linked consumers.
#[linkme::distributed_slice]
pub static BUILTINS: [Builtin];

pub(crate) fn collect(builtins: &[Builtin]) -> Result<Entries, Error> {
    let mut entries: Vec<_> = builtins
        .iter()
        .flat_map(|builtin| {
            builtin
                .entries
                .iter()
                .map(move |&(address, compressed)| (address, compressed, builtin.source))
        })
        .collect();
    entries.sort_unstable_by_key(|&(address, _, source)| (address, source));

    for pair in entries.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(Error::message(format!(
                "duplicate builtin catalog address {:?}: {} and {}",
                pair[0].0, pair[0].2, pair[1].2
            )));
        }
    }

    Ok(entries
        .into_iter()
        .map(|(address, compressed, _)| (Cow::Borrowed(address), Content::Builtin(compressed)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{Builtin, collect};
    use crate::Content;
    use std::borrow::Cow;

    #[test]
    fn initialization_borrows_the_original_blocks_and_addresses() {
        static BYTES: &[u8] = b"not even a compressed frame";
        static ADDRESS: &str = "example";
        static TABLE: &[(&str, &[u8])] = &[(ADDRESS, BYTES)];
        let entries = collect(&[Builtin {
            entries: TABLE,
            source: "test",
        }])
        .unwrap();
        let (address, content) = entries.get_key_value(ADDRESS).unwrap();
        assert!(matches!(address, Cow::Borrowed(_)));
        assert_eq!(address.as_ptr(), ADDRESS.as_ptr());
        assert!(matches!(content, Content::Builtin(_)));
        assert!(std::ptr::eq(content.bytes(), BYTES));
    }
}
