//! A logger that hands records to the application instead of writing them anywhere.
//!
//! A terminal UI owns the screen, so a log line printed to stderr lands in the middle of it. These
//! go on a channel, and the application keeps the recent ones to show on request (see
//! [`Model::log`](crate::model::Model::log)).

use log::{Level, LevelFilter, Log, Metadata, Record};
use smol::channel;
use std::str::FromStr;

/// One captured record: enough to show it, no formatting decided yet.
#[derive(Debug, Clone)]
pub struct Entry {
    pub level: Level,
    pub message: String,
}

struct Logger {
    tx: channel::Sender<Entry>,
    level: LevelFilter,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let entry = Entry { level: record.level(), message: record.args().to_string() };
        // Unbounded, so this only fails once the application is gone; dropping records then is
        // exactly right.
        let _ = self.tx.try_send(entry);
    }

    fn flush(&self) {}
}

/// Installs the logger and returns the receiver its records arrive on. The level comes from
/// `$RUST_LOG`, defaulting to warnings and errors -- the ones worth a user's attention.
pub fn start() -> channel::Receiver<Entry> {
    let (tx, rx) = channel::unbounded();
    let level =
        std::env::var("RUST_LOG").ok().as_deref().and_then(|s| LevelFilter::from_str(s).ok()).unwrap_or(LevelFilter::Warn);
    log::set_max_level(level);
    // Fails only if something already installed a logger, in which case ours simply does not run.
    if log::set_boxed_logger(Box::new(Logger { tx, level })).is_err() {
        log::warn!("a logger was already installed");
    }
    rx
}
