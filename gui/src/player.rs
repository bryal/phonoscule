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

#[derive(Debug, Clone)]
pub enum Cmd {
    /// Replace the queue and start playing at index `start`.
    SetQueue { tracks: Vec<PathBuf>, start: usize },
    /// Append to the queue without interrupting playback.
    Append { tracks: Vec<PathBuf> },
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
    Playing(bool),
    QueueEnded,
}

/// Handle to the running engine. Dropping it stops both tasks.
pub struct Engine {
    pub cmd: channel::Sender<Cmd>,
    pub events: channel::Receiver<Event>,
    _pulse_task: smol::Task<()>,
    _player_task: smol::Task<()>,
}

pub fn start() -> Engine {
    // Commands come from the (synchronous) GUI update fn, so the channel is unbounded to make
    // sending non-blocking there.
    let (cmd_tx, cmd_rx) = channel::unbounded::<Cmd>();
    let (event_tx, event_rx) = channel::bounded::<Event>(256);
    let (audio_tx, audio_rx) = sample_channel::<512, OutSample>(1);
    let _pulse_task = smol::spawn(pulse_task(audio_rx));
    let _player_task = smol::spawn(player_task(cmd_rx, event_tx, audio_tx));
    Engine { cmd: cmd_tx, events: event_rx, _pulse_task, _player_task }
}

async fn pulse_task(audio_rx: RecvSamples<512, OutSample>) {
    let pulse_sink = PulseSink::new();
    let mut chan_to_sink = ConnectSink::from_input(audio_rx, pulse_sink);
    while chan_to_sink.push().await.is_some() {}
    log::debug!("pulse sink closing");
}

async fn player_task(
    cmd_rx: channel::Receiver<Cmd>,
    events: channel::Sender<Event>,
    mut audio_tx: SendSamples<512, OutSample>,
) {
    let mut queue: Vec<PathBuf> = vec![];
    let mut ix = 0usize;
    let mut start_at: u64 = 0; // samples into the track to start from
    let mut playing = false;

    // Applies the commands that make sense in every state. Returns true if the current track (if
    // any) must be abandoned and the outer loop restarted.
    fn apply_cmd(
        cmd: Cmd,
        queue: &mut Vec<PathBuf>,
        ix: &mut usize,
        start_at: &mut u64,
        playing: &mut bool,
    ) -> bool {
        match cmd {
            Cmd::SetQueue { tracks, start } => {
                *queue = tracks;
                *ix = min(start, queue.len());
                *start_at = 0;
                *playing = true;
                true
            }
            Cmd::Append { tracks } => {
                queue.extend(tracks);
                false
            }
            Cmd::JumpTo(i) => {
                *ix = i;
                *start_at = 0;
                *playing = true;
                true
            }
            Cmd::Next => {
                *ix += 1;
                *start_at = 0;
                true
            }
            Cmd::Prev => {
                *ix = ix.saturating_sub(1);
                *start_at = 0;
                true
            }
            Cmd::TogglePlayPause => {
                *playing = !*playing;
                false
            }
            Cmd::Seek(_) => false, // meaningless without a playing track
        }
    }

    'next_track: loop {
        // Idle when there is nothing (left) to play.
        let Some(path) = queue.get(ix).cloned() else {
            playing = false;
            if events.send(Event::QueueEnded).await.is_err() {
                return;
            }
            loop {
                let Ok(cmd) = cmd_rx.recv().await else { return };
                let jump = apply_cmd(cmd, &mut queue, &mut ix, &mut start_at, &mut playing);
                // Tracks may just have been appended, and/or play was pressed: leave the idle
                // state once there is something at the current index to play.
                if jump || (playing && queue.get(ix).is_some()) {
                    continue 'next_track;
                }
                // No autoplay surprises: don't let e.g. a play press on an empty queue linger
                // and start playback whenever tracks eventually arrive.
                playing = false;
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
        let _ = events.send(Event::Playing(playing)).await;

        let mut source = track.samples;
        let mut pos = source.fast_forward(start_at).await.unwrap_or(0);
        start_at = 0;
        let _ = events.send(Event::Progress(t_of(pos))).await;
        let mut prev_status_pos = pos;
        let mut chan_from_source = ConnectSource::to_output(source, &mut audio_tx);

        loop {
            let maybe_cmd = if !playing {
                match cmd_rx.recv().await {
                    Ok(cmd) => Some(cmd),
                    Err(_) => return,
                }
            } else {
                match cmd_rx.try_recv() {
                    Ok(cmd) => Some(cmd),
                    Err(channel::TryRecvError::Empty) => None,
                    Err(channel::TryRecvError::Closed) => return,
                }
            };
            match maybe_cmd {
                Some(Cmd::Seek(t)) => {
                    let target = (t.as_secs_f64() * sample_rate as f64) as u64;
                    match chan_from_source.source_mut().seek_samples(target).await {
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
                    let toggled = matches!(cmd, Cmd::TogglePlayPause);
                    if apply_cmd(cmd, &mut queue, &mut ix, &mut start_at, &mut playing) {
                        continue 'next_track;
                    }
                    if toggled {
                        let _ = events.send(Event::Playing(playing)).await;
                    }
                }
                None => (),
            }
            if playing {
                let Some(n) = chan_from_source.pull().await else {
                    log::error!("error while decoding {path:?}, skipping to next track");
                    ix += 1;
                    continue 'next_track;
                };
                if n == 0 {
                    ix += 1;
                    continue 'next_track;
                }
                pos += n;

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

struct StaticVec<const N: usize, T> {
    len: usize,
    buf: [T; N],
}

impl<const N: usize, T> StaticVec<N, T> {
    fn from(buf: [T; N]) -> Self {
        Self { len: N, buf }
    }

    fn truncate(&mut self, len: usize) {
        self.len = min(self.len, len);
    }

    fn as_slice(&self) -> &[T] {
        &self.buf[..self.len]
    }
}

struct SendSamples<const N: usize, Sample>(channel::Sender<StaticVec<N, Sample>>);

impl<const N: usize, Sample> SourceOutput<Sample> for SendSamples<N, Sample>
where
    Sample: Default + Copy,
{
    async fn read_samples_from<S: Source<Sample>>(&mut self, source: &mut S) -> Option<u64> {
        let mut buf = [Sample::default(); N];
        let nread = source.read_samples(&mut buf).await?;
        let mut vec = StaticVec::from(buf);
        vec.truncate(nread);
        self.0.send(vec).await.ok()?;
        Some(nread as u64)
    }
}

struct RecvSamples<const N: usize, Sample>(channel::Receiver<StaticVec<N, Sample>>);

impl<const N: usize, Sample> SinkInput<Sample> for RecvSamples<N, Sample> {
    async fn write_samples_to<S: Sink<Sample>>(&mut self, sink: &mut S) -> Option<u64> {
        let samples = self.0.recv().await.ok()?;
        let n = sink.write_samples(samples.as_slice()).await?;
        Some(n as u64)
    }
}

fn sample_channel<const N: usize, Sample>(nchunks: usize) -> (SendSamples<N, Sample>, RecvSamples<N, Sample>) {
    let (tx, rx) = channel::bounded(nchunks);
    (SendSamples(tx), RecvSamples(rx))
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
}

impl Sink<Stereo<PcmS16Le>> for PulseSink {
    async fn write_samples(&mut self, samples: &[Stereo<PcmS16Le>]) -> Option<usize> {
        if samples.is_empty() {
            return Some(0);
        }
        let pulse_samples = unsafe { core::mem::transmute::<&[Stereo<PcmS16Le>], &[[i16; 2]]>(samples) };
        assert_eq!(core::mem::size_of_val(samples), core::mem::size_of_val(pulse_samples));
        self.0.write(pulse_samples);
        Some(samples.len())
    }
}

unsafe impl Send for PulseSink {}
