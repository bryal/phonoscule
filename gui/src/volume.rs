//! Per-application volume, delegated to the OS mixer -- never applied to the samples we decode.
//!
//! The interface is platform-neutral: [`start`] spawns whatever backend the platform has and
//! hands back a [`VolumeControl`], whose readings -- the initial one, and external changes from
//! any mixer UI -- stream out as plain factors (1.0 = 100%), so the GUI can mirror the mixer
//! without polling. A platform without a backend simply never reports (the GUI shows no volume
//! bar) and swallows sets.
//!
//! The Linux backend speaks PulseAudio (PipeWire included, via pipewire-pulse): a dedicated
//! thread finds the application's sink inputs (by process id -- the playback stream comes from
//! `pulse-simple`, which doesn't expose its stream), subscribes to their changes, and applies
//! [`VolumeControl::set`] commands. The server remembers the volume across restarts
//! (stream-restore), so nothing is persisted here. A future Windows port would slot a WASAPI
//! backend (`ISimpleAudioVolume` on our audio session) in beside it.

use smol::channel;
use std::sync::mpsc;

/// The ceiling the GUI exposes, as a factor of the mixer's 100%: PulseAudio allows software
/// amplification above normal, useful for quiet recordings. (WASAPI session volume ends at 100%,
/// so a Windows backend would cap this.)
pub const MAX_VOLUME: f32 = 1.25;

/// Handle for adjusting the volume; readings arrive on [`VolumeControl::events`].
pub struct VolumeControl {
    cmd: mpsc::Sender<f32>,
    /// The volume as the mixer reports it, as a factor of 100%: an initial reading shortly after
    /// startup, then one per change -- our own sets included, and external ones (say, a system
    /// mixer applet) too.
    pub events: channel::Receiver<f32>,
}

impl VolumeControl {
    /// Requests the given volume (a factor of 100%, clamped to `0.0..=`[`MAX_VOLUME`]) for all of
    /// the application's audio streams. Fire-and-forget; the eventual reading echoes back on
    /// [`events`](VolumeControl::events).
    pub fn set(&self, volume: f32) {
        // Sends only fail when there is no backend (or its server is gone): ignored by design.
        let _ = self.cmd.send(volume.clamp(0.0, MAX_VOLUME));
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

/// The mixerless fallback: readings never arrive, so the GUI never shows a volume bar, and the
/// dropped command channel makes sets no-ops.
#[cfg(not(target_os = "linux"))]
mod backend {
    pub fn spawn(_commands: std::sync::mpsc::Receiver<f32>, _events: smol::channel::Sender<f32>) {}
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
        /// The last volume reported to the GUI, to swallow no-op change events (our own sets echo
        /// back as one).
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
