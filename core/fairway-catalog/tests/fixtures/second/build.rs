fn main() -> Result<(), Box<dyn std::error::Error>> {
    owner::build_items([("lorem/sit/dolorem-ipsum".into(), b"dolor".to_vec())])?;
    Ok(())
}
