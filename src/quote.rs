//! Building the argument string that the remote bootstrap passes to `ProcessStartInfo.Arguments`.
//!
//! - Executables (`claude.exe`): MSVC C runtime rules (`CommandLineToArgvW`).
//! - Batch files (`claude.cmd`/`.bat`): Windows runs these through cmd.exe, which parses the
//!   command line again. This follows the escaping approach of Rust's standard library
//!   (`make_bat_command_line`, the "BatBadBut" fix in Rust 1.77). Arguments containing
//!   `\r`, `\n` or NUL are rejected because they can't be passed safely.

use std::borrow::Cow;

use anyhow::{Result, bail};

/// Returns true if Windows would run `command` through cmd.exe. Windows ignores trailing
/// dots and spaces in file names, so `claude.cmd.` is also a batch file.
pub fn is_batch_file(command: &str) -> bool {
    let trimmed = command.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    trimmed.ends_with(".cmd") || trimmed.ends_with(".bat")
}

/// Joins arguments for an executable using MSVC quoting rules.
pub fn join_windows_args(args: &[&str]) -> String {
    args.iter()
        .map(|arg| quote_windows_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quotes `arg` so that the MSVC runtime parses it back as exactly one argument.
pub fn quote_windows_arg(arg: &str) -> Cow<'_, str> {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return Cow::Borrowed(arg);
    }

    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Backslashes preceding a quote must be doubled, plus one to escape the quote.
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    // Trailing backslashes precede the closing quote, so they must be doubled.
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    Cow::Owned(quoted)
}

/// Builds the arguments for `cmd.exe` to run `script` with `args`:
/// `/e:ON /v:OFF /d /c ""<script>" <escaped args>"`.
///
/// `/e:ON` is needed for the `%` workaround below, `/v:OFF` stops `!` expansion, and `/d`
/// skips AutoRun commands. `/c` strips the outer pair of quotes.
pub fn batch_command_arguments(script: &str, args: &[&str]) -> Result<String> {
    if script.contains('"') || script.ends_with('\\') {
        bail!("batch file path {script:?} must not contain '\"' or end with '\\'");
    }
    let mut cmd = String::from("/e:ON /v:OFF /d /c \"\"");
    cmd.push_str(script);
    cmd.push('"');
    for arg in args {
        if arg.contains(['\r', '\n', '\0']) {
            bail!(
                "argument {arg:?} contains a newline or NUL character, which cannot be passed \
                 safely to a .cmd/.bat file; use claude.exe instead"
            );
        }
        cmd.push(' ');
        append_batch_arg(&mut cmd, arg);
    }
    cmd.push('"');
    Ok(cmd)
}

fn append_batch_arg(cmd: &mut String, arg: &str) {
    // Quote everything except arguments made only of characters known to be safe. A trailing
    // backslash is quoted so that a script using `"%~1"` can't have its closing quote escaped.
    const SAFE_UNQUOTED: &str = r"#$*+-./:?@\_";
    let quote = arg.is_empty()
        || arg.ends_with('\\')
        || arg.chars().any(|c| {
            (c.is_ascii() && !(c.is_ascii_alphanumeric() || SAFE_UNQUOTED.contains(c)))
                || c.is_control()
        });

    if quote {
        cmd.push('"');
    }
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                // Double the preceding backslashes, and escape the quote by doubling it, which
                // keeps cmd.exe's view of what's quoted in step with the MSVC runtime's.
                cmd.extend(std::iter::repeat_n('\\', backslashes));
                cmd.push('"');
            } else if c == '%' {
                // `%cd:~,%` expands to an empty string, which stops cmd from expanding
                // `%VAR%` sequences in the argument.
                cmd.push_str("%%cd:~,");
            }
            backslashes = 0;
        }
        cmd.push(c);
    }
    if quote {
        cmd.extend(std::iter::repeat_n('\\', backslashes));
        cmd.push('"');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_arg_is_unchanged() {
        assert_eq!(quote_windows_arg("--verbose"), "--verbose");
    }

    #[test]
    fn empty_arg_is_quoted() {
        assert_eq!(quote_windows_arg(""), "\"\"");
    }

    #[test]
    fn arg_with_space_is_quoted() {
        assert_eq!(quote_windows_arg("hello world"), "\"hello world\"");
    }

    #[test]
    fn embedded_quote_is_escaped() {
        assert_eq!(quote_windows_arg("a\"b"), r#""a\"b""#);
    }

    #[test]
    fn backslashes_before_embedded_quote_are_doubled() {
        assert_eq!(quote_windows_arg(r#"a\"b"#), r#""a\\\"b""#);
    }

    #[test]
    fn trailing_backslash_is_doubled_when_quoted() {
        assert_eq!(quote_windows_arg(r"C:\my dir\"), r#""C:\my dir\\""#);
    }

    #[test]
    fn inner_backslashes_are_not_doubled() {
        assert_eq!(quote_windows_arg(r"C:\my dir\x"), r#""C:\my dir\x""#);
    }

    #[test]
    fn join_quotes_each_arg() {
        assert_eq!(
            join_windows_args(&["-p", "hello world", r#"{"a":1}"#]),
            r#"-p "hello world" "{\"a\":1}""#
        );
    }

    #[test]
    fn cmd_and_bat_are_batch_files() {
        assert!(is_batch_file(r"C:\npm\claude.CMD") && is_batch_file("x.bat"));
    }

    #[test]
    fn trailing_dots_and_spaces_are_ignored_for_batch_detection() {
        assert!(is_batch_file("claude.cmd. ."));
    }

    #[test]
    fn exe_is_not_a_batch_file() {
        assert!(!is_batch_file(r"C:\bin\claude.exe"));
    }

    #[test]
    fn batch_command_wraps_script_and_args() {
        assert_eq!(
            batch_command_arguments(r"C:\npm dir\claude.cmd", &["--version"]).unwrap(),
            r#"/e:ON /v:OFF /d /c ""C:\npm dir\claude.cmd" --version""#
        );
    }

    #[test]
    fn batch_arg_with_metacharacters_is_quoted() {
        assert_eq!(
            batch_command_arguments("c.cmd", &["a & b | c"]).unwrap(),
            r#"/e:ON /v:OFF /d /c ""c.cmd" "a & b | c"""#
        );
    }

    #[test]
    fn batch_arg_quotes_are_doubled() {
        assert_eq!(
            batch_command_arguments("c.cmd", &[r#"{"x":"y"}"#]).unwrap(),
            r#"/e:ON /v:OFF /d /c ""c.cmd" "{""x"":""y""}"""#
        );
    }

    #[test]
    fn batch_arg_percent_is_neutralized() {
        assert_eq!(
            batch_command_arguments("c.cmd", &["%PATH%"]).unwrap(),
            r#"/e:ON /v:OFF /d /c ""c.cmd" "%%cd:~,%PATH%%cd:~,%"""#
        );
    }

    #[test]
    fn batch_empty_arg_is_quoted() {
        assert_eq!(
            batch_command_arguments("c.cmd", &[""]).unwrap(),
            r#"/e:ON /v:OFF /d /c ""c.cmd" """"#
        );
    }

    #[test]
    fn batch_trailing_backslash_is_quoted_and_doubled() {
        assert_eq!(
            batch_command_arguments("c.cmd", &[r"C:\dir\"]).unwrap(),
            r#"/e:ON /v:OFF /d /c ""c.cmd" "C:\dir\\"""#
        );
    }

    #[test]
    fn batch_arg_with_newline_is_rejected() {
        assert!(batch_command_arguments("c.cmd", &["a\nb"]).is_err());
    }

    #[test]
    fn batch_script_path_with_quote_is_rejected() {
        assert!(batch_command_arguments("c\".cmd", &[]).is_err());
    }
}
