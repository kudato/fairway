fn main() -> Result<(), Box<dyn std::error::Error>> {
    owner::build_skills([("git/hooks/pre-commit".into(), b"hook".to_vec())])?;
    Ok(())
}
