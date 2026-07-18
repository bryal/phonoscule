//! The audio playback engine: a background task that owns the play queue, decodes tracks with
//! phonoscule, and pushes samples to a PulseAudio sink task.
//!
//! Largely mirrors the player task of `phonoscule-cli`; if the duplication grows it should move
//! into a shared crate.

use embedded_io_adapters::futures_03::FromFutures;
use phonoscule::{
    io::{Skippable, Take},
    metadata::StaticMetadata,
    opus::{OggOpus, OpusReader},
    plumbing::*,
    sample::{MultiReader, PcmS16Le, Stereo},
    wav::Wav,
};
use smol::{
    channel,
    fs::File,
    io::{AsyncReadExt, BufReader},
};
use std::{
    cmp::min,
    path::{Path, PathBuf},
    time::Duration,
};

pub const PLAYBACK_SAMPLE_RATE: u32 = 48000;

type OutSample = Stereo<PcmS16Le>;

/// Frames decoded and written to PulseAudio per loop iteration.
const CHUNK: usize = 512;

#[derive(Debug, Clone)]
pub enum Cmd {
    /// Replace the queue, opening the track at index `start` in the given state: `Playing` starts
    /// playback, `Paused` readies the track (its length is reported) without starting -- how a
    /// restored session comes back up.
    SetQueue {
        tracks: Vec<PathBuf>,
        start: usize,
        play: PlayState,
    },
    /// Append to the queue without interrupting playback.
    Append {
        tracks: Vec<PathBuf>,
    },
    /// Jump to the given queue index.
    JumpTo(usize),
    TogglePlayPause,
    Next,
    Prev,
    /// Absolute seek within the current track.
    Seek(Duration),
}

#[derive(Debug, Clone)]
pub enum Event {
    TrackStarted { ix: usize, len: Option<Duration> },
    Progress(Duration),
    PlayState(PlayState),
    QueueEnded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Playing,
    Paused,
}

impl PlayState {
    pub fn toggled(self) -> Self {
        match self {
            PlayState::Playing => PlayState::Paused,
            PlayState::Paused => PlayState::Playing,
        }
    }
}

/// Handle to the running engine. Dropping it stops the audio thread (which exits on its own once
/// its channels close); the OS reclaims the PulseAudio connection on exit.
pub struct Engine {
    pub cmd: channel::Sender<Cmd>,
    pub events: channel::Receiver<Event>,
    _audio: std::thread::JoinHandle<()>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Close both channels so the loop returns whether it is parked on the next command or on
        // sending an event. We deliberately do not join: the thread exits within a frame, and
        // blocking the GUI's own teardown on it risks stalling shutdown for no real gain (the OS
        // tears the PulseAudio connection down on exit regardless).
        self.cmd.close();
        self.events.close();
    }
}

pub fn start() -> Engine {
    // Commands come from the (synchronous) GUI update fn, so the channel is unbounded to make
    // sending non-blocking there.
    let (cmd_tx, cmd_rx) = channel::unbounded::<Cmd>();
    let (event_tx, event_rx) = channel::bounded::<Event>(256);
    // The engine owns a dedicated thread running a thread-local `block_on`. Decoding is async but
    // the PulseAudio writes block; a dedicated thread keeps that blocking off the GUI's shared
    // executor (which would also force these futures to be `Send`), and lets the loop park when
    // idle. Communication stays on the (`Send`) channels.
    let audio = std::thread::Builder::new()
        .name("phonoscule-audio".into())
        .spawn(move || smol::block_on(player_loop(cmd_rx, event_tx)))
        .expect("cannot spawn the audio thread");
    Engine { cmd: cmd_tx, events: event_rx, _audio: audio }
}

async fn player_loop(cmd_rx: channel::Receiver<Cmd>, events: channel::Sender<Event>) {
    // Opened once and reused across tracks; its blocking writes below pace playback to real time.
    let mut sink = PulseSink::new();
    let mut buf = [OutSample::default(); CHUNK];

    let mut queue: Vec<PathBuf> = vec![];
    let mut ix = 0usize;
    let mut start_at: u64 = 0; // samples into the track to start from
    let mut play_state = PlayState::Paused;
    // A non-seek command read early while coalescing a burst of seeks (see the Seek arm), taken
    // ahead of the channel on the next command read. At most one: coalescing overshoots by one.
    let mut buffered: Option<Cmd> = None;

    /// What the player loop must do after a command has been applied.
    #[must_use]
    enum AfterCmd {
        /// Abandon the current track (if any) and (re)open at the current queue index.
        Reopen,
        /// Carry on in the current state.
        Continue,
    }

    // Applies the commands that make sense in every state.
    fn apply_cmd(
        cmd: Cmd,
        queue: &mut Vec<PathBuf>,
        ix: &mut usize,
        start_at: &mut u64,
        play_state: &mut PlayState,
    ) -> AfterCmd {
        match cmd {
            Cmd::SetQueue { tracks, start, play } => {
                *queue = tracks;
                *ix = min(start, queue.len());
                *start_at = 0;
                *play_state = play;
                AfterCmd::Reopen
            }
            Cmd::Append { tracks } => {
                queue.extend(tracks);
                AfterCmd::Continue
            }
            Cmd::JumpTo(i) => {
                *ix = i;
                *start_at = 0;
                *play_state = PlayState::Playing;
                AfterCmd::Reopen
            }
            Cmd::Next => {
                *ix += 1;
                *start_at = 0;
                AfterCmd::Reopen
            }
            Cmd::Prev => {
                *ix = ix.saturating_sub(1);
                *start_at = 0;
                AfterCmd::Reopen
            }
            Cmd::TogglePlayPause => {
                *play_state = play_state.toggled();
                AfterCmd::Continue
            }
            Cmd::Seek(_) => AfterCmd::Continue, // meaningless without an open track
        }
    }

    'next_track: loop {
        // Idle when there is nothing (left) to play.
        let Some(path) = queue.get(ix).cloned() else {
            play_state = PlayState::Paused;
            if events.send(Event::QueueEnded).await.is_err() {
                return;
            }
            loop {
                let cmd = match buffered.take() {
                    Some(cmd) => cmd,
                    None => match cmd_rx.recv().await {
                        Ok(cmd) => cmd,
                        Err(_) => return,
                    },
                };
                match apply_cmd(cmd, &mut queue, &mut ix, &mut start_at, &mut play_state) {
                    AfterCmd::Reopen => continue 'next_track,
                    AfterCmd::Continue => match (play_state, queue.get(ix)) {
                        // Tracks appended and play pressed, in either order: start playing.
                        (PlayState::Playing, Some(_)) => continue 'next_track,
                        // Play on a finished queue: start it over from the top.
                        (PlayState::Playing, None) if !queue.is_empty() => {
                            ix = 0;
                            continue 'next_track;
                        }
                        // No autoplay surprises: a play press on an empty queue must not
                        // linger and start playback whenever tracks eventually arrive.
                        (PlayState::Playing, None) => play_state = PlayState::Paused,
                        (PlayState::Paused, _) => (),
                    },
                }
            }
        };

        log::debug!("opening file {path:?}");
        let Some(track) = Track::open(&path).await else {
            log::error!("failed to open {path:?}, skipping");
            ix += 1;
            start_at = 0;
            continue 'next_track;
        };
        let sample_rate = track.sample_rate;
        let t_of = |samples: u64| Duration::from_secs_f64(samples as f64 / sample_rate as f64);
        let t_end = track.len_samples.map(t_of);
        if events.send(Event::TrackStarted { ix, len: t_end }).await.is_err() {
            return;
        }
        let _ = events.send(Event::PlayState(play_state)).await;

        let mut source = track.samples;
        let mut pos = source.fast_forward(start_at).await.unwrap_or(0);
        start_at = 0;
        let _ = events.send(Event::Progress(t_of(pos))).await;
        let mut prev_status_pos = pos;

        loop {
            let maybe_cmd = match buffered.take() {
                Some(cmd) => Some(cmd),
                None => match play_state {
                    // Paused: nothing to do but wait for the next command.
                    PlayState::Paused => match cmd_rx.recv().await {
                        Ok(cmd) => Some(cmd),
                        Err(_) => return,
                    },
                    PlayState::Playing => match cmd_rx.try_recv() {
                        Ok(cmd) => Some(cmd),
                        Err(channel::TryRecvError::Empty) => None,
                        Err(channel::TryRecvError::Closed) => return,
                    },
                },
            };
            match maybe_cmd {
                Some(Cmd::Seek(t)) => {
                    // Coalesce a burst of seeks -- a held arrow key sends far more than the
                    // decoder can service (especially in debug builds), and every intermediate
                    // target is abandoned the instant the next arrives. Skip to the latest,
                    // stashing the command that ends the run so it is still handled in order.
                    let mut t = t;
                    loop {
                        match cmd_rx.try_recv() {
                            Ok(Cmd::Seek(next)) => t = next,
                            Ok(other) => {
                                buffered = Some(other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    let target = (t.as_secs_f64() * sample_rate as f64) as u64;
                    match source.seek_samples(target).await {
                        Some(new_pos) => {
                            pos = new_pos;
                            prev_status_pos = pos;
                            if events.send(Event::Progress(t_of(pos))).await.is_err() {
                                return;
                            }
                        }
                        None => {
                            // Restart the current track, fast-forwarding to the target.
                            start_at = target;
                            continue 'next_track;
                        }
                    }
                }
                Some(Cmd::Prev) if pos > 3 * sample_rate as u64 => {
                    // Like most players: an early "previous" goes to the previous track (handled
                    // by apply_cmd), a later one restarts the current track.
                    start_at = 0;
                    continue 'next_track;
                }
                Some(cmd) => {
                    let before = play_state;
                    match apply_cmd(cmd, &mut queue, &mut ix, &mut start_at, &mut play_state) {
                        AfterCmd::Reopen => continue 'next_track,
                        AfterCmd::Continue => {
                            if play_state != before {
                                let _ = events.send(Event::PlayState(play_state)).await;
                            }
                        }
                    }
                }
                None => (),
            }
            match play_state {
                PlayState::Paused => continue,
                PlayState::Playing => (),
            }
            let Some(n) = source.read_samples(&mut buf).await else {
                log::error!("error while decoding {path:?}, skipping to next track");
                ix += 1;
                continue 'next_track;
            };
            if n == 0 {
                ix += 1;
                continue 'next_track;
            }
            sink.write(&buf[..n]); // blocks until PulseAudio takes the chunk -- this is our pacing
            pos += n as u64;

            let progress_updates_per_sec = 16;
            let progress_interval = sample_rate as u64 / progress_updates_per_sec;
            if pos < prev_status_pos || pos - prev_status_pos > progress_interval {
                if events.send(Event::Progress(t_of(pos))).await.is_err() {
                    return;
                }
                prev_status_pos = pos;
            }
        }
    }
}

type FileReader = Skippable<FromFutures<BufReader<File>>>;

enum TrackSamples {
    Wav(MultiReader<Take<FileReader>>),
    // Boxed: the opus reader is large (decoder state + a decoded-frame buffer).
    Opus(Box<OpusReader<FileReader>>),
}

impl Source<OutSample> for TrackSamples {
    async fn read_samples(&mut self, buf: &mut [OutSample]) -> Option<usize> {
        match self {
            TrackSamples::Wav(s) => s.read_samples(buf).await,
            TrackSamples::Opus(s) => s.read_samples(buf).await,
        }
    }
}

impl FastForward for TrackSamples {
    async fn fast_forward(&mut self, nsamples: u64) -> Option<u64> {
        match self {
            TrackSamples::Wav(s) => s.fast_forward(nsamples).await,
            TrackSamples::Opus(s) => s.fast_forward(nsamples).await,
        }
    }
}

impl TrackSamples {
    /// Seeks within the open track when the format supports it. `None` means the caller should
    /// fall back to reopening the track and skipping forward (which for wav is itself cheap:
    /// skipping is a plain file seek there).
    async fn seek_samples(&mut self, target: u64) -> Option<u64> {
        match self {
            TrackSamples::Wav(_) => None,
            TrackSamples::Opus(s) => s.seek_samples(target).await,
        }
    }
}

struct Track {
    sample_rate: u32,
    len_samples: Option<u64>,
    samples: TrackSamples,
}

impl Track {
    async fn open(path: &Path) -> Option<Track> {
        let mut magic = [0u8; 4];
        File::open(path).await.ok()?.read_exact(&mut magic).await.ok()?;
        let f = Skippable(FromFutures::new(BufReader::new(File::open(path).await.ok()?)));
        match &magic {
            b"RIFF" => {
                let wav = Wav::<StaticMetadata, _>::parse(f).await?;
                Some(Track {
                    sample_rate: wav.format.sample_rate(),
                    len_samples: Some(wav.format.len_samples()),
                    samples: TrackSamples::Wav(wav.samples),
                })
            }
            b"OggS" => {
                let opus = OggOpus::<StaticMetadata, _>::parse_seekable(f).await?;
                Some(Track {
                    sample_rate: opus.format.sample_rate(),
                    len_samples: opus.format.len_samples,
                    samples: TrackSamples::Opus(Box::new(opus.samples)),
                })
            }
            _ => {
                log::error!("{path:?} has an unrecognized format (neither RIFF/WAVE nor Ogg)");
                None
            }
        }
    }
}

struct PulseSink(pulse_simple::Playback<[i16; 2]>);

impl PulseSink {
    fn new() -> Self {
        Self(pulse_simple::Playback::<[i16; 2]>::new(
            "phonoscule-gui",
            "GUI application based on the Phonoscule music player library",
            None,
            PLAYBACK_SAMPLE_RATE,
        ))
    }

    /// Writes one chunk, blocking until PulseAudio accepts it (its buffer paces us to real time).
    fn write(&mut self, samples: &[OutSample]) {
        if samples.is_empty() {
            return;
        }
        let pulse_samples = unsafe { core::mem::transmute::<&[OutSample], &[[i16; 2]]>(samples) };
        assert_eq!(core::mem::size_of_val(samples), core::mem::size_of_val(pulse_samples));
        self.0.write(pulse_samples);
    }
}
