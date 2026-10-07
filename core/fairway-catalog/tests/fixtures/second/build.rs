fn main() -> Result<(), catalog::BuildError> {
    catalog::build([
        ("resource/git/convention".into(), b"convention".to_vec()),
        ("skill/git/hooks/pre-commit".into(), b"hook".to_vec()),
    ])
}
