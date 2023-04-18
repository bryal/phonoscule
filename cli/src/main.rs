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
use phonoscule::{metadata::*, wav::*};
use std::{
    fs::File,
    io,
    io::{BufReader, Read, Write},
    sync::{
        atomic::{AtomicU32, Ordering::Relaxed},
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
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    simple_logger::init().unwrap();

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Cmd>(2);

    let t_playback_s = Arc::new(AtomicU32::new(0));
    let t_playback_s1 = t_playback_s.clone();

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

        let mut wav = open_stream_wav(path);
        let mut samples = wav.format_samples().expect("format should be supported").convert::<StereoSample>();
        let mut t = Duration::from_secs(0);

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
                }
            }
            if !playing {
                continue;
            }

            match samples.next_chunk::<128>() {
                Ok(samples) => player.write(&samples),
                Err(rest) => {
                    player.write(rest.as_slice());
                    break;
                }
            };
            t += Duration::from_secs_f64(128.0 / PLAYBACK_SAMPLE_RATE as f64);
            t_playback_s1.store(t.as_secs() as u32, Relaxed)
        }
    });

    let mut w = io::stdout();
    execute!(w, terminal::EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;

    let mut events = EventStream::new();
    let mut refresh = interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, state: _ }))) => match (code, modifiers) {
                    // (KeyCode::Left, _) => (),
                    // (KeyCode::Right, _) => (),
                    (KeyCode::Char(' '), _) => cmd_tx.send(Cmd::PlayPause).await.unwrap(),
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
            _ = refresh.tick() => {
                queue!(w, style::ResetColor, terminal::Clear(ClearType::All), cursor::Hide, cursor::MoveTo(1, 1))?;

                for line in MENU.split('\n') {
                    queue!(w, style::Print(line), cursor::MoveToNextLine(1))?;
                }
                queue!(w, style::Print(""), cursor::MoveToNextLine(1))?;

                let t = t_playback_s.load(Relaxed);
                let mins = t / 60;
                let secs = t % 60;
                queue!(w, style::Print(format!("{mins:02}:{secs:02}")), cursor::MoveToNextLine(1))?;

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
 - space - play/pause
"#;
// - left  - seek backward
// - right - seek forward
