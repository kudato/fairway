//! Prints builtin or overridden skills; the addresses of more skills to
//! print are the arguments.

use std::collections::BTreeMap;

use skill_fixture_notes as _;
use skill_fixture_plugin as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Fairway's part: the registry test and initialization at startup.
    skill::validate_registry();
    tokio::runtime::Builder::new_current_thread()
        .build()?
        .block_on(skill::initialize())?;

    let reference = skill::reference("reference/alpha/example-reference", ())?;
    let example = skill::skill(
        "skill/alpha/example-skill",
        BTreeMap::from([("ipsum", reference.as_ref())]),
    )?;
    print!("{}", example.as_ref());
    println!("{}", skill::description("skill/alpha/example-skill")?);
    print!("{}", skill::skill("skill/beta/other-skill", ())?.as_ref());
    println!("{}", skill::description("skill/beta/other-skill")?);
    for address in std::env::args().skip(1) {
        print!("{}", skill::skill(&address, ())?.as_ref());
    }
    Ok(())
}
