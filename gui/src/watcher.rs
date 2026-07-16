//! Filesystem watching: notices changes under the music directory (inotify on Linux), so the
//! library can rescan promptly instead of waiting for the next periodic poll.
//!
//! Consumers get one `()` per *settled burst* of changes: filesystem events arrive in storms
//! (copying an album in produces hundreds), so nothing is reported until the directory has been
//! quiet for a while. What changed is irrelevant here -- the rescan is incremental anyway.

use futures::{Stream, stream};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use smol::channel;
use std::path::Path;
use std::time::Duration;

pub struct Watcher {
    /// Raw change events, one per non-access filesystem event; debounced by [`Watcher::changes`].
    raw: channel::Receiver<()>,
    /// Kept alive: dropping it removes the OS watches, which disconnects `raw` (and so ends any
    /// debounce stream reading it).
    _watcher: Option<RecommendedWatcher>,
    quiet: Duration,
}

/// How long the directory must stay quiet before a burst of changes is reported.
const QUIET_PERIOD: Duration = Duration::from_secs(2);

pub fn start(root: &Path) -> Watcher {
    start_with(root, QUIET_PERIOD)
}

fn start_with(root: &Path, quiet: Duration) -> Watcher {
    let (raw_tx, raw_rx) = channel::unbounded();
    let watcher = notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
        match event {
            // Access events fire for mere reads -- including our own playback and scanning --
            // and can't change what a scan would find.
            Ok(event) if matches!(event.kind, notify::EventKind::Access(_)) => (),
            Ok(_) => {
                let _ = raw_tx.try_send(());
            }
            Err(e) => log::warn!("filesystem watch error: {e}"),
        }
    })
    .and_then(|mut watcher| {
        watcher.watch(root, RecursiveMode::Recursive)?;
        Ok(watcher)
    });
    let watcher = match watcher {
        Ok(watcher) => Some(watcher),
        Err(e) => {
            log::warn!("cannot watch {root:?} for changes (periodic rescans still work): {e}");
            None
        }
    };

    Watcher { raw: raw_rx, _watcher: watcher, quiet }
}

impl Watcher {
    /// The raw change-event receiver and the quiet period, for building a debounced subscription
    /// (see [`debounce`]). The pieces are exposed rather than the stream itself because iced's
    /// `run_with` takes a builder it can call to (re)create the stream, not a one-shot value.
    pub fn change_source(&self) -> (channel::Receiver<()>, Duration) {
        (self.raw.clone(), self.quiet)
    }
}

/// One `()` per settled burst of changes, as a stream for a subscription to drive -- so the
/// debounce runs on iced's own executor rather than a thread of ours. After the first raw event,
/// absorb the rest until the directory has been quiet for `quiet`, then yield once, and repeat.
/// Ends when the watcher is dropped (`raw` disconnects). Backpressure is natural -- the next burst
/// is only picked up once the consumer asks for it.
pub fn debounce(raw: channel::Receiver<()>, quiet: Duration) -> impl Stream<Item = ()> + Send + 'static {
    stream::unfold(raw, move |raw| async move {
        // The first event of a burst...
        raw.recv().await.ok()?;
        // ...then absorb the rest until nothing has happened for `quiet`.
        loop {
            let more = smol::future::or(async { Some(raw.recv().await) }, async {
                smol::Timer::after(quiet).await;
                None
            })
            .await;
            match more {
                Some(Ok(())) => continue,    // still busy
                Some(Err(_)) => return None, // the watcher is gone: end the stream
                None => break,               // settled
            }
        }
        Some(((), raw))
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::StreamExt;

    /// Pulls the next debounced event with a timeout, from outside the async world.
    fn next_within(changes: &mut (impl Stream<Item = ()> + Unpin), timeout: Duration) -> bool {
        smol::block_on(smol::future::or(async { changes.next().await.is_some() }, async {
            smol::Timer::after(timeout).await;
            false
        }))
    }

    #[test]
    fn bursts_are_debounced_into_single_events() {
        let root = std::env::temp_dir().join(format!("phonoscule-watcher-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let watcher = start_with(&root, Duration::from_millis(200));
        if watcher._watcher.is_none() {
            eprintln!("skipping: no filesystem watching in this environment");
            return;
        }
        let (raw, quiet) = watcher.change_source();
        let mut changes = debounce(raw, quiet).boxed();
        // Quiet directory: no events.
        assert!(!next_within(&mut changes, Duration::from_millis(400)));

        // A burst of changes produces exactly one event...
        for name in ["a.opus", "b.opus", "c.opus"] {
            std::fs::write(root.join(name), b"x").unwrap();
        }
        assert!(next_within(&mut changes, Duration::from_secs(5)));
        // ...and nothing more once settled.
        assert!(!next_within(&mut changes, Duration::from_millis(400)));

        let _ = std::fs::remove_dir_all(&root);
    }
}
