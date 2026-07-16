//! Phonoscule GUI: an album-focused music player.
//!
//! Two views: a library browser (play or queue whole albums) and an iPod-style Cover Flow of the
//! play queue with a seekable playback bar. Follows the model/update/view architecture; this file
//! only boots the application and wires up its event sources.

mod model;
mod update;
mod view;

use model::{App, View, boot, flow_target, glow_animating};
use phonoscule_gui::conf::{self, Conf};
use phonoscule_gui::watcher;
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

fn main() -> anyhow::Result<()> {
    simple_logger::SimpleLogger::new().with_level(log::LevelFilter::Info).env().init().unwrap();

    let mut args = std::env::args().skip(1);
    let arg_conf_path = args.next().map(PathBuf::from);
    anyhow::ensure!(args.next().is_none(), "expected at most one argument: a path to a config file");
    let conf = smol::block_on(Conf::load(conf::locate(arg_conf_path)))?;

    let app = iced::application(boot(conf), update, view)
        .title("Phonoscule")
        .subscription(subscription)
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

/// How often the music directory is polled for changes, as the fallback behind the filesystem
/// watcher. Unchanged files are never re-read (the tag cache is validated by stat data), so a
/// quiet poll costs directory listings and stats.
const RESCAN_INTERVAL: Duration = Duration::from_secs(30);

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
    let watch = watch_subscription(app);
    let rescan = iced::time::every(RESCAN_INTERVAL).map(|_| Msg::Rescan);

    let animating = (app.view == View::NowPlaying && app.anim_pos != flow_target(app)) || glow_animating(app);
    let frames = if animating { iced::time::every(Duration::from_millis(16)).map(Msg::Frame) } else { Subscription::none() };

    // While a metadata push is pending (throttled out), tick once a second to flush it -- so the
    // final track after a burst still reaches MPRIS. Idle otherwise.
    let media_sync =
        if app.media_dirty { iced::time::every(Duration::from_secs(1)).map(|_| Msg::SyncMedia) } else { Subscription::none() };

    // `listen` yields only keyboard events a focused widget ignored, so shortcuts never shadow a
    // widget's own key handling (e.g. arrow keys while the seek bar has focus).
    let keys = keyboard::listen().filter_map(|event| match event {
        keyboard::Event::KeyPressed { key, modifiers, repeat, .. } => key_to_msg(key, modifiers, repeat),
        _ => None,
    });

    Subscription::batch([player, media, watch, rescan, frames, media_sync, keys])
}
