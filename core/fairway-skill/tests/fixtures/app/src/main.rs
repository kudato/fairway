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

    let ste = skill::reference("reference/writing/ste", ())?;
    let technical = skill::skill(
        "skill/writing/technical",
        BTreeMap::from([("ste", ste.as_ref())]),
    )?;
    print!("{}", technical.as_ref());
    println!("{}", skill::description("skill/writing/technical")?);
    print!("{}", skill::skill("skill/editing/proofread", ())?.as_ref());
    println!("{}", skill::description("skill/editing/proofread")?);
    for address in std::env::args().skip(1) {
        print!("{}", skill::skill(&address, ())?.as_ref());
    }
    Ok(())
}
