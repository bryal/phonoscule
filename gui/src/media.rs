//! OS media integration via souvlaki: MPRIS on Linux (media keys, `playerctl -p phonoscule`,
//! desktop media widgets), SystemMediaTransportControls on Windows.
//!
//! The update loop [`publish`](Media::publish)es a [`Snapshot`] of the now-playing state on every
//! change; a long-running [`MediaWorker`] coalesces bursts of them down to the latest before
//! pushing to the OS. souvlaki's D-Bus service digests updates only ~1/s, so pushing every track
//! flashed past while scrubbing the queue would just back its queue up (and lag, or stall
//! shutdown). Coalescing lives entirely in the worker, so the update loop just fires and forgets.

use smol::channel;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use std::time::Duration;

/// A snapshot of the now-playing state to show on the OS media integration.
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
    /// A `file://` URL to the cover image, for the OS to display.
    pub cover_url: Option<String>,
    pub duration: Option<Duration>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Playback {
    Playing,
    Paused,
    Stopped,
}

/// Handle the update loop keeps: publishes state changes and receives control events.
pub struct Media {
    updates: channel::Sender<Snapshot>,
    pub events: channel::Receiver<MediaControlEvent>,
    active: bool,
}

impl Media {
    /// Whether OS media integration is actually up.
    pub fn active(&self) -> bool {
        self.active
    }

    /// Publish the latest now-playing state. Never blocks (the channel is unbounded); the worker
    /// coalesces a burst of these down to the latest before pushing (see [`MediaWorker::run`]).
    pub fn publish(&self, snapshot: Snapshot) {
        // Fails only once the worker is gone (app shutting down).
        let _ = self.updates.try_send(snapshot);
    }
}

/// Owns the OS media handle and pushes coalesced updates to it; [`run`](MediaWorker::run) is a
/// long-running task. Kept separate from [`Media`] so it can be moved onto the executor while the
/// handle stays with the update loop.
pub struct MediaWorker {
    controls: Option<MediaControls>,
    updates: channel::Receiver<Snapshot>,
}

impl MediaWorker {
    /// Coalesce bursts of published snapshots down to the latest and push it, no more than ~once a
    /// second. Parks when idle; ends when the update channel closes (the app is shutting down).
    pub async fn run(self) {
        /// Minimum spacing between pushes: souvlaki's D-Bus service digests updates at about this
        /// rate, so a faster push just backs up its queue.
        const MIN_INTERVAL: Duration = Duration::from_secs(1);

        let MediaWorker { controls, updates } = self;
        let Some(mut controls) = controls else { return };
        let mut pushed_meta: Option<Meta> = None;
        loop {
            // Park until something changes, then take everything already queued behind it -- only
            // the last snapshot of a burst matters.
            let Ok(mut snapshot) = updates.recv().await else { return };
            while let Ok(next) = updates.try_recv() {
                snapshot = next;
            }

            // Metadata is comparatively expensive and rarely changes between position ticks, so
            // push it only when it actually differs from what the OS already has.
            if snapshot.meta != pushed_meta {
                if let Some(meta) = &snapshot.meta {
                    let metadata = MediaMetadata {
                        title: Some(&meta.title),
                        album: Some(&meta.album),
                        artist: Some(&meta.artist),
                        cover_url: meta.cover_url.as_deref(),
                        duration: meta.duration,
                    };
                    if let Err(e) = controls.set_metadata(metadata) {
                        log::warn!("failed to update media metadata: {e:?}");
                    }
                }
                pushed_meta = snapshot.meta.clone();
            }

            let progress = Some(MediaPosition(snapshot.position));
            let playback = match snapshot.state {
                Playback::Playing => MediaPlayback::Playing { progress },
                Playback::Paused => MediaPlayback::Paused { progress },
                Playback::Stopped => MediaPlayback::Stopped,
            };
            if let Err(e) = controls.set_playback(playback) {
                log::warn!("failed to update media playback state: {e:?}");
            }

            // Hold off before the next push; anything arriving during the wait coalesces into it.
            smol::Timer::after(MIN_INTERVAL).await;
        }
    }
}

pub fn start() -> (Media, MediaWorker) {
    start_named("Phonoscule", "phonoscule")
}

fn start_named(display_name: &str, dbus_name: &str) -> (Media, MediaWorker) {
    let (event_tx, event_rx) = channel::unbounded();
    let controls = MediaControls::new(PlatformConfig {
        display_name,
        dbus_name,
        // Only used on Windows, where the media controls need a window handle; plumbing one out
        // of iced is a problem for the day this runs there.
        hwnd: None,
    })
    .and_then(|mut controls| {
        controls.attach(move |event| {
            // Unbounded: only fails when the app is gone.
            let _ = event_tx.try_send(event);
        })?;
        Ok(controls)
    });
    let controls = match controls {
        Ok(controls) => Some(controls),
        Err(e) => {
            log::warn!("no OS media integration: {e:?}");
            None
        }
    };
    let active = controls.is_some();
    let (update_tx, update_rx) = channel::unbounded();
    (Media { updates: update_tx, events: event_rx, active }, MediaWorker { controls, updates: update_rx })
}

#[cfg(test)]
mod test {
    use super::*;
    use std::process::Command;

    /// Registers the real MPRIS service, then controls & inspects it from the outside like a
    /// `playerctl`/`busctl` user would. Skips (rather than fails) where there is no session bus
    /// or no busctl, e.g. headless environments.
    #[test]
    fn mpris_roundtrip() {
        const PATH: &str = "/org/mpris/MediaPlayer2";
        const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
        // A unique bus name: colliding with a really running phonoscule would silently address
        // all the busctl calls below at it (and toggle the user's playback!).
        let dbus_name = format!("phonoscule_test_{}", std::process::id());
        let mpris = format!("org.mpris.MediaPlayer2.{dbus_name}");

        let (media, worker) = start_named("Phonoscule roundtrip test", &dbus_name);
        if !media.active() {
            eprintln!("skipping: no media integration in this environment");
            return;
        }
        // Drive the worker for the test; it stops when `media` (holding the sender) is dropped.
        std::thread::spawn(move || smol::block_on(worker.run()));
        media.publish(Snapshot {
            meta: Some(Meta {
                title: "Roundtrip Test".into(),
                album: String::new(),
                artist: String::new(),
                cover_url: None,
                duration: None,
            }),
            state: Playback::Playing,
            position: Duration::ZERO,
        });
        // Give the worker and the service thread a moment to push and register.
        std::thread::sleep(Duration::from_millis(500));

        // What we published is visible on the bus...
        let status = Command::new("busctl").args(["--user", "get-property", &mpris, PATH, PLAYER, "PlaybackStatus"]).output();
        let Ok(status) = status else {
            eprintln!("skipping: no busctl in this environment");
            return;
        };
        assert!(String::from_utf8_lossy(&status.stdout).contains("Playing"), "{status:?}");
        let metadata =
            Command::new("busctl").args(["--user", "get-property", &mpris, PATH, PLAYER, "Metadata"]).output().unwrap();
        assert!(String::from_utf8_lossy(&metadata.stdout).contains("Roundtrip Test"), "{metadata:?}");

        // ...and a control call from the outside arrives as an event.
        let call = Command::new("busctl").args(["--user", "call", &mpris, PATH, PLAYER, "PlayPause"]).status().unwrap();
        assert!(call.success());
        let event = smol::block_on(smol::future::or(async { media.events.recv().await.ok() }, async {
            smol::Timer::after(Duration::from_secs(3)).await;
            None
        }));
        assert!(matches!(event, Some(MediaControlEvent::Toggle | MediaControlEvent::Pause)), "{event:?}");
    }
}
