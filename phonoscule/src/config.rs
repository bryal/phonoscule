//! An optional TOML config file, shared by the phonoscule players on a machine.
//!
//! Nothing else in the framework depends on this: it is one convenient way to obtain settings, not
//! the way. A player that has no filesystem (or keeps its settings in NVS, or a registry) skips this
//! module and passes the same values in directly.
//!
//! Settings every player needs -- so far just [`music_dir`](Conf::music_dir) -- sit at the top
//! level. Each player's own settings live in its `[app.<name>]` table, read with
//! [`Conf::app_float`] and friends; the framework never looks inside `app`, whose subtables belong
//! to the applications, this repository's or anyone else's.

use anyhow::Context as _;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Conf {
    /// Where the settings were read from (kept for future format-preserving saving).
    path: PathBuf,
    /// The application these were loaded for, naming its `[app.<name>]` table.
    app: String,
    doc: toml_edit::DocumentMut,
    /// Path to the music library, from `music-dir`, defaulting to `~/Music`. `~` and environment
    /// variables are expanded, and a relative path resolves against the config file's own directory
    /// rather than the process's working directory.
    pub music_dir: PathBuf,
}

/// Loads the config for the player named `app` (lowercase, e.g. `"gui"`), from the first of:
/// `path`, `$PHONOSCULE_<APP>_CONF`, `$XDG_CONFIG_HOME/phonoscule.toml`, `~/.config/phonoscule.toml`.
///
/// A file asked for explicitly -- by `path` or the environment -- must exist; the default location
/// may be absent, which just means default settings.
pub async fn load(app: &str, path: Option<PathBuf>) -> anyhow::Result<Conf> {
    match locate(app, path) {
        (path, Required::Yes) => Conf::open(app, &path).await.with_context(|| format!("failed to read config from {path:?}")),
        (path, Required::No) => match Conf::open(app, &path).await {
            Ok(conf) => Ok(conf),
            Err(e) if is_not_found(&e) => {
                log::info!("no config file at {path:?}, using default settings");
                Ok(Conf { path, app: app.to_string(), doc: toml_edit::DocumentMut::new(), music_dir: default_music_dir() })
            }
            Err(e) => Err(e).with_context(|| format!("failed to read config from {path:?}")),
        },
    }
}

/// Whether a missing config file at this location is an error.
enum Required {
    Yes,
    No,
}

/// The config file to read, and whether it is allowed to be missing.
fn locate(app: &str, arg: Option<PathBuf>) -> (PathBuf, Required) {
    let env = |var: &str| std::env::var(var).ok().filter(|s| !s.is_empty());
    if let Some(path) = arg {
        return (path, Required::Yes);
    }
    // Per-player rather than one shared variable, so pointing two players at different files takes
    // no more than setting one of them.
    if let Some(path) = env(&env_var(app)) {
        return (PathBuf::from(path), Required::Yes);
    }
    let config_home =
        env("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join(".config"));
    (config_home.join("phonoscule.toml"), Required::No)
}

/// The environment variable naming `app`'s config file, e.g. `PHONOSCULE_GUI_CONF`.
fn env_var(app: &str) -> String {
    format!("PHONOSCULE_{}_CONF", app.to_uppercase())
}

/// The music directory used when there is no config file at all.
fn default_music_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_default().join("Music")
}

impl Conf {
    async fn open(app: &str, path: &Path) -> anyhow::Result<Self> {
        log::debug!("reading conf from: {}", path.display());
        let path = path.canonicalize()?;
        let src = smol::fs::read_to_string(&path).await?;
        let doc: toml_edit::DocumentMut = src.parse()?;

        let item = doc.get("music-dir").context("missing key `music-dir` in config")?;
        let value = item.as_str().context("config key `music-dir` should be a string")?;
        let mut music_dir = PathBuf::from(shellexpand::full(value)?.as_ref());
        if music_dir.is_relative()
            && let Some(dir) = path.parent()
        {
            music_dir = dir.join(&music_dir);
        }
        Ok(Self { path, app: app.to_string(), doc, music_dir })
    }

    /// The file these settings came from -- the default location, when there was no file to read.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A number from this player's `[app.<name>]` table, accepting an integer or a float so both
    /// `key = 2` and `key = 1.25` work. `None` when the table or the key is absent, leaving the
    /// caller's default to apply; an error only when the key is there but is not a number.
    pub fn app_float(&self, key: &str) -> anyhow::Result<Option<f32>> {
        let Some(item) = self.app_setting(key) else { return Ok(None) };
        let n = item.as_float().or_else(|| item.as_integer().map(|i| i as f64));
        let n = n.with_context(|| format!("config key `{}` should be a number", self.app_key(key)))?;
        Ok(Some(n as f32))
    }

    /// A string from this player's `[app.<name>]` table. `None` when the table or the key is absent.
    pub fn app_str(&self, key: &str) -> anyhow::Result<Option<&str>> {
        let Some(item) = self.app_setting(key) else { return Ok(None) };
        let s = item.as_str().with_context(|| format!("config key `{}` should be a string", self.app_key(key)))?;
        Ok(Some(s))
    }

    fn app_setting(&self, key: &str) -> Option<&toml_edit::Item> {
        self.doc.get("app")?.as_table_like()?.get(&self.app)?.as_table_like()?.get(key)
    }

    /// A setting's dotted name, for error messages.
    fn app_key(&self, key: &str) -> String {
        format!("app.{}.{key}", self.app)
    }
}

fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

/// Help describing the config file for the player named `app`, for its `--help`: where the file is
/// looked for, and the settings understood here. The player appends its own `[app.<name>]` ones.
pub fn config_help(app: &str) -> String {
    format!(
        "\
Configuration file:
  A TOML file, looked for in this order: the CONFIG path above, then
  ${var}, then $XDG_CONFIG_HOME/phonoscule.toml, then
  ~/.config/phonoscule.toml. If no file is found, default values are used.

  Settings:
    music-dir  Path to the music library. Required once a config file exists
               (without one it defaults to ~/Music). `~` and environment
               variables are expanded; a relative path resolves against the
               config file's own directory.

  Settings under [app.{app}] are this player's own. Other phonoscule players
  read their own [app.<name>] tables from the same file, so one file can
  configure them all.
",
        var = env_var(app),
    )
}

#[cfg(test)]
mod test {
    use super::*;

    /// Parses a config from a string by writing it to a temporary file, since resolving the music
    /// directory depends on where the file itself lives. One file per case, so the tests can share a
    /// directory without racing.
    fn parse(case: &str, src: &str) -> anyhow::Result<Conf> {
        let dir = std::env::temp_dir().join(format!("phonoscule-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{case}.toml"));
        std::fs::write(&path, src).unwrap();
        smol::block_on(Conf::open("gui", &path))
    }

    #[test]
    fn parse_music_dir() {
        assert_eq!(
            parse("music-dir", "music-dir = \"/somewhere/else\"\n").unwrap().music_dir,
            PathBuf::from("/somewhere/else")
        );
    }

    #[test]
    fn parse_expands_tilde_and_env() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(parse("tilde", "music-dir = \"~/Transcoded\"").unwrap().music_dir, home.join("Transcoded"));
        assert_eq!(parse("env", "music-dir = \"$HOME/Transcoded\"").unwrap().music_dir, home.join("Transcoded"));
    }

    #[test]
    fn relative_music_dir_is_relative_to_config_file() {
        let conf = parse("relative", "music-dir = \"./Library\"").unwrap();
        assert_eq!(conf.music_dir, conf.path().parent().unwrap().join("./Library"));
    }

    #[test]
    fn parse_errors_are_descriptive() {
        assert!(parse("empty", "").unwrap_err().to_string().contains("missing key `music-dir`"));
        assert!(parse("not-a-string", "music-dir = 7").unwrap_err().to_string().contains("should be a string"));
    }

    /// A player reads its own table; an absent table or key leaves its default in place, and only a
    /// present-but-wrongly-typed value is an error.
    #[test]
    fn app_settings_come_from_the_players_own_table() {
        let conf = parse("app-absent", "music-dir = \"/m\"").unwrap();
        assert_eq!(conf.app_float("scaling").unwrap(), None, "no [app] table at all");

        let conf = parse("app-other", "music-dir = \"/m\"\n[app.tui]\nscaling = 3").unwrap();
        assert_eq!(conf.app_float("scaling").unwrap(), None, "another player's table is not ours");

        let conf = parse("app-gui", "music-dir = \"/m\"\n[app.gui]\nscaling = 1.25\nprotocol = \"kitty\"").unwrap();
        assert_eq!(conf.app_float("scaling").unwrap(), Some(1.25));
        assert_eq!(conf.app_str("protocol").unwrap(), Some("kitty"));

        let conf = parse("app-bad", "music-dir = \"/m\"\n[app.gui]\nscaling = \"big\"").unwrap();
        let err = conf.app_float("scaling").unwrap_err().to_string();
        assert!(err.contains("`app.gui.scaling` should be a number"), "{err}");
    }

    /// Whatever another player puts in its own table, we neither read nor validate it.
    #[test]
    fn other_players_tables_are_left_alone() {
        let src = "music-dir = \"/m\"\n[app.someoneelses]\nwhatever = { nested = [1, \"two\"] }";
        assert!(parse("third-party", src).is_ok());
    }
}
