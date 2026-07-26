//! Phonoscule TUI: an album-focused music player for the terminal.
//!
//! The terminal counterpart of `phonoscule-gui`: the same album-centric browsing, drawn in text,
//! with the playing album's cover art shown through whatever image protocol the terminal speaks.
//! Follows the model/update/view architecture; this file boots it and runs the event loop.

mod cache;
mod covers;
mod keys;
mod logger;
mod model;
mod paths;
mod update;
mod view;

use futures::StreamExt;
use model::Model;
use phonoscule::{config, library, mpris, player, session, watcher};
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

/// This player's own config settings, listed after the shared ones.
fn config_help_tui() -> String {
    format!(
        "
  [app.tui]
    image-protocol
               Draw cover art with this terminal image protocol instead of
               asking the terminal which it speaks: one of {}.
               Optional; `halfblocks` works anywhere and needs no protocol.
",
        covers::PROTOCOL_NAMES,
    )
}

fn help() -> String {
    format!("{HELP}{}{}", config::config_help(APP), config_help_tui())
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
    let forced_protocol = conf.app_str("image-protocol")?.map(str::to_owned);
    let index = smol::block_on(library::load_index(paths::album_index_file()));
    let restored = smol::block_on(session::load(paths::playlist_file(), paths::player_file()));

    // Installs a panic hook that restores the terminal first, so a panic leaves a usable shell
    // rather than a raw-mode one with the alternate screen still up.
    let mut terminal = ratatui::init();
    // Between entering the alternate screen and reading any terminal event, which is where the
    // protocol query has to happen (it writes to stdout and reads the reply from stdin).
    let engine = player::start(player::Client {
        name: "phonoscule-tui".into(),
        description: "Terminal application based on the Phonoscule music player library".into(),
    });
    let picker = covers::picker(forced_protocol.as_deref());
    let covers = covers::Covers::new(picker, paths::covers_dir());
    let model = Model::restored(conf, covers, engine, index, restored);
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
    read_terminal_events(tx.clone());
    let (media, media_worker) = mpris::start("Phonoscule TUI", "phonoscule_tui");
    let watcher = watcher::start(&model.conf.music_dir);
    let (changes, quiet) = watcher.change_source();
    let sources = [
        forward(logs.map(Msg::Log), tx.clone()),
        forward(model.engine.events.clone().map(Msg::Player), tx.clone()),
        forward(media.events.clone().map(Msg::Media), tx.clone()),
        forward(library::scan(update::scan_options(&model)).map(Msg::Library), tx.clone()),
        // The music directory noticed changing, and a slow poll behind it in case it never is.
        forward(watcher::debounce(changes, quiet).map(|()| Msg::Rescan), tx.clone()),
        forward(every(RESCAN_INTERVAL).map(|()| Msg::Rescan), tx.clone()),
    ];
    // Runs for the session; it publishes to the bus and yields nothing of its own.
    let media_task = smol::spawn(media_worker.run());

    // The engine is told what a previous run left, opened but not playing.
    if !model.queue.is_empty() {
        model.send(player::Cmd::SetRepeat(model.repeat));
        model.send(player::Cmd::SetQueue {
            tracks: update::entries(&model.queue),
            start: model.current,
            play: player::PlayState::Paused,
        });
    }

    terminal.draw(|frame| view::view(frame, &mut model))?;
    while let Ok(msg) = rx.recv().await {
        let mut redraw = apply(&mut model, msg, &tx);
        // A burst -- a scan reporting hundreds of albums, a held key -- is applied together and
        // drawn once. Bounded by a frame's worth of time, so a library that takes a while to scan
        // still redraws (and still answers the keyboard) while it does, rather than freezing until
        // the last album lands.
        let deadline = Instant::now() + FRAME;
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(msg) => redraw |= apply(&mut model, msg, &tx),
                Err(_) => break,
            }
        }
        if model.quit {
            break;
        }
        if redraw {
            update::reconcile(&mut model);
            terminal.draw(|frame| view::view(frame, &mut model))?;
            // Drawing is what discovers which covers are wanted, and at what size, so the loads it
            // asked for are started once the frame is out.
            load_covers(&mut model, &tx);
        }
        update::publish_media(&model, &media);
        for write in update::save_session(&mut model) {
            smol::spawn(write).detach();
        }
    }
    drop(sources);
    drop(media_task);
    Ok(())
}

/// How often the music directory is polled for changes, behind the filesystem watcher. Unchanged
/// files are never reopened -- the tag cache is checked against their stat data -- so a quiet poll
/// costs directory listings.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// A stream that yields every `interval`, for whatever wants doing periodically.
fn every(interval: Duration) -> impl futures::Stream<Item = ()> + Send {
    futures::stream::unfold((), move |()| async move {
        smol::Timer::after(interval).await;
        Some(((), ()))
    })
}

/// Starts the cover loads the last frame asked for. Each runs on the executor and lands back as a
/// message, so the few milliseconds of resizing and encoding never hold up a keypress.
fn load_covers(model: &mut Model, tx: &channel::Sender<Msg>) {
    let dir = model.covers.dir();
    let layout = model.covers.layout();
    for request in model.covers.take_wanted() {
        let (picker, dir, layout, tx) = (model.covers.picker.clone(), dir.clone(), layout.clone(), tx.clone());
        smol::spawn(async move {
            let load = covers::load(picker, dir, layout, request).await;
            let _ = tx.send(Msg::Cover(load)).await;
        })
        .detach();
    }
}

/// How long messages are absorbed before drawing. Enough to swallow a burst whole, short enough that
/// a scan's steady stream of albums still yields a frame several times a second.
const FRAME: Duration = Duration::from_millis(16);

/// Applies one message, reporting whether the frame needs redrawing.
fn apply(model: &mut Model, msg: Msg, tx: &channel::Sender<Msg>) -> bool {
    match update(model, msg) {
        After::Redraw => true,
        After::Idle => false,
        After::SaveIndex => {
            let save = library::save_index(paths::album_index_file(), &model.albums);
            smol::spawn(save).detach();
            true
        }
        After::Rescan => {
            // Detached rather than held: a rescan ends on its own, and there is nothing to cancel it
            // for -- the next one is only started once this has reported it is done.
            let scan = library::scan(update::scan_options(model)).map(Msg::Library);
            forward(scan, tx.clone()).detach();
            true
        }
    }
}

/// Reads key presses and resizes into the message channel, on a thread of its own.
///
/// Its own thread, and blocking reads, because nothing may come between a key press and the loop
/// that answers it. Sharing an executor with the library scan is what made the player unable to
/// type while it ran: the scan is a task that is almost always ready, so an event source beside it
/// waits its turn. Anything else the terminal reports (mouse, focus, pastes) is bound to nothing yet.
fn read_terminal_events(tx: channel::Sender<Msg>) {
    let spawned = std::thread::Builder::new().name("phonoscule-input".into()).spawn(move || {
        loop {
            let msg = match crossterm::event::read() {
                Ok(crossterm::event::Event::Key(key)) => Msg::Key(key),
                Ok(crossterm::event::Event::Resize(..)) => Msg::Resize,
                Ok(_) => continue,
                Err(e) => {
                    log::warn!("cannot read terminal events: {e}");
                    return;
                }
            };
            // Fails once the loop is gone, which is how this thread learns to stop.
            if tx.send_blocking(msg).is_err() {
                return;
            }
        }
    });
    if let Err(e) = spawned {
        log::error!("cannot start the input thread, the keyboard will not work: {e}");
    }
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
