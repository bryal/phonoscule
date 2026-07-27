//! The Linux [`media`](crate::media) backend: a small bespoke MPRIS server on the D-Bus session
//! bus, giving a player media keys, `playerctl -p <name>`, and desktop media widgets. Built
//! directly on zbus rather than via a wrapper crate, so updates reach the bus the instant they are
//! pushed (no polling loop) and only the subset of MPRIS actually used is implemented.
//!
//! Applications talk to [`media`](crate::media), not to this module; it is public because an MPRIS
//! server is a worthwhile thing to reach for on purpose.
//!
//! Wants std and a D-Bus session bus.

use crate::media::{Changed, Control, Playback, Snapshot};
use smol::channel;
use std::collections::HashMap;
use std::time::Duration;
use zbus::zvariant::{ObjectPath, Value};

/// The single object every MPRIS player exposes its two interfaces at.
const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

/// The registered service, and the interface updates are applied to.
pub struct Server {
    iface: zbus::object_server::InterfaceRef<PlayerInterface>,
    /// Held for the session: dropping the connection unregisters the name and the objects on it.
    _connection: zbus::Connection,
}

/// Registers the two MPRIS interfaces on the session bus under `org.mpris.MediaPlayer2.<name>`, so
/// `name` must be a valid D-Bus *bus* name element -- letters, digits, underscores and hyphens, not
/// starting with a digit -- and unique among running players. Hyphens are allowed here, unlike in
/// interface and member names.
pub fn start(identity: &str, name: &str, events: channel::Sender<Control>) -> Result<Server, String> {
    // A session-bus socket is cheap to open, so this is done up front rather than in the worker:
    // whether there is any media integration at all is known before the first frame.
    smol::block_on(async {
        let root = RootInterface { identity: identity.to_string() };
        let state = Snapshot { meta: None, state: Playback::Stopped, position: Duration::ZERO };
        let player = PlayerInterface { events, state };
        let connection = zbus::connection::Builder::session()?
            .name(format!("org.mpris.MediaPlayer2.{name}"))?
            .serve_at(OBJECT_PATH, root)?
            .serve_at(OBJECT_PATH, player)?
            .build()
            .await?;
        let iface = connection.object_server().interface::<_, PlayerInterface>(OBJECT_PATH).await?;
        Ok(Server { iface, _connection: connection })
    })
    .map_err(|e: zbus::Error| e.to_string())
}

impl Server {
    /// Stores the snapshot on the served interface and emits the change signals for whatever
    /// actually changed. Position is deliberately not among them: MPRIS has clients read it on
    /// demand rather than have it signalled.
    pub async fn apply(&mut self, snapshot: &Snapshot, changed: Changed) {
        let mut player = self.iface.get_mut().await;
        player.state = snapshot.clone();
        let emitter = self.iface.signal_emitter();
        if changed.meta {
            let _ = player.metadata_changed(emitter).await;
        }
        if changed.state {
            let _ = player.playback_status_changed(emitter).await;
        }
    }
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
    use crate::media::Meta;
    use std::process::Command;

    /// Registers the real MPRIS service, then controls & inspects it from the outside like a
    /// `playerctl`/`busctl` user would. Skips (rather than fails) where there is no session bus
    /// or no busctl, e.g. headless environments.
    ///
    /// Driven through [`media`](crate::media) rather than this module's own `start`, since that is
    /// how a player reaches it -- which puts the coalescing worker in the loop too.
    #[test]
    fn mpris_roundtrip() {
        const PATH: &str = "/org/mpris/MediaPlayer2";
        const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
        // A unique bus name: colliding with a really running phonoscule would silently address
        // all the busctl calls below at it (and toggle the user's playback!). Hyphenated, so that
        // bus names with hyphens stay exercised -- the players use them.
        let dbus_name = format!("phonoscule-test-{}", std::process::id());
        let mpris = format!("org.mpris.MediaPlayer2.{dbus_name}");

        let (media, worker) = crate::media::start("Phonoscule roundtrip test", &dbus_name);
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
