//! OS media integration via souvlaki: MPRIS on Linux (media keys, `playerctl -p phonoscule`,
//! desktop media widgets), SystemMediaTransportControls on Windows.

use smol::channel;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};

pub struct Media {
    /// `None` when the OS integration could not be set up; the player works fine without it.
    controls: Option<MediaControls>,
    pub events: channel::Receiver<MediaControlEvent>,
}

pub fn start() -> Media {
    start_named("Phonoscule", "phonoscule")
}

fn start_named(display_name: &str, dbus_name: &str) -> Media {
    let (tx, rx) = channel::unbounded();
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
            let _ = tx.try_send(event);
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
    Media { controls, events: rx }
}

impl Drop for Media {
    fn drop(&mut self) {
        // souvlaki's `MediaControls::drop` stops its D-Bus service thread by signalling it and
        // then *joining* it -- but that thread only notices the signal between its blocking
        // `Connection::process` calls, so the join can stall for many seconds (observed ~50s on
        // Wayland). Since `Media` is only ever dropped as the app exits, that stall would freeze
        // the whole GUI teardown. Leak the controls instead: the OS reaps the thread and releases
        // the MPRIS name when the process goes.
        if let Some(controls) = self.controls.take() {
            std::mem::forget(controls);
        }
    }
}

impl Media {
    /// Whether OS media integration is actually up.
    pub fn active(&self) -> bool {
        self.controls.is_some()
    }

    pub fn set_metadata(&mut self, metadata: MediaMetadata) {
        if let Some(controls) = &mut self.controls
            && let Err(e) = controls.set_metadata(metadata)
        {
            log::warn!("failed to update media metadata: {e:?}");
        }
    }

    pub fn set_playback(&mut self, playback: MediaPlayback) {
        if let Some(controls) = &mut self.controls
            && let Err(e) = controls.set_playback(playback)
        {
            log::warn!("failed to update media playback state: {e:?}");
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::process::Command;
    use std::time::Duration;

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

        let mut media = start_named("Phonoscule roundtrip test", &dbus_name);
        if !media.active() {
            eprintln!("skipping: no media integration in this environment");
            return;
        }
        media.set_metadata(MediaMetadata { title: Some("Roundtrip Test"), ..Default::default() });
        media.set_playback(MediaPlayback::Playing { progress: None });
        // Give the service thread a moment to register and process.
        std::thread::sleep(Duration::from_millis(500));

        // What we pushed is visible on the bus...
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
