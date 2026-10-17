//! Rules for addresses and namespace names.
//!
//! An address has exactly three parts: `skill/<namespace>/<name>` or
//! `reference/<namespace>/<name>`. The namespace and the name are not empty and
//! contain neither whitespace, control characters, `/` nor `*`.

use std::fmt;

/// What an address points to; the first part of the address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Skill,
    Reference,
}

impl Kind {
    /// The first part of addresses of this kind, which is also the name of the
    /// catalog namespace that stores them.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Reference => "reference",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A valid address, split into its parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Address<'a> {
    pub(crate) kind: Kind,
    pub(crate) namespace: &'a str,
    /// The address in the catalog namespace of its kind: `<namespace>/<name>`.
    pub(crate) entry: &'a str,
}

/// Splits an address into its parts and returns the reason it is invalid,
/// phrased to follow the address in a message.
pub(crate) fn parse(address: &str) -> Result<Address<'_>, &'static str> {
    let mut parts = address.split('/');
    let (Some(kind), Some(namespace), Some(name), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("must have three parts: `skill/<namespace>/<name>` or \
                    `reference/<namespace>/<name>`");
    };
    let kind = match kind {
        "skill" => Kind::Skill,
        "reference" => Kind::Reference,
        _ => return Err("must start with `skill/` or `reference/`"),
    };
    for part in [namespace, name] {
        match check_part(part) {
            Ok(()) => {}
            Err(Problem::Empty) => return Err("has an empty part"),
            Err(Problem::Space) => return Err("contains whitespace or a control character"),
            // Parts are split at `/`, so they never contain it.
            Err(Problem::Slash) => unreachable!("an address part cannot contain `/`"),
            Err(Problem::Star) => return Err("contains `*`"),
        }
    }
    Ok(Address {
        kind,
        namespace,
        entry: &address[kind.as_str().len() + 1..],
    })
}

/// Checks a namespace name and returns the reason it is invalid.
pub(crate) const fn check_namespace(name: &str) -> Result<(), &'static str> {
    match check_part(name) {
        Ok(()) => Ok(()),
        Err(Problem::Empty) => Err("a skill namespace cannot be empty"),
        Err(Problem::Space) => {
            Err("a skill namespace cannot contain whitespace or control characters")
        }
        Err(Problem::Slash) => Err("a skill namespace cannot contain `/`"),
        Err(Problem::Star) => Err("a skill namespace cannot contain `*`"),
    }
}

/// Rejects an invalid namespace name; in a static initializer, at compile time.
pub(crate) const fn valid_namespace(name: &str) {
    if let Err(reason) = check_namespace(name) {
        panic!("{}", reason);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Problem {
    Empty,
    Space,
    Slash,
    Star,
}

/// Checks the namespace or the name of an address.
const fn check_part(part: &str) -> Result<(), Problem> {
    let bytes = part.as_bytes();
    if bytes.is_empty() {
        return Err(Problem::Empty);
    }
    let mut i = 0;
    while i < bytes.len() {
        // Decode one scalar in const evaluation; str::chars is not const.
        // The input is &str, so every sequence is already valid UTF-8.
        let first = bytes[i];
        let (mut scalar, width) = match first {
            0..=0x7f => (first as u32, 1),
            0xc0..=0xdf => ((first & 0x1f) as u32, 2),
            0xe0..=0xef => ((first & 0x0f) as u32, 3),
            _ => ((first & 0x07) as u32, 4),
        };
        let mut offset = 1;
        while offset < width {
            scalar = (scalar << 6) | (bytes[i + offset] & 0x3f) as u32;
            offset += 1;
        }
        let character = char::from_u32(scalar).expect("a str contains valid Unicode scalars");
        if character.is_whitespace() || character.is_control() {
            return Err(Problem::Space);
        }
        if character == '/' {
            return Err(Problem::Slash);
        }
        if character == '*' {
            return Err(Problem::Star);
        }
        i += width;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Kind, check_namespace, parse};

    #[test]
    fn addresses_are_split_or_rejected_with_a_reason() {
        for (address, kind, namespace, entry) in [
            (
                "skill/alpha/example-skill",
                Kind::Skill,
                "alpha",
                "alpha/example-skill",
            ),
            (
                "reference/рыба/lorem.ipsum2_dolor-1",
                Kind::Reference,
                "рыба",
                "рыба/lorem.ipsum2_dolor-1",
            ),
            // Parts are literal, so dots are not path components.
            ("skill/../..", Kind::Skill, "..", "../.."),
        ] {
            let parsed =
                parse(address).map(|address| (address.kind, address.namespace, address.entry));
            assert_eq!(parsed, Ok((kind, namespace, entry)), "address {address:?}");
        }

        let three = "must have three parts: `skill/<namespace>/<name>` or \
                     `reference/<namespace>/<name>`";
        for (address, reason) in [
            ("", three),
            ("skill", three),
            ("skill/alpha", three),
            ("skill/alpha/example-skill/extra", three),
            ("skill/alpha/example-skill/", three),
            (
                "skills/alpha/example-skill",
                "must start with `skill/` or `reference/`",
            ),
            (
                "/alpha/example-skill",
                "must start with `skill/` or `reference/`",
            ),
            ("skill//example-skill", "has an empty part"),
            ("skill/alpha/", "has an empty part"),
            (
                "skill/alpha/two words",
                "contains whitespace or a control character",
            ),
            (
                "skill/alpha/two\u{3000}words",
                "contains whitespace or a control character",
            ),
            (
                "skill/al\u{85}pha/x",
                "contains whitespace or a control character",
            ),
            (
                "skill/alpha/tab\t",
                "contains whitespace or a control character",
            ),
            ("skill/alpha/*", "contains `*`"),
            ("reference/al*ha/x", "contains `*`"),
        ] {
            assert_eq!(parse(address), Err(reason), "address {address:?}");
        }
    }

    #[test]
    fn a_namespace_name_is_one_part() {
        for (name, expected) in [
            ("alpha", Ok(())),
            ("рыба.猫.🌍", Ok(())),
            ("", Err("a skill namespace cannot be empty")),
            (
                "two words",
                Err("a skill namespace cannot contain whitespace or control characters"),
            ),
            ("a/b", Err("a skill namespace cannot contain `/`")),
            ("a*", Err("a skill namespace cannot contain `*`")),
        ] {
            assert_eq!(check_namespace(name), expected, "name {name:?}");
        }
    }
}
