//! Where this player keeps its files.
//!
//! Its own directories, not shared ones: the framework takes paths rather than deciding them, so
//! another phonoscule player on the same machine has its own state and caches and cannot overwrite
//! ours. (Pointing two of them at one directory is then a deliberate act, not the default.)

use phonoscule::library;
use std::path::PathBuf;

/// The name our directories go by, under the XDG roots.
const DIR: &str = "phonoscule";

/// State survives a cache wipe: `$XDG_STATE_HOME/phonoscule`, falling back to
/// `~/.local/state/phonoscule`. `None` when neither is determinable, which means we don't persist.
fn state_dir() -> Option<PathBuf> {
    Some(xdg_dir("XDG_STATE_HOME", ".local/state")?.join(DIR))
}

/// Regenerable caches: `$XDG_CACHE_HOME/phonoscule`, falling back to `~/.cache/phonoscule`.
fn cache_dir() -> Option<PathBuf> {
    Some(xdg_dir("XDG_CACHE_HOME", ".cache")?.join(DIR))
}

fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var(var).ok().filter(|s| !s.is_empty()).map(PathBuf::from).or_else(|| Some(std::env::home_dir()?.join(fallback)))
}

/// The play queue: just its list of tracks.
pub fn playlist_file() -> Option<PathBuf> {
    Some(state_dir()?.join("playlist.json"))
}

/// The session state around the queue: current track, repeat mode, sort order.
pub fn player_file() -> Option<PathBuf> {
    Some(state_dir()?.join("player.json"))
}

/// The tag cache, so a rescan only opens files that changed.
pub fn tag_cache_file() -> Option<PathBuf> {
    Some(cache_dir()?.join("library.json"))
}

/// The album index, so a launch shows the whole library before the scan finishes.
pub fn album_index_file() -> Option<PathBuf> {
    Some(cache_dir()?.join("albums.json"))
}

/// The decoded cover thumbnails.
pub fn covers_dir() -> Option<PathBuf> {
    Some(library::covers_dir(&cache_dir()?))
}
