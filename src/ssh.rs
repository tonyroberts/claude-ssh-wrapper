//! Construction of the `ssh` command line.

use std::process::Command;

use crate::config::Config;

/// Whether ssh should allocate a pseudo-terminal on the remote side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtyMode {
    /// Interactive use from a console: `-t`.
    Force,
    /// Piped stdio (e.g. an IDE using stream-json): `-T`, keeping the byte stream clean.
    Disable,
}

/// Builds the ssh command that runs `remote_command` as the configured user.
pub fn build_command(config: &Config, remote_command: &str, tty: TtyMode) -> Command {
    let ssh = &config.ssh;
    let mut command = Command::new(&ssh.path);

    command.arg(match tty {
        TtyMode::Force => "-t",
        TtyMode::Disable => "-T",
    });
    command.arg("-l").arg(&ssh.user);
    if let Some(port) = ssh.port {
        command.arg("-p").arg(port.to_string());
    }
    if let Some(identity_file) = &ssh.identity_file {
        command.arg("-i").arg(identity_file);
    }
    for (key, value) in &ssh.options {
        command.arg("-o").arg(format!("{key}={value}"));
    }
    command.args(&ssh.extra_args);
    command.arg(&ssh.host);
    command.arg(remote_command);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(yaml: &str) -> Config {
        Config::from_yaml(yaml).unwrap()
    }

    fn args_of(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    const BASIC: &str = "
ssh:
  user: claude
claude:
  command: claude.exe
";

    #[test]
    fn minimal_config_builds_expected_args() {
        let command = build_command(&config(BASIC), "REMOTE", TtyMode::Disable);
        assert_eq!(
            args_of(&command),
            ["-T", "-l", "claude", "localhost", "REMOTE"]
        );
    }

    #[test]
    fn force_tty_passes_dash_t() {
        let command = build_command(&config(BASIC), "REMOTE", TtyMode::Force);
        assert_eq!(args_of(&command)[0], "-t");
    }

    #[test]
    fn optional_ssh_settings_precede_host() {
        let yaml = "
ssh:
  user: claude
  host: example
  port: 2222
  identity_file: C:\\keys\\id_ed25519
  options:
    BatchMode: 'yes'
  extra_args: ['-q']
claude:
  command: claude
";
        let command = build_command(&config(yaml), "REMOTE", TtyMode::Disable);
        assert_eq!(
            args_of(&command),
            [
                "-T",
                "-l",
                "claude",
                "-p",
                "2222",
                "-i",
                r"C:\keys\id_ed25519",
                "-o",
                "BatchMode=yes",
                "-q",
                "example",
                "REMOTE",
            ]
        );
    }
}
