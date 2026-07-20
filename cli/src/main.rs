mod logger;

use clap::{CommandFactory, FromArgMatches, Parser};
use crossterm::{
    self as ct,
    event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind},
};
use embedded_io_adapters::futures_03::FromFutures;
use futures::{FutureExt, StreamExt};
use phonoscule::{
    io::*,
    metadata::*,
    opus::{OggOpus, OpusReader},
    plumbing::*,
    sample::*,
    wav::*,
};
use smol::{
    Timer, channel,
    fs::File,
    io::{AsyncReadExt, BufReader},
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const PLAYBACK_SAMPLE_RATE: u32 = 48000;

type OutSample = Stereo<PcmS16Le>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayState {
    Playing,
    Paused,
}

impl PlayState {
    fn toggled(self) -> Self {
        match self {
            PlayState::Playing => PlayState::Paused,
            PlayState::Paused => PlayState::Playing,
        }
    }
}

#[derive(Debug)]
enum Cmd {
    PlayPause,
    Restart,
    SeekForward(Duration),
    SeekBackward(Duration),
    Prev,
    Next,
}

#[derive(Debug)]
enum Status {
    Track(String),
    /// Current position and, if known, the total length of the track.
    Progress(Duration, Option<Duration>),
    Finished,
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
    title: String,
    sample_rate: u32,
    /// Total number of samples, when the container states it up front (WAV does, Ogg doesn't).
    len_samples: Option<u64>,
    samples: TrackSamples,
}

impl Track {
    async fn open(path: &Path) -> Option<Track> {
        let mut magic = [0u8; 4];
        File::open(path).await.ok()?.read_exact(&mut magic).await.ok()?;
        let f = Skippable(FromFutures::new(BufReader::new(File::open(path).await.ok()?)));
        let mut title = String::new();
        let on_tag = |tag: Tag<'_>| {
            if let Tag::Title(s) = tag {
                s.clone_into(&mut title);
            }
        };
        match &magic {
            b"RIFF" => {
                let wav = Wav::parse(f, on_tag).await?;
                Some(Track {
                    title,
                    sample_rate: wav.format.sample_rate(),
                    len_samples: Some(wav.format.len_samples()),
                    samples: TrackSamples::Wav(wav.samples),
                })
            }
            b"OggS" => {
                let opus = OggOpus::parse_seekable(f, on_tag).await?;
                Some(Track {
                    title,
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

// something like this when feature "seek" or "cache" or "std" or whatever?
// A wrapper for a Source that remembers history & reads ahead in chunks.
// Size limit. Don't cache all history forever for a live stream, for example.
// If trying to seek uncached chunk, call seek on underlying source.
// If that seek returns None, just propagate that. UI will simply fail to seek beyond cache limits. That should be fine and handled.

struct PulseSink {
    out: pulse_simple::Playback<[i16; 2]>,
    /// The rate the stream was opened at, so the player loop can tell when a track needs a new one.
    rate: u32,
}
impl PulseSink {
    fn new(rate: u32) -> Self {
        let out = pulse_simple::Playback::<[i16; 2]>::new(
            "phonoscule-cli",
            "CLI-based application based on the Phonoscule music player library",
            None,
            rate,
        );
        Self { out, rate }
    }

    /// Reopens the stream at `rate` if it differs from the current one. The samples are always
    /// 16-bit stereo, so only the rate varies between tracks; the audio server converts to the
    /// device rate, so we never resample ourselves. Mirrors the GUI engine's sink.
    fn ensure_rate(&mut self, rate: u32) {
        if self.rate != rate {
            *self = Self::new(rate);
        }
    }
}
impl Sink<Stereo<PcmS16Le>> for PulseSink {
    async fn write_samples(&mut self, samples: &[Stereo<PcmS16Le>]) -> Option<usize> {
        if samples.is_empty() {
            return Some(0);
        }
        let pulse_samples = unsafe { core::mem::transmute::<&[Stereo<PcmS16Le>], &[[i16; 2]]>(samples) };
        assert_eq!(core::mem::size_of_val(samples), core::mem::size_of_val(pulse_samples));
        self.out.write(pulse_samples);
        Some(samples.len())
    }
}
unsafe impl Send for PulseSink {}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct CliArgs {
    /// Audio files to play
    files: Vec<PathBuf>,
}

fn main() {
    smol::block_on(main_())
}

async fn main_() {
    let (print_tx, print_rx) = channel::bounded::<String>(512);
    logger::Logger::new(print_tx, vec![]).init().unwrap();

    let args = CliArgs::from_arg_matches(
        &CliArgs::command()
            .help_template("{usage-heading} {usage}\n\n{about}\n\n{all-args}\n\nMed vänliga hälsningar, {author}")
            .get_matches(),
    )
    .unwrap();
    let playlist = args.files;

    let (cmd_tx, cmd_rx) = channel::bounded::<Cmd>(8);
    let (status_tx, status_rx) = channel::bounded::<Status>(4);

    // The task is cancelled when this handle drops at the end of main. Decoding and output share
    // one loop (like the GUI engine): the sink's blocking writes pace playback, and the sink is
    // reopened only when a track's sample rate differs from the stream's (see `ensure_rate`).
    let _join_player = smol::spawn(async move {
        let mut sink = PulseSink::new(PLAYBACK_SAMPLE_RATE);
        let mut buf = [OutSample::default(); 512];
        let mut pls_ix = 0usize;
        let mut start_at = 0;
        'pls_entry: while let Some(path) = playlist.get(pls_ix) {
            log::debug!("opening file {:?}", path);
            let Some(track) = Track::open(path).await else {
                log::error!("failed to open {path:?}, skipping");
                pls_ix += 1;
                start_at = 0;
                continue 'pls_entry;
            };
            status_tx
                .send(Status::Track(match track.title.as_str() {
                    "" => path.to_string_lossy().to_string(),
                    name => name.to_string(),
                }))
                .await
                .expect("status channel should be open");
            let sample_rate = track.sample_rate;
            // Match the output stream to this track's rate (a no-op unless it changed).
            sink.ensure_rate(sample_rate);
            let t_end = track.len_samples.map(|n| Duration::from_secs_f64(n as f64 / sample_rate as f64));
            let mut source = track.samples;
            let mut pos = source.fast_forward(start_at).await.unwrap();

            let mut play_state = PlayState::Playing;
            let mut prev_status_pos = 0;

            loop {
                start_at = 0;
                let maybe_cmd = match play_state {
                    // Paused: nothing to do but wait for the next command.
                    PlayState::Paused => cmd_rx.recv().await.ok(),
                    PlayState::Playing => match cmd_rx.try_recv() {
                        Ok(cmd) => Some(cmd),
                        Err(channel::TryRecvError::Empty) => None,
                        Err(err) => panic!("{}", err),
                    },
                };
                match maybe_cmd {
                    Some(Cmd::PlayPause) => play_state = play_state.toggled(),
                    Some(Cmd::Restart) => continue 'pls_entry,
                    Some(Cmd::SeekForward(dt)) => {
                        let target = pos + (dt.as_secs_f64() * sample_rate as f64) as u64;
                        match source.seek_samples(target).await {
                            Some(new_pos) => pos = new_pos,
                            None => {
                                start_at = target;
                                break;
                            }
                        }
                    }
                    Some(Cmd::SeekBackward(dt)) => {
                        let target = pos.saturating_sub((dt.as_secs_f64() * sample_rate as f64) as u64);
                        match source.seek_samples(target).await {
                            Some(new_pos) => pos = new_pos,
                            None => {
                                start_at = target;
                                break;
                            }
                        }
                    }
                    Some(Cmd::Prev) => {
                        pls_ix = pls_ix.saturating_sub(1);
                        continue 'pls_entry;
                    }
                    Some(Cmd::Next) => {
                        pls_ix += 1;
                        continue 'pls_entry;
                    }
                    None => (),
                }
                match play_state {
                    PlayState::Paused => continue,
                    PlayState::Playing => (),
                }
                let n = source.read_samples(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    pls_ix += 1;
                    continue 'pls_entry;
                }
                sink.write_samples(&buf[..n]).await;
                pos += n as u64;

                let progress_updates_per_sec = 16;
                let progress_interval = PLAYBACK_SAMPLE_RATE as u64 / progress_updates_per_sec;
                if pos < prev_status_pos || pos - prev_status_pos > progress_interval {
                    let t_current = Duration::from_secs_f64(pos as f64 / sample_rate as f64);
                    status_tx.send(Status::Progress(t_current, t_end)).await.unwrap();
                    prev_status_pos = pos;
                }
            }
        }
        status_tx.send(Status::Finished).await
    });

    let mut w = std::io::stdout();
    ct::terminal::enable_raw_mode().unwrap();

    let mut events = EventStream::new().fuse();
    let mut refresh = StreamExt::fuse(Timer::interval(Duration::from_millis(100)));

    fn clear_player(w: &mut std::io::Stdout) {
        ct::queue!(
            w,
            ct::style::ResetColor,
            ct::terminal::Clear(ct::terminal::ClearType::CurrentLine),
            ct::cursor::MoveToPreviousLine(1),
            ct::terminal::Clear(ct::terminal::ClearType::CurrentLine),
            ct::cursor::Hide,
            ct::cursor::MoveToColumn(0)
        )
        .unwrap();
    }
    fn println(w: &mut std::io::Stdout, line: impl std::fmt::Display) {
        ct::queue!(w, ct::style::Print(line), ct::style::Print("\n"), ct::cursor::MoveToColumn(0)).unwrap();
    }
    fn render_player(w: &mut std::io::Stdout, track: &str, t_current: Duration, t_end: Option<Duration>) {
        clear_player(w);
        let (mins_current, secs_current) = (t_current.as_secs() / 60, t_current.as_secs() % 60);
        let time = match t_end {
            Some(t_end) => {
                let (mins_end, secs_end) = (t_end.as_secs() / 60, t_end.as_secs() % 60);
                format!("{mins_current:02}:{secs_current:02} / {mins_end:02}:{secs_end:02}")
            }
            None => format!("{mins_current:02}:{secs_current:02}"),
        };
        ct::queue!(w, ct::style::Print(track), ct::style::Print("\n"), ct::cursor::MoveToColumn(0), ct::style::Print(time))
            .unwrap();
        std::io::Write::flush(w).unwrap();
    }

    let (mut t_current, mut t_end) = (Duration::from_secs(0), None::<Duration>);
    let mut track = "<nothing playing>".to_string();
    loop {
        futures::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, state: _ }))) => match (code, modifiers) {
                    (KeyCode::Left, _) => cmd_tx.send(Cmd::SeekBackward(Duration::from_secs(5))).await.unwrap(),
                    (KeyCode::Right, _) => cmd_tx.send(Cmd::SeekForward(Duration::from_secs(5))).await.unwrap(),
                    (KeyCode::Char('<'), _) => cmd_tx.send(Cmd::Prev).await.unwrap(),
                    (KeyCode::Char('>'), _) => cmd_tx.send(Cmd::Next).await.unwrap(),
                    (KeyCode::Char(' '), _) => cmd_tx.send(Cmd::PlayPause).await.unwrap(),
                    (KeyCode::Char('r'), _) => cmd_tx.send(Cmd::Restart).await.unwrap(),
                    (KeyCode::Char('q'), _) => {
                        ct::execute!(w, ct::cursor::SetCursorStyle::DefaultUserShape).unwrap();
                        break;
                    }
                    (c, _) => log::trace!("ignored key: {c:?}"),
                },
                Some(Ok(event)) => log::trace!("ignored event: {:?}", event),
                Some(Err(err)) => log::error!("crossterm read event error: {:?}", err),
                None => break,
            },
            status = status_rx.recv().fuse() => match status {
                Err(_) => {
                    log::error!("status channel droppet");
                    break
                }
                Ok(Status::Track(t)) => {
                    track = t;
                    t_current = Duration::from_secs(0);
                    t_end = None;
                }
                Ok(Status::Progress(t_c, t_e)) => {
                    t_current = t_c;
                    t_end = t_e
                }
                Ok(Status::Finished) => {
                    clear_player(&mut w);
                    println(&mut w, "Finished playing all tracks in playlist");
                    break
                }
            },
            // custom logger that sends messages to this channel, which are printed "above" player with line breaks corrected for raw mode
            msg = print_rx.recv().fuse() => {
                let msg = msg.ok();
                let msg = msg.as_deref().unwrap_or("print channel closed");
                clear_player(&mut w);
                for line in msg.lines() {
                    println(&mut w, line)
                }
                render_player(&mut w, &track, t_current, t_end)
            },
            _ = refresh.next() => {
                render_player(&mut w, &track, t_current, t_end)
            },
        }
    }

    ct::execute!(w, ct::style::ResetColor, ct::cursor::Show).ok();
    ct::terminal::disable_raw_mode().ok();
    // join_player.await.ok();
    // join_pulse.await.ok();
}
