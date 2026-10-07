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
            ("*", "skill/git/hooks/pre-commit", true),
            ("skill/*", "skill/git/commit", true),
            ("skill/git/*", "skill/git/hooks/pre-commit", true),
            ("skill/git/*", "skill/git", false),
            ("*/git", "skill/git", true),
            ("*/git", "skill/git/commit", false),
            ("*/git/*", "resource/git/convention", true),
            ("*/git/*", "resource/github/convention", false),
            ("*git*", "git", true),
            ("*git*", "git/git", true),
            ("**git***", "skill/git/commit", true),
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
            ("skill/*", "other/skill/git", false),
            ("*задача*", "моя/задача/✓", true),
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
