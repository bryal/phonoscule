//! The audio output: where the [`player`](crate::player) engine's decoded frames go.
//!
//! One blocking interface - [`Sink::write`] returns once the device has taken the chunk - because
//! that blocking *is* the engine's pacing: it decodes exactly as fast as the audio clock drains.
//! Only the rate can change between tracks (everything is reduced to 16-bit stereo before it gets
//! here), which [`Sink::ensure_rate`] handles by reopening the stream and letting the audio server
//! resample, so we never resample ourselves.
//!
//! Two backends, picked at compile time. Linux speaks PulseAudio through `pulse-simple` (PipeWire
//! included, via pipewire-pulse). Windows speaks WASAPI in shared mode, letting the audio engine
//! convert our rate to the device's (`AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`) - the same division of
//! labour. Anywhere else - and on either platform when no device can be opened - playback falls
//! back to [`Silence`], which paces off the wall clock so the queue still advances in real time.
//!
//! Wants std and an audio device.

use crate::sample::{PcmS16Le, Stereo};
use std::time::{Duration, Instant};

/// One frame of output: interleaved 16-bit little-endian stereo, what every decoder reduces to.
pub type Frame = Stereo<PcmS16Le>;

/// How the audio server identifies this application, e.g. in a volume mixer.
#[derive(Debug, Clone)]
pub struct Client {
    pub name: String,
    pub description: String,
}

/// An open output stream. Reopened only when a track's sample rate differs from the current one.
pub struct Sink {
    client: Client,
    /// The rate the stream was opened at, so the caller can tell when a track needs a new one.
    rate: u32,
    out: Out,
}

enum Out {
    Device(backend::Sink),
    /// No device could be opened: the queue still runs, just inaudibly.
    Silent(Silence),
}

impl Sink {
    /// Opens the platform's output at `rate`. Never fails: without a device the frames go nowhere
    /// (see [`Silence`]), which keeps a missing or busy sound card from taking the player down.
    pub fn new(client: &Client, rate: u32) -> Self {
        let out = match backend::Sink::open(client, rate) {
            Ok(sink) => Out::Device(sink),
            Err(e) => {
                log::warn!("no audio output ({e}); playing silently");
                Out::Silent(Silence::new(rate))
            }
        };
        Sink { client: client.clone(), rate, out }
    }

    /// Reopens the stream at `rate` if it differs from the current one. The frames we output are
    /// always 16-bit stereo, so the rate is the only thing that can change between tracks.
    pub fn ensure_rate(&mut self, rate: u32) {
        if self.rate != rate {
            *self = Self::new(&self.client.clone(), rate);
        }
    }

    /// Writes one chunk, blocking until the device has taken it - this is the engine's pacing.
    ///
    /// A device lost mid-stream (unplugged, or the default endpoint changed under us) is reopened
    /// once here rather than surfaced: the chunk that hit the error is dropped, and playback
    /// continues on the new device within a chunk.
    pub fn write(&mut self, frames: &[Frame]) {
        if frames.is_empty() {
            return;
        }
        match &mut self.out {
            Out::Silent(silence) => silence.write(frames.len()),
            Out::Device(sink) => {
                if let Err(e) = sink.write(frames) {
                    log::warn!("audio output failed ({e}); reopening");
                    *self = Self::new(&self.client.clone(), self.rate);
                }
            }
        }
    }
}

/// The deviceless fallback: paces off the wall clock so a queue played without an audio device
/// still advances at the speed it would have, rather than spinning through itself as fast as it
/// decodes.
struct Silence {
    rate: u32,
    /// When the stream started, and how many frames have been "played" since - the two together
    /// give the instant the next chunk is due, without drift accumulating per chunk.
    started: Instant,
    frames: u64,
}

impl Silence {
    fn new(rate: u32) -> Self {
        Silence { rate, started: Instant::now(), frames: 0 }
    }

    fn write(&mut self, frames: usize) {
        self.frames += frames as u64;
        let due = Duration::from_secs_f64(self.frames as f64 / self.rate as f64);
        // Saturating: if we have fallen behind (a slow decode, a suspended process) the chunk is
        // already overdue and we carry straight on.
        if let Some(wait) = due.checked_sub(self.started.elapsed()) {
            std::thread::sleep(wait);
        }
    }
}

/// The PulseAudio backend. `pulse-simple`'s writes block until the server takes the chunk, which is
/// exactly the contract [`Sink::write`] wants, so there is little to do here.
#[cfg(target_os = "linux")]
mod backend {
    use super::{Client, Frame};

    pub struct Sink(pulse_simple::Playback<[i16; 2]>);

    impl Sink {
        pub fn open(client: &Client, rate: u32) -> Result<Self, String> {
            Ok(Sink(pulse_simple::Playback::new(&client.name, &client.description, None, rate)))
        }

        pub fn write(&mut self, frames: &[Frame]) -> Result<(), String> {
            // `Frame` is a `#[repr(C)]` pair of little-endian 16-bit scalars, so it is laid out
            // exactly like the `[i16; 2]` the binding wants on the little-endian targets Pulse
            // runs on.
            let samples = unsafe { core::mem::transmute::<&[Frame], &[[i16; 2]]>(frames) };
            assert_eq!(core::mem::size_of_val(frames), core::mem::size_of_val(samples));
            self.0.write(samples);
            Ok(())
        }
    }
}

/// The WASAPI backend (see the module docs).
///
/// A shared-mode render stream in our own audio session - which is what makes the process show up
/// in the Windows volume mixer, and what [`volume`](crate::volume) then attaches to. The stream is
/// opened at the track's own rate with `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`, so the audio engine
/// resamples to the device's mix format for us.
#[cfg(target_os = "windows")]
mod backend {
    use super::{Client, Frame};
    use std::cell::Cell;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
        AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        WAVE_FORMAT_PCM, WAVEFORMATEX, eConsole, eRender,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    /// How much audio the device buffers, in 100-nanosecond units (WASAPI's `REFERENCE_TIME`).
    /// 200 ms is roomy enough that a slow decode - a debug build opening a cold file - does not
    /// underrun, and short enough that a pause or a skip is not heard trailing off.
    const BUFFER_DURATION: i64 = 200 * 10_000;

    /// How long to wait for the device to ask for more before looking at the buffer again. A cap
    /// on the wait, so a wedged or vanished endpoint cannot park the audio thread forever:
    /// generous next to [`BUFFER_DURATION`], but finite.
    const WAIT_TIMEOUT_MS: u32 = 2000;

    pub struct Sink {
        client: IAudioClient,
        render: IAudioRenderClient,
        /// Signalled by WASAPI whenever it has drained a period's worth and wants more.
        ready: HANDLE,
        /// The render buffer's size in frames, against which `GetCurrentPadding` says how much of
        /// it is still unplayed.
        capacity: u32,
    }

    // The COM interfaces are apartment-bound, but the sink never leaves the audio thread that
    // opened it - the engine owns it start to finish - so this is only about moving it there.
    unsafe impl Send for Sink {}

    impl Sink {
        pub fn open(client: &Client, rate: u32) -> Result<Self, String> {
            unsafe { open(client, rate) }.map_err(|e| e.message())
        }

        pub fn write(&mut self, frames: &[Frame]) -> Result<(), String> {
            unsafe { self.write_all(frames) }.map_err(|e| e.message())
        }

        /// Feeds the whole chunk to the device, waiting for room whenever the buffer is full.
        /// Returns once the last frame has been handed over - not once it has been heard.
        unsafe fn write_all(&mut self, frames: &[Frame]) -> windows::core::Result<()> {
            let mut rest = frames;
            while !rest.is_empty() {
                let padding = unsafe { self.client.GetCurrentPadding() }?;
                let room = self.capacity.saturating_sub(padding) as usize;
                if room == 0 {
                    // The buffer is full: nothing to do until the device has played some of it.
                    // A timeout is not an error - we just look at the padding again.
                    unsafe { WaitForSingleObject(self.ready, WAIT_TIMEOUT_MS) };
                    continue;
                }
                let n = room.min(rest.len());
                let buffer = unsafe { self.render.GetBuffer(n as u32) }?;
                let (src, tail) = rest.split_at(n);
                // WASAPI hands back an uninitialised region of exactly `n` frames in the format the
                // stream was opened with, which is `Frame`'s own layout: interleaved 16-bit
                // little-endian stereo.
                unsafe { std::ptr::copy_nonoverlapping(src.as_ptr().cast::<u8>(), buffer, size_of_val(src)) };
                unsafe { self.render.ReleaseBuffer(n as u32, 0) }?;
                rest = tail;
            }
            Ok(())
        }
    }

    impl Drop for Sink {
        fn drop(&mut self) {
            // Stop before the event handle goes: WASAPI must not be left holding a closed handle.
            let _ = unsafe { self.client.Stop() };
            let _ = unsafe { CloseHandle(self.ready) };
        }
    }

    unsafe fn open(client: &Client, rate: u32) -> windows::core::Result<Sink> {
        init_com();
        // 16-bit stereo PCM. Plain `WAVEFORMATEX` rather than the extensible form: for one or two
        // channels of standard PCM that is exactly what WASAPI expects.
        let channels: u16 = 2;
        let bits: u16 = 16;
        let block_align = channels * bits / 8;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * block_align as u32,
            nBlockAlign: block_align,
            wBitsPerSample: bits,
            cbSize: 0,
        };

        let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;
        // The default endpoint for ordinary playback, and whatever the user later makes default:
        // reopening on `AUDCLNT_E_DEVICE_INVALIDATED` (see `Sink::write`) is what follows them.
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }?;
        let audio: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }?;
        unsafe {
            audio.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                // Event-driven, so a full buffer parks the thread rather than polling it; and
                // AUTOCONVERTPCM so the engine resamples our rate to the device's mix format
                // (without it, `Initialize` rejects any rate but the device's own).
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                BUFFER_DURATION,
                0, // shared mode takes the engine's own period
                &format,
                None,
            )
        }?;

        // Auto-reset and initially unsignalled: each wait consumes one "device wants more".
        let ready = unsafe { CreateEventW(None, false, false, None) }?;
        unsafe { audio.SetEventHandle(ready) }?;
        let capacity = unsafe { audio.GetBufferSize() }?;
        let render: IAudioRenderClient = unsafe { audio.GetService() }?;
        unsafe { audio.Start() }?;

        // The mixer entry is this application's; name it before anyone looks. Failing to is
        // cosmetic (the mixer falls back to the executable name), so it must not fail the open.
        if let Err(e) = unsafe { name_session(&audio, client) } {
            log::debug!("could not name the audio session: {e}");
        }
        Ok(Sink { client: audio, render, ready, capacity })
    }

    /// Labels our audio session, so the Windows volume mixer lists the player under the name it
    /// gave rather than `phonoscule-gui.exe`.
    unsafe fn name_session(audio: &IAudioClient, client: &Client) -> windows::core::Result<()> {
        use windows::Win32::Media::Audio::IAudioSessionControl;
        use windows::core::HSTRING;

        let control: IAudioSessionControl = unsafe { audio.GetService() }?;
        unsafe { control.SetDisplayName(&HSTRING::from(&client.description), std::ptr::null()) }
    }

    /// COM, once per thread. The audio thread lives as long as the engine does, so there is no
    /// matching `CoUninitialize`: tearing the apartment down while the sink's interfaces are still
    /// alive is what would be wrong.
    fn init_com() {
        thread_local! { static DONE: Cell<bool> = const { Cell::new(false) } }
        DONE.with(|done| {
            if !done.replace(true) {
                // A failure here is `RPC_E_CHANGED_MODE` - the thread is already in an apartment,
                // which is fine for us - or genuinely fatal, in which case the very next COM call
                // says so with a better message.
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            }
        });
    }
}

/// The deviceless fallback for platforms with no backend of their own: opening always fails, so
/// [`Sink`] plays through [`Silence`].
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod backend {
    use super::{Client, Frame};

    pub struct Sink(());

    impl Sink {
        pub fn open(_client: &Client, _rate: u32) -> Result<Self, String> {
            Err("no audio backend for this platform".into())
        }

        pub fn write(&mut self, _frames: &[Frame]) -> Result<(), String> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The fallback's whole job is to take real time doing nothing, so the engine above it decodes
    /// at playback speed rather than as fast as it can.
    #[test]
    fn silence_paces_off_the_wall_clock() {
        let mut silence = Silence::new(48000);
        let start = Instant::now();
        for _ in 0..10 {
            silence.write(4800); // 100 ms each
        }
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(950), "a second of frames took only {elapsed:?}");
        assert!(elapsed < Duration::from_millis(2000), "a second of frames took {elapsed:?}");
    }

    /// Chunks are due at absolute offsets from the start, so a chunk that ran late is not paid for
    /// twice - an early stall must not push the whole stream back.
    #[test]
    fn silence_does_not_accumulate_drift() {
        let mut silence = Silence::new(48000);
        std::thread::sleep(Duration::from_millis(300)); // fall behind before writing anything
        let start = Instant::now();
        silence.write(4800); // 100 ms, already overdue
        assert!(start.elapsed() < Duration::from_millis(50), "an overdue chunk still waited");
    }
}
