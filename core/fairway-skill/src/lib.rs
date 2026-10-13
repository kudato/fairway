//! Skills and references for Fairway plugins: embedded at build time,
//! overridden by configuration.
//!
//! A skill is an instruction for a model; a reference is material that skills
//! point to. Both are Markdown templates stored at addresses:
//! `skill/<namespace>/<name>` and `reference/<namespace>/<name>`. A namespace
//! and a name are not empty and contain neither whitespace, control
//! characters, `/` nor `*`. A skill file starts with frontmatter that holds
//! only the skill's description; a reference file has no frontmatter:
//!
//! ```markdown
//! ---
//! description: Write technical documentation in plain English.
//! ---
//! # Technical writing
//!
//! For English text, follow these rules: {{ ste }}
//! ```
//!
//! # Getting skills and references
//!
//! [`skill`] and [`reference`](fn@reference) render a template with the
//! caller's variables and return Markdown; a skill is returned without its
//! frontmatter.
//! [`description`] returns a skill's description, which is not a template.
//! The three functions work in memory after [`initialize`]. Variables are
//! optional and are up to the caller: a template that uses a variable the
//! caller does not pass fails to render. To include a reference in a skill,
//! get the reference and pass its text as a variable; or write the
//! reference's address in the skill.
//!
//! ```no_run
//! use fairway_codec::serde::Serialize;
//!
//! #[derive(Serialize)]
//! #[serde(crate = "fairway_codec::serde")]
//! struct Technical<'a> {
//!     ste: &'a str,
//! }
//!
//! let ste = fairway_skill::reference("reference/writing/ste", ())?;
//! let technical = fairway_skill::skill(
//!     "skill/writing/technical",
//!     Technical { ste: ste.as_ref() },
//! )?;
//! let description = fairway_skill::description("skill/writing/technical")?;
//! # Ok::<(), fairway_skill::Error>(())
//! ```
//!
//! # Builtin skills and references
//!
//! A plugin embeds its files in the program. The crate depends on
//! `fairway-skill`, and its build script on `fairway-skill` with the `build`
//! feature:
//!
//! ```toml
//! [dependencies]
//! fairway-skill.workspace = true
//!
//! [build-dependencies]
//! fairway-skill = { workspace = true, features = ["build"] }
//! ```
//!
//! The build script calls `build` once with every file of every namespace of
//! the crate; it checks and compresses them. The crate declares each of its
//! namespaces with [`namespace!`] and includes the files with [`embed!`]:
//!
//! ```rust,ignore
//! // build.rs
//! use fairway_skill::Entry;
//!
//! fn main() -> Result<(), fairway_skill::BuildError> {
//!     fairway_skill::build([
//!         Entry::new("skill/writing/technical", "skills/technical.md"),
//!         Entry::new("reference/writing/ste", "skills/references/ste.md"),
//!         Entry::new("skill/editing/proofread", "skills/proofread.md"),
//!     ])
//! }
//!
//! // src/lib.rs
//! fairway_skill::namespace!("writing");
//! fairway_skill::namespace!("editing");
//! fairway_skill::embed!();
//! ```
//!
//! A namespace belongs to the crate that declares it, which is checked by
//! [`validate_registry`]. A crate may declare a namespace without builtins.
//!
//! # Overrides
//!
//! The `[skill]` configuration table replaces any builtin and adds skills and
//! references at new addresses, in any namespace, including one that no
//! plugin declares:
//!
//! ```toml
//! [skill]
//! skills_dir = "/Users/me/skills"
//! "skill/writing/technical" = "my/technical.md"
//! "skill/my/notes" = "/Users/me/notes.md"
//! ```
//!
//! A key with `/` is an address, and its value is the path of a file. A
//! relative path starts at `skills_dir`, an absolute one is used as it is.
//! `skills_dir` is `skills` by default; a relative `skills_dir` starts at the
//! Fairway directory ([`fairway_fs::home`]). [`initialize`] reads and checks
//! every file at startup.

mod address;
#[cfg(feature = "build")]
mod build;
mod document;
mod error;
mod overrides;
mod registry;
mod store;

use fairway_codec::{Markdown, serde::Serialize};
use fairway_template::Template;
use tokio::sync::OnceCell;

use crate::address::Kind;

#[cfg(feature = "build")]
pub use build::{Entry, build};
#[cfg(feature = "build")]
pub use error::BuildError;
pub use error::{Error, ErrorKind};

// The first completed result is kept; a cancelled call lets a later one retry.
static INITIALIZED: OnceCell<Result<(), Error>> = OnceCell::const_new();

/// Loads the overrides from the configuration and makes skills and references
/// available.
///
/// Fairway calls this at startup, after it loads the configuration; tests call
/// it themselves. The catalog and the configuration are initialized first if
/// they are not yet. Every file of the `[skill]` table is read and checked as
/// `build` checks builtins: a skill has frontmatter with only a nonempty
/// `description`, a reference has no frontmatter, and the template of either
/// compiles. Then all overrides are stored, or none: an override replaces the
/// builtin at its address.
///
/// Repeated and concurrent calls return the result of the first one.
///
/// # Errors
///
/// Returns an error of [`ErrorKind::Initialize`] if the catalog or the
/// configuration cannot be initialized or the Fairway directory cannot be
/// determined, or listing every override file that cannot be read or is
/// invalid, with its address and path.
pub async fn initialize() -> Result<(), Error> {
    INITIALIZED.get_or_init(overrides::load).await.clone()
}

/// Renders the skill at `address` with `variables` and returns its body
/// without frontmatter.
///
/// `variables` is any [`Serialize`] value that serializes to a map, such as a
/// struct; its fields are the template's variables. Pass `()` for none.
///
/// ```no_run
/// let technical = fairway_skill::skill("skill/writing/technical", ())?;
/// println!("{}", technical.as_ref());
/// # Ok::<(), fairway_skill::Error>(())
/// ```
///
/// # Errors
///
/// Returns an error of [`ErrorKind::NotInitialized`] before [`initialize`]
/// succeeds, [`ErrorKind::InvalidAddress`] if `address` is not a valid skill
/// address, [`ErrorKind::NotFound`] if no skill is stored there, and
/// [`ErrorKind::Render`] if the template cannot be rendered with `variables`.
pub fn skill<C: Serialize>(address: &str, variables: C) -> Result<Markdown, Error> {
    let text = text(address, Kind::Skill)?;
    let skill = document::split(&text).expect("stored skills have frontmatter");
    skill
        .template(address)
        .and_then(|template| template.render(variables))
        .map(Markdown::parse)
        .map_err(|source| render_error(Kind::Skill, address, source))
}

/// Renders the reference at `address` with `variables`.
///
/// `variables` is any [`Serialize`] value that serializes to a map, such as a
/// struct; its fields are the template's variables. Pass `()` for none.
///
/// ```no_run
/// let ste = fairway_skill::reference("reference/writing/ste", ())?;
/// println!("{}", ste.as_ref());
/// # Ok::<(), fairway_skill::Error>(())
/// ```
///
/// # Errors
///
/// Returns an error of [`ErrorKind::NotInitialized`] before [`initialize`]
/// succeeds, [`ErrorKind::InvalidAddress`] if `address` is not a valid
/// reference address, [`ErrorKind::NotFound`] if no reference is stored
/// there, and [`ErrorKind::Render`] if the template cannot be rendered with
/// `variables`.
pub fn reference<C: Serialize>(address: &str, variables: C) -> Result<Markdown, Error> {
    let text = text(address, Kind::Reference)?;
    Template::compile(address, text)
        .and_then(|template| template.render(variables))
        .map(Markdown::parse)
        .map_err(|source| render_error(Kind::Reference, address, source))
}

/// Returns the description of the skill at `address`, from its frontmatter.
///
/// ```no_run
/// let description = fairway_skill::description("skill/writing/technical")?;
/// # Ok::<(), fairway_skill::Error>(())
/// ```
///
/// # Errors
///
/// Returns an error of [`ErrorKind::NotInitialized`] before [`initialize`]
/// succeeds, [`ErrorKind::InvalidAddress`] if `address` is not a valid skill
/// address, and [`ErrorKind::NotFound`] if no skill is stored there.
pub fn description(address: &str) -> Result<String, Error> {
    let text = text(address, Kind::Skill)?;
    let skill = document::split(&text).expect("stored skills have frontmatter");
    Ok(skill
        .description()
        .expect("stored skills have a valid description"))
}

/// Returns the stored text at an address of `kind`.
fn text(address: &str, kind: Kind) -> Result<String, Error> {
    if !matches!(INITIALIZED.get(), Some(Ok(()))) {
        return Err(Error::new(
            ErrorKind::NotInitialized,
            "skills are not initialized successfully".to_owned(),
        ));
    }
    let parsed = address::parse(address).map_err(|reason| {
        Error::new(
            ErrorKind::InvalidAddress,
            format!("address {address:?} {reason}"),
        )
    })?;
    if parsed.kind != kind {
        return Err(Error::new(
            ErrorKind::InvalidAddress,
            format!("address {address:?} is not a {kind} address"),
        ));
    }
    store::namespace(kind)
        .get(parsed.entry)
        .map(store::decode)
        .ok_or_else(|| Error::new(ErrorKind::NotFound, format!("no {kind} at {address:?}")))
}

fn render_error(kind: Kind, address: &str, source: fairway_template::Error) -> Error {
    Error::caused_by(
        ErrorKind::Render,
        format!("cannot render {kind} {address:?}"),
        source,
    )
}

/// Checks the namespaces that linked crates declare and embed builtins into.
///
/// The application's test calls this; it reads neither the configuration nor
/// any file.
///
/// # Panics
///
/// Panics listing every namespace declared more than once, and every
/// namespace with builtins of a crate that does not declare it with
/// [`namespace!`], with the declarations and [`embed!`] calls involved.
pub fn validate_registry() {
    if let Err(problems) = registry::check(
        &registry::FAIRWAY_SKILL_NAMESPACES,
        &registry::FAIRWAY_SKILL_EMBEDDED,
    ) {
        panic!("{problems}");
    }
}

/// Declares a namespace of skills and references that belongs to this crate.
///
/// The name is a literal: it is not empty and contains neither whitespace,
/// control characters, `/` nor `*`, otherwise the declaration does not
/// compile. A crate may declare several namespaces, with or without builtins.
/// [`validate_registry`] reports a namespace declared more than once.
///
/// ```
/// fairway_skill::namespace!("writing");
/// fairway_skill::namespace!("editing");
/// ```
#[macro_export]
macro_rules! namespace {
    ($name:literal $(,)?) => {
        const _: () = {
            #[$crate::__private::linkme::distributed_slice(
                $crate::__private::FAIRWAY_SKILL_NAMESPACES
            )]
            #[linkme(crate = $crate::__private::linkme)]
            #[allow(
                unsafe_code,
                reason = "linkme places the declaration in a linker section"
            )]
            static REGISTRATION: $crate::__private::Registration =
                $crate::__private::Registration::new(
                    $name,
                    ::core::module_path!(),
                    ::core::concat!(
                        ::core::module_path!(),
                        " (",
                        ::core::file!(),
                        ":",
                        ::core::line!(),
                        ")"
                    ),
                );
        };
    };
}

/// Includes the builtin skills and references that this crate's build script
/// prepared with `build`.
///
/// Call it once per crate, and only in a crate whose build script calls
/// `build`. Every namespace of the builtins must be declared with
/// [`namespace!`] in the same crate, which [`validate_registry`] checks.
#[macro_export]
macro_rules! embed {
    () => {
        $crate::__private::catalog::embed!("skill");
        $crate::__private::catalog::embed!("reference");

        const _: () = {
            #[$crate::__private::linkme::distributed_slice(
                $crate::__private::FAIRWAY_SKILL_EMBEDDED
            )]
            #[linkme(crate = $crate::__private::linkme)]
            #[allow(
                unsafe_code,
                reason = "linkme places the declaration in a linker section"
            )]
            static REGISTRATION: $crate::__private::Embedded = $crate::__private::Embedded::new(
                ::core::include!(::core::concat!(
                    ::core::env!("OUT_DIR"),
                    "/fairway-skill/namespaces.rs"
                )),
                ::core::module_path!(),
                ::core::concat!(
                    ::core::module_path!(),
                    " (",
                    ::core::file!(),
                    ":",
                    ::core::line!(),
                    ")"
                ),
            );
        };
    };
}

/// Macro support, not part of the plugin API.
#[doc(hidden)]
pub mod __private {
    pub use crate::registry::{
        Embedded, FAIRWAY_SKILL_EMBEDDED, FAIRWAY_SKILL_NAMESPACES, Registration,
    };
    pub use fairway_catalog as catalog;
    pub use linkme;
}
