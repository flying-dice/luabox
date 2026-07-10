<#
.SYNOPSIS
    SPEC.md §16.1 perf gates (CI-blocking): cold start < 50 ms; `check` on a
    100-kLOC corpus < 1 s warm. Windows counterpart to scripts/perf-gate.sh
    (CI runs the bash version on ubuntu-latest; this is for local dev use).

.DESCRIPTION
    Gates: cold start, `fmt --check` throughput (kept as a wider safety
    net), and the real `check` gate (live since ticket #6).

.PARAMETER Factor
    Multiplier applied to every budget, for slow/loaded machines
    (antivirus scanning new binaries, Windows process-creation overhead,
    an underpowered dev laptop). Defaults to the LUABOX_PERF_FACTOR env
    var, or 1.0. CI is the real enforcement point (Linux, scripts/perf-gate.sh);
    this override exists so local runs on a noisy Windows box aren't
    misleading, not to relax the actual gate.

.EXAMPLE
    scripts/perf-gate.ps1
    scripts/perf-gate.ps1 -Factor 3.0
    $env:LUABOX_PERF_FACTOR = "3.0"; scripts/perf-gate.ps1
#>
[CmdletBinding()]
param(
    [double]$Factor = $(if ($env:LUABOX_PERF_FACTOR) { [double]$env:LUABOX_PERF_FACTOR } else { 1.0 })
)

$ErrorActionPreference = "Stop"
# PowerShell 7.3+ treats a native command's stderr output as a terminating
# error when $ErrorActionPreference is "Stop". `luabox fmt --check` writes
# an expected diagnostic to stderr (files needing reformat) — that's not a
# script failure, so opt out of that behavior for native command calls.
$PSNativeCommandUseErrorActionPreference = $false

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

$ColdStartBudgetBaseMs = 50
$FmtBudgetBaseMs = 2000
$CheckBudgetBaseMs = 1000

$coldStartBudget = $ColdStartBudgetBaseMs * $Factor
$fmtBudget = $FmtBudgetBaseMs * $Factor

Write-Host "perf-gate: LUABOX_PERF_FACTOR=$Factor (cold-start budget $([math]::Round($coldStartBudget)) ms, fmt budget $([math]::Round($fmtBudget)) ms)"

Write-Host "perf-gate: building release binaries..."
cargo build --release -p luabox-cli
if ($LASTEXITCODE -ne 0) { throw "cargo build -p luabox-cli failed (exit $LASTEXITCODE)" }
cargo build --release --manifest-path tools/gen-corpus/Cargo.toml --target-dir target/gen-corpus
if ($LASTEXITCODE -ne 0) { throw "cargo build gen-corpus failed (exit $LASTEXITCODE)" }

# From here on, native calls are redirected with `*>` so their output can
# be suppressed/measured cleanly. PowerShell wraps a redirected native
# command's stderr text into terminating ErrorRecords under EAP "Stop"
# (this is independent of $PSNativeCommandUseErrorActionPreference, which
# only governs exit codes) — `luabox fmt --check` legitimately writes to
# stderr on a nonzero exit, which isn't a script failure. Switch to
# "Continue" and check $LASTEXITCODE explicitly where it matters instead.
$ErrorActionPreference = "Continue"

$luaboxBin = Join-Path $repoRoot "target/release/luabox.exe"
if (-not (Test-Path $luaboxBin)) { $luaboxBin = Join-Path $repoRoot "target/release/luabox" }
$genCorpusBin = Join-Path $repoRoot "target/gen-corpus/release/gen-corpus.exe"
if (-not (Test-Path $genCorpusBin)) { $genCorpusBin = Join-Path $repoRoot "target/gen-corpus/release/gen-corpus" }

$corpusDir = Join-Path ([System.IO.Path]::GetTempPath()) ("luabox-perf-corpus-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $corpusDir -Force | Out-Null

$fail = $false

try {
    Write-Host "perf-gate: generating ~100 kLOC corpus into $corpusDir ..."
    & $genCorpusBin --out (Join-Path $corpusDir "src") --seed 42 --files 50 --lines-per-file 2000
    if ($LASTEXITCODE -ne 0) { throw "gen-corpus failed (exit $LASTEXITCODE)" }

    $manifest = @'
[package]
name = "perf-gate-corpus"
version = "0.0.0"
edition = "5.4"

[build]
target = "5.4"
out = "dist"

[types]
strict = true

[dependencies]
'@
    Set-Content -Path (Join-Path $corpusDir "luabox.toml") -Value $manifest -NoNewline

    # --- Cold start: MIN of N runs ----------------------------------------
    # Min (not mean/median) is the right statistic for a cold-start
    # *ceiling*: it's the best this machine can do free of scheduler/IO
    # noise from other processes (or, on Windows, a Defender scan kicking
    # in on some runs but not others). Percentiles would blend that noise
    # in; min isolates the binary's own startup cost.
    Write-Host ""
    Write-Host "perf-gate: cold start (luabox --version), 10 runs, taking MIN..."
    $times = @()
    for ($i = 1; $i -le 10; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $luaboxBin --version *> $null
        $sw.Stop()
        $ms = $sw.Elapsed.TotalMilliseconds
        Write-Host ("  run {0}: {1:N1} ms" -f $i, $ms)
        $times += $ms
    }
    $minMs = ($times | Measure-Object -Minimum).Minimum

    if ($minMs -lt $coldStartBudget) {
        Write-Host ("PASS cold start: {0:N1} ms < {1:N1} ms" -f $minMs, $coldStartBudget)
    } else {
        Write-Host ("FAIL cold start: {0:N1} ms >= {1:N1} ms" -f $minMs, $coldStartBudget)
        $fail = $true
    }

    # --- fmt --check throughput proxy gate (warm) --------------------------
    # The corpus is synthetic and not guaranteed to already be in
    # canonical form, so `fmt --check` may legitimately exit nonzero here;
    # that's not a gate failure — only the elapsed time is.
    Write-Host ""
    Write-Host "perf-gate: fmt --check throughput proxy on corpus (warm)..."
    Push-Location $corpusDir
    try {
        & $luaboxBin fmt --check *> $null
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $luaboxBin fmt --check *> $null
        $sw.Stop()
    } finally {
        Pop-Location
    }
    $fmtMs = $sw.Elapsed.TotalMilliseconds

    if ($fmtMs -lt $fmtBudget) {
        Write-Host ("PASS fmt --check (warm, proxy for check < 1 s): {0:N1} ms < {1:N1} ms" -f $fmtMs, $fmtBudget)
    } else {
        Write-Host ("FAIL fmt --check (warm, proxy for check < 1 s): {0:N1} ms >= {1:N1} ms" -f $fmtMs, $fmtBudget)
        $fail = $true
    }

    # --- CHECK GATE ----------------------------------------------------------
    # SPEC.md §16.1: `check` on the 100-kLOC corpus < 1 s warm. Live since
    # ticket #6; the fmt --check proxy above stays as a wider safety net.
    $checkBudget = $CheckBudgetBaseMs * $Factor
    Write-Host ""
    Write-Host "perf-gate: check throughput on corpus (warm)..."
    Push-Location $corpusDir
    try {
        & $luaboxBin check *> $null
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $luaboxBin check *> $null
        $sw.Stop()
    } finally {
        Pop-Location
    }
    $checkMs = $sw.Elapsed.TotalMilliseconds
    if ($checkMs -lt $checkBudget) {
        Write-Host ("PASS check (warm): {0:N1} ms < {1:N1} ms" -f $checkMs, $checkBudget)
    } else {
        Write-Host ("FAIL check (warm): {0:N1} ms >= {1:N1} ms" -f $checkMs, $checkBudget)
        $fail = $true
    }

    Write-Host ""
    if (-not $fail) {
        Write-Host "perf-gate: ALL GATES PASSED"
    } else {
        Write-Host "perf-gate: GATES FAILED"
    }
} finally {
    Remove-Item -Recurse -Force $corpusDir -ErrorAction SilentlyContinue
}

if ($fail) { exit 1 } else { exit 0 }
