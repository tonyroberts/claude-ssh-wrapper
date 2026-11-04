//! Local (wrapper) side: runs as the IDE's user, builds the launch spec and runs ssh.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Stdio};

use anyhow::{Context, Result};

use crate::FAILURE_EXIT_CODE;
use crate::config::{self, Config};
use crate::log::{Level, Logger};
use crate::powershell::{self, SpecDelivery};
use crate::protocol::{self, LaunchSpec};
use crate::quote;
use crate::ssh::{self, TtyMode};

pub fn run(claude_args: &[String]) -> i32 {
    let (config, config_path) = match load_config() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("claude-ssh: {err:#}");
            return FAILURE_EXIT_CODE;
        }
    };
    let logger = open_logger(&config, &config_path);
    logger.debug(format_args!("config: {}", config_path.display()));

    match launch(&config, claude_args, &logger) {
        Ok(code) => {
            logger.info(format_args!("exited with code {code}"));
            code
        }
        Err(err) => {
            logger.error(format_args!("{err:#}"));
            eprintln!("claude-ssh: {err:#}");
            FAILURE_EXIT_CODE
        }
    }
}

fn load_config() -> Result<(Config, PathBuf)> {
    let path = config::locate()?;
    let config = Config::load(&path)?;
    Ok((config, path))
}

/// Falls back to a disabled logger if the log file can't be opened: a logging problem
/// shouldn't stop Claude Code from running.
fn open_logger(config: &Config, config_path: &Path) -> Logger {
    let Some(file) = &config.logging.file else {
        return Logger::disabled();
    };
    // Relative log paths are resolved against the config file's directory.
    let path = config_path
        .parent()
        .map_or_else(|| file.clone(), |dir| dir.join(file));
    Logger::open(&path, config.logging.level).unwrap_or_else(|err| {
        eprintln!("claude-ssh: cannot open log file {}: {err}", path.display());
        Logger::disabled()
    })
}

fn launch(config: &Config, claude_args: &[String], logger: &Logger) -> Result<i32> {
    let cwd = match std::env::current_dir() {
        Ok(dir) => Some(dir),
        Err(err) => {
            logger.warn(format_args!("cannot determine current directory: {err}"));
            None
        }
    };

    let vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    if logger.enabled(Level::Debug) {
        let names: Vec<_> = vars
            .iter()
            .map(|(name, _)| name.to_string_lossy())
            .collect();
        logger.debug(format_args!(
            "environment variables present: {}",
            names.join(", ")
        ));
    }
    let env = forwarded_env(&config.env.forward, vars);

    let claude_args = match claude_args.split_first() {
        Some((first, rest)) if is_wrapped_claude_path(first) => {
            logger.debug(format_args!(
                "ignoring the IDE's own Claude Code path (process wrapper call): {first}"
            ));
            rest
        }
        _ => claude_args,
    };
    let args: Vec<&str> = config
        .claude
        .args
        .iter()
        .chain(claude_args)
        .map(String::as_str)
        .collect();

    logger.info(format_args!(
        "launching {} as {}@{}",
        config.claude.command, config.ssh.user, config.ssh.host
    ));
    logger.debug(format_args!(
        "args: {args:?}; cwd: {cwd:?}; forwarded env: {:?}",
        env.keys().collect::<Vec<_>>()
    ));

    let spec_json = launch_spec(&config.claude.command, &args, cwd, env)?.to_json()?;
    let tty = if std::io::stdin().is_terminal() {
        TtyMode::Force
    } else {
        TtyMode::Disable
    };
    // In TTY mode stdin is the terminal, so the spec has to travel inside the command.
    let script = powershell::bootstrap_script(match tty {
        TtyMode::Force => SpecDelivery::Inline(&spec_json),
        TtyMode::Disable => SpecDelivery::Stdin,
    });
    let remote_command = powershell::remote_command(&config.claude.powershell, &script)?;

    let mut command = ssh::build_command(config, &remote_command, tty);
    command.stderr(Stdio::piped());
    if tty == TtyMode::Disable {
        command.stdin(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start ssh ({})", config.ssh.path.display()))?;
    if let Some(stdin) = child.stdin.take() {
        spawn_stdin_pump(stdin, format!("{spec_json}\n"));
    }
    let stderr = child.stderr.take().context("ssh stderr was not captured")?;

    let status = std::thread::scope(|scope| {
        scope.spawn(|| forward_stderr(BufReader::new(stderr), &mut std::io::stderr(), logger));
        child.wait()
    })
    .context("failed waiting for ssh")?;

    Ok(status.code().unwrap_or(FAILURE_EXIT_CODE))
}

/// IDEs with a "process wrapper" setting (e.g. VS Code's Claude Code extension) run
/// `<wrapper> <path to the IDE's bundled claude> <args...>`. That path means nothing to the
/// target user, so it's recognised here and dropped: an absolute path whose file name is
/// `claude` (any extension). Claude Code itself never takes such a path as its first argument.
fn is_wrapped_claude_path(arg: &str) -> bool {
    let path = Path::new(arg);
    path.is_absolute()
        && path
            .file_stem()
            .is_some_and(|stem| stem.eq_ignore_ascii_case("claude"))
}

/// Builds the spec, quoting the arguments for the kind of target: batch files are run
/// through cmd.exe (`program: None`) and need cmd-safe escaping.
fn launch_spec(
    command: &str,
    args: &[&str],
    cwd: Option<PathBuf>,
    env: BTreeMap<String, String>,
) -> Result<LaunchSpec> {
    let (program, arguments) = if quote::is_batch_file(command) {
        (None, quote::batch_command_arguments(command, args)?)
    } else {
        (Some(command.to_owned()), quote::join_windows_args(args))
    };
    Ok(LaunchSpec {
        target: command.to_owned(),
        program,
        arguments,
        cwd,
        env,
    })
}

/// Sends the spec line, then copies the wrapper's stdin to ssh until EOF. When this thread
/// ends, the pipe is dropped, so EOF reaches Claude Code.
///
/// The thread is detached on purpose: if ssh exits while stdin is still open, a blocked
/// read must not stop the wrapper from exiting.
fn spawn_stdin_pump(mut ssh_stdin: ChildStdin, spec_line: String) {
    std::thread::spawn(move || {
        if ssh_stdin.write_all(spec_line.as_bytes()).is_ok() {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut ssh_stdin);
        }
    });
}

/// Selects the environment variables allowed by `patterns`. A pattern matches a name exactly,
/// or as a prefix if it ends in `*`. Matching ignores case, as Windows variable names do.
/// Variables that aren't valid Unicode are skipped.
fn forwarded_env(
    patterns: &[String],
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> BTreeMap<String, String> {
    vars.into_iter()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(name, _)| {
            patterns
                .iter()
                .any(|pattern| env_name_matches(pattern, name))
        })
        .collect()
}

fn env_name_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix)),
        None => name.eq_ignore_ascii_case(pattern),
    }
}

/// Copies the ssh stderr stream to `output`. Bootstrap diagnostic lines go to the log
/// instead, and errors are also shown to the user.
///
/// Output is forwarded a line at a time, so text without a trailing newline is held back
/// until the newline (or the end of the stream) arrives.
fn forward_stderr(mut input: impl BufRead, output: &mut impl Write, logger: &Logger) {
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line);
        if let Some((level, message)) = protocol::parse_diagnostic(&text) {
            logger.log(level, format_args!("remote: {message}"));
            if level == Level::Error {
                let _ = writeln!(output, "claude-ssh: {message}");
            }
            continue;
        }
        logger.debug(format_args!("stderr: {}", text.trim_end()));
        let _ = output.write_all(&line);
        let _ = output.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(names: &[&str]) -> Vec<(OsString, OsString)> {
        names
            .iter()
            .map(|name| (OsString::from(name), OsString::from("value")))
            .collect()
    }

    fn forwarded_names(patterns: &[&str], names: &[&str]) -> Vec<String> {
        let patterns: Vec<String> = patterns.iter().map(|p| (*p).to_owned()).collect();
        forwarded_env(&patterns, vars(names)).into_keys().collect()
    }

    #[test]
    fn exact_pattern_matches_case_insensitively() {
        assert_eq!(
            forwarded_names(&["TERM_PROGRAM"], &["term_program", "TERM"]),
            ["term_program"]
        );
    }

    #[test]
    fn wildcard_pattern_matches_prefix() {
        assert_eq!(
            forwarded_names(
                &["CLAUDE_CODE_*"],
                &["CLAUDE_CODE_SSE_PORT", "CLAUDE", "PATH"]
            ),
            ["CLAUDE_CODE_SSE_PORT"]
        );
    }

    #[test]
    fn nothing_is_forwarded_without_patterns() {
        assert!(forwarded_names(&[], &["PATH", "USERNAME"]).is_empty());
    }

    #[test]
    fn ide_bundled_claude_path_is_recognised() {
        assert!(is_wrapped_claude_path(
            r"c:\Users\me\.vscode\extensions\anthropic.claude-code-2.1.317-win32-x64\resources\native-binary\claude.exe"
        ));
    }

    #[test]
    fn relative_claude_is_not_a_wrapped_path() {
        assert!(!is_wrapped_claude_path("claude.exe"));
    }

    #[test]
    fn ordinary_first_arg_is_not_a_wrapped_path() {
        assert!(!is_wrapped_claude_path("--output-format"));
    }

    #[test]
    fn absolute_path_to_other_file_is_not_a_wrapped_path() {
        assert!(!is_wrapped_claude_path(r"C:\work\notes.txt"));
    }

    #[test]
    fn exe_spec_runs_program_directly() {
        let spec = launch_spec(r"C:\bin\claude.exe", &["-p", "a b"], None, BTreeMap::new());
        let spec = spec.unwrap();
        assert_eq!(
            (spec.program.as_deref(), spec.arguments.as_str()),
            (Some(r"C:\bin\claude.exe"), r#"-p "a b""#)
        );
    }

    #[test]
    fn batch_spec_runs_through_comspec() {
        let spec = launch_spec(r"C:\npm\claude.cmd", &["-p"], None, BTreeMap::new()).unwrap();
        assert_eq!(
            (spec.program, spec.arguments.as_str()),
            (None, r#"/e:ON /v:OFF /d /c ""C:\npm\claude.cmd" -p""#)
        );
    }

    #[test]
    fn stderr_passes_through_ordinary_output() {
        let mut output = Vec::new();
        forward_stderr(&b"hello\nworld"[..], &mut output, &Logger::disabled());
        assert_eq!(output, b"hello\nworld");
    }

    #[test]
    fn stderr_drops_non_error_diagnostics() {
        let input = format!(
            "{}\nafter\n",
            protocol::format_diagnostic(Level::Warn, "cd failed")
        );
        let mut output = Vec::new();
        forward_stderr(input.as_bytes(), &mut output, &Logger::disabled());
        assert_eq!(output, b"after\n");
    }

    #[test]
    fn stderr_shows_error_diagnostics_to_user() {
        let input = format!("{}\n", protocol::format_diagnostic(Level::Error, "boom"));
        let mut output = Vec::new();
        forward_stderr(input.as_bytes(), &mut output, &Logger::disabled());
        assert_eq!(output, b"claude-ssh: boom\n");
    }
}
