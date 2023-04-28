#![allow(incomplete_features)]
#![feature(iter_next_chunk, async_fn_in_trait, maybe_uninit_uninit_array_transpose, maybe_uninit_slice)]

mod logger;

use clap::{CommandFactory, FromArgMatches, Parser};
use core::{cmp::min, mem::MaybeUninit};
use crossterm::{
    self as ct,
    event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind},
};
use embedded_io::adapters::FromTokio;
use futures::StreamExt;
use phonoscule::{io::*, metadata::*, plumbing::*, sample::*, wav::*};
use std::{path::PathBuf, time::Duration};
use tokio::{fs::File, io::BufReader, sync::mpsc as async_mpsc, task::spawn, time::interval};

const PLAYBACK_SAMPLE_RATE: u32 = 48000;

type OutSample = Stereo<PcmS16Le>;

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
    Progress(Duration, Duration),
    Finished,
}

// something like this when feature "seek" or "cache" or "std" or whatever?
// A wrapper for a Source that remembers history & reads ahead in chunks.
// Size limit. Don't cache all history forever for a live stream, for example.
// If trying to seek uncached chunk, call seek on underlying source.
// If that seek returns None, just propagate that. UI will simply fail to seek beyond cache limits. That should be fine and handled.

struct StaticVec<const N: usize, T> {
    len: usize,
    buf: [MaybeUninit<T>; N],
}

impl<const N: usize, T> StaticVec<N, T> {
    fn from(buf: [T; N]) -> Self {
        Self { len: N, buf: MaybeUninit::new(buf).transpose() }
    }

    fn truncate(&mut self, len: usize) {
        self.len = min(self.len, len);
    }

    fn as_slice(&self) -> &[T] {
        unsafe { MaybeUninit::slice_assume_init_ref(&self.buf[..self.len]) }
    }
}

struct SendSamples<const N: usize, Sample>(async_mpsc::Sender<StaticVec<N, Sample>>);

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

struct RecvSamples<const N: usize, Sample>(async_mpsc::Receiver<StaticVec<N, Sample>>);

impl<const N: usize, Sample> SinkInput<Sample> for RecvSamples<N, Sample> {
    async fn write_samples_to<S: Sink<Sample>>(&mut self, sink: &mut S) -> Option<u64> {
        let samples = self.0.recv().await?;
        let n = sink.write_samples(samples.as_slice()).await?;
        Some(n as u64)
    }
}

fn sample_channel<const N: usize, Sample>(nchunks: usize) -> (SendSamples<N, Sample>, RecvSamples<N, Sample>) {
    let (tx, rx) = async_mpsc::channel(nchunks);
    (SendSamples(tx), RecvSamples(rx))
}

struct PulseSink(pulse_simple::Playback<[i16; 2]>);
impl PulseSink {
    fn new() -> Self {
        Self(pulse_simple::Playback::<[i16; 2]>::new(
            "phonoscule-cli",
            "CLI-based application based on the Phonoscule music player library",
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

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct CliArgs {
    /// Audio files to play
    files: Vec<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (print_tx, mut print_rx) = tokio::sync::mpsc::channel::<String>(512);
    logger::Logger::new(print_tx, vec![]).init().unwrap();

    let args = CliArgs::from_arg_matches(
        &CliArgs::command()
            .help_template("{usage-heading} {usage}\n\n{about}\n\n{all-args}\n\nMed vänliga hälsningar, {author}")
            .get_matches(),
    )
    .unwrap();
    let playlist = args.files;

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Cmd>(8);
    let (status_tx, mut status_rx) = tokio::sync::mpsc::channel::<Status>(4);
    let (mut audio_tx, audio_rx) = sample_channel::<512, OutSample>(1);

    let _join_pulse = tokio::task::spawn(async {
        let pulse_sink = PulseSink::new();
        let mut chan_to_sink = ConnectSink::from_input(audio_rx, pulse_sink);
        while chan_to_sink.push().await.is_some() {}
        log::warn!("pulse sink closing")
    });

    let _join_player = spawn(async move {
        let mut pls_ix = 0usize;
        let mut start_at = 0;
        'pls_entry: while let Some(path) = playlist.get(pls_ix) {
            log::debug!("opening file {:?}", path);
            let f = Skippable(FromTokio::new(BufReader::new(File::open(path).await.unwrap())));
            let wav = Wav::<StaticMetadata, _>::parse(f).await.unwrap();
            status_tx
                .send(Status::Track(match wav.metadata.title() {
                    "" => path.to_string_lossy().to_string(),
                    name => name.to_string(),
                }))
                .await
                .expect("status channel should be open");
            let mut source = wav.samples;
            let mut pos = source.fast_forward(start_at).await.unwrap();
            let mut chan_from_source = ConnectSource::to_output(source, &mut audio_tx);

            let mut playing = true;
            let mut prev_status_pos = 0;

            loop {
                start_at = 0;
                let maybe_cmd = if !playing {
                    cmd_rx.recv().await
                } else {
                    match cmd_rx.try_recv() {
                        Ok(cmd) => Some(cmd),
                        Err(async_mpsc::error::TryRecvError::Empty) => None,
                        Err(err) => panic!("{}", err),
                    }
                };
                match maybe_cmd {
                    Some(Cmd::PlayPause) => playing = !playing,
                    Some(Cmd::Restart) => continue 'pls_entry,
                    Some(Cmd::SeekForward(dt)) => {
                        let n = (dt.as_secs_f64() * wav.format.sample_rate() as f64) as u64;
                        start_at = pos + n;
                        break;
                    }
                    Some(Cmd::SeekBackward(dt)) => {
                        let i = pos.saturating_sub((dt.as_secs_f64() * wav.format.sample_rate() as f64) as u64);
                        start_at = i;
                        break;
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
                if playing {
                    let n = chan_from_source.pull().await.unwrap();
                    if n == 0 {
                        pls_ix += 1;
                        continue 'pls_entry;
                    }
                    pos += n;

                    let progress_updates_per_sec = 16;
                    let progress_interval = PLAYBACK_SAMPLE_RATE as u64 / progress_updates_per_sec;
                    if pos < prev_status_pos || pos - prev_status_pos > progress_interval {
                        let t_current = Duration::from_secs_f64(pos as f64 / wav.format.sample_rate() as f64);
                        let t_end =
                            Duration::from_secs_f64(wav.format.len_samples() as f64 / wav.format.sample_rate() as f64);
                        status_tx.send(Status::Progress(t_current, t_end)).await.unwrap();
                        prev_status_pos = pos;
                    }
                }
            }
        }
        status_tx.send(Status::Finished).await
    });

    let mut w = std::io::stdout();
    ct::terminal::enable_raw_mode().unwrap();

    let mut events = EventStream::new();
    let mut refresh = interval(Duration::from_millis(100));

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
    fn render_player(w: &mut std::io::Stdout, track: &str, t_current: Duration, t_end: Duration) {
        clear_player(w);
        let (mins_current, secs_current) = (t_current.as_secs() / 60, t_current.as_secs() % 60);
        let (mins_end, secs_end) = (t_end.as_secs() / 60, t_end.as_secs() % 60);
        ct::queue!(
            w,
            ct::style::Print(track),
            ct::style::Print("\n"),
            ct::cursor::MoveToColumn(0),
            ct::style::Print(format!("{mins_current:02}:{secs_current:02} / {mins_end:02}:{secs_end:02}"))
        )
        .unwrap();
        std::io::Write::flush(w).unwrap();
    }

    let (mut t_current, mut t_end) = (Duration::from_secs(0), Duration::from_secs(0));
    let mut track = "<nothing playing>".to_string();
    loop {
        tokio::select! {
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
            status = status_rx.recv() => match status {
                None => {
                    log::error!("status channel droppet");
                    break
                }
                Some(Status::Track(t)) => {
                    track = t;
                }
                Some(Status::Progress(t_c, t_e)) => {
                    t_current = t_c;
                    t_end = t_e
                }
                Some(Status::Finished) => {
                    clear_player(&mut w);
                    println(&mut w, "Finished playing all tracks in playlist");
                    break
                }
            },
            // custom logger that sends messages to this channel, which are printed "above" player with line breaks corrected for raw mode
            msg = print_rx.recv() => {
                let msg = msg.as_deref().unwrap_or("print channel closed");
                clear_player(&mut w);
                for line in msg.lines() {
                    println(&mut w, line)
                }
                render_player(&mut w, &track, t_current, t_end)
            },
            _ = refresh.tick() => {
                render_player(&mut w, &track, t_current, t_end)
            },
        }
    }

    ct::execute!(w, ct::style::ResetColor, ct::cursor::Show).ok();
    ct::terminal::disable_raw_mode().ok();
    // join_player.await.ok();
    // join_pulse.await.ok();
}
