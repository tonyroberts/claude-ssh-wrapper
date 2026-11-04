//! Generation of the remote command: `powershell.exe ... -EncodedCommand <bootstrap>`.
//!
//! Windows sshd runs the remote command through cmd.exe. `-EncodedCommand` takes base64
//! (UTF-16LE) so cmd has nothing to interpret. Nothing needs to be installed for the
//! target user: the bootstrap script (`src/bootstrap.ps1`) is generated on every launch.

use anyhow::{Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

const BOOTSTRAP: &str = include_str!("bootstrap.ps1");
const SPEC_SOURCE_PLACEHOLDER: &str = "__SPEC_SOURCE__";

const POWERSHELL_ARGS: &[&str] = &[
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-OutputFormat",
    "Text",
];

/// cmd.exe's maximum command line length.
const MAX_REMOTE_COMMAND_LEN: usize = 8191;

/// Characters that cmd.exe would interpret in the PowerShell path. If any of these are in
/// the path, cmd's `/c` quote handling strips our quotes, so such paths are rejected.
const CMD_SPECIAL_CHARS: &[char] = &['"', '%', '&', '<', '>', '(', ')', '@', '^', '|', '!'];

/// How the launch spec reaches the bootstrap.
#[derive(Debug, Clone, Copy)]
pub enum SpecDelivery<'a> {
    /// As the first line of stdin. No size limit, but needs a piped stdin.
    Stdin,
    /// Embedded in the script. Used in TTY mode, where stdin is the terminal.
    Inline(&'a str),
}

/// Returns the bootstrap script with the spec source filled in and comments removed.
pub fn bootstrap_script(delivery: SpecDelivery<'_>) -> String {
    let source = match delivery {
        SpecDelivery::Stdin => "Read-SpecLine".to_owned(),
        SpecDelivery::Inline(json) => format!(
            "[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}'))",
            STANDARD.encode(json)
        ),
    };
    minify(&BOOTSTRAP.replace(SPEC_SOURCE_PLACEHOLDER, &source))
}

/// Strips indentation, blank lines and comment lines to keep the encoded command short.
fn minify(script: &str) -> String {
    script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Encodes a script for `-EncodedCommand`: base64 of the UTF-16LE text.
pub fn encode_command(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    STANDARD.encode(utf16)
}

/// The full command string run by the remote shell.
pub fn remote_command(powershell: &str, script: &str) -> Result<String> {
    if powershell.contains(CMD_SPECIAL_CHARS) {
        bail!(
            "claude.powershell path {powershell} contains a character that cmd.exe treats \
             specially (one of {})",
            CMD_SPECIAL_CHARS.iter().collect::<String>()
        );
    }
    // Always quoting is safe: cmd /c either keeps the quotes (path with spaces) or strips
    // the first and last quote characters, which are exactly ours.
    let command = format!(
        "\"{powershell}\" {} -EncodedCommand {}",
        POWERSHELL_ARGS.join(" "),
        encode_command(script)
    );
    if command.len() > MAX_REMOTE_COMMAND_LEN {
        bail!(
            "remote command is {} characters, over the cmd.exe limit of {MAX_REMOTE_COMMAND_LEN}; \
             reduce the arguments or forwarded environment variables",
            command.len()
        );
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::{Command, Output, Stdio};

    use super::*;
    use crate::log::Level;
    use crate::protocol::{DIAGNOSTIC_PREFIX, LaunchSpec, parse_diagnostic};

    const ENV_VALUE: &str = r#"a & b | "q" %PATH% ^ !x!"#;

    /// A spec that runs `cmd.exe /d /c "<command>"` with one awkward environment variable.
    fn cmd_spec(command: &str, cwd: Option<PathBuf>) -> LaunchSpec {
        LaunchSpec {
            target: r"C:\Windows\System32\cmd.exe".to_owned(),
            program: None,
            arguments: format!("/d /c \"{command}\""),
            cwd,
            env: BTreeMap::from([("CLAUDE_SSH_TEST".to_owned(), ENV_VALUE.to_owned())]),
        }
    }

    fn run_powershell(script: &str, stdin_data: Option<&[u8]>) -> Output {
        let mut child = Command::new("powershell.exe")
            .args(POWERSHELL_ARGS)
            .arg("-EncodedCommand")
            .arg(encode_command(script))
            .stdin(if stdin_data.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(data) = stdin_data {
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(data).unwrap();
        }
        child.wait_with_output().unwrap()
    }

    fn run_via_stdin(spec: &LaunchSpec, rest: &str) -> Output {
        let data = format!("{}\n{rest}", spec.to_json().unwrap());
        run_powershell(
            &bootstrap_script(SpecDelivery::Stdin),
            Some(data.as_bytes()),
        )
    }

    fn stdout_lines(output: &Output) -> Vec<String> {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn diagnostics(output: &Output) -> Vec<(Level, String)> {
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .filter_map(|line| parse_diagnostic(line).map(|(l, m)| (l, m.to_owned())))
            .collect()
    }

    #[test]
    fn minify_removes_comments_and_indentation() {
        assert_eq!(minify("# c\n  a\n\n    # d\n  b  \n"), "a\nb");
    }

    #[test]
    fn script_has_no_unreplaced_placeholder() {
        assert!(!bootstrap_script(SpecDelivery::Stdin).contains(SPEC_SOURCE_PLACEHOLDER));
    }

    #[test]
    fn bootstrap_uses_protocol_diagnostic_prefix() {
        assert!(BOOTSTRAP.contains(DIAGNOSTIC_PREFIX));
    }

    #[test]
    fn encoded_command_is_utf16le_base64() {
        assert_eq!(encode_command("ab"), STANDARD.encode([b'a', 0, b'b', 0]));
    }

    #[test]
    fn remote_command_quotes_powershell_path() {
        let command = remote_command("powershell.exe", "x").unwrap();
        assert!(command.starts_with("\"powershell.exe\" -NoLogo -NoProfile"));
    }

    #[test]
    fn powershell_path_with_parentheses_is_rejected() {
        assert!(remote_command(r"C:\Program Files (x86)\pwsh.exe", "x").is_err());
    }

    #[test]
    fn overlong_remote_command_is_rejected() {
        assert!(remote_command("powershell.exe", &"x".repeat(MAX_REMOTE_COMMAND_LEN)).is_err());
    }

    /// TTY mode embeds the spec in the command, so the bootstrap must leave room for a
    /// realistic interactive launch.
    #[test]
    fn typical_interactive_spec_fits_in_cmd_limit() {
        let spec = LaunchSpec {
            target: r"C:\Users\claude\.local\bin\claude.exe".to_owned(),
            program: Some(r"C:\Users\claude\.local\bin\claude.exe".to_owned()),
            arguments: r#"--model opus --permission-mode plan --allowedTools "Bash(npm run *)" "Read" --resume 0f8fad5b-d9cb-469f-a165-70867728950e"#.to_owned(),
            cwd: Some(PathBuf::from(r"C:\Users\someone\source\repos\a-fairly-long-project-name\sub\dir")),
            env: (0..8)
                .map(|i| (format!("CLAUDE_CODE_SETTING_{i}"), "some-value-1234".to_owned()))
                .collect(),
        };
        let json = spec.to_json().unwrap();
        let script = bootstrap_script(SpecDelivery::Inline(&json));
        assert!(remote_command("powershell.exe", &script).is_ok());
    }

    // The tests below run the real bootstrap through powershell.exe.

    #[test]
    fn stdin_after_spec_line_reaches_child() {
        let output = run_via_stdin(&cmd_spec("findstr /n ^^", None), "one\n{\"j\":\"x & y\"}\n");
        assert_eq!(stdout_lines(&output), ["1:one", "2:{\"j\":\"x & y\"}"]);
    }

    #[test]
    fn env_values_are_passed_verbatim() {
        let output = run_via_stdin(&cmd_spec("set CLAUDE_SSH_TEST", None), "");
        assert_eq!(
            stdout_lines(&output),
            [format!("CLAUDE_SSH_TEST={ENV_VALUE}")]
        );
    }

    #[test]
    fn child_exit_code_is_returned() {
        let output = run_via_stdin(&cmd_spec("exit 7", None), "");
        assert_eq!(output.status.code(), Some(7));
    }

    #[test]
    fn bootstrap_writes_nothing_to_stdout() {
        let output = run_via_stdin(&cmd_spec("exit 0", None), "");
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn child_starts_in_requested_directory() {
        let output = run_via_stdin(&cmd_spec("cd", Some(PathBuf::from(r"C:\Windows"))), "");
        assert_eq!(stdout_lines(&output)[0].to_lowercase(), r"c:\windows");
    }

    #[test]
    fn unusable_directory_falls_back_to_home() {
        let spec = cmd_spec("cd", Some(PathBuf::from(r"C:\claude-ssh\does\not\exist")));
        let output = run_via_stdin(&spec, "");
        let home = std::env::var("USERPROFILE").unwrap();
        assert_eq!(stdout_lines(&output)[0].to_lowercase(), home.to_lowercase());
    }

    #[test]
    fn unusable_directory_reports_warning() {
        let spec = cmd_spec(
            "exit 0",
            Some(PathBuf::from(r"C:\claude-ssh\does\not\exist")),
        );
        let output = run_via_stdin(&spec, "");
        assert!(
            diagnostics(&output)
                .iter()
                .any(|(level, _)| *level == Level::Warn)
        );
    }

    #[test]
    fn missing_target_reports_error() {
        let mut spec = cmd_spec("exit 0", None);
        spec.target = r"C:\claude-ssh\does\not\exist\claude.cmd".to_owned();
        let output = run_via_stdin(&spec, "");
        assert_eq!(
            (
                output.status.code(),
                diagnostics(&output).first().map(|(l, _)| *l)
            ),
            (Some(1), Some(Level::Error))
        );
    }

    #[test]
    fn relative_target_is_not_checked() {
        let mut spec = cmd_spec("exit 0", None);
        spec.target = "claude.exe".to_owned();
        assert_eq!(run_via_stdin(&spec, "").status.code(), Some(0));
    }

    #[test]
    fn missing_spec_reports_error() {
        let output = run_powershell(&bootstrap_script(SpecDelivery::Stdin), Some(b""));
        assert_eq!(
            (
                output.status.code(),
                diagnostics(&output).first().map(|(l, _)| *l)
            ),
            (Some(1), Some(Level::Error))
        );
    }

    #[test]
    fn inline_spec_is_used_without_stdin() {
        let json = cmd_spec("set CLAUDE_SSH_TEST", None).to_json().unwrap();
        let output = run_powershell(&bootstrap_script(SpecDelivery::Inline(&json)), None);
        assert_eq!(
            stdout_lines(&output),
            [format!("CLAUDE_SSH_TEST={ENV_VALUE}")]
        );
    }
}
