#![feature(iter_array_chunks)]
// #![allow(clippy::cognitive_complexity)]

use crossterm::event::KeyEventKind;
pub use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent},
    execute, queue, style,
    terminal::{self, ClearType},
    Command,
};
use phonoscule::{metadata::*, wav::*};
use std::{
    fs::File,
    io,
    io::{BufReader, Read, Write},
    sync::mpsc,
};

const PLAYBACK_SAMPLE_RATE: u32 = 48000;

fn main() -> io::Result<()> {
    let (toggle_pause_tx, toggle_pause_rx) = mpsc::channel::<()>();

    std::thread::spawn(move || {
        let player = pulse_simple::Playback::<[i16; 2]>::new(
            "phonoscule-cli",
            "CLI-based application based on the Phonoscule music player library",
            None,
            PLAYBACK_SAMPLE_RATE,
        );
        let f = BufReader::new(File::open("../assets/Listless.wav").unwrap());
        let mut wav = WavStream::<StaticMetadata, _>::parse(f.bytes().map(|b| b.unwrap())).unwrap();
        let mut chunks =
            wav.format_samples().expect("format should be supported").convert::<[i16; 2]>().array_chunks::<256>();
        let mut playing = true;
        loop {
            if !playing {
                toggle_pause_rx.recv().unwrap();
                playing = true;
            } else {
                match toggle_pause_rx.try_recv() {
                    Ok(()) => {
                        playing = false;
                        continue;
                    }
                    Err(mpsc::TryRecvError::Empty) => (),
                    Err(mpsc::TryRecvError::Disconnected) => panic!("toggle_pause channel disconnected"),
                }
            }
            let chunk = chunks.next();
            match chunk {
                Some(samples) => player.write(&samples),
                None => {
                    player.write(chunks.into_remainder().unwrap().as_slice());
                    break;
                }
            }
        }
    });

    let mut w = io::stdout();

    execute!(w, terminal::EnterAlternateScreen)?;

    terminal::enable_raw_mode()?;

    loop {
        queue!(w, style::ResetColor, terminal::Clear(ClearType::All), cursor::Hide, cursor::MoveTo(1, 1))?;

        for line in MENU.split('\n') {
            queue!(w, style::Print(line), cursor::MoveToNextLine(1))?;
        }

        w.flush()?;

        if let Ok(Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, state: _ })) = event::read() {
            match (code, modifiers) {
                // (KeyCode::Left, _) => (),
                // (KeyCode::Right, _) => (),
                (KeyCode::Char(' '), _) => toggle_pause_tx.send(()).unwrap(),
                (KeyCode::Char('q'), _) => {
                    execute!(w, cursor::SetCursorStyle::DefaultUserShape).unwrap();
                    break;
                }
                (c, _) => eprintln!("ignored key: {c:?}"),
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
