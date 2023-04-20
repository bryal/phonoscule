#![feature(atomic_bool_fetch_not)]

use crossterm::event::KeyEventKind;
pub use crossterm::{
    cursor,
    event::{self, Event, EventStream, KeyCode, KeyEvent},
    execute, queue, style,
    terminal::{self, ClearType},
    Command,
};
use futures::StreamExt;
use phonoscule::{metadata::*, pcm::*, wav::*};
use std::{
    fs::File,
    io,
    io::{BufReader, Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        Arc,
    },
    time::Duration,
};
use tokio::time::interval;

const PLAYBACK_SAMPLE_RATE: u32 = 48000;

type StereoSample = [i16; 2];

#[derive(Debug)]
enum Cmd {
    PlayPause,
    Restart,
    SeekForward(Duration),
    SeekBackward(Duration),
}

#[derive(Debug)]
enum Status {
    Progress(Duration, Duration),
}

trait Sink {
    fn buffer_samples(&mut self, samples: &mut impl Iterator<Item = StereoSample>) -> Option<usize>;
}

struct PulseSimpleSink<const N: usize> {
    stop: Arc<AtomicBool>,
    buf: ringbuf::Producer<StereoSample, Arc<ringbuf::StaticRb<StereoSample, N>>>,
}

impl<const N: usize> PulseSimpleSink<N> {
    fn start() -> Self {
        let (prod, mut cons) = ringbuf::StaticRb::<StereoSample, N>::default().split();
        let stop = Arc::new(AtomicBool::new(false));
        let stop1 = stop.clone();
        std::thread::spawn(move || {
            let pulse = pulse_simple::Playback::<StereoSample>::new(
                "phonoscule-cli",
                "CLI-based application based on the Phonoscule music player library",
                None,
                PLAYBACK_SAMPLE_RATE,
            );
            let mut buf = [StereoSample::default(); N];
            while !stop1.load(Relaxed) {
                let n = cons.pop_slice(&mut buf[..]);
                let buf = &buf[..n];
                if !buf.is_empty() {
                    pulse.write(buf)
                }
            }
        });
        Self { stop, buf: prod }
    }
}

impl<const N: usize> Drop for PulseSimpleSink<N> {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed)
    }
}

impl<const N: usize> Sink for PulseSimpleSink<N> {
    fn buffer_samples(&mut self, samples: &mut impl Iterator<Item = StereoSample>) -> Option<usize> {
        let free = self.buf.free_len();
        let pushed = self.buf.push_iter(samples);
        (pushed >= free).then_some(pushed)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    simple_logger::init().unwrap();

    let mut sink = PulseSimpleSink::<256>::start();

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Cmd>(2);
    let (status_tx, mut status_rx) = tokio::sync::mpsc::channel::<Status>(2);

    std::thread::spawn(move || {
        let path = "../assets/Listless.wav";
        let mut n_played_samples = 0;
        let (format, mut samples) = play_file(&mut n_played_samples, path);

        fn play_file(
            n_played_samples: &mut usize,
            path: &str,
        ) -> (Format, PcmReader<impl Iterator<Item = u8>, StereoSample>) {
            *n_played_samples = 0;
            let f = BufReader::new(File::open(path).unwrap());
            let wav = WavStream::<StaticMetadata, _>::parse(f.bytes().map(|b| b.unwrap())).unwrap();
            let (format, samples) = wav.into_format_samples().expect("format should be supported");
            (format, samples.convert::<StereoSample>())
        }

        fn seek_sample(i: usize, samples: &mut impl Iterator<Item = StereoSample>) -> usize {
            samples.take(i).count()
        }

        let mut playing = true;
        loop {
            let maybe_cmd = if !playing {
                Some(cmd_rx.blocking_recv().unwrap())
            } else {
                match cmd_rx.try_recv() {
                    Ok(cmd) => Some(cmd),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => None,
                    Err(err) => panic!("{}", err),
                }
            };
            if let Some(cmd) = maybe_cmd {
                match cmd {
                    Cmd::PlayPause => playing = !playing,
                    Cmd::Restart => {
                        samples = play_file(&mut n_played_samples, path).1;
                    }
                    Cmd::SeekForward(dt) => {
                        let n = (dt.as_secs_f64() * format.sample_rate() as f64) as usize;
                        n_played_samples += seek_sample(n, &mut samples);
                    }
                    Cmd::SeekBackward(dt) => {
                        let i =
                            n_played_samples.saturating_sub((dt.as_secs_f64() * format.sample_rate() as f64) as usize);
                        samples = play_file(&mut n_played_samples, path).1;
                        n_played_samples = seek_sample(i, &mut samples);
                    }
                }
            }
            if !playing {
                continue;
            }

            n_played_samples += sink.buffer_samples(&mut samples).unwrap_or(0);
            let t_current = Duration::from_secs_f64(n_played_samples as f64 / format.sample_rate() as f64);
            let t_end = Duration::from_secs_f64(format.len_samples() as f64 / format.sample_rate() as f64);
            status_tx.blocking_send(Status::Progress(t_current, t_end)).unwrap();
        }
    });

    let mut w = io::stdout();
    execute!(w, terminal::EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;

    let mut events = EventStream::new();
    let mut refresh = interval(Duration::from_millis(100));

    let (mut t_current, mut t_end) = (Duration::from_secs(0), Duration::from_secs(0));
    loop {
        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, state: _ }))) => match (code, modifiers) {
                    (KeyCode::Left, _) => cmd_tx.send(Cmd::SeekBackward(Duration::from_secs(5))).await.unwrap(),
                    (KeyCode::Right, _) => cmd_tx.send(Cmd::SeekForward(Duration::from_secs(5))).await.unwrap(),
                    (KeyCode::Char(' '), _) => cmd_tx.send(Cmd::PlayPause).await.unwrap(),
                    (KeyCode::Char('r'), _) => cmd_tx.send(Cmd::Restart).await.unwrap(),
                    (KeyCode::Char('q'), _) => {
                        execute!(w, cursor::SetCursorStyle::DefaultUserShape).unwrap();
                        break;
                    }
                    (c, _) => log::debug!("ignored key: {c:?}"),
                },
                Some(Ok(event)) => log::debug!("ignored event: {:?}", event),
                Some(Err(err)) => log::error!("crossterm read event error: {:?}", err),
                None => break,
            },
            status = status_rx.recv() => match status.unwrap() {
                Status::Progress(t_c, t_e) => {
                    t_current = t_c;
                    t_end = t_e
                }
            },
            _ = refresh.tick() => {
                queue!(w, style::ResetColor, terminal::Clear(ClearType::All), cursor::Hide, cursor::MoveTo(1, 1))?;

                for line in MENU.split('\n') {
                    queue!(w, style::Print(line), cursor::MoveToNextLine(1))?;
                }
                queue!(w, style::Print(""), cursor::MoveToNextLine(1))?;

                let (mins_current, secs_current) = (t_current.as_secs() / 60, t_current.as_secs() % 60);
                let (mins_end, secs_end) = (t_end.as_secs() / 60, t_end.as_secs() % 60);
                queue!(w, style::Print(format!("{mins_current:02}:{secs_current:02} / {mins_end:02}:{secs_end:02}")), cursor::MoveToNextLine(1))?;

                w.flush()?;
            }
        }
    }

    execute!(w, style::ResetColor, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()
}

const MENU: &str = r#"Phonoscule CLI Demo
Controls:
 - Q - quit (or return to this menu)
 - R - restart track
 - space - play/pause
"#;
// - left  - seek backward
// - right - seek forward
