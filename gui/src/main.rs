//! Phonoscule GUI: an album-focused music player.
//!
//! Two views: a library browser (play or queue whole albums) and an iPod-style Cover Flow of the
//! play queue with a seekable playback bar. Follows the model/update/view architecture; this file
//! only boots the application and wires up its event sources.

mod model;
mod update;
mod view;

use model::{App, Modal, View, boot, flow_target, glow_animating};
use phonoscule::library;
use phonoscule::watcher;
use phonoscule_gui::conf::{self, Conf};
use phonoscule_gui::playlist;
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

/// `--help` text: what the program is and how to run it. The configuration section is printed after
/// it from [`conf::CONFIG_HELP`], which lives next to the parser it documents.
const HELP: &str = "\
Phonoscule: an album-art-focused music player with an iPod-style Cover Flow.

Usage: phonoscule-gui [CONFIG]

Arguments:
  CONFIG         (optional) Path to a config file.

Options:
  -h, --help     Print this help and exit.
  -V, --version  Print version information and exit.

";

fn main() {
    // Any startup failure -- a bad argument, an unreadable config, a failed window init -- prints
    // the help after it, so a misuse points the user at how to run the program and where its
    // config lives.
    if let Err(e) = run() {
        eprintln!("Error: {e:?}\n");
        eprint!("{HELP}{}", conf::CONFIG_HELP);
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
                print!("{HELP}{}", conf::CONFIG_HELP);
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

    let conf = smol::block_on(Conf::load(conf::locate(arg_conf_path)))?;
    let restored = smol::block_on(playlist::load(playlist::playlist_file(), playlist::player_file()));
    let index = smol::block_on(library::load_index(library::default_index_file()));

    let app = iced::application(boot(conf, restored, index), update, view)
        .title("Phonoscule")
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
