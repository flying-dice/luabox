<#
.SYNOPSIS
    SPEC.md §16.1 perf gates (CI-blocking): cold start < 50 ms; `check` on a
    100-kLOC corpus < 1 s warm. Windows counterpart to scripts/perf-gate.sh
    (CI runs the bash version on ubuntu-latest; this is for local dev use).

.DESCRIPTION
    Gates: cold start, `fmt --check` throughput (kept as a wider safety
    net), the real `check` gate (live since GL#6), and a diagnostics-heavy
    `lint` + `check` gate.

    Why that fourth gate exists: the ~100-kLOC corpus the first three legs
    use is *clean* (`check: 0 errors, 0 warnings`), so none of them ever
    exercises per-diagnostic work. That blind spot hid an
    O(diagnostics x file size) line lookup: 32 k findings in one file took
    ~32 s to lint, while the same file with a single finding took 0.35 s.

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
# Diagnostics-heavy legs, on one file holding $DiagCorpusFindings findings.
# Calibrated on the Linux dev baseline (see CHANGELOG "Fixed", wave 8):
#   lint   357 ms fixed / 14 163 ms with the quadratic lookup restored
#   check  663 ms fixed /  4 183 ms ditto
# The budgets sit ~3x above the fixed numbers (headroom for a noisy box, on
# top of -Factor) and ~3-12x below the broken ones.
$DiagLintBudgetBaseMs = 1200
$DiagCheckBudgetBaseMs = 1500
$DiagCorpusFindings = 20000

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

    # --- fmt --check throughput gate (warm) --------------------------------
    # The corpus is synthetic and not guaranteed to already be in
    # canonical form, so `fmt --check` may legitimately exit nonzero here;
    # that's not a gate failure — only the elapsed time is.
    Write-Host ""
    Write-Host "perf-gate: fmt --check throughput on corpus (warm)..."
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
        Write-Host ("PASS fmt --check (warm): {0:N1} ms < {1:N1} ms" -f $fmtMs, $fmtBudget)
    } else {
        Write-Host ("FAIL fmt --check (warm): {0:N1} ms >= {1:N1} ms" -f $fmtMs, $fmtBudget)
        $fail = $true
    }

    # --- CHECK GATE ----------------------------------------------------------
    # SPEC.md §16.1: `check` on the 100-kLOC corpus < 1 s warm. Live since
    # GL#6; the fmt --check gate above stays as the wider safety net.
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

    # --- DIAGNOSTICS-HEAVY GATE ---------------------------------------------
    # The corpus above is clean, so nothing so far times per-diagnostic
    # work. These two legs do: one file, $DiagCorpusFindings findings,
    # every one of them *suppressed*. Suppressing them is deliberate — it
    # keeps the renderer (which formats one snippet per reported
    # diagnostic, and is linear in the file for each) out of the
    # measurement, so what is left is exactly the per-finding bookkeeping
    # this gate is about. Both commands still compute every finding and
    # resolve every suppression directive.
    $diagLintBudget = $DiagLintBudgetBaseMs * $Factor
    $diagCheckBudget = $DiagCheckBudgetBaseMs * $Factor

    $diagManifest = @'
[package]
name = "perf-gate-diagnostics"
version = "0.0.0"
edition = "5.4"

[build]
target = "5.4"
out = "dist"

[dependencies]
'@
    $diagRoot = Join-Path $corpusDir "diagnostics-heavy"
    foreach ($project in @("lint", "check")) {
        New-Item -ItemType Directory -Path (Join-Path $diagRoot "$project/src") -Force | Out-Null
        Set-Content -Path (Join-Path $diagRoot "$project/luabox.toml") -Value $diagManifest -NoNewline
    }

    Write-Host ""
    Write-Host "perf-gate: generating diagnostics-heavy corpus ($DiagCorpusFindings findings each) ..."

    # `unused-local` (LB0501) x N, each silenced by its own
    # `---@luabox-ignore`.
    $lintLines = [System.Collections.Generic.List[string]]::new()
    $lintLines.Add("local function main()")
    for ($i = 0; $i -lt $DiagCorpusFindings; $i++) {
        $lintLines.Add("    ---@luabox-ignore unused-local perf-gate corpus")
        $lintLines.Add("    local unused_$i = $i")
    }
    $lintLines.Add("end")
    $lintLines.Add("return main")
    Set-Content -Path (Join-Path $diagRoot "lint/src/main.lua") -Value $lintLines

    # `undefined-field` (LB0306) x N, silenced file-wide by luals' own
    # `---@diagnostic disable`, which is the checker's line-keyed path.
    $checkLines = [System.Collections.Generic.List[string]]::new()
    $checkLines.Add("---@diagnostic disable: undefined-field")
    $checkLines.Add("---@class Point")
    $checkLines.Add("---@field x number")
    $checkLines.Add("local Point = { x = 1 }")
    $checkLines.Add("")
    $checkLines.Add("---@type Point")
    $checkLines.Add("local p = Point")
    $checkLines.Add("")
    for ($i = 0; $i -lt $DiagCorpusFindings; $i++) {
        $checkLines.Add("local v$i = p.nope$i")
        $checkLines.Add("print(v$i)")
    }
    $checkLines.Add("return p")
    Set-Content -Path (Join-Path $diagRoot "check/src/main.lua") -Value $checkLines

    Write-Host ""
    Write-Host "perf-gate: lint on a diagnostics-heavy file (warm)..."
    Push-Location (Join-Path $diagRoot "lint")
    try {
        & $luaboxBin lint *> $null
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $luaboxBin lint *> $null
        $sw.Stop()
    } finally {
        Pop-Location
    }
    $diagLintMs = $sw.Elapsed.TotalMilliseconds
    if ($diagLintMs -lt $diagLintBudget) {
        Write-Host ("PASS lint (diagnostics-heavy): {0:N1} ms < {1:N1} ms" -f $diagLintMs, $diagLintBudget)
    } else {
        Write-Host ("FAIL lint (diagnostics-heavy): {0:N1} ms >= {1:N1} ms" -f $diagLintMs, $diagLintBudget)
        $fail = $true
    }

    Write-Host ""
    Write-Host "perf-gate: check on a diagnostics-heavy file (warm)..."
    Push-Location (Join-Path $diagRoot "check")
    try {
        & $luaboxBin check *> $null
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $luaboxBin check *> $null
        $sw.Stop()
    } finally {
        Pop-Location
    }
    $diagCheckMs = $sw.Elapsed.TotalMilliseconds
    if ($diagCheckMs -lt $diagCheckBudget) {
        Write-Host ("PASS check (diagnostics-heavy): {0:N1} ms < {1:N1} ms" -f $diagCheckMs, $diagCheckBudget)
    } else {
        Write-Host ("FAIL check (diagnostics-heavy): {0:N1} ms >= {1:N1} ms" -f $diagCheckMs, $diagCheckBudget)
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
