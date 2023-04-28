//! A custom logger that sends the message formatted with color & timestamp on a [`tokio::sync::mpsc::channel`] channel
//! instead of immediately it printing anywhere.
//!
//! Based on `simple_logger` by Sam Clements / @borntyping on Github.

use crossterm::style::Stylize;
use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::str::FromStr;
use time::{format_description::FormatItem, OffsetDateTime};
use tokio::sync::mpsc as async_mpsc;

const TIMESTAMP_FORMAT_UTC: &[FormatItem] =
    time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

pub struct Logger {
    tx: async_mpsc::Sender<String>,
    default_level: LevelFilter,
    module_levels: Vec<(String, LevelFilter)>,
}

impl Logger {
    pub fn new(tx: async_mpsc::Sender<String>, module_levels: Vec<(String, LevelFilter)>) -> Self {
        Self {
            tx,
            default_level: std::env::var("RUST_LOG")
                .ok()
                .as_deref()
                .map(log::LevelFilter::from_str)
                .and_then(Result::ok)
                .unwrap_or(LevelFilter::Info),
            module_levels,
        }
    }
}

impl Logger {
    pub fn init(mut self) -> Result<(), SetLoggerError> {
        // Sort all module levels from most specific to least specific. The length of the module name is used instead of
        // its actual depth to avoid module name parsing.
        self.module_levels.sort_by_key(|(name, _level)| name.len().wrapping_neg());
        let max_level = self.module_levels.iter().map(|(_name, level)| level).copied().max();
        let max_level = max_level.map(|lvl| lvl.max(self.default_level)).unwrap_or(self.default_level);
        log::set_max_level(max_level);
        log::set_boxed_logger(Box::new(self))?;
        Ok(())
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        &metadata.level().to_level_filter()
            <= self
                .module_levels
                .iter()
                // At this point the Vec is already sorted so that we can simply take the first match
                .find(|(name, _level)| metadata.target().starts_with(name))
                .map(|(_name, level)| level)
                .unwrap_or(&self.default_level)
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let level_string = match record.level() {
                Level::Error => format!("{:<5}", record.level().to_string()).red().to_string(),
                Level::Warn => format!("{:<5}", record.level().to_string()).yellow().to_string(),
                Level::Info => format!("{:<5}", record.level().to_string()).cyan().to_string(),
                Level::Debug => format!("{:<5}", record.level().to_string()).dark_green().to_string(),
                Level::Trace => format!("{:<5}", record.level().to_string()).grey().to_string(),
            };
            let target =
                if !record.target().is_empty() { record.target() } else { record.module_path().unwrap_or_default() };
            let timestamp = format!("{} ", OffsetDateTime::now_utc().format(TIMESTAMP_FORMAT_UTC).unwrap());
            let message = format!("{}{} [{}] {}", timestamp, level_string, target, record.args());
            if let Err(message) = self.tx.try_send(message) {
                eprintln!("{}", message)
            }
        }
    }

    fn flush(&self) {}
}
