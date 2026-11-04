//! Minimal append-only file logger. Logging is disabled unless `logging.file` is configured.
//!
//! Several wrapper processes may log to the same file at once (IDEs often start more than one
//! `claude`), so each entry is written with a single append and tagged with the process id.

use std::fmt::Display;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use serde::Deserialize;

/// Log levels, from least to most verbose.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "error" => Some(Self::Error),
            "warn" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            _ => None,
        }
    }
}

pub struct Logger {
    level: Level,
    file: Option<Mutex<File>>,
}

impl Logger {
    pub fn disabled() -> Self {
        Self {
            level: Level::Error,
            file: None,
        }
    }

    /// Opens `path` for appending, creating it and its parent directories if needed.
    pub fn open(path: &Path, level: Level) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            level,
            file: Some(Mutex::new(file)),
        })
    }

    pub fn enabled(&self, level: Level) -> bool {
        self.file.is_some() && level <= self.level
    }

    pub fn log(&self, level: Level, message: impl Display) {
        if !self.enabled(level) {
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        let line = format!(
            "{} [{}] {:<5} {message}\n",
            humantime::format_rfc3339_millis(SystemTime::now()),
            std::process::id(),
            level.as_str(),
        );
        // Logging must never break the wrapper, so failures are ignored.
        if let Ok(mut file) = file.lock() {
            let _ = file.write_all(line.as_bytes());
        }
    }

    pub fn error(&self, message: impl Display) {
        self.log(Level::Error, message);
    }

    pub fn warn(&self, message: impl Display) {
        self.log(Level::Warn, message);
    }

    pub fn info(&self, message: impl Display) {
        self.log(Level::Info, message);
    }

    pub fn debug(&self, message: impl Display) {
        self.log(Level::Debug, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_logger_is_never_enabled() {
        assert!(!Logger::disabled().enabled(Level::Error));
    }

    #[test]
    fn level_round_trips_through_text() {
        for level in [Level::Error, Level::Warn, Level::Info, Level::Debug] {
            assert_eq!(Level::parse(level.as_str()), Some(level));
        }
    }

    #[test]
    fn debug_is_more_verbose_than_info() {
        assert!(Level::Debug > Level::Info);
    }
}
