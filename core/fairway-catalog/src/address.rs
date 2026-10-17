//! Rules for namespace names and addresses.
//!
//! A full address is `namespace/address`, for example `skill/git/commit`.
//! Its parts are separated by `/`, none of them is empty, and none contains
//! `*`, which lists use as a mask. A namespace name is therefore one part.

/// Checks a namespace name and returns the reason it is invalid.
pub(crate) const fn check_name(name: &str) -> Result<(), &'static str> {
    let bytes = name.as_bytes();
    if bytes.is_empty() {
        return Err("a catalog namespace name cannot be empty");
    }
    let mut i = 0;
    while i < bytes.len() {
        // `/` and `*` are ASCII, so they never occur inside a multibyte character.
        match bytes[i] {
            b'/' => return Err("a catalog namespace name cannot contain `/`"),
            b'*' => return Err("a catalog namespace name cannot contain `*`"),
            _ => i += 1,
        }
    }
    Ok(())
}

/// Rejects an invalid namespace name; in a static initializer, at compile time.
pub(crate) const fn valid_name(name: &str) {
    if let Err(reason) = check_name(name) {
        panic!("{}", reason);
    }
}

/// Checks an address within a namespace and returns the reason it is invalid,
/// phrased to follow the full address in a message.
pub(crate) fn check(address: &str) -> Result<(), &'static str> {
    if address.split('/').any(str::is_empty) {
        Err("has an empty part")
    } else if address.contains('*') {
        Err("contains `*`")
    } else {
        Ok(())
    }
}

/// The address as messages show it, with its namespace.
pub(crate) fn full(namespace: &str, address: &str) -> String {
    format!("{namespace}/{address}")
}

#[cfg(test)]
mod tests {
    use super::check_name;

    #[test]
    fn a_namespace_name_is_one_part() {
        for (name, expected) in [
            ("skill", Ok(())),
            ("plugin-guide_2", Ok(())),
            ("задачи", Ok(())),
            ("..", Ok(())),
            ("", Err("a catalog namespace name cannot be empty")),
            (
                "skill/git",
                Err("a catalog namespace name cannot contain `/`"),
            ),
            ("/", Err("a catalog namespace name cannot contain `/`")),
            ("skill*", Err("a catalog namespace name cannot contain `*`")),
            ("*", Err("a catalog namespace name cannot contain `*`")),
        ] {
            assert_eq!(check_name(name), expected, "name {name:?}");
        }
    }
}
