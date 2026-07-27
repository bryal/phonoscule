//! The Windows [`media`](crate::media) backend: the System Media Transport Controls -- the
//! now-playing flyout above the volume OSD, the lock-screen card, and the media keys that go with
//! them.
//!
//! Applications talk to [`media`](crate::media), not to this module.
//!
//! The controls are taken from a `MediaPlayer` we create and never play anything through, rather
//! than from a window (`ISystemMediaTransportControlsInterop::GetForWindow`). Two reasons: a player
//! built on this framework need not have a window at all -- the terminal one does not -- and the
//! window route would only hand the controls over once the window exists, where
//! [`media::start`](crate::media::start) wants to know at boot whether there is any integration to
//! be had. Its own command manager is switched off, so the idle player contributes no session of its
//! own and only the controls remain.
//!
//! Wants std and Windows 10 or later.

use crate::media::{Changed, Control, Meta, Playback, Snapshot};
use smol::channel;
use std::time::Duration;
use windows::Foundation::TypedEventHandler;
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType,
    Playback::{MediaPlaybackCommandManager, MediaPlayer},
    PlaybackPositionChangeRequestedEventArgs, SystemMediaTransportControls, SystemMediaTransportControlsButton,
    SystemMediaTransportControlsButtonPressedEventArgs, SystemMediaTransportControlsTimelineProperties,
};
use windows::Storage::Streams::RandomAccessStreamReference;
use windows::core::{HSTRING, Ref};

/// The live controls.
pub struct Server {
    controls: SystemMediaTransportControls,
    /// The controls belong to this player and die with it, so it is held for the session even
    /// though nothing is ever played through it.
    _player: MediaPlayer,
    /// The cover last handed to the display updater, so a position tick does not re-read the file.
    /// Compared by URL, which is what the metadata carries.
    shown_cover: Option<String>,
}

pub fn start(identity: &str, _name: &str, events: channel::Sender<Control>) -> Result<Server, String> {
    build(identity, events).map_err(|e| e.message())
}

fn build(identity: &str, events: channel::Sender<Control>) -> windows::core::Result<Server> {
    let player = MediaPlayer::new()?;
    // Without this the player publishes a session of its own -- an idle one, since we never play
    // through it -- which would show up alongside the controls we are about to configure.
    let manager: MediaPlaybackCommandManager = player.CommandManager()?;
    manager.SetIsEnabled(false)?;

    let controls = player.SystemMediaTransportControls()?;
    controls.SetIsEnabled(true)?;
    // Which buttons the flyout offers, and so which media keys reach us. Mirrors what the MPRIS
    // interface reports as its `Can*` properties.
    controls.SetIsPlayEnabled(true)?;
    controls.SetIsPauseEnabled(true)?;
    controls.SetIsStopEnabled(true)?;
    controls.SetIsNextEnabled(true)?;
    controls.SetIsPreviousEnabled(true)?;

    let updater = controls.DisplayUpdater()?;
    updater.SetType(MediaPlaybackType::Music)?;
    updater.SetAppMediaId(&HSTRING::from(identity))?;
    updater.Update()?;

    // Both handlers run on a WinRT thread pool thread and do nothing but hand the request over: the
    // channel is unbounded, so neither can block the OS's dispatch.
    controls.ButtonPressed(&TypedEventHandler::new({
        let events = events.clone();
        move |_, args: Ref<SystemMediaTransportControlsButtonPressedEventArgs>| {
            if let Some(args) = args.as_ref()
                && let Ok(button) = args.Button()
                && let Some(control) = as_control(button)
            {
                let _ = events.try_send(control);
            }
            Ok(())
        }
    }))?;

    // The flyout's scrubber. MPRIS calls this SetPosition; the shape is the same -- an absolute
    // target within the current track.
    controls.PlaybackPositionChangeRequested(&TypedEventHandler::new(
        move |_, args: Ref<PlaybackPositionChangeRequestedEventArgs>| {
            if let Some(args) = args.as_ref()
                && let Ok(span) = args.RequestedPlaybackPosition()
            {
                // A WinRT TimeSpan is 100-nanosecond units, and a negative one is not a position.
                if let Ok(micros) = u64::try_from(span.Duration / 10) {
                    let _ = events.try_send(Control::SetPosition(Duration::from_micros(micros)));
                }
            }
            Ok(())
        },
    ))?;

    Ok(Server { controls, _player: player, shown_cover: None })
}

/// The transport button as a [`Control`], or `None` for the ones we do not offer (record, channel
/// up/down and friends -- reachable on some remotes even when the flyout does not show them).
fn as_control(button: SystemMediaTransportControlsButton) -> Option<Control> {
    match button {
        SystemMediaTransportControlsButton::Play => Some(Control::Play),
        SystemMediaTransportControlsButton::Pause => Some(Control::Pause),
        SystemMediaTransportControlsButton::Stop => Some(Control::Stop),
        SystemMediaTransportControlsButton::Next => Some(Control::Next),
        SystemMediaTransportControlsButton::Previous => Some(Control::Prev),
        // The single play/pause key on most keyboards arrives as Play or Pause depending on the
        // status we last published, so there is no Toggle to map -- which is why publishing the
        // playback status promptly matters here.
        _ => None,
    }
}

impl Server {
    /// Pushes the snapshot to the controls. Errors are logged and dropped: a desktop widget that
    /// missed an update is not worth interrupting playback over, and the next snapshot will carry
    /// the same state along.
    pub async fn apply(&mut self, snapshot: &Snapshot, changed: Changed) {
        if let Err(e) = self.push(snapshot, changed) {
            log::debug!("could not update the media controls: {}", e.message());
        }
    }

    fn push(&mut self, snapshot: &Snapshot, changed: Changed) -> windows::core::Result<()> {
        if changed.state {
            self.controls.SetPlaybackStatus(match snapshot.state {
                Playback::Playing => MediaPlaybackStatus::Playing,
                Playback::Paused => MediaPlaybackStatus::Paused,
                Playback::Stopped => MediaPlaybackStatus::Stopped,
            })?;
        }
        if changed.meta {
            self.update_display(snapshot.meta.as_ref())?;
        }
        // Every apply, not just on a metadata change: this is what moves the flyout's scrubber, and
        // the position is the part that keeps changing. Cheap next to the display updater.
        self.update_timeline(snapshot)?;
        Ok(())
    }

    /// Title, artist, album and cover -- the card the desktop draws. `Update` commits the lot in
    /// one go, so it is called once at the end rather than per field.
    fn update_display(&mut self, meta: Option<&Meta>) -> windows::core::Result<()> {
        let updater = self.controls.DisplayUpdater()?;
        let Some(meta) = meta else {
            updater.ClearAll()?;
            self.shown_cover = None;
            // ClearAll resets the type as well, and a music card without it renders as unknown
            // media.
            updater.SetType(MediaPlaybackType::Music)?;
            return updater.Update();
        };
        updater.SetType(MediaPlaybackType::Music)?;
        let music = updater.MusicProperties()?;
        music.SetTitle(&HSTRING::from(&meta.title))?;
        music.SetArtist(&HSTRING::from(&meta.artist))?;
        music.SetAlbumTitle(&HSTRING::from(&meta.album))?;

        // The cover is read from the file by the shell, asynchronously, so handing it the same
        // reference twice is wasted work on both sides -- hence tracking what it already has.
        if self.shown_cover.as_deref() != meta.cover_url.as_deref() {
            let thumbnail = meta.cover_url.as_deref().and_then(|url| stream_from_url(url).ok());
            match &thumbnail {
                Some(stream) => updater.SetThumbnail(stream)?,
                None => updater.SetThumbnail(None)?,
            }
            // Remember the URL we asked for either way: a cover the shell could not open will not
            // start working on the next position tick.
            self.shown_cover = meta.cover_url.clone();
        }
        updater.Update()
    }

    /// The scrubber's extent and where in it we are. Start and end frame the track rather than the
    /// queue, matching what the position is measured against.
    fn update_timeline(&self, snapshot: &Snapshot) -> windows::core::Result<()> {
        let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
        let end = snapshot.meta.as_ref().and_then(|meta| meta.duration).unwrap_or(snapshot.position);
        timeline.SetStartTime(time_span(Duration::ZERO))?;
        timeline.SetMinSeekTime(time_span(Duration::ZERO))?;
        timeline.SetEndTime(time_span(end))?;
        timeline.SetMaxSeekTime(time_span(end))?;
        timeline.SetPosition(time_span(snapshot.position.min(end)))?;
        self.controls.UpdateTimelineProperties(&timeline)
    }
}

/// A `Duration` as a WinRT `TimeSpan`, which counts 100-nanosecond units.
fn time_span(d: Duration) -> windows::Foundation::TimeSpan {
    let ticks = i64::try_from(d.as_nanos() / 100).unwrap_or(i64::MAX);
    windows::Foundation::TimeSpan { Duration: ticks }
}

/// A stream reference the shell can read the cover from, given the `file://` URL the metadata
/// carries.
fn stream_from_url(url: &str) -> windows::core::Result<RandomAccessStreamReference> {
    let uri = windows::Foundation::Uri::CreateUri(&HSTRING::from(url))?;
    RandomAccessStreamReference::CreateFromUri(&uri)
}

#[cfg(test)]
mod test {
    use super::*;
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

    /// Publishes a snapshot and then reads it back the way any other program on the machine would --
    /// through the session manager the OS offers -- since what matters is not that the calls
    /// returned `Ok` but that Windows made a now-playing session out of them.
    ///
    /// Skips (rather than fails) where there is no session manager to ask.
    #[test]
    fn windows_sees_what_we_publish() {
        // Unique, so a really running phonoscule (or anything else) cannot be mistaken for us.
        let title = format!("Phonoscule roundtrip {}", std::process::id());
        let (media, worker) = crate::media::start("Phonoscule roundtrip test", "phonoscule-test");
        if !media.active() {
            eprintln!("skipping: no media integration in this environment");
            return;
        }
        // Drive the worker for the test; it stops when `media` (holding the sender) is dropped.
        std::thread::spawn(move || smol::block_on(worker.run()));
        media.publish(Snapshot {
            meta: Some(Meta {
                title: title.clone(),
                album: "Roundtrip Album".into(),
                artist: "Roundtrip Artist".into(),
                cover_url: None,
                duration: Some(Duration::from_secs(200)),
            }),
            // Paused, not Playing: a test has no business taking the media keys off whatever the
            // person at the keyboard is actually listening to. The session is published either way.
            state: Playback::Paused,
            position: Duration::from_secs(20),
        });

        // The publish goes through the worker and then through the OS, neither on our schedule.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut seen = None;
        while std::time::Instant::now() < deadline && seen.is_none() {
            std::thread::sleep(Duration::from_millis(100));
            seen = match smol::block_on(ours(&title)) {
                Ok(found) => found,
                Err(e) => {
                    eprintln!("skipping: no media session manager here ({})", e.message());
                    return;
                }
            };
        }
        let (album, artist) = seen.unwrap_or_else(|| panic!("Windows never published a session titled {title:?}"));
        assert_eq!(album, "Roundtrip Album");
        assert_eq!(artist, "Roundtrip Artist");
    }

    /// The album and artist of the published session whose title is `title`, or `None` while no
    /// session has it. Errors mean the manager itself could not be reached.
    async fn ours(title: &str) -> windows::core::Result<Option<(String, String)>> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.await?;
        for session in manager.GetSessions()? {
            // A session that will not describe itself is somebody else's problem, not a failure
            // here: another player may be mid-teardown.
            let Ok(properties) = session.TryGetMediaPropertiesAsync()?.await else { continue };
            if properties.Title().map(|t| t.to_string()).as_deref() != Ok(title) {
                continue;
            }
            let album = properties.AlbumTitle()?.to_string();
            let artist = properties.Artist()?.to_string();
            return Ok(Some((album, artist)));
        }
        Ok(None)
    }
}
