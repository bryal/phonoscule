#![feature(iter_next_chunk)]

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
    time::Duration,
};
use tokio::time::interval;

const PLAYBACK_SAMPLE_RATE: u32 = 48000;

type StereoSample = [i16; 2];

#[derive(Debug)]
enum Cmd {
    PlayPause,
    Restart,
}

#[derive(Debug)]
enum Status {
    Progress(Duration, Duration),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    simple_logger::init().unwrap();

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Cmd>(2);
    let (status_tx, mut status_rx) = tokio::sync::mpsc::channel::<Status>(2);

    std::thread::spawn(move || {
        let player = pulse_simple::Playback::<StereoSample>::new(
            "phonoscule-cli",
            "CLI-based application based on the Phonoscule music player library",
            None,
            PLAYBACK_SAMPLE_RATE,
        );

        fn open_stream_wav(path: &str) -> WavStream<StaticMetadata, impl Iterator<Item = u8>> {
            let f = BufReader::new(File::open(path).unwrap());
            WavStream::<StaticMetadata, _>::parse(f.bytes().map(|b| b.unwrap())).unwrap()
        }
        let path = "../assets/Listless.wav";

        let (format, samples) = open_stream_wav(path).into_format_samples().expect("format should be supported");
        let mut samples = samples.convert::<StereoSample>();

        fn play_chunk(
            n_played_samples: &mut u64,
            samples: &mut PcmReader<impl Iterator<Item = u8>, StereoSample>,
            play: impl FnOnce(&[StereoSample]),
        ) -> bool {
            match samples.next_chunk::<128>() {
                Ok(samples) => {
                    *n_played_samples += samples.len() as u64;
                    play(&samples);
                    false
                }
                Err(rest) => {
                    let samples = rest.as_slice();
                    *n_played_samples += samples.len() as u64;
                    play(samples);
                    true
                }
            }
        }

        let mut n_played_samples = 0;
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
                        n_played_samples = 0;
                        samples = open_stream_wav(path)
                            .into_format_samples()
                            .expect("format should be supported")
                            .1
                            .convert::<StereoSample>();
                    }
                }
            }
            if !playing {
                continue;
            }

            let done = play_chunk(&mut n_played_samples, &mut samples, |chunk| {
                player.write(chunk);
            });
            if done {
                break;
            }
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
                    // (KeyCode::Left, _) => (),
                    // (KeyCode::Right, _) => (),
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
