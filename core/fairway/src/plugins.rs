//! Plugins linked into the application.
//!
//! Plugins register their commands and settings at link time, and the
//! application never refers to them by name. Add a `use <plugin> as _;` line
//! for each plugin so that it ends up in the executable.
//!
//! Application tests replace this file with their own plugin list and reuse
//! the rest of the entry point, so keep only these lines here.
