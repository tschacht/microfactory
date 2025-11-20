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

/// Returns the ordered list of `.env` paths to probe.
///
/// 1. `$MICROFACTORY_HOME/.env` (if set)
/// 2. `$HOME/.env` or `%USERPROFILE%/.env` as a fallback when the sandbox
///    override does not have its own secrets.
pub fn env_file_candidates() -> Vec<PathBuf> {
    env_file_candidates_from(home_dir(), os_home_dir())
}

fn env_file_candidates_from(primary_home: Option<PathBuf>, fallback_home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(mut home) = primary_home {
        home.push(".env");
        candidates.push(home);
    }
    if let Some(mut fallback) = fallback_home {
        fallback.push(".env");
        if !candidates.iter().any(|existing| existing == &fallback) {
            candidates.push(fallback);
        }
    }
    candidates
}

fn os_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_candidates_prioritize_microfactory_home() {
        let primary = PathBuf::from("micro-home");
        let fallback = PathBuf::from("real-home");
        let candidates = env_file_candidates_from(Some(primary.clone()), Some(fallback.clone()));
        assert_eq!(candidates[0], primary.join(".env"));
        assert_eq!(candidates[1], fallback.join(".env"));
    }

    #[test]
    fn env_candidates_deduplicate_when_paths_match() {
        let home = PathBuf::from("real-home");
        let candidates = env_file_candidates_from(Some(home.clone()), Some(home.clone()));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], home.join(".env"));
    }

    #[test]
    fn env_candidates_handle_missing_primary() {
        let fallback = PathBuf::from("real-home");
        let candidates = env_file_candidates_from(None, Some(fallback.clone()));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], fallback.join(".env"));
    }
}
