//! `claude-ssh`: a drop-in replacement for the Claude Code CLI (`claude`) that runs
//! Claude Code as a different local Windows user by connecting over SSH.
//!
//! The wrapper runs `ssh <user>@<host> powershell.exe -EncodedCommand <bootstrap>`. The
//! bootstrap script reads the launch spec (arguments, working directory, environment) from
//! the first line of stdin, then starts Claude Code with the rest of the stdio stream.
//! Nothing from this project needs to be installed for, or accessible to, the target user.

mod client;
mod config;
mod log;
mod powershell;
mod protocol;
mod quote;
mod ssh;

use std::ffi::OsString;

use anyhow::{Result, anyhow};

/// Exit code used when the wrapper itself fails, or when a child process
/// terminated without reporting an exit code.
const FAILURE_EXIT_CODE: i32 = 1;

fn main() {
    let code = match std::env::args_os()
        .skip(1)
        .map(into_utf8)
        .collect::<Result<Vec<_>>>()
    {
        Ok(args) => client::run(&args),
        Err(err) => {
            eprintln!("claude-ssh: {err:#}");
            FAILURE_EXIT_CODE
        }
    };
    std::process::exit(code);
}

/// Arguments are sent to the remote side as JSON strings, so they must be valid Unicode.
fn into_utf8(arg: OsString) -> Result<String> {
    arg.into_string()
        .map_err(|arg| anyhow!("argument is not valid Unicode: {}", arg.to_string_lossy()))
}
