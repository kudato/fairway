pub(crate) struct Pattern<'a> {
    parts: Vec<&'a str>,
}

impl<'a> Pattern<'a> {
    pub(crate) fn new(pattern: &'a str) -> Self {
        Self {
            parts: pattern.split('*').collect(),
        }
    }

    pub(crate) fn matches(&self, address: &str) -> bool {
        let Some(mut rest) = address.strip_prefix(self.parts[0]) else {
            return false;
        };
        let Some((suffix, middle)) = self.parts[1..].split_last() else {
            return rest.is_empty();
        };
        for part in middle {
            let Some(offset) = rest.find(part) else {
                return false;
            };
            rest = &rest[offset + part.len()..];
        }
        rest.ends_with(suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::Pattern;

    #[test]
    fn masks_match_complete_literal_addresses() {
        for (pattern, address, expected) in [
            ("*", "", true),
            ("*", "alpha-items/lorem/sit/dolorem-ipsum", true),
            ("alpha-items/*", "alpha-items/lorem/ipsum", true),
            (
                "alpha-items/lorem/*",
                "alpha-items/lorem/sit/dolorem-ipsum",
                true,
            ),
            ("alpha-items/lorem/*", "alpha-items/lorem", false),
            ("*/lorem", "alpha-items/lorem", true),
            ("*/lorem", "alpha-items/lorem/ipsum", false),
            ("*/lorem/*", "beta-items/lorem/elit", true),
            ("*/lorem/*", "beta-items/loremque/elit", false),
            ("*lorem*", "lorem", true),
            ("*lorem*", "lorem/lorem", true),
            ("**lorem***", "alpha-items/lorem/ipsum", true),
            ("a*a", "a", false),
            ("a*a", "aa", true),
            ("*ab*bc", "abc", false),
            ("*ab*bc", "abbc", true),
            ("*ab*bc", "abbbc", true),
            ("a", "aa", false),
            ("a", "a", true),
            ("a", "A", false),
            ("", "", true),
            ("", "a", false),
            ("alpha-items/*", "other/alpha-items/lorem", false),
            ("*рыба*", "лорем/рыба/✓", true),
            ("*🙂*🙂", "🙂", false),
            ("*🙂*🙂", "🙂/🙂", true),
            ("?[]\\", "?[]\\", true),
            ("?", "x", false),
        ] {
            assert_eq!(
                Pattern::new(pattern).matches(address),
                expected,
                "pattern {pattern:?}, address {address:?}"
            );
        }
    }
}
