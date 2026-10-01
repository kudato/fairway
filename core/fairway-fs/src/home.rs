use std::{io, path::PathBuf, sync::OnceLock};

use crate::{Error, Operation};

/// The Fairway directory, or the error that prevented determining it; set on
/// first use and never changed afterwards.
static HOME: OnceLock<Result<PathBuf, Error>> = OnceLock::new();

/// Returns the Fairway directory, determining it on the first call.
pub(crate) fn resolved() -> Result<PathBuf, Error> {
    HOME.get_or_init(resolve).clone()
}

/// Determines the Fairway directory from `FAIRWAY_HOME` or the user's home
/// directory.
fn resolve() -> Result<PathBuf, Error> {
    let path = match std::env::var_os("FAIRWAY_HOME") {
        Some(value) if value.is_empty() => {
            return Err(Error::message(
                Operation::ResolveHome,
                None,
                io::ErrorKind::InvalidInput,
                "FAIRWAY_HOME is empty",
            ));
        }
        Some(value) => PathBuf::from(value),
        None => std::env::home_dir()
            .ok_or_else(|| {
                Error::message(
                    Operation::ResolveHome,
                    None,
                    io::ErrorKind::NotFound,
                    "cannot determine the user's home directory",
                )
            })?
            .join(".fairway"),
    };
    super::absolute(&path)
}

/// Implements [`crate::__private::initialize`].
pub(crate) fn initialize() -> Result<(), Error> {
    resolved().map(|_| ())
}

/// Returns the path of the Fairway directory.
///
/// The Fairway directory holds Fairway's configuration and other state; a
/// plugin can keep its own data in a subdirectory named after the plugin. The
/// path is the value of the `FAIRWAY_HOME` environment variable, resolved
/// against the working directory if it is relative, or `.fairway` in the
/// user's home directory if the variable is not set. It is always absolute.
///
/// The path is determined once per process and does not change afterwards,
/// even if the environment does. The directory itself is not created and may
/// not exist yet.
///
/// # Errors
///
/// Returns an error of [`Operation::ResolveHome`] if `FAIRWAY_HOME` is set but
/// empty ([`InvalidInput`](io::ErrorKind::InvalidInput)), or if it is not set
/// and the user's home directory cannot be determined
/// ([`NotFound`](io::ErrorKind::NotFound)). Because the path is determined
/// only once, every later call returns the same error. Inside Fairway, `home`
/// does not fail: Fairway determines the path at startup and does not start
/// if it cannot.
///
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// let data = fairway_fs::home().await?.join("my-plugin");
/// fairway_fs::mkdir(&data).await?;
/// fairway_fs::write(data.join("state.json"), "{}".to_owned()).await?;
/// # Ok(())
/// # }
/// ```
pub async fn home() -> Result<PathBuf, Error> {
    super::blocking(Operation::ResolveHome, None, resolved).await
}
