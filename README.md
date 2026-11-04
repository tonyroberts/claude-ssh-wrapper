# claude-ssh-wrapper

Run [Claude Code](https://code.claude.com/docs) as a different local Windows user, so you
can control exactly what it's allowed to access.

`claude-ssh.exe` is a drop-in replacement for the `claude` command. Point your IDE or terminal
at it instead of `claude`, and it launches Claude Code over SSH as a dedicated local account
on the same PC. That account's Windows permissions then decide which files Claude Code can
read, write and execute, while you use Claude Code exactly as before.

Arguments, input/output, the working directory and the exit code are all passed through,
so IDE integrations work as normal.

If you find it useful, please consider buying me a coffee.

<a href="https://www.buymeacoffee.com/tonyroberts" target="_blank"><img src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" alt="Buy Me A Coffee" height="50px" width="210px"></a>

## Things to know

- Claude Code uses **the other account's** installation, login, settings, memory, MCP servers
  and history. Nothing is shared with your own account. Project-level settings in the
  repository are still used.
- The other account needs access to your project folders. If it can't access the current
  folder, Claude Code starts in that account's home folder instead.
- Your environment variables are not passed on, except for the ones you allow in the config.

## Setup

The steps below use `claude` as the name of the account Claude Code runs as.

1. **Create a standard (non-administrator) local account**, e.g. `claude`.
2. **Enable OpenSSH Server**: *Settings → System → Optional features → OpenSSH Server*. Then
   start the `sshd` service and set it to start automatically.
3. **Set up key-based login** from your account to `claude`, because IDEs can't answer
   password prompts. Create a key with `ssh-keygen -t ed25519` if you don't have one, and add
   your public key to `C:\Users\claude\.ssh\authorized_keys`.
4. **Connect once by hand** with `ssh claude@localhost`. This accepts the host key and checks
   that login works.
5. **Install Claude Code** in that ssh session, then run `claude` once to sign in. The native
   installer is recommended.
6. **Give `claude` access** to the project folders you want to work on.
7. **Install the wrapper**: copy `claude-ssh.exe` to a folder of your choice, and create
   `claude-ssh.yaml` next to it (see below).
8. **Test it**: `claude-ssh.exe --version`.

## Configuration

Put `claude-ssh.yaml` next to `claude-ssh.exe`, or set the `CLAUDE_SSH_CONFIG` environment
variable to its path. A minimal config:

```yaml
ssh:
  path: C:\Windows\System32\OpenSSH\ssh.exe
  user: claude
  options:
    BatchMode: "yes"
claude:
  command: C:\Users\claude\.local\bin\claude.exe
```

[claude-ssh.example.yaml](claude-ssh.example.yaml) documents every option, including:

- **SSH settings:** host, port, identity file and other options.
- **Environment variables:** which ones to pass through to Claude Code. The example includes
  the ones VS Code sets.
- **Logging:** an optional log file, useful for troubleshooting.

## Using it from an IDE

**VS Code:** set the Claude Code extension's *Claude Process Wrapper* setting
(`claudeCode.claudeProcessWrapper`) to the full path of `claude-ssh.exe`.

**JetBrains IDEs:** in the IDE settings, under *Claude Code*, set *Claude Command* to the full path
of `claude-ssh.exe`.

## Troubleshooting

Turn on logging in the config (`logging.file`, and `logging.level: debug`) and check the log.

| Problem | Fix |
| --- | --- |
| `Permission denied (publickey)` | Add your public key to the other account's `authorized_keys` (Setup, step 3). |
| `Host key verification failed` | Connect once by hand with `ssh claude@localhost` (step 4). |
| `Claude Code not found at ...` | Check `claude.command` in the config. If it's correct, reinstall Claude Code as the other account. |
| Claude Code says you're not signed in | Sign in as the other account (step 5). |

## Building

Requires [Rust](https://rustup.rs/) (MSVC toolchain) on Windows.

```powershell
cargo build --release   # produces target\release\claude-ssh.exe
cargo test
```

## License

MIT. See [LICENSE.md](LICENSE.md).
