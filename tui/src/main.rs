//! Phonoscule TUI: an album-focused music player for the terminal.
//!
//! The terminal counterpart of `phonoscule-gui`: the same album-centric browsing, drawn in text,
//! with the playing album's cover art shown through whatever image protocol the terminal speaks.
//! Follows the model/update/view architecture; this file boots it and runs the event loop.

mod covers;
mod keys;
mod logger;
mod model;
mod paths;
mod update;
mod view;

use futures::StreamExt;
use model::Model;
use phonoscule::{config, library};
use smol::channel;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use update::{After, Msg, update};

/// The name this player goes by: its `[app.tui]` config table and its `$PHONOSCULE_TUI_CONF`.
const APP: &str = "tui";

/// `--help` text: what the program is and how to run it. [`help`] follows it with the config
/// section.
const HELP: &str = "\
Phonoscule: an album-art-focused music player for the terminal.

Usage: phonoscule-tui [CONFIG]

Arguments:
  CONFIG         (optional) Path to a config file.

Options:
  -h, --help     Print this help and exit.
  -V, --version  Print version information and exit.

";

fn help() -> String {
    format!("{HELP}{}", config::config_help(APP))
}

fn main() {
    // Any startup failure -- a bad argument, an unreadable config, a terminal that cannot be put
    // into raw mode -- prints the help after it, pointing at how to run the program and where its
    // config lives.
    if let Err(e) = run() {
        eprintln!("Error: {e:?}\n");
        eprint!("{}", help());
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    // Deliberately tiny hand-rolled argument handling (no dependency): the two flags, and at most
    // one positional -- a config path.
    let mut arg_conf_path = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{}", help());
                return Ok(());
            }
            "-V" | "--version" => {
                println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ if arg.starts_with('-') => anyhow::bail!("unknown option `{arg}`"),
            _ if arg_conf_path.is_none() => arg_conf_path = Some(PathBuf::from(arg)),
            _ => anyhow::bail!("expected at most one argument: a path to a config file"),
        }
    }

    // Before the terminal is taken over, so failures here print normally.
    let logs = logger::start();
    let conf = smol::block_on(config::load(APP, arg_conf_path))?;
    let index = smol::block_on(library::load_index(paths::album_index_file()));

    // Installs a panic hook that restores the terminal first, so a panic leaves a usable shell
    // rather than a raw-mode one with the alternate screen still up.
    let mut terminal = ratatui::init();
    // Between entering the alternate screen and reading any terminal event, which is where the
    // protocol query has to happen (it writes to stdout and reads the reply from stdin).
    let model = Model::new(conf, covers::picker(), index);
    // The query's bytes went out behind ratatui's back, and a terminal that did not understand them
    // will have printed them; wipe the screen before the first frame. Through the backend, whose
    // clear is a plain escape sequence -- `Terminal::clear` snapshots the cursor position first, and
    // reading that back needs a reply the terminal may never send.
    ratatui::backend::Backend::clear(terminal.backend_mut())?;
    let result = smol::block_on(event_loop(&mut terminal, model, logs));
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    mut model: Model,
    logs: channel::Receiver<logger::Entry>,
) -> anyhow::Result<()> {
    // Every source fans into one channel, so the loop has a single thing to await and messages stay
    // in the order they arrived.
    let (tx, rx) = channel::unbounded::<Msg>();
    let sources = [
        forward(terminal_events(), tx.clone()),
        forward(logs.map(Msg::Log), tx.clone()),
        forward(library::scan(update::scan_options(&model)).map(Msg::Library), tx.clone()),
    ];

    terminal.draw(|frame| view::view(frame, &mut model))?;
    while let Ok(msg) = rx.recv().await {
        let mut redraw = apply(&mut model, msg);
        // A burst -- a scan reporting hundreds of albums, a held key -- is applied together and
        // drawn once. Bounded by a frame's worth of time, so a library that takes a while to scan
        // still redraws (and still answers the keyboard) while it does, rather than freezing until
        // the last album lands.
        let deadline = Instant::now() + FRAME;
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(msg) => redraw |= apply(&mut model, msg),
                Err(_) => break,
            }
        }
        if model.quit {
            break;
        }
        if redraw {
            update::reconcile(&mut model);
            terminal.draw(|frame| view::view(frame, &mut model))?;
        }
    }
    drop(sources);
    Ok(())
}

/// How long messages are absorbed before drawing. Enough to swallow a burst whole, short enough that
/// a scan's steady stream of albums still yields a frame several times a second.
const FRAME: Duration = Duration::from_millis(16);

/// Applies one message, reporting whether the frame needs redrawing.
fn apply(model: &mut Model, msg: Msg) -> bool {
    match update(model, msg) {
        After::Redraw => true,
        After::Idle => false,
        After::SaveIndex => {
            let save = library::save_index(paths::album_index_file(), &model.albums);
            smol::spawn(save).detach();
            true
        }
    }
}

/// Key presses and resizes, as messages. Anything else the terminal reports (mouse, focus, pastes)
/// is not bound to anything yet.
fn terminal_events() -> impl futures::Stream<Item = Msg> + Send {
    crossterm::event::EventStream::new().filter_map(|event| async {
        match event {
            Ok(crossterm::event::Event::Key(key)) => Some(Msg::Key(key)),
            Ok(crossterm::event::Event::Resize(..)) => Some(Msg::Resize),
            Ok(_) => None,
            Err(e) => {
                log::warn!("terminal event error: {e}");
                None
            }
        }
    })
}

/// Pumps a stream into the message channel for as long as the returned task is held. Dropping it
/// stops the source, which is how the scan is cancelled on exit.
fn forward(stream: impl futures::Stream<Item = Msg> + Send + 'static, tx: channel::Sender<Msg>) -> smol::Task<()> {
    smol::spawn(async move {
        let mut stream = std::pin::pin!(stream);
        while let Some(msg) = stream.next().await {
            if tx.send(msg).await.is_err() {
                return;
            }
        }
    })
}
