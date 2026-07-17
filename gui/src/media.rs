//! OS media integration: a small bespoke MPRIS server on the D-Bus session bus, giving us media
//! keys, `playerctl -p phonoscule`, and desktop media widgets. Built directly on zbus rather than
//! via a wrapper crate, so updates reach the bus the instant we push them (no polling loop) and
//! we implement only the subset of MPRIS we actually use. Linux / D-Bus only.
//!
//! The update loop [`publish`](Media::publish)es a [`Snapshot`] of the now-playing state on every
//! change; the [`MediaWorker`] coalesces bursts of them down to the latest before applying them to
//! the served interface, so scrubbing the queue with a held key emits a handful of `properties
//! changed` signals rather than one per track. Control requests from the bus (play/pause/next/...)
//! arrive back as [`Control`] events on [`Media::events`].

use smol::channel;
use std::collections::HashMap;
use std::time::Duration;
use zbus::zvariant::{ObjectPath, Value};

/// A snapshot of the now-playing state to reflect on the bus.
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

/// A control request from the bus (a media key, `playerctl`, or a widget button).
#[derive(Debug, Clone)]
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

/// Handle the update loop keeps: publishes state changes and receives control events.
pub struct Media {
    updates: channel::Sender<Snapshot>,
    pub events: channel::Receiver<Control>,
    active: bool,
}

impl Media {
    /// Whether the MPRIS service actually came up (a session bus was reachable).
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

/// Owns the served MPRIS connection and applies coalesced updates to it; [`run`](MediaWorker::run)
/// is a long-running task. Kept separate from [`Media`] so it can move onto the executor.
pub struct MediaWorker {
    connection: Option<zbus::Connection>,
    updates: channel::Receiver<Snapshot>,
}

impl MediaWorker {
    /// Coalesce bursts of published snapshots down to the latest and apply it to the served
    /// interface, emitting the change signals. Parks when idle; ends when the update channel
    /// closes (the app is shutting down).
    pub async fn run(self) {
        /// Minimum spacing between applied updates. zbus emits in real time, so this isn't about
        /// keeping up -- it just collapses the fastest scrub through the queue into a couple of
        /// signals rather than one per track, while staying prompt.
        const MIN_INTERVAL: Duration = Duration::from_millis(50);

        let Some(connection) = self.connection else { return };
        let iface = match connection.object_server().interface::<_, PlayerInterface>(OBJECT_PATH).await {
            Ok(iface) => iface,
            Err(e) => {
                log::warn!("MPRIS player interface missing: {e}");
                return;
            }
        };

        let mut shown_meta: Option<Meta> = None;
        let mut shown_state: Option<Playback> = None;
        loop {
            // Park until something changes, then take everything already queued behind it -- only
            // the last snapshot of a burst matters.
            let Ok(mut snapshot) = self.updates.recv().await else { return };
            while let Ok(next) = self.updates.try_recv() {
                snapshot = next;
            }

            let mut player = iface.get_mut().await;
            player.state = snapshot.clone();
            let emitter = iface.signal_emitter();
            // Metadata and status are signalled only when they actually change (Metadata is
            // comparatively heavy, and clients that redraw on it don't want a signal per position
            // tick). Position is read on demand, not signalled, per the MPRIS spec.
            if snapshot.meta != shown_meta {
                let _ = player.metadata_changed(emitter).await;
                shown_meta = snapshot.meta.clone();
            }
            if Some(snapshot.state) != shown_state {
                let _ = player.playback_status_changed(emitter).await;
                shown_state = Some(snapshot.state);
            }
            drop(player);

            // Hold off before the next apply; anything arriving during the wait coalesces into it.
            smol::Timer::after(MIN_INTERVAL).await;
        }
    }
}

/// The single object every MPRIS player exposes its two interfaces at.
const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

pub fn start() -> (Media, MediaWorker) {
    start_named("Phonoscule", "phonoscule")
}

fn start_named(identity: &str, bus_name: &str) -> (Media, MediaWorker) {
    let (update_tx, update_rx) = channel::unbounded();
    let (event_tx, event_rx) = channel::unbounded();
    // Build the connection now (a session-bus socket is cheap to open) so `active` is known and
    // the worker just has to push updates.
    let connection = match smol::block_on(serve(identity, bus_name, event_tx)) {
        Ok(connection) => Some(connection),
        Err(e) => {
            log::warn!("no OS media integration: {e}");
            None
        }
    };
    let active = connection.is_some();
    (Media { updates: update_tx, events: event_rx, active }, MediaWorker { connection, updates: update_rx })
}

/// Registers the two MPRIS interfaces on the session bus under `org.mpris.MediaPlayer2.<bus_name>`.
async fn serve(identity: &str, bus_name: &str, events: channel::Sender<Control>) -> zbus::Result<zbus::Connection> {
    let root = RootInterface { identity: identity.to_string() };
    let player = PlayerInterface { events, state: Snapshot { meta: None, state: Playback::Stopped, position: Duration::ZERO } };
    zbus::connection::Builder::session()?
        .name(format!("org.mpris.MediaPlayer2.{bus_name}"))?
        .serve_at(OBJECT_PATH, root)?
        .serve_at(OBJECT_PATH, player)?
        .build()
        .await
}

/// The `org.mpris.MediaPlayer2` root interface: identity and the (unsupported) app-level actions.
struct RootInterface {
    identity: String,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl RootInterface {
    fn raise(&self) {}
    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        &self.identity
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec![]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        vec![]
    }
}

/// The `org.mpris.MediaPlayer2.Player` interface: the transport controls and now-playing state.
struct PlayerInterface {
    events: channel::Sender<Control>,
    state: Snapshot,
}

impl PlayerInterface {
    fn send(&self, control: Control) {
        // Unbounded: only fails when the app is gone.
        let _ = self.events.try_send(control);
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerInterface {
    fn next(&self) {
        self.send(Control::Next);
    }
    fn previous(&self) {
        self.send(Control::Prev);
    }
    fn pause(&self) {
        self.send(Control::Pause);
    }
    fn play_pause(&self) {
        self.send(Control::Toggle);
    }
    fn stop(&self) {
        self.send(Control::Stop);
    }
    fn play(&self) {
        self.send(Control::Play);
    }
    fn seek(&self, offset: i64) {
        self.send(Control::Seek(offset));
    }
    fn set_position(&self, _track: ObjectPath<'_>, position: i64) {
        if let Ok(micros) = u64::try_from(position) {
            self.send(Control::SetPosition(Duration::from_micros(micros)));
        }
    }
    fn open_uri(&self, _uri: String) {}

    #[zbus(property)]
    fn playback_status(&self) -> &'static str {
        match self.state.state {
            Playback::Playing => "Playing",
            Playback::Paused => "Paused",
            Playback::Stopped => "Stopped",
        }
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<&'static str, Value<'static>> {
        let mut dict = HashMap::new();
        // A valid, stable track id -- required for SetPosition; we only ever have one "track".
        let trackid = ObjectPath::try_from("/org/mpris/MediaPlayer2/track").expect("valid path");
        dict.insert("mpris:trackid", Value::new(trackid));
        if let Some(meta) = &self.state.meta {
            if let Some(duration) = meta.duration {
                dict.insert("mpris:length", Value::new(duration.as_micros() as i64));
            }
            if let Some(url) = &meta.cover_url {
                dict.insert("mpris:artUrl", Value::new(url.clone()));
            }
            dict.insert("xesam:title", Value::new(meta.title.clone()));
            dict.insert("xesam:artist", Value::new(vec![meta.artist.clone()]));
            dict.insert("xesam:album", Value::new(meta.album.clone()));
        }
        dict
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        i64::try_from(self.state.position.as_micros()).unwrap_or(0)
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn set_volume(&self, _volume: f64) {}
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
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
        // Give the worker a moment to apply the update.
        std::thread::sleep(Duration::from_millis(300));

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
        assert!(matches!(event, Some(Control::Toggle)), "{event:?}");
    }
}
