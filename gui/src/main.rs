//! Phonoscule GUI: an album-focused music player.
//!
//! Two views: a library browser (play or queue whole albums) and an iPod-style Cover Flow of the
//! play queue with a seekable playback bar. Follows the model/update/view architecture; this file
//! only boots the application and wires up its event sources.

mod model;
mod update;
mod view;

use model::{App, View, boot, flow_target};
use phonoscule_gui::conf::{self, Conf};
use smol::channel;
use std::path::PathBuf;
use std::time::Duration;
use update::{Msg, update};
use view::{style, theme, view};

use iced::Subscription;

fn main() -> anyhow::Result<()> {
    simple_logger::SimpleLogger::new().with_level(log::LevelFilter::Info).env().init().unwrap();

    let mut args = std::env::args().skip(1);
    let arg_conf_path = args.next().map(PathBuf::from);
    anyhow::ensure!(args.next().is_none(), "expected at most one argument: a path to a config file");
    let conf = smol::block_on(Conf::load(conf::locate(arg_conf_path)))?;

    iced::application(boot(conf), update, view)
        .title("Phonoscule")
        .subscription(subscription)
        .theme(theme)
        .style(style)
        .run()?;
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

/// How often the music directory is polled for changes. Unchanged files are never re-read (the
/// tag cache is validated by stat data), so a quiet poll costs directory listings and stats.
const RESCAN_INTERVAL: Duration = Duration::from_secs(30);

fn subscription(app: &App) -> Subscription<Msg> {
    let player = channel_subscription("player-events", app.engine.events.clone()).map(Msg::Player);
    let media = channel_subscription("media-events", app.media.events.clone()).map(Msg::Media);
    let rescan = iced::time::every(RESCAN_INTERVAL).map(|_| Msg::Rescan);

    let animating = app.view == View::NowPlaying && app.anim_pos != flow_target(app);
    let frames = if animating {
        iced::time::every(Duration::from_millis(16)).map(Msg::Frame)
    } else {
        Subscription::none()
    };

    Subscription::batch([player, media, rescan, frames])
}
