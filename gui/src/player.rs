//! The audio playback engine: a background task that owns the play queue, decodes tracks with
//! phonoscule, and pushes samples to a PulseAudio sink task.
//!
//! Largely mirrors the player task of `phonoscule-cli`; if the duplication grows it should move
//! into a shared crate.

use embedded_io_adapters::futures_03::FromFutures;
use phonoscule::{
    io::{Skippable, Take},
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

/// A queue entry: the track, and the album it belongs to as an opaque grouping key (equal keys on
/// adjacent entries form an album run) -- what repeat-album advancement walks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub album: u64,
}

/// What happens when a track ends on its own. Manual skips always move (a Next during
/// [`Repeat::Track`] plays -- and then repeats -- the next track); at the queue's ends they wrap
/// only under [`Repeat::Playlist`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Repeat {
    /// Play through the queue once and stop.
    #[default]
    Off,
    /// Loop the current track.
    Track,
    /// Loop the current album run.
    Album,
    /// Wrap around at the end of the queue.
    Playlist,
}

impl Repeat {
    /// The next mode in the cycle the UI's repeat button steps through.
    pub fn cycled(self) -> Self {
        match self {
            Repeat::Off => Repeat::Track,
            Repeat::Track => Repeat::Album,
            Repeat::Album => Repeat::Playlist,
            Repeat::Playlist => Repeat::Off,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Cmd {
    /// Replace the queue, opening the track at index `start` in the given state: `Playing` starts
    /// playback, `Paused` readies the track (its length is reported) without starting -- how a
    /// restored session comes back up.
    SetQueue {
        tracks: Vec<Entry>,
        start: usize,
        play: PlayState,
    },
    /// Append to the queue without interrupting playback.
    Append {
        tracks: Vec<Entry>,
    },
    /// Replace the queue with a reordering of itself -- same tracks, new order -- without
    /// interrupting playback: `current` must be the playing track's index in the new order.
    /// How a shuffle lands.
    Reorder {
        tracks: Vec<Entry>,
        current: usize,
    },
    /// Jump to the given queue index.
    JumpTo(usize),
    TogglePlayPause,
    Next,
    Prev,
    /// Absolute seek within the current track.
    Seek(Duration),
    SetRepeat(Repeat),
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
    // Reused across tracks (its blocking writes pace playback to real time), reopened only when a
    // track's sample rate differs from the stream's -- see `ensure_rate` before each track.
    let mut sink = PulseSink::new(PLAYBACK_SAMPLE_RATE);
    let mut buf = [OutSample::default(); CHUNK];

    let mut queue: Vec<Entry> = vec![];
    let mut ix = 0usize;
    let mut start_at: u64 = 0; // samples into the track to start from
    let mut play_state = PlayState::Paused;
    let mut repeat = Repeat::Off;
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
        queue: &mut Vec<Entry>,
        ix: &mut usize,
        start_at: &mut u64,
        play_state: &mut PlayState,
        repeat: &mut Repeat,
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
            Cmd::Reorder { tracks, current } => {
                // Same tracks in a new order: the open track keeps decoding, only the cursor
                // follows it (the caller guarantees `tracks[current]` is the playing track).
                *queue = tracks;
                *ix = min(current, queue.len());
                AfterCmd::Continue
            }
            Cmd::JumpTo(i) => {
                *ix = i;
                *start_at = 0;
                *play_state = PlayState::Playing;
                AfterCmd::Reopen
            }
            // Manual skips move even under Repeat::Track (the new track repeats instead), and
            // wrap at the queue's ends only under Repeat::Playlist.
            Cmd::Next => {
                *ix = if *repeat == Repeat::Playlist && *ix + 1 >= queue.len() && !queue.is_empty() { 0 } else { *ix + 1 };
                *start_at = 0;
                AfterCmd::Reopen
            }
            Cmd::Prev => {
                *ix = match (*repeat, *ix) {
                    (Repeat::Playlist, 0) if !queue.is_empty() => queue.len() - 1,
                    _ => ix.saturating_sub(1),
                };
                *start_at = 0;
                AfterCmd::Reopen
            }
            Cmd::TogglePlayPause => {
                *play_state = play_state.toggled();
                AfterCmd::Continue
            }
            Cmd::Seek(_) => AfterCmd::Continue, // meaningless without an open track
            Cmd::SetRepeat(mode) => {
                *repeat = mode;
                AfterCmd::Continue
            }
        }
    }

    'next_track: loop {
        // Idle when there is nothing (left) to play.
        let Some(path) = queue.get(ix).map(|entry| entry.path.clone()) else {
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
                match apply_cmd(cmd, &mut queue, &mut ix, &mut start_at, &mut play_state, &mut repeat) {
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
        // Match the output stream to this track's rate (a no-op unless it changed): the samples
        // are played at the rate they were decoded at, so nothing plays fast or slow.
        sink.ensure_rate(sample_rate);
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
                    match apply_cmd(cmd, &mut queue, &mut ix, &mut start_at, &mut play_state, &mut repeat) {
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
                // Plain +1 regardless of the repeat mode: repeating a broken track would loop the
                // error forever.
                log::error!("error while decoding {path:?}, skipping to next track");
                ix += 1;
                continue 'next_track;
            };
            if n == 0 {
                // The track ended on its own: the repeat mode decides what plays next.
                ix = next_track_ix(&queue, ix, repeat);
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

/// The queue index to play after the track at `ix` ends on its own, per the repeat mode. An index
/// past the end means "stop" -- the loop's idle branch takes over.
fn next_track_ix(queue: &[Entry], ix: usize, repeat: Repeat) -> usize {
    match repeat {
        Repeat::Off => ix + 1,
        Repeat::Track => ix,
        Repeat::Album => {
            // The next track of the current album run, wrapping to the run's first track.
            let Some(album) = queue.get(ix).map(|entry| entry.album) else { return ix + 1 };
            if queue.get(ix + 1).is_some_and(|entry| entry.album == album) {
                ix + 1
            } else {
                let mut first = ix;
                while first > 0 && queue[first - 1].album == album {
                    first -= 1;
                }
                first
            }
        }
        Repeat::Playlist => {
            if ix + 1 >= queue.len() && !queue.is_empty() {
                0
            } else {
                ix + 1
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
                // Playback doesn't use the tags (the queue items carry them): drop them.
                let wav = Wav::parse(f, |_| {}).await?;
                Some(Track {
                    sample_rate: wav.format.sample_rate(),
                    len_samples: Some(wav.format.len_samples()),
                    samples: TrackSamples::Wav(wav.samples),
                })
            }
            b"OggS" => {
                let opus = OggOpus::parse_seekable(f, |_| {}).await?;
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

struct PulseSink {
    out: pulse_simple::Playback<[i16; 2]>,
    /// The rate the stream was opened at, so the loop can tell when a track needs a new one.
    rate: u32,
}

impl PulseSink {
    fn new(rate: u32) -> Self {
        let out = pulse_simple::Playback::<[i16; 2]>::new(
            "phonoscule-gui",
            "GUI application based on the Phonoscule music player library",
            None,
            rate,
        );
        Self { out, rate }
    }

    /// Reopens the stream at `rate` if it differs from the current one. The samples we output are
    /// always 16-bit stereo, so only the rate can change between tracks; the audio server handles
    /// converting it to the device's rate, so we never resample ourselves.
    fn ensure_rate(&mut self, rate: u32) {
        if self.rate != rate {
            *self = Self::new(rate);
        }
    }

    /// Writes one chunk, blocking until PulseAudio accepts it (its buffer paces us to real time).
    fn write(&mut self, samples: &[OutSample]) {
        if samples.is_empty() {
            return;
        }
        let pulse_samples = unsafe { core::mem::transmute::<&[OutSample], &[[i16; 2]]>(samples) };
        assert_eq!(core::mem::size_of_val(samples), core::mem::size_of_val(pulse_samples));
        self.out.write(pulse_samples);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Two albums: run 1 is tracks 0-2, run 2 is tracks 3-4.
    fn queue() -> Vec<Entry> {
        [(0, 1), (1, 1), (2, 1), (3, 2), (4, 2)]
            .into_iter()
            .map(|(n, album)| Entry { path: PathBuf::from(format!("{n}.opus")), album })
            .collect()
    }

    #[test]
    fn auto_advance_by_repeat_mode() {
        let q = queue();
        assert_eq!(next_track_ix(&q, 1, Repeat::Off), 2, "off: the next track");
        assert_eq!(next_track_ix(&q, 4, Repeat::Off), 5, "off: past the end means stop");
        assert_eq!(next_track_ix(&q, 1, Repeat::Track), 1, "track: loop the current track");
        assert_eq!(next_track_ix(&q, 1, Repeat::Album), 2, "album: the next track of the run");
        assert_eq!(next_track_ix(&q, 2, Repeat::Album), 0, "album: the run's end wraps to its start");
        assert_eq!(next_track_ix(&q, 4, Repeat::Album), 3, "album: ditto for the final run");
        assert_eq!(next_track_ix(&q, 1, Repeat::Playlist), 2, "playlist: the next track");
        assert_eq!(next_track_ix(&q, 4, Repeat::Playlist), 0, "playlist: the queue's end wraps to its start");
    }
}
