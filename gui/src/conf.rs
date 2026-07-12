//! Config file support.
//!
//! The config is a TOML file located by [`locate`]: an explicit path (command line argument or
//! `$PHONOSCULE_CONFIG`) is required to exist, while the default location
//! (`$XDG_CONFIG_HOME/phonoscule.toml`, falling back to `~/.config/phonoscule.toml`) may be
//! absent, in which case default settings are used.

use anyhow::Context as _;
use std::ops::Deref;
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Debug)]
pub struct Conf {
    /// Where the settings were read from (kept for future format-preserving saving).
    path: PathBuf,
    settings: Settings,
}

impl Deref for Conf {
    type Target = Settings;
    fn deref(&self) -> &Settings {
        &self.settings
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub music_dir: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Self { music_dir: std::env::home_dir().unwrap_or_default().join("Music") }
    }
}

/// Where the config file should be read from, and whether it is allowed to be missing.
pub enum Location {
    /// Explicitly requested (command line argument or `$PHONOSCULE_CONFIG`): must exist.
    Explicit(PathBuf),
    /// The default, XDG-derived location: missing just means default settings.
    Default(PathBuf),
}

pub fn locate(arg: Option<PathBuf>) -> Location {
    let env = |var: &str| std::env::var(var).ok().filter(|s| !s.is_empty());
    if let Some(path) = arg.or_else(|| env("PHONOSCULE_CONFIG").map(PathBuf::from)) {
        return Location::Explicit(path);
    }
    let config_home = env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join(".config"));
    Location::Default(config_home.join("phonoscule.toml"))
}

impl Conf {
    pub async fn load(location: Location) -> anyhow::Result<Self> {
        match location {
            Location::Explicit(path) => {
                Self::open(&path).await.with_context(|| format!("failed to read config from {path:?}"))
            }
            Location::Default(path) => match Self::open(&path).await {
                Ok(conf) => Ok(conf),
                Err(e) if is_not_found(&e) => {
                    log::info!("no config file at {path:?}, using default settings");
                    Ok(Self { path, settings: Settings::default() })
                }
                Err(e) => Err(e).with_context(|| format!("failed to read config from {path:?}")),
            },
        }
    }

    async fn open(path: &Path) -> anyhow::Result<Self> {
        log::debug!("reading conf from: {}", path.display());
        let path = path.canonicalize()?;
        let src = smol::fs::read_to_string(&path).await?;
        let mut settings = Settings::parse(&src)?;
        // Relative paths in the config are relative to the config file, not to the (arbitrary)
        // working directory of the process.
        if settings.music_dir.is_relative()
            && let Some(dir) = path.parent()
        {
            settings.music_dir = dir.join(&settings.music_dir);
        }
        Ok(Self { path, settings })
    }
}

fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

impl Settings {
    fn parse(src: &str) -> anyhow::Result<Self> {
        let toml: toml_edit::DocumentMut = src.parse()?;

        let music_dir_item = toml.get("music-dir").context("missing key `music-dir` in config")?;
        let music_dir_str = music_dir_item.as_str().context("config key `music-dir` should be a string")?;
        let music_dir = PathBuf::from(shellexpand::full(music_dir_str)?.as_ref());

        Ok(Self { music_dir })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_music_dir() {
        let s = Settings::parse("music-dir = \"/somewhere/else\"\n").unwrap();
        assert_eq!(s.music_dir, PathBuf::from("/somewhere/else"));
    }

    #[test]
    fn parse_expands_tilde_and_env() {
        let home = std::env::home_dir().unwrap();
        let s = Settings::parse("music-dir = \"~/Transcoded\"").unwrap();
        assert_eq!(s.music_dir, home.join("Transcoded"));
        let s = Settings::parse("music-dir = \"$HOME/Transcoded\"").unwrap();
        assert_eq!(s.music_dir, home.join("Transcoded"));
    }

    #[test]
    fn relative_music_dir_is_relative_to_config_file() {
        let dir = std::env::temp_dir().join("phonoscule-conf-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("conf.toml");
        std::fs::write(&path, "music-dir = \"./Library\"").unwrap();
        let conf = smol::block_on(Conf::open(&path)).unwrap();
        assert_eq!(conf.music_dir, dir.canonicalize().unwrap().join("./Library"));
    }

    #[test]
    fn parse_errors_are_descriptive() {
        assert!(Settings::parse("").unwrap_err().to_string().contains("missing key `music-dir`"));
        assert!(Settings::parse("music-dir = 7").unwrap_err().to_string().contains("should be a string"));
    }
}
