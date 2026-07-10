<#
.SYNOPSIS
    Convenience wrapper for the differential-execution harness (SPEC.md
    §16.1/§16.2, ticket #23): builds tools/differ and sweeps corpus/differ,
    comparing lowered output against source on every Lua runtime found on
    PATH. Pairs whose runtime is missing are SKIPPED with a note (the full
    five-runtime matrix runs in CI — .github/workflows/differential.yml).

.PARAMETER Corpus
    Corpus directory of annotated .lua files (default: corpus/differ).

.PARAMETER Filter
    Only run corpus files whose name contains this substring.

.PARAMETER TimeoutSec
    Per-run wall-clock timeout in seconds (default: 10). Catches lowering
    bugs that turn into infinite loops.

.PARAMETER Detail
    Pass --verbose to the harness: per-pair polyfill/warning notes and full
    stream dumps on mismatch.

.EXAMPLE
    scripts/differ.ps1
    scripts/differ.ps1 -Filter goto -Detail
    scripts/differ.ps1 -Corpus corpus/differ -TimeoutSec 30
#>
[CmdletBinding()]
param(
    [string]$Corpus = "corpus/differ",
    [string]$Filter,
    [int]$TimeoutSec = 10,
    [switch]$Detail
)

$ErrorActionPreference = "Stop"
# The harness legitimately writes mismatch detail to stderr; a nonzero exit
# is reported through $LASTEXITCODE, not as a terminating PowerShell error.
$PSNativeCommandUseErrorActionPreference = $false

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

Write-Host "differ: building harness (release)..."
cargo build --release --manifest-path tools/differ/Cargo.toml
if ($LASTEXITCODE -ne 0) { throw "cargo build differ failed (exit $LASTEXITCODE)" }

$differBin = Join-Path $repoRoot "tools/differ/target/release/differ.exe"
if (-not (Test-Path $differBin)) { $differBin = Join-Path $repoRoot "tools/differ/target/release/differ" }

$differArgs = @("--corpus", $Corpus, "--timeout", $TimeoutSec)
if ($Filter) { $differArgs += @("--filter", $Filter) }
if ($Detail) { $differArgs += "--verbose" }

& $differBin @differArgs
exit $LASTEXITCODE
