use skill::Entry;

fn main() -> Result<(), skill::BuildError> {
    skill::build([
        Entry::new("skill/alpha/example-skill", "skills/example-skill.md"),
        Entry::new(
            "reference/alpha/example-reference",
            "skills/references/example-reference.md",
        ),
        Entry::new("skill/beta/other-skill", "skills/other-skill.md"),
    ])
}
