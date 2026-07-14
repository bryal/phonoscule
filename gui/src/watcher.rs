//! Filesystem watching: notices changes under the music directory (inotify on Linux), so the
//! library can rescan promptly instead of waiting for the next periodic poll.
//!
//! Consumers get one `()` per *settled burst* of changes: filesystem events arrive in storms
//! (copying an album in produces hundreds), so nothing is reported until the directory has been
//! quiet for a while. What changed is irrelevant here -- the rescan is incremental anyway.

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use smol::channel;
use std::path::Path;
use std::time::Duration;

pub struct Watcher {
    /// Emits one `()` per settled burst of filesystem changes.
    pub events: channel::Receiver<()>,
    /// Kept alive: dropping it removes the OS watches (which in turn ends the debounce task).
    _watcher: Option<RecommendedWatcher>,
    _debounce_task: Option<smol::Task<()>>,
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

    let (tx, rx) = channel::bounded(1);
    let debounce_task = watcher.is_some().then(|| {
        smol::spawn(async move {
            loop {
                // The first event of a burst...
                let Ok(()) = raw_rx.recv().await else { return };
                // ...then absorb the rest until nothing has happened for `quiet`.
                loop {
                    let event = smol::future::or(
                        async { Some(raw_rx.recv().await) },
                        async {
                            smol::Timer::after(quiet).await;
                            None
                        },
                    )
                    .await;
                    match event {
                        Some(Ok(())) => continue, // still busy
                        Some(Err(_)) => return,   // the watcher is gone
                        None => break,            // settled
                    }
                }
                // Full only means a report is already pending: the burst coalesces into it.
                if tx.try_send(()).is_err() && tx.is_closed() {
                    return;
                }
            }
        })
    });
    Watcher { events: rx, _watcher: watcher, _debounce_task: debounce_task }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Receives with a timeout, from outside the async world.
    fn recv_within(rx: &channel::Receiver<()>, timeout: Duration) -> bool {
        smol::block_on(smol::future::or(
            async { rx.recv().await.is_ok() },
            async {
                smol::Timer::after(timeout).await;
                false
            },
        ))
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
        // Quiet directory: no events.
        assert!(!recv_within(&watcher.events, Duration::from_millis(400)));

        // A burst of changes produces exactly one event...
        for name in ["a.opus", "b.opus", "c.opus"] {
            std::fs::write(root.join(name), b"x").unwrap();
        }
        assert!(recv_within(&watcher.events, Duration::from_secs(5)));
        // ...and nothing more once settled.
        assert!(!recv_within(&watcher.events, Duration::from_millis(400)));

        let _ = std::fs::remove_dir_all(&root);
    }
}
