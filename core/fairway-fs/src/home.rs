use std::{io, path::PathBuf, sync::OnceLock};

static HOME: OnceLock<Result<PathBuf, (io::ErrorKind, String)>> = OnceLock::new();

pub(crate) fn resolved() -> io::Result<PathBuf> {
    match HOME.get_or_init(|| resolve().map_err(|error| (error.kind(), error.to_string()))) {
        Ok(path) => Ok(path.clone()),
        Err((kind, message)) => Err(io::Error::new(*kind, message.clone())),
    }
}

fn resolve() -> io::Result<PathBuf> {
    let path = match std::env::var_os("FAIRWAY_HOME") {
        Some(value) if value.is_empty() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "FAIRWAY_HOME is empty",
            ));
        }
        Some(value) => PathBuf::from(value),
        None => std::env::home_dir()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "cannot determine the user's home directory",
                )
            })?
            .join(".fairway"),
    };
    super::absolute(&path)
}

pub(crate) fn initialize() -> io::Result<()> {
    let locks = resolved()?.join("locks");
    if locks.try_exists()? {
        super::lock::clean_stale(&locks)?;
    }
    Ok(())
}

/// Returns the fixed Fairway home path without creating it.
pub async fn home() -> io::Result<PathBuf> {
    super::blocking(resolved).await
}
