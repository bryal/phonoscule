//! Where this player keeps its files: its own directories, not shared with any other phonoscule
//! player. Point two of them at one directory deliberately if you want that.

use phonoscule::library;
use std::path::PathBuf;

/// The name our directories go by, under the XDG roots.
const DIR: &str = "phonoscule-tui";

/// Regenerable caches: `$XDG_CACHE_HOME/phonoscule-tui`, falling back to `~/.cache/phonoscule-tui`.
fn cache_dir() -> Option<PathBuf> {
    Some(xdg_dir("XDG_CACHE_HOME", ".cache")?.join(DIR))
}

fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var(var).ok().filter(|s| !s.is_empty()).map(PathBuf::from).or_else(|| Some(std::env::home_dir()?.join(fallback)))
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
