//! Loading and validation of the `claude-ssh.yaml` configuration file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::log::Level;

/// Name of the configuration file looked up next to the executable.
pub const CONFIG_FILE_NAME: &str = "claude-ssh.yaml";

/// Environment variable that, when set, overrides the configuration file location.
pub const CONFIG_PATH_ENV_VAR: &str = "CLAUDE_SSH_CONFIG";

/// Top-level configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub ssh: SshConfig,
    pub claude: ClaudeConfig,
    #[serde(default)]
    pub env: EnvConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

/// Environment variables passed to Claude Code.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvConfig {
    /// Allowlist of variable names to forward from the wrapper's environment. A trailing `*`
    /// matches any suffix. Matching ignores case.
    #[serde(default)]
    pub forward: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    /// Log file to append to. Logging is disabled when unset. Relative paths are resolved
    /// against the config file's directory.
    #[serde(default)]
    pub file: Option<PathBuf>,
    #[serde(default)]
    pub level: Level,
}

/// How to connect to the target user.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshConfig {
    /// Path to the ssh client executable.
    #[serde(default = "default_ssh_path")]
    pub path: PathBuf,
    /// Local user that Claude Code runs as. Passed with `-l`, so `user@domain` works.
    pub user: String,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub identity_file: Option<PathBuf>,
    /// Passed to ssh as `-o Key=Value`.
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    /// Passed to ssh verbatim, before the destination.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

/// How to launch Claude Code on the remote side.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaudeConfig {
    /// Path to the Claude Code executable or `.cmd`/`.bat` script, as seen by the target user.
    pub command: String,
    /// Arguments inserted before the arguments the wrapper was called with.
    #[serde(default)]
    pub args: Vec<String>,
    /// PowerShell executable on the remote side that runs the bootstrap script.
    #[serde(default = "default_powershell")]
    pub powershell: String,
}

fn default_powershell() -> String {
    "powershell.exe".to_owned()
}

fn default_ssh_path() -> PathBuf {
    PathBuf::from("ssh.exe")
}

fn default_host() -> String {
    "localhost".to_owned()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        Self::from_yaml(&text).with_context(|| format!("invalid config file {}", path.display()))
    }

    pub fn from_yaml(text: &str) -> Result<Self> {
        let config: Self = serde_yaml_ng::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        validate_ssh_value("ssh.user", &self.ssh.user)?;
        validate_ssh_value("ssh.host", &self.ssh.host)?;
        if self.claude.command.trim().is_empty() {
            bail!("claude.command must not be empty");
        }
        if self.claude.powershell.trim().is_empty() {
            bail!("claude.powershell must not be empty");
        }
        for pattern in &self.env.forward {
            let name = pattern.strip_suffix('*').unwrap_or(pattern);
            if name.is_empty() || name.contains(['*', '=']) {
                bail!(
                    "env.forward entry {pattern:?} is invalid: it must be a variable name, \
                     optionally ending in '*'"
                );
            }
        }
        Ok(())
    }
}

/// User and host are passed to ssh as arguments; reject values ssh could treat as options.
fn validate_ssh_value(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field} must not be empty");
    }
    if value.starts_with('-') {
        bail!("{field} must not start with '-'");
    }
    if value.chars().any(char::is_control) {
        bail!("{field} must not contain control characters");
    }
    Ok(())
}

/// Returns the configuration file path: `$CLAUDE_SSH_CONFIG` if set, otherwise
/// `claude-ssh.yaml` in the same directory as the running executable.
pub fn locate() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG_PATH_ENV_VAR).filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().context("failed to determine the executable path")?;
    let dir = exe
        .parent()
        .context("executable path has no parent directory")?;
    Ok(dir.join(CONFIG_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "
ssh:
  user: claude
claude:
  command: claude.cmd
";

    #[test]
    fn host_defaults_to_localhost() {
        let config = Config::from_yaml(MINIMAL).unwrap();
        assert_eq!(config.ssh.host, "localhost");
    }

    #[test]
    fn ssh_path_defaults_to_ssh_exe() {
        let config = Config::from_yaml(MINIMAL).unwrap();
        assert_eq!(config.ssh.path, PathBuf::from("ssh.exe"));
    }

    #[test]
    fn missing_user_is_rejected() {
        let yaml = "ssh: {}\nclaude:\n  command: claude.cmd\n";
        assert!(Config::from_yaml(yaml).is_err());
    }

    #[test]
    fn unknown_field_is_rejected() {
        let yaml = format!("{MINIMAL}unexpected: true\n");
        assert!(Config::from_yaml(&yaml).is_err());
    }

    #[test]
    fn user_starting_with_dash_is_rejected() {
        let yaml = "ssh:\n  user: -oProxyCommand=x\nclaude:\n  command: claude.cmd\n";
        assert!(Config::from_yaml(yaml).is_err());
    }

    #[test]
    fn logging_is_disabled_by_default() {
        let config = Config::from_yaml(MINIMAL).unwrap();
        assert!(config.logging.file.is_none());
    }

    #[test]
    fn logging_level_is_parsed() {
        let yaml = format!("{MINIMAL}logging:\n  file: claude-ssh.log\n  level: debug\n");
        let config = Config::from_yaml(&yaml).unwrap();
        assert_eq!(config.logging.level, Level::Debug);
    }

    #[test]
    fn env_forward_accepts_trailing_wildcard() {
        let yaml = format!("{MINIMAL}env:\n  forward: ['CLAUDE_CODE_*', 'TERM_PROGRAM']\n");
        assert!(Config::from_yaml(&yaml).is_ok());
    }

    #[test]
    fn env_forward_rejects_bare_wildcard() {
        let yaml = format!("{MINIMAL}env:\n  forward: ['*']\n");
        assert!(Config::from_yaml(&yaml).is_err());
    }

    #[test]
    fn env_forward_rejects_inner_wildcard() {
        let yaml = format!("{MINIMAL}env:\n  forward: ['A*B']\n");
        assert!(Config::from_yaml(&yaml).is_err());
    }

    #[test]
    fn empty_claude_command_is_rejected() {
        let yaml = "ssh:\n  user: claude\nclaude:\n  command: ''\n";
        assert!(Config::from_yaml(yaml).is_err());
    }
}
