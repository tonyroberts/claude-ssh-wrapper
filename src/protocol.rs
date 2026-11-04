//! Data exchanged with the remote bootstrap script (`src/bootstrap.ps1`).
//!
//! Local -> remote: a [`LaunchSpec`] as one line of JSON. It is sent either as the first line
//! of the ssh stdin stream (normal case), or embedded in the encoded bootstrap command (TTY mode).
//!
//! Remote -> local: diagnostic lines on stderr starting with [`DIAGNOSTIC_PREFIX`]. The local
//! wrapper takes these out of the stderr stream and writes them to its log.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::log::Level;

pub const DIAGNOSTIC_PREFIX: &str = "##claude-ssh-remote## ";

/// Everything the bootstrap needs to launch Claude Code.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct LaunchSpec {
    /// The configured Claude Code path (`claude.command`). If it is absolute, the bootstrap
    /// checks that it exists, so a missing or unreadable install gives a clear error.
    pub target: String,
    /// Executable to start, or `None` for `%ComSpec%` (used for `.cmd`/`.bat` targets).
    pub program: Option<String>,
    /// Complete, already-quoted argument string passed to `ProcessStartInfo.Arguments`.
    pub arguments: String,
    /// Requested working directory. The bootstrap falls back to the user's home if it can't be used.
    pub cwd: Option<PathBuf>,
    /// Environment variables to set, in addition to the remote session's own environment.
    pub env: BTreeMap<String, String>,
}

impl LaunchSpec {
    /// Serializes to single-line JSON (serde_json escapes any newlines inside strings).
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).context("failed to serialize launch spec")
    }
}

/// Parses a line from the remote stderr stream. Returns `None` for ordinary output.
pub fn parse_diagnostic(line: &str) -> Option<(Level, &str)> {
    let rest = line.strip_prefix(DIAGNOSTIC_PREFIX)?;
    let (level, message) = rest.split_once(' ')?;
    Some((Level::parse(level)?, message.trim_end()))
}

#[cfg(test)]
pub fn format_diagnostic(level: Level, message: &str) -> String {
    format!("{DIAGNOSTIC_PREFIX}{} {message}", level.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_json_is_a_single_line() {
        let spec = LaunchSpec {
            target: "claude.exe".to_owned(),
            program: None,
            arguments: "a\nb".to_owned(),
            cwd: None,
            env: BTreeMap::from([("X".to_owned(), "1\r\n2".to_owned())]),
        };
        assert!(!spec.to_json().unwrap().contains(['\r', '\n']));
    }

    #[test]
    fn diagnostic_round_trips() {
        let line = format_diagnostic(Level::Warn, "cannot use directory");
        assert_eq!(
            parse_diagnostic(&format!("{line}\r\n")),
            Some((Level::Warn, "cannot use directory"))
        );
    }

    #[test]
    fn diagnostic_with_unknown_level_is_ignored() {
        assert_eq!(
            parse_diagnostic(&format!("{DIAGNOSTIC_PREFIX}loud hello")),
            None
        );
    }

    #[test]
    fn ordinary_output_is_not_a_diagnostic() {
        assert_eq!(parse_diagnostic("Error: something went wrong"), None);
    }
}
