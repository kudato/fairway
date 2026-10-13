use skill::Entry;

fn main() -> Result<(), skill::BuildError> {
    skill::build([
        Entry::new("skill/writing/technical", "skills/technical.md"),
        Entry::new("reference/writing/ste", "skills/references/ste.md"),
        Entry::new("skill/editing/proofread", "skills/proofread.md"),
    ])
}
