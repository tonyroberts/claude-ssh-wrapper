# CLAUDE.md

Notes for working on this project. Keep this file up to date as design decisions are made,
and record the *why* as well as the *what*.

## Project summary

`claude-ssh.exe` is a Windows console app (Rust) that acts as a drop-in replacement for the
Claude Code CLI (`claude`). It forwards every invocation over `ssh` to a different **local**
Windows user on the same PC, so that Claude Code runs with that user's (restricted) permissions.
It's mainly for IDEs (VS Code, JetBrains) that launch `claude` as a subprocess.
Configuration lives in `claude-ssh.yaml` next to the executable.

## Hard requirements

- **Drop-in replacement:** same arguments, same stdio behaviour, same exit codes as `claude`.
- **Binary-clean stdio:** IDEs use stream-json over stdin/stdout. Nothing may inject bytes,
  banners, prompts or CRLF translation into the stream.
- **Non-interactive:** never prompt. Failures go to stderr with a non-zero exit code.
- **Nothing from this project is accessible to the target user:** the remote side only uses
  built-in Windows PowerShell.
- Windows only (MSVC toolchain).

## Architecture

1. **Wrapper** (runs as the IDE user): loads the config, opens the log, and builds a
   `LaunchSpec` {target, program, pre-quoted arguments, cwd, allowed env}. It then runs
   `ssh -T -l <user> <host> "powershell.exe" -NoLogo -NoProfile -NonInteractive -OutputFormat Text -EncodedCommand <bootstrap>`.
   - stdin is piped: a detached thread writes the spec as one JSON line, then copies the wrapper's stdin.
   - stdout is inherited.
   - stderr is piped through a thread that sends diagnostic lines to the log and forwards
     everything else.
2. **Bootstrap** (`src/bootstrap.ps1`, embedded with `include_str!`, comments stripped,
   UTF-16LE base64): reads the spec line from stdin **one byte at a time**, so nothing meant for
   Claude Code is consumed. It checks that the target exists, sets the env vars, and starts the
   program via `ProcessStartInfo` (no redirection, so stdio is inherited) in the cwd. If that
   fails, it retries in `%USERPROFILE%`. It returns the child's exit code. Diagnostics go to
   stderr as `##claude-ssh-remote## <level> <message>` lines.
3. **TTY mode** (`-t`, wrapper's stdin is a console): the spec is embedded in the script as
   base64 instead, and stdin is inherited.

## Layout

```
Cargo.toml                 package `claude-ssh-wrapper`, binary `claude-ssh`
build.rs                   embeds the Windows version resource (winresource; version from Cargo.toml)
clippy.toml                allow unwrap/expect in tests
.github/workflows/ci.yml   fmt check, clippy and tests on every push/PR (windows-latest)
.github/workflows/release.yml  on release published: set Cargo.toml version from the tag (vX.Y.Z), test, build, zip exe + README + LICENSE + example config, upload to the release
claude-ssh.example.yaml    commented example config (a real `claude-ssh.yaml` is git-ignored)
src/main.rs                entry point: UTF-8 args, exit code
src/client.rs              wrapper: config + logger, env allowlist, spec, run ssh, stdin/stderr pumps
src/bootstrap.ps1          remote PowerShell bootstrap script
src/powershell.rs          bootstrap generation/encoding, remote command string (+ real PowerShell tests)
src/quote.rs               argument quoting: MSVC rules for .exe, cmd-safe escaping for .cmd/.bat
src/protocol.rs            LaunchSpec JSON + diagnostic line format
src/ssh.rs                 builds the ssh `Command`
src/config.rs              YAML config structs, defaults, validation, config file lookup
src/log.rs                 minimal append-only file logger and log levels
```

## Commands

```powershell
cargo build                                   # debug build
cargo build --release                         # release build -> target\release\claude-ssh.exe
cargo test                                    # must run on Windows (runs powershell.exe)
cargo clippy --all-targets -- -D warnings
cargo fmt
```

## Conventions

- Follow the `rust-best-practices` skill (`.claude/skills/rust-best-practices`).
- `anyhow` for error handling. No `unwrap`/`expect` outside tests (clippy lint).
- `unsafe` is denied crate-wide.
- Tests sit next to the code (`#[cfg(test)] mod tests`), one behaviour per test, with descriptive names.
- The wrapper never writes to stdout. Its diagnostics go to stderr prefixed with `claude-ssh:`,
  and to the log if enabled.
- The bootstrap only reports through `Write-Diagnostic`. It must never write to stdout or read
  stdin past the spec line.
- Logging failures must never stop Claude Code from running.

## Design decisions

| Decision | Rationale |
| --- | --- |
| Use the system `ssh.exe` as a subprocess, not an SSH library | Reuses the user's keys, agent, `known_hosts` and `ssh_config`. No crypto code here. |
| Config is `claude-ssh.yaml` next to the exe, overridable with `CLAUDE_SSH_CONFIG` | A fixed name means renaming the exe doesn't change which config is used. |
| `serde_yaml_ng` for YAML, with `deny_unknown_fields` | `serde_yaml` is unmaintained. Typos in security-relevant config fail loudly. |
| User passed with `ssh -l`; `ssh.user`/`ssh.host` that start with `-` or contain control chars are rejected | Supports `user@domain` names. Prevents option injection. |
| `-t` when the wrapper's stdin is a console, otherwise `-T` | Interactive use needs a PTY. IDE stdio must stay binary-clean. |
| Exit via `std::process::exit(i32)` with the child's code | `ExitCode` only takes `u8`. |
| Remote side = PowerShell `-EncodedCommand` bootstrap, with the spec sent as the first stdin line | sshd runs commands through cmd.exe. Base64 gives cmd nothing to interpret, and stdin avoids cmd's 8191-char limit. Nothing of ours needs to be accessible to the target user. Covered by tests: PowerShell doesn't read ahead on stdin, the child inherits the remaining stdin, and stdout stays clean. |
| Arguments are pre-quoted by the wrapper into one `ProcessStartInfo.Arguments` string. `.exe`: MSVC rules. `.cmd`/`.bat`: `%ComSpec% /e:ON /v:OFF /d /c ""script" args"` using Rust std's batch escaping (`"` doubled, `%` neutralised, `\r`/`\n`/NUL rejected) | Windows PowerShell 5.1 mangles embedded quotes when it passes arguments to native programs. Batch files re-parse arguments through cmd ("BatBadBut"). |
| PowerShell path is always quoted; paths with `" % & < > ( ) @ ^ \| !` are rejected. Remote command capped at 8191 chars | Fits cmd `/c` quote handling and its line limit. Only TTY mode (inline spec) can get close to the limit. |
| Working dir = wrapper's cwd, falling back to `%USERPROFILE%` with a warning | The target user may not have access to the IDE user's directory. |
| Env vars are forwarded only by allowlist (`env.forward`; exact or trailing `*`, case-insensitive) | Never leak the IDE user's secrets to the other user. |
| Remote diagnostics are prefixed stderr lines; the wrapper logs them, and also shows `error` ones on stderr | One log in one place, which the target user doesn't need to be able to write to. Errors stay visible with logging off. |
| Small custom logger (`log.rs`) | Only needs append, level filter, timestamp and pid. |
| At `debug`, log env var *names* (never values) | Shows which variables an IDE sets, for tuning the allowlist. |
| Drop a first argument that is an absolute path to `claude.*` | VS Code's `claudeCode.claudeProcessWrapper` calls `<wrapper> <bundled claude.exe> <args>`. JetBrains' "Claude Command" setting replaces `claude` outright, so it calls with no such argument. Claude Code never takes such a path as its first argument, so one config works for both. |
| The bootstrap checks that an absolute `claude.command` exists | Broken installs (e.g. an interrupted npm auto-update) give a clear error instead of cmd's "is not recognized". |
| Detached thread for the stdin pump | A blocked stdin read must not stop the wrapper from exiting after ssh exits. |
