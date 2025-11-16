use std::path::PathBuf;

/// Returns the user's home directory using common environment variables.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("MICROFACTORY_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
        })
}

/// Returns the default path to ~/.env (or $MICROFACTORY_HOME/.env if set).
pub fn home_env_path() -> Option<PathBuf> {
    home_dir().map(|mut dir| {
        dir.push(".env");
        dir
    })
}

/// Returns the data directory where session state will be persisted.
pub fn data_dir() -> PathBuf {
    if let Some(mut dir) = home_dir() {
        dir.push(".microfactory");
        dir
    } else {
        PathBuf::from(".microfactory")
    }
}
