//! Where the platform keeps a program's files.
//!
//! The roots and their layout, which is a fact about the OS -- not what a player puts in them, which
//! stays the player's own business: it passes its name, gets its directories, and decides the files
//! inside (see the players' own `paths` modules). The framework itself still takes paths rather than
//! deciding them; this is only here so each player does not carry its own copy of the same
//! platform-shaped guesswork.
//!
//! On Linux the XDG base directories, honouring `$XDG_*_HOME` and falling back to their defaults
//! under `~`. On Windows the two AppData roots: `%APPDATA%` for settings, which is the one that
//! roams with a user, and `%LOCALAPPDATA%` for state and caches, which should not. Windows draws no
//! distinction of its own between state and cache, so they become subdirectories of the player's
//! own -- keeping a cache still something you can delete on its own, as on Linux.
//!
//! Every function returns `None` when the directory cannot be determined at all (no home, no
//! AppData), which callers take as "then we do not persist this".
//!
//! Wants std.

use std::path::PathBuf;

/// State that must survive a cache wipe -- a saved session, say -- for the player named `app`:
/// `$XDG_STATE_HOME/<app>`, or `%LOCALAPPDATA%\<app>\state`.
pub fn state_dir(app: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        Some(local_app_data()?.join(app).join("state"))
    } else {
        Some(xdg("XDG_STATE_HOME", ".local/state")?.join(app))
    }
}

/// Regenerable caches -- thumbnails, a tag cache -- for the player named `app`:
/// `$XDG_CACHE_HOME/<app>`, or `%LOCALAPPDATA%\<app>\cache`.
pub fn cache_dir(app: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        Some(local_app_data()?.join(app).join("cache"))
    } else {
        Some(xdg("XDG_CACHE_HOME", ".cache")?.join(app))
    }
}

/// The directory holding the framework's shared `phonoscule.toml` (see [`config`](crate::config)):
/// `$XDG_CONFIG_HOME`, or `%APPDATA%\phonoscule`.
///
/// Shared, not per-player -- one file serves every player on the machine, each reading its own
/// `[app.<name>]` table -- so unlike the two above this takes no name. On Windows it is a directory
/// of our own rather than the root of `%APPDATA%`, where a loose file has no business being.
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) { Some(app_data()?.join("phonoscule")) } else { xdg("XDG_CONFIG_HOME", ".config") }
}

/// An XDG base directory: the variable if it names one, else its default under the home directory.
/// An empty variable counts as unset, per the specification.
fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    env(var).or_else(|| Some(std::env::home_dir()?.join(fallback)))
}

/// `%APPDATA%`, the roaming one. Falling back to the standard location under the home directory, so
/// a process started without the usual environment still finds it.
fn app_data() -> Option<PathBuf> {
    env("APPDATA").or_else(|| Some(std::env::home_dir()?.join("AppData").join("Roaming")))
}

/// `%LOCALAPPDATA%`, the machine-local one.
fn local_app_data() -> Option<PathBuf> {
    env("LOCALAPPDATA").or_else(|| Some(std::env::home_dir()?.join("AppData").join("Local")))
}

fn env(var: &str) -> Option<PathBuf> {
    std::env::var(var).ok().filter(|s| !s.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod test {
    use super::*;

    /// A cache must be deletable without taking the session with it, on either platform's layout.
    #[test]
    fn state_and_cache_are_separate_directories() {
        let state = state_dir("phonoscule").expect("a home directory");
        let cache = cache_dir("phonoscule").expect("a home directory");
        assert_ne!(state, cache);
        assert!(!state.starts_with(&cache), "wiping the cache would take the state with it");
        assert!(!cache.starts_with(&state), "the cache sits inside the state");
    }

    /// Two players must not share either, or one would overwrite the other's session.
    #[test]
    fn players_get_their_own_directories() {
        assert_ne!(state_dir("phonoscule").unwrap(), state_dir("phonoscule-tui").unwrap());
        assert_ne!(cache_dir("phonoscule").unwrap(), cache_dir("phonoscule-tui").unwrap());
    }

    /// The layout is the platform's, and getting it wrong is the whole failure mode this module
    /// exists to avoid -- a Windows player scattering dotfiles through the user's profile.
    #[test]
    fn the_layout_is_the_platforms_own() {
        let cache = cache_dir("phonoscule").unwrap();
        let config = config_dir().unwrap();
        if cfg!(windows) {
            assert!(cache.starts_with(local_app_data().unwrap()), "{cache:?} is not under LOCALAPPDATA");
            assert!(config.starts_with(app_data().unwrap()), "{config:?} is not under APPDATA");
        } else {
            assert!(cache.ends_with("phonoscule"), "{cache:?} is not the player's own directory");
            assert!(config.ends_with(".config") || env("XDG_CONFIG_HOME").is_some(), "{config:?} is not an XDG root");
        }
    }
}
