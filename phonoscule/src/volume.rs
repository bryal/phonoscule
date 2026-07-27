//! Per-application volume, delegated to the OS mixer -- never applied to the samples we decode.
//!
//! The interface is platform-neutral: [`start`] spawns whatever backend the platform has and
//! hands back a [`VolumeControl`], whose readings -- the initial one, and external changes from
//! any mixer UI -- stream out as plain factors (1.0 = 100%), so an application can mirror the mixer
//! without polling. A platform without a backend simply never reports and swallows sets.
//!
//! The Linux backend speaks PulseAudio (PipeWire included, via pipewire-pulse): a dedicated
//! thread finds the application's sink inputs (by process id -- the playback stream comes from
//! `pulse-simple`, which doesn't expose its stream), subscribes to their changes, and applies
//! [`VolumeControl::set`] commands. The Windows one does the same over WASAPI, holding
//! `ISimpleAudioVolume` on the audio sessions the [`sink`](crate::sink) opened for this process.
//! Either server remembers the volume across restarts (PulseAudio's stream-restore, and Windows'
//! own per-application mixer state), so nothing is persisted here.
//!
//! Wants std and an OS mixer.

use smol::channel;
use std::sync::mpsc;

/// Handle for adjusting the volume; readings arrive on [`VolumeControl::events`].
pub struct VolumeControl {
    cmd: mpsc::Sender<f32>,
    /// The volume as the mixer reports it, as a factor of 100%: an initial reading shortly after
    /// startup, then one per change -- our own sets included, and external ones (say, a system
    /// mixer applet) too.
    pub events: channel::Receiver<f32>,
}

impl VolumeControl {
    /// Requests the given volume for all of the application's audio streams.
    /// Fire-and-forget; the eventual reading echoes back on [`events`](VolumeControl::events).
    pub fn set(&self, volume: f32) {
        // Sends only fail when there is no backend (or its server is gone): ignored by design.
        let _ = self.cmd.send(volume);
    }
}

/// Spawns the platform's mixer backend and returns its control handle. Without one (unsupported
/// platform, unreachable server) no readings ever arrive and set commands go nowhere.
pub fn start() -> VolumeControl {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (event_tx, event_rx) = channel::unbounded();
    backend::spawn(cmd_rx, event_tx);
    VolumeControl { cmd: cmd_tx, events: event_rx }
}

/// The mixerless fallback: readings never arrive, and the dropped command channel makes sets
/// no-ops.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod backend {
    pub fn spawn(_commands: std::sync::mpsc::Receiver<f32>, _events: smol::channel::Sender<f32>) {}
}

/// The WASAPI backend: `ISimpleAudioVolume` on this process's own audio sessions, which is the
/// per-application slider the Windows volume mixer shows.
///
/// Polled rather than event-driven, unlike the PulseAudio backend. Following a session with
/// `IAudioSessionEvents` would report external changes a little sooner, but the sessions
/// themselves come and go - [`sink`](crate::sink) opens a fresh stream whenever a track's sample
/// rate differs from the last one's, and again if the default endpoint changes - so a backend that
/// registered callbacks would spend its time re-registering them. Re-enumerating on a timer
/// handles all of that in one path, and at this interval it is far below noticeable either way.
#[cfg(target_os = "windows")]
mod backend {
    use smol::channel;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::{Duration, Instant};
    use windows::Win32::Media::Audio::{
        AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
        MMDeviceEnumerator, eConsole, eRender,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::core::Interface;

    /// How long a round waits for a [`super::VolumeControl::set`] before going back to reading the
    /// mixer. Sets do not wait for it - the wait ends the moment one arrives - so this is only
    /// how promptly an *external* change (the mixer applet, another tool) is noticed.
    const POLL: Duration = Duration::from_millis(200);

    /// How often to look for new audio sessions once we have some. Short enough that a stream
    /// reopened mid-track is picked back up within a beat, long enough to stay idle-cheap.
    const RESCAN: Duration = Duration::from_secs(2);

    pub fn spawn(commands: mpsc::Receiver<f32>, events: channel::Sender<f32>) {
        std::thread::Builder::new()
            .name("volume-mixer".into())
            .spawn(move || {
                if let Err(e) = serve(commands, events) {
                    log::warn!("volume control unavailable: {e}");
                }
            })
            .expect("spawning the volume thread");
    }

    fn serve(commands: mpsc::Receiver<f32>, events: channel::Sender<f32>) -> Result<(), String> {
        // The mixer thread is the only one touching these interfaces, and it initialises its own
        // apartment. No CoUninitialize: the thread outlives the interfaces it holds.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|e| e.message())?;

        // Our sessions, and the last volume reported, to swallow no-op change events (our own sets
        // echo back as one).
        let mut ours: Vec<ISimpleAudioVolume> = vec![];
        let mut reported: Option<f32> = None;
        let mut scanned: Option<Instant> = None;

        loop {
            // Rescan on a timer once we have sessions; keep looking every round until then, since
            // at startup the engine may not have opened its stream yet.
            let due = match scanned {
                None => true,
                Some(at) => ours.is_empty() || at.elapsed() >= RESCAN,
            };
            if due {
                match sessions(&enumerator) {
                    Ok(found) => ours = found,
                    // A transient failure (the endpoint being switched under us) must not end the
                    // thread - the next round looks again.
                    Err(e) => log::debug!("could not enumerate audio sessions: {e}"),
                }
                scanned = Some(Instant::now());
            }

            // Park until a set arrives or it is time to read the mixer again.
            let requested = match commands.recv_timeout(POLL) {
                Ok(volume) => {
                    // A burst (wheel, held key) collapses to its last value.
                    let mut last = volume;
                    while let Ok(next) = commands.try_recv() {
                        last = next;
                    }
                    Some(last)
                }
                Err(RecvTimeoutError::Timeout) => None,
                // The application is gone.
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            };
            if let Some(volume) = requested {
                for session in &ours {
                    let _ = unsafe { session.SetMasterVolume(volume.clamp(0.0, 1.0), std::ptr::null()) };
                }
            }

            // Report what the mixer now says. Every session of ours carries the same value (we set
            // them together, and Windows applies its own per-application volume to all of them),
            // so the first one speaks for the application.
            if let Some(session) = ours.first() {
                match unsafe { session.GetMasterVolume() } {
                    Ok(volume) if reported != Some(volume) => {
                        reported = Some(volume);
                        let _ = events.try_send(volume);
                    }
                    Ok(_) => (),
                    // The session expired between the scan and now: drop them all and rescan.
                    Err(e) => {
                        log::debug!("audio session went away: {}", e.message());
                        ours.clear();
                    }
                }
            }
        }
    }

    /// This process's live render sessions on the default endpoint. Ours by process id, the same
    /// test the PulseAudio backend makes: the sink's stream does not hand its session back, and
    /// there is no need for it to.
    fn sessions(enumerator: &IMMDeviceEnumerator) -> Result<Vec<ISimpleAudioVolume>, String> {
        let us = std::process::id();
        let found = (|| -> windows::core::Result<Vec<ISimpleAudioVolume>> {
            let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }?;
            let manager: IAudioSessionManager2 = unsafe { device.Activate(CLSCTX_ALL, None) }?;
            let sessions = unsafe { manager.GetSessionEnumerator() }?;
            let mut ours = vec![];
            for i in 0..unsafe { sessions.GetCount() }? {
                let control = unsafe { sessions.GetSession(i) }?;
                // Expired sessions linger in the enumeration; setting their volume does nothing
                // and reading it would report a stale value as though it were current.
                if unsafe { control.GetState() }? == AudioSessionStateExpired {
                    continue;
                }
                if unsafe { control.cast::<IAudioSessionControl2>()?.GetProcessId() }? != us {
                    continue;
                }
                ours.push(control.cast()?);
            }
            Ok(ours)
        })();
        found.map_err(|e| e.message())
    }
}

/// The PulseAudio backend (see the module docs).
#[cfg(target_os = "linux")]
mod backend {
    use libpulse_binding as pulse;
    use pulse::callbacks::ListResult;
    use pulse::context::subscribe::{Facility, InterestMaskSet, Operation};
    use pulse::context::{Context, FlagSet, State};
    use pulse::mainloop::standard::{IterateResult, Mainloop};
    use pulse::volume::{ChannelVolumes, Volume};
    use smol::channel;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::time::Duration;

    pub fn spawn(commands: mpsc::Receiver<f32>, events: channel::Sender<f32>) {
        std::thread::Builder::new()
            .name("volume-mixer".into())
            .spawn(move || {
                if let Err(e) = serve(commands, events) {
                    log::warn!("volume control unavailable: {e}");
                }
            })
            .expect("spawning the volume thread");
    }

    /// What the mixer thread knows about the application's sink inputs, shared with the libpulse
    /// callbacks (all on the mixer thread).
    struct Mixer {
        /// Our sink inputs: index -> channel count (needed to build a matching volume structure).
        ours: HashMap<u32, u8>,
        /// The last volume reported, to swallow no-op change events (our own sets echo back as
        /// one).
        reported: Option<f32>,
        events: channel::Sender<f32>,
    }

    impl Mixer {
        /// Digests one sink-input listing: adopt it if it belongs to this process, and report its
        /// volume when it changed. The proplist carries the process id for every local client.
        fn digest(&mut self, info: &pulse::context::introspect::SinkInputInfo) {
            let pid = info.proplist.get_str(pulse::proplist::properties::APPLICATION_PROCESS_ID);
            if pid.as_deref() != Some(std::process::id().to_string().as_str()) {
                return;
            }
            self.ours.insert(info.index, info.volume.len());
            let volume = info.volume.avg().0 as f32 / Volume::NORMAL.0 as f32;
            if self.reported != Some(volume) {
                self.reported = Some(volume);
                let _ = self.events.try_send(volume);
            }
        }
    }

    fn serve(commands: mpsc::Receiver<f32>, events: channel::Sender<f32>) -> Result<(), String> {
        let mut mainloop = Mainloop::new().ok_or("no mainloop")?;
        let mut context = Context::new(&mainloop, "phonoscule-volume").ok_or("no context")?;
        context.connect(None, FlagSet::NOFLAGS, None).map_err(|e| format!("connect: {e}"))?;

        // Drive the mainloop until the context is ready (or failed -- no server, usually).
        loop {
            iterate(&mut mainloop)?;
            match context.get_state() {
                State::Ready => break,
                State::Failed | State::Terminated => return Err("could not reach the audio server".into()),
                _ => (),
            }
        }

        let mixer = Rc::new(RefCell::new(Mixer { ours: HashMap::new(), reported: None, events }));

        // Track the application's sink inputs: an initial listing adopts the ones already playing
        // (the engine opens its stream at startup), and the subscription follows every later change
        // -- new streams, our own volume sets echoing back, and external mixers alike.
        let digest_one = {
            let mixer = Rc::clone(&mixer);
            move |result: ListResult<&pulse::context::introspect::SinkInputInfo>| {
                if let ListResult::Item(info) = result {
                    mixer.borrow_mut().digest(info);
                }
            }
        };
        context.introspect().get_sink_input_info_list(digest_one.clone());

        let introspect = context.introspect();
        context.set_subscribe_callback(Some(Box::new({
            let mixer = Rc::clone(&mixer);
            move |facility, operation, index| {
                if facility != Some(Facility::SinkInput) {
                    return;
                }
                match operation {
                    Some(Operation::New) | Some(Operation::Changed) => {
                        introspect.get_sink_input_info(index, digest_one.clone());
                    }
                    Some(Operation::Removed) => {
                        mixer.borrow_mut().ours.remove(&index);
                    }
                    None => (),
                }
            }
        })));
        context.subscribe(InterestMaskSet::SINK_INPUT, |_| {});

        // Pump libpulse and poll for commands. Polling (rather than blocking) keeps the loop simple
        // -- libpulse's mainloop can't be woken from another thread through safe bindings -- and a
        // 25 ms nap between empty rounds keeps it idle-cheap while sets still feel immediate.
        let mut introspect = context.introspect();
        loop {
            iterate(&mut mainloop)?;
            let mut requested = None;
            while let Ok(volume) = commands.try_recv() {
                requested = Some(volume); // a burst (wheel, held key) collapses to its last value
            }
            match requested {
                Some(volume) => {
                    let value = Volume((volume * Volume::NORMAL.0 as f32).round() as u32);
                    for (&index, &channels) in &mixer.borrow().ours {
                        let mut volumes = ChannelVolumes::default();
                        volumes.set(channels, value);
                        introspect.set_sink_input_volume(index, &volumes, None);
                    }
                }
                None => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }

    /// One non-blocking pump of the mainloop; errors end the thread.
    fn iterate(mainloop: &mut Mainloop) -> Result<(), String> {
        match mainloop.iterate(false) {
            IterateResult::Success(_) => Ok(()),
            IterateResult::Quit(_) => Err("mainloop quit".into()),
            IterateResult::Err(e) => Err(format!("mainloop: {e}")),
        }
    }
}

/// Exercises whichever backend the platform has against the real mixer. Both find the application's
/// streams by process id, so there is nothing to read until something is actually playing - hence
/// the sink, and hence the `sink` feature.
#[cfg(all(test, feature = "sink"))]
mod test {
    use super::*;
    use crate::sink;
    use std::time::Duration;

    /// The first reading, or `None` if none arrived in time.
    fn next_reading(control: &VolumeControl, within: Duration) -> Option<f32> {
        smol::block_on(smol::future::or(async { control.events.recv().await.ok() }, async {
            smol::Timer::after(within).await;
            None
        }))
    }

    /// The mixer must find this process's own playback and report its volume, and a set must come
    /// back as a reading - which together is the whole contract an application mirrors.
    ///
    /// Skips (rather than fails) where there is no mixer or no audio device: a headless CI box has
    /// neither, and this test is about the wiring, not about the machine having a sound card.
    #[test]
    fn reports_and_applies_our_own_volume() {
        let client =
            sink::Client { name: "phonoscule-volume-test".into(), description: "Phonoscule volume round-trip test".into() };
        // Held open for the test: the session exists only while a stream does.
        let _sink = sink::Sink::new(&client, crate::player::PLAYBACK_SAMPLE_RATE);
        let control = start();

        // The backend has to find the session first, which it does on its own schedule.
        let Some(initial) = next_reading(&control, Duration::from_secs(5)) else {
            eprintln!("skipping: no volume mixer in this environment");
            return;
        };
        assert!((0.0..=1.0).contains(&initial), "volume {initial} is not a factor of 100%");

        // A set echoes back. Half of whatever it was at, so the test neither depends on the
        // starting point nor lands on it (which would report nothing, the value being unchanged).
        let target = if initial > 0.02 { initial / 2.0 } else { 0.5 };
        control.set(target);
        let reading = next_reading(&control, Duration::from_secs(5));
        let reading = reading.expect("a set should be reported back");
        // Loose: mixers quantize (PulseAudio to 1/65536, Windows to its own curve).
        assert!((reading - target).abs() < 0.01, "set {target} but the mixer reported {reading}");

        // Leave the machine as we found it - both mixers remember a per-application volume.
        control.set(initial);
        let _ = next_reading(&control, Duration::from_secs(5));
    }
}
