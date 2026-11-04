# Remote bootstrap for claude-ssh.
#
# Runs as the target user via `powershell.exe -EncodedCommand`. It gets the launch spec
# (one line of JSON), sets the environment variables, and starts Claude Code in the requested
# working directory (falling back to the user's home), with inherited stdio.
#
# Rules:
# - Never write to stdout: it belongs to Claude Code.
# - Never read stdin past the spec line: the rest is Claude Code's input.
# - Diagnostics go to stderr in the format parsed by `protocol::parse_diagnostic`.
# - Lines that start with '#' (after indentation) are removed before encoding, so keep
#   comments on their own lines. `__SPEC_SOURCE__` is replaced by src/powershell.rs.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

function Write-Diagnostic($level, $message) {
    [Console]::Error.WriteLine("##claude-ssh-remote## $level $message")
}

# Reads a single line from stdin one byte at a time, so nothing after it is consumed.
function Read-SpecLine {
    $stdin = [Console]::OpenStandardInput()
    $buffer = New-Object IO.MemoryStream
    while (($byte = $stdin.ReadByte()) -ne 10) {
        if ($byte -lt 0) { throw 'stdin closed before the launch spec was received' }
        $buffer.WriteByte($byte)
    }
    [Text.Encoding]::UTF8.GetString($buffer.ToArray())
}

# `arguments` is a complete, pre-quoted command line tail built by the wrapper. Passing it
# through ProcessStartInfo avoids PowerShell's own native argument quoting.
function Start-Claude($spec, $directory) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = if ($spec.program) { $spec.program } else { $env:ComSpec }
    $info.Arguments = $spec.arguments
    $info.UseShellExecute = $false
    $info.WorkingDirectory = $directory
    [Diagnostics.Process]::Start($info)
}

try {
    $spec = (__SPEC_SOURCE__) | ConvertFrom-Json
    if ([IO.Path]::IsPathRooted($spec.target) -and -not (Test-Path -LiteralPath $spec.target -PathType Leaf)) {
        throw "Claude Code not found at $($spec.target) (check claude.command, and that $env:USERNAME can read it)"
    }
    foreach ($var in $spec.env.PSObject.Properties) {
        [Environment]::SetEnvironmentVariable($var.Name, [string]$var.Value)
    }
    $process = $null
    if ($spec.cwd) {
        try {
            $process = Start-Claude $spec $spec.cwd
        } catch {
            Write-Diagnostic warn "cannot start in working directory $($spec.cwd) ($($_.Exception.Message)); falling back to $env:USERPROFILE"
        }
    }
    if (-not $process) {
        $process = Start-Claude $spec $env:USERPROFILE
    }
    $process.WaitForExit()
    exit $process.ExitCode
} catch {
    Write-Diagnostic error $_.Exception.Message
    exit 1
}
