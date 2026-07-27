//! Phonoscule GUI: an album-focused music player.
//!
//! Two views: a library browser (play or queue whole albums) and an iPod-style Cover Flow of the
//! play queue with a seekable playback bar. Follows the model/update/view architecture; this file
//! only boots the application and wires up its event sources.

// A graphical program, so on Windows it is linked as a subsystem application: starting it from
// Explorer, a shortcut or the Start menu must not conjure a console window alongside the player.
// That costs the standard handles, which [`console::prepare`] sorts out before anything is printed -
// including adopting the console of a shell that did start it, so a terminal run is unchanged.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod album_grid;
mod background;
mod console;
mod coverflow;
mod model;
mod paths;
mod update;
mod view;

use model::{App, Modal, View, boot, flow_target, glow_animating};
use phonoscule::library;
use phonoscule::{config, session, watcher};
use smol::channel;
use std::path::PathBuf;
use std::time::Duration;
use update::{Msg, key_to_msg, update};
use view::{style, theme, view};

use iced::Subscription;
use iced::keyboard;

/// All fonts are embedded and pinned, so the application looks the same everywhere without the
/// user installing anything: Iosevka for text, Font Awesome for symbols & icons (which would
/// otherwise be at the mercy of the system's fallback fonts -- colored emoji and all).
static FONTS_DATA: &[&[u8]] = &[
    include_bytes!("../assets/font-iosevka/IosevkaFixedSS05-ExtraLight.ttf"),
    include_bytes!("../assets/font-iosevka/IosevkaFixedSS05-ExtraLightItalic.ttf"),
    include_bytes!("../assets/font-iosevka/IosevkaFixedSS05-Regular.ttf"),
    include_bytes!("../assets/font-iosevka/IosevkaFixedSS05-Italic.ttf"),
    include_bytes!("../assets/font-iosevka/IosevkaFixedSS05-SemiBold.ttf"),
    include_bytes!("../assets/font-iosevka/IosevkaFixedSS05-SemiBoldItalic.ttf"),
    include_bytes!("../assets/font-iosevka/IosevkaFixedSS05-ExtraBold.ttf"),
    include_bytes!("../assets/font-iosevka/IosevkaFixedSS05-ExtraBoldItalic.ttf"),
    include_bytes!("../assets/font-awesome/Font Awesome 7 Free-Solid-900.otf"),
];

/// The name this player goes by: its `[app.gui]` config table and its `$PHONOSCULE_GUI_CONF`.
const APP: &str = "gui";

/// The window icon: the same mark the executable itself carries as a resource (see `build.rs`).
///
/// Both are needed, and they cover different surfaces. This one is what the window system asks the
/// running process for - the title bar on Windows (winit sets only `ICON_SMALL` from it) and the
/// window on X11 and Wayland. The executable's resource is what Windows reads off disk for the file
/// in Explorer, and what the taskbar and Alt-Tab fall back to, `ICON_BIG` being left unset. Between
/// them there is nowhere the icon is missing.
///
/// Only the largest form is embedded, and the window system scales it: it is a couple of hundred
/// times smaller than the fonts already in here, and one image beats keeping several in step.
/// Rendered from the artwork at build time, hence `OUT_DIR` rather than a path into `assets`.
static ICON_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/window-icon.png"));

/// The decoded icon, or `None` if it will not decode - which costs a generic window icon and
/// nothing else, so it is not worth refusing to start over.
fn window_icon() -> Option<iced::window::Icon> {
    match iced::window::icon::from_file_data(ICON_DATA, Some(image::ImageFormat::Png)) {
        Ok(icon) => Some(icon),
        Err(e) => {
            log::warn!("could not decode the window icon: {e}");
            None
        }
    }
}

/// Range the UI scale factor is clamped to -- both the configured `scaling` and the live Ctrl +/-
/// zoom (see [`Zoom`](update::Zoom)). Wide enough to be useful, narrow enough to stay usable.
pub const SCALE_MIN: f32 = 0.5;
pub const SCALE_MAX: f32 = 3.0;

/// `--help` text: what the program is and how to run it. [`help`] follows it with the config
/// sections.
const HELP: &str = "\
Phonoscule: an album-art-focused music player with an iPod-style Cover Flow.

Usage: phonoscule-gui [CONFIG]

Arguments:
  CONFIG         (optional) Path to a config file.

Options:
  -h, --help     Print this help and exit.
  -V, --version  Print version information and exit.

";

/// This player's own config settings, listed after the shared ones. Keep the range in step with
/// [`SCALE_MIN`] / [`SCALE_MAX`].
const CONFIG_HELP_GUI: &str = "
  [app.gui]
    scaling    UI scale factor for high-DPI displays: 1.0 is unscaled, larger is
               bigger. Optional, default 1.0, clamped to 0.5 to 3.0.
";

fn help() -> String {
    format!("{HELP}{}{CONFIG_HELP_GUI}", config::config_help(APP))
}

fn main() {
    // Before anything is printed, the first log line included: settles whether there is anywhere for
    // output to go at all (see the `console` module).
    let output = console::prepare();

    // Any startup failure -- a bad argument, an unreadable config, a failed window init -- prints
    // the help after it, so a misuse points the user at how to run the program and where its
    // config lives.
    if let Err(e) = run() {
        match output {
            console::Output::Live => {
                eprintln!("Error: {e:?}\n");
                eprint!("{}", help());
            }
            // Started from a graphical shell, where that help would go to NUL and the window would
            // simply never appear. Say what went wrong in the one place it can be seen, and point at
            // a terminal for the rest rather than fitting a page of help into a dialog.
            console::Output::Discarded => console::alert(
                "Phonoscule",
                &format!("{e:?}\n\nRun `phonoscule-gui --help` in a terminal for usage, and for where the config file lives."),
            ),
        }
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    // Deliberately tiny hand-rolled argument handling (no dependency): the two flags, and at most
    // one positional -- a config path. We do not intend to grow a real flag set.
    let mut arg_conf_path = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{}", help());
                return Ok(());
            }
            "-V" | "--version" => {
                println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ if arg.starts_with('-') => anyhow::bail!("unknown option `{arg}`"),
            _ if arg_conf_path.is_none() => arg_conf_path = Some(PathBuf::from(arg)),
            _ => anyhow::bail!("expected at most one argument: a path to a config file"),
        }
    }

    simple_logger::SimpleLogger::new().with_level(log::LevelFilter::Info).env().init().unwrap();

    let conf = smol::block_on(config::load(APP, arg_conf_path))?;
    // `scaling` is ours alone -- a terminal player can't scale -- so it lives in our own config
    // table. Out-of-range values are clamped rather than rejected.
    let scaling = conf.app_float("scaling")?.unwrap_or(1.0).clamp(SCALE_MIN, SCALE_MAX);
    let restored = smol::block_on(session::load(paths::playlist_file(), paths::player_file()));
    let index = smol::block_on(library::load_index(paths::album_index_file()));

    let app = iced::application(boot(conf, scaling, restored, index), update, view)
        .title("Phonoscule")
        .window(iced::window::Settings { icon: window_icon(), ..Default::default() })
        .subscription(subscription)
        .scale_factor(|app| app.scale)
        .theme(theme)
        .style(style)
        .default_font(iced::Font { family: iced::font::Family::Name("Iosevka Fixed SS05"), ..iced::Font::DEFAULT });
    FONTS_DATA.iter().fold(app, |app, font| app.font(*font)).run()?;
    Ok(())
}

/// A [`Subscription`] yielding everything received on the channel. The tag (not the channel)
/// identifies the subscription across [`subscription`] calls, so use a unique one per channel.
fn channel_subscription<T: Send + 'static>(tag: &'static str, rx: channel::Receiver<T>) -> Subscription<T> {
    struct Tagged<T>(&'static str, channel::Receiver<T>);
    impl<T> std::hash::Hash for Tagged<T> {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            self.0.hash(state)
        }
    }
    Subscription::run_with(Tagged(tag, rx), |tagged| tagged.1.clone())
}

/// How often the music directory is polled for changes, as the fallback behind the filesystem watcher.
/// Unchanged files are never re-read (the tag cache is validated by stat data),
/// so a quiet poll costs directory listings and stats.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Debounces the watcher's raw change events into one `Msg::Rescan` per settled burst, driven on
/// iced's own executor. `run_with` keeps a stable identity (the tag) and (re)builds the debounce
/// stream from the raw receiver and quiet period.
fn watch_subscription(app: &App) -> Subscription<Msg> {
    struct Debounce(&'static str, channel::Receiver<()>, Duration);
    impl std::hash::Hash for Debounce {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            self.0.hash(state)
        }
    }
    let (raw, quiet) = app.watcher.change_source();
    Subscription::run_with(Debounce("watch-events", raw, quiet), |d| watcher::debounce(d.1.clone(), d.2)).map(|()| Msg::Rescan)
}

fn subscription(app: &App) -> Subscription<Msg> {
    let player = channel_subscription("player-events", app.engine.events.clone()).map(Msg::Player);
    let media = channel_subscription("media-events", app.media.events.clone()).map(Msg::Media);
    let mixer = channel_subscription("volume-events", app.mixer.events.clone()).map(Msg::VolumeChanged);
    let watch = watch_subscription(app);
    let rescan = iced::time::every(RESCAN_INTERVAL).map(|_| Msg::Rescan);

    let animating = (app.view == View::Player && app.anim_pos != flow_target(app)) || glow_animating(app);
    let frames = if animating { iced::time::every(Duration::from_millis(16)).map(Msg::Frame) } else { Subscription::none() };

    // `listen` yields only keyboard events a focused widget ignored, so shortcuts never shadow a
    // widget's own key handling (e.g. arrow keys while the seek bar has focus). Bindings depend on
    // the active view and which modal is up; subscription closures must be non-capturing, so pair
    // that state in with `with` (it hashes the value, so the subscription rebuilds whenever it
    // changes and never goes stale).
    let keys =
        keyboard::listen().with((app.view, app.modal.as_ref().map(Modal::kind))).filter_map(
            |((view, modal), event)| match event {
                keyboard::Event::KeyPressed { key, modified_key, modifiers, repeat, .. } => {
                    key_to_msg(view, modal, key, modified_key, modifiers, repeat)
                }
                _ => None,
            },
        );

    Subscription::batch([player, media, mixer, watch, rescan, frames, keys])
}
