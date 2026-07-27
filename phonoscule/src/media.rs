//! OS media integration: media keys, and the now-playing state the desktop displays.
//!
//! The application [`publish`](Media::publish)es a [`Snapshot`] of its now-playing state on every
//! change; the [`MediaWorker`] coalesces bursts of them down to the latest before applying them to
//! the OS, so scrubbing the queue with a held key reaches the desktop as a handful of updates
//! rather than one per track. Requests coming back the other way - a media key, a button on a
//! desktop widget - arrive as [`Control`] events on [`Media::events`].
//!
//! Two backends, picked at compile time, both of which the coalescing loop drives the same way:
//! [`mpris`](crate::mpris) on Linux (a small MPRIS server on the D-Bus session bus, which also
//! gives `playerctl -p <name>`), and the System Media Transport Controls on Windows (the flyout
//! above the volume OSD, and the now-playing card on the lock screen). Where there is neither - a
//! platform with no backend, or a Linux session with no bus - [`Media::active`] reports false and
//! the player simply runs without media integration.
//!
//! Wants std, and whatever the platform's backend does.

use smol::channel;
use std::time::Duration;

/// A snapshot of the now-playing state to reflect to the OS.
#[derive(Clone, PartialEq)]
pub struct Snapshot {
    /// The track's metadata, or `None` when nothing is loaded.
    pub meta: Option<Meta>,
    pub state: Playback,
    pub position: Duration,
}

#[derive(Clone, PartialEq)]
pub struct Meta {
    pub title: String,
    pub album: String,
    pub artist: String,
    /// A `file://` URL to the cover image, for the desktop to display.
    pub cover_url: Option<String>,
    pub duration: Option<Duration>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Playback {
    Playing,
    Paused,
    Stopped,
}

/// A control request from the OS (a media key, `playerctl`, or a widget button).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Play,
    Pause,
    Toggle,
    Stop,
    Next,
    Prev,
    /// Relative seek by this signed microsecond offset (MPRIS `Seek`).
    Seek(i64),
    /// Absolute seek to this position (MPRIS `SetPosition`).
    SetPosition(Duration),
}

/// Which parts of a [`Snapshot`] differ from the one last applied, so a backend can skip the
/// expensive half of an update. Metadata is comparatively heavy on both platforms - a properties
/// signal with a full dictionary, or a display-updater commit - and clients that redraw on it do
/// not want one per position tick.
#[derive(Debug, Clone, Copy)]
pub struct Changed {
    pub meta: bool,
    pub state: bool,
}

/// Handle the application keeps: publishes state changes and receives control events.
pub struct Media {
    updates: channel::Sender<Snapshot>,
    pub events: channel::Receiver<Control>,
    active: bool,
}

impl Media {
    /// Whether the media service actually came up (a session bus, or the OS's own controls).
    pub fn active(&self) -> bool {
        self.active
    }

    /// Publish the latest now-playing state. Never blocks (the channel is unbounded); the worker
    /// coalesces a burst of these down to the latest before applying it (see [`MediaWorker::run`]).
    pub fn publish(&self, snapshot: Snapshot) {
        // Fails only once the worker is gone (app shutting down).
        let _ = self.updates.try_send(snapshot);
    }
}

/// Owns the platform's media service and applies coalesced updates to it; [`run`](MediaWorker::run)
/// is a long-running task. Kept separate from [`Media`] so it can move onto the executor.
pub struct MediaWorker {
    server: Option<backend::Server>,
    updates: channel::Receiver<Snapshot>,
}

impl MediaWorker {
    /// Coalesce bursts of published snapshots down to the latest and apply it to the OS. Parks when
    /// idle; ends when the update channel closes (the app is shutting down).
    pub async fn run(mut self) {
        /// Minimum spacing between applied updates. Neither backend needs the throttling to keep
        /// up - it just collapses the fastest scrub through the queue into a couple of updates
        /// rather than one per track, while staying prompt.
        const MIN_INTERVAL: Duration = Duration::from_millis(50);

        let Some(server) = self.server.as_mut() else { return };
        let mut shown_meta: Option<Meta> = None;
        let mut shown_state: Option<Playback> = None;
        loop {
            // Park until something changes, then take everything already queued behind it - only
            // the last snapshot of a burst matters.
            let Ok(mut snapshot) = self.updates.recv().await else { return };
            while let Ok(next) = self.updates.try_recv() {
                snapshot = next;
            }

            let changed = Changed { meta: snapshot.meta != shown_meta, state: Some(snapshot.state) != shown_state };
            if changed.meta {
                shown_meta = snapshot.meta.clone();
            }
            if changed.state {
                shown_state = Some(snapshot.state);
            }
            server.apply(&snapshot, changed).await;

            // Hold off before the next apply; anything arriving during the wait coalesces into it.
            smol::Timer::after(MIN_INTERVAL).await;
        }
    }
}

/// Brings the media service up. `identity` is the name desktops display. `name` identifies the
/// player among running ones where the platform needs that - on Linux it is the last element of
/// the `org.mpris.MediaPlayer2.<name>` bus name, so it must be a valid D-Bus bus-name element:
/// letters, digits, underscores and hyphens, not starting with a digit.
///
/// No service is not an error: [`Media::active`] then reports false and the player runs without
/// media integration.
pub fn start(identity: &str, name: &str) -> (Media, MediaWorker) {
    let (update_tx, update_rx) = channel::unbounded();
    let (event_tx, event_rx) = channel::unbounded();
    // Built now, rather than in the worker, so `active` is known before the application draws its
    // first frame and the worker has only to push updates.
    let server = match backend::start(identity, name, event_tx) {
        Ok(server) => Some(server),
        Err(e) => {
            log::warn!("no OS media integration: {e}");
            None
        }
    };
    let active = server.is_some();
    (Media { updates: update_tx, events: event_rx, active }, MediaWorker { server, updates: update_rx })
}

/// The MPRIS backend, over the D-Bus session bus.
#[cfg(target_os = "linux")]
use crate::mpris as backend;

/// The System Media Transport Controls backend.
#[cfg(target_os = "windows")]
use crate::smtc as backend;

/// The fallback for platforms with neither: starting always fails, so [`Media::active`] reports
/// false and published snapshots go nowhere.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod backend {
    use super::{Changed, Control, Snapshot};
    use smol::channel;

    pub struct Server(());

    pub fn start(_identity: &str, _name: &str, _events: channel::Sender<Control>) -> Result<Server, String> {
        Err("no media integration for this platform".into())
    }

    impl Server {
        pub async fn apply(&mut self, _snapshot: &Snapshot, _changed: Changed) {}
    }
}
