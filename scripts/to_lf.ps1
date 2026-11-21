# Convert CRLF line endings to LF in the worktree.
#
#   .\scripts\to_lf.ps1              # convert every tracked text file
#   .\scripts\to_lf.ps1 -Check       # report CRLF files, exit 1 if any
#   .\scripts\to_lf.ps1 -DryRun      # print what would change, write nothing
#   .\scripts\to_lf.ps1 -Paths src   # limit the run to files or directories
#
# Thin wrapper: the implementation is scripts/to_lf.py.
param(
    [switch]$Check,
    [switch]$DryRun,
    [switch]$Untracked,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Paths
)

$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$python = Get-Command python3 -ErrorAction SilentlyContinue
if (-not $python) { $python = Get-Command python -ErrorAction SilentlyContinue }
if (-not $python) { $python = Get-Command py -ErrorAction SilentlyContinue }
if (-not $python) {
    Write-Error "to_lf: no python interpreter found; install Python and retry."
    exit 1
}

$arguments = @((Join-Path $here "to_lf.py"))
if ($Check) { $arguments += "--check" }
if ($DryRun) { $arguments += "--dry-run" }
if ($Untracked) { $arguments += "--untracked" }
if ($Paths) { $arguments += $Paths }

& $python.Source @arguments
exit $LASTEXITCODE
