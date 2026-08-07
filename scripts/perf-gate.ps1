<#
.SYNOPSIS
    SPEC.md §16.1 perf gates (CI-blocking): cold start < 50 ms; `check` on a
    100-kLOC corpus < 1 s warm. Windows counterpart to scripts/perf-gate.sh
    (CI runs the bash version on ubuntu-latest; this is for local dev use).

.DESCRIPTION
    Gates: cold start, `fmt --check` throughput (kept as a wider safety
    net), the real `check` gate (live since GL#6), a diagnostics-heavy
    `lint` + `check` gate in two variants — findings suppressed, and
    findings reported — a peak-RSS gate on the same 100-kLOC corpus
    (decisions/07 accepted a ~1.9x RSS trade; the ceiling side of that
    bargain is now enforced), and a second, independent peak-RSS + wall-time
    gate (the RETAINED-TYPEENV REGRESSION GATE) on a 500-file corpus shaped
    to catch a different regression the first RSS gate's corpus is too
    small to widen: round 4 review R14's reverted retained-`Vec<TypeEnv>`
    change, at ~25x peak RSS on this one at N=500 against ~5x on the
    100-kLOC corpus.

    Why those last gates exist: the ~100-kLOC corpus the first three legs
    use is *clean* (`check: 0 errors, 0 warnings`), so none of them ever
    exercises per-diagnostic work. That blind spot hid an
    O(diagnostics x file size) line lookup: 32 k findings in one file took
    ~32 s to lint, while the same file with a single finding took 0.35 s.
    The suppressed pair times the per-finding bookkeeping with the
    renderer out of the way; the rendered pair times the whole path a user
    pays for, renderer included — which is where a second, larger
    O(diagnostics x file size) cost lived (one byte-0 line scan *and* one
    whole-file clone per label) until the renderers were given a line
    table of their own.

.PARAMETER Factor
    Multiplier applied to every budget, for slow/loaded machines
    (antivirus scanning new binaries, Windows process-creation overhead,
    an underpowered dev laptop). Defaults to the LUABOX_PERF_FACTOR env
    var, or 1.0. CI is the real enforcement point (Linux, scripts/perf-gate.sh);
    this override exists so local runs on a noisy Windows box aren't
    misleading, not to relax the actual gate. It does NOT scale the
    peak-RSS budget — see $env:LUABOX_RSS_BUDGET_MIB for that.

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

# New-PerfManifest -Path -Name -Strict — every generated corpus below needs
# a luabox.toml differing only in `name` and whether `[types] strict = true`
# is present; two near-identical here-strings (the bash gate's N42 finding
# applies here too) is exactly the kind of duplication that lets one copy
# drift from the other silently. One writer, one place to read the shape
# from — mirrors scripts/perf-gate-lib.sh's write_perf_manifest.
function New-PerfManifest {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Name,
        [bool]$Strict = $false
    )
    $body = @"
[package]
name = "$Name"
version = "0.0.0"
edition = "5.4"

[build]
target = "5.4"
out = "dist"

"@
    if ($Strict) {
        $body += @"
[types]
strict = true

"@
    }
    $body += "[dependencies]"
    Set-Content -Path $Path -Value $body -NoNewline
}

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
# The same two corpora with nothing suppressed, so every finding is also
# *rendered*. Calibrated the same way (Linux dev baseline, 20 k findings):
#   lint   162 ms fixed /  9 295 ms with the quadratic renderer restored
#   check  788 ms fixed / 15 462 ms ditto
# Budgets ~3x above the fixed numbers and >=6x below the broken ones.
# Rendered `lint` is *faster* than the suppressed leg above because
# resolving 20 k `---@luabox-ignore` directives costs more than printing
# 20 k frames — the suppressed leg is not a subset of this one, which is
# why both stay.
$DiagLintRenderedBudgetBaseMs = 600
$DiagCheckRenderedBudgetBaseMs = 2400
$DiagCorpusFindings = 20000
# Peak-RSS ceiling for `check` on the 100-kLOC corpus, in MiB. decisions/07
# accepted 64 -> 123 MiB; measured 123 MiB on the Linux dev baseline, so the
# budget sits at ~2.4x that. Wide on purpose: this catches a *regime* change
# (a whole-project structure retained, a per-file clone that used to be a
# borrow), not a 10% drift — and Windows working-set accounting differs from
# Linux RSS enough that a tight number would only produce false failures.
$RssBudgetMib = $(if ($env:LUABOX_RSS_BUDGET_MIB) { [int]$env:LUABOX_RSS_BUDGET_MIB } else { 300 })

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

    New-PerfManifest -Path (Join-Path $corpusDir "luabox.toml") -Name "perf-gate-corpus" -Strict $true

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

    # --- PEAK-RSS GATE -------------------------------------------------------
    # The other half of decisions/07's bargain: it *accepted* a ~1.9x peak-RSS
    # regression (64 -> 123 MiB on this corpus) in exchange for one read/parse
    # per file, and nothing enforced the ceiling side of that trade.
    #
    # Windows has no rusage, so this is not scripts/peak-rss.py's mechanism:
    # `Process.PeakWorkingSet64` is the equivalent the OS does expose, read off
    # the child after it exits (the property stays readable on an exited
    # Process object, which is why the process is started by hand rather than
    # with `&`). Same corpus, same command, same budget as the bash gate.
    #
    # NOT scaled by -Factor: a slow machine runs the same allocations, it just
    # takes longer over them, so a CPU multiplier has no business loosening a
    # memory ceiling. $env:LUABOX_RSS_BUDGET_MIB overrides it, for when the
    # budget itself is being renegotiated.
    Write-Host ""
    Write-Host "perf-gate: peak RSS of check on corpus (warm)..."
    $info = [System.Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $luaboxBin
    $info.Arguments = "check"
    $info.WorkingDirectory = $corpusDir
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $proc = [System.Diagnostics.Process]::Start($info)
    # Drain both pipes before waiting, or a report larger than the pipe buffer
    # deadlocks the child — the same hazard tests/broken_pipe.rs works around.
    $proc.StandardOutput.ReadToEnd() | Out-Null
    $proc.StandardError.ReadToEnd() | Out-Null
    $proc.WaitForExit()
    $rssMib = [int]($proc.PeakWorkingSet64 / 1MB)
    $proc.Dispose()
    if ($rssMib -lt $RssBudgetMib) {
        Write-Host ("PASS check peak RSS: {0} MiB < {1} MiB" -f $rssMib, $RssBudgetMib)
    } else {
        Write-Host ("FAIL check peak RSS: {0} MiB >= {1} MiB" -f $rssMib, $RssBudgetMib)
        Write-Host "     decisions/07 accepted 123 MiB on this corpus"
        $fail = $true
    }

    # --- RETAINED-TYPEENV REGRESSION GATE -------------------------------------
    # Windows counterpart of scripts/perf-gate.sh's leg of the same name
    # (round 4 review R14, reverted by round 4 review finding 6 — see
    # check_cmd.rs's run_passes/check_one doc comments and
    # luabox-types/src/lib.rs's build_file_env doc comment). The bash gate's
    # comment carries the full history and the measured table (Linux, release
    # build, three runs each):
    #
    #   N      transient (correct)   retained (R14, reintroduced)   ratio
    #   100     11 MiB                 51 MiB                        4.6x
    #   200     15 MiB                145 MiB                        9.3x
    #   300     20 MiB                290 MiB                       14.5x
    #   500     29 MiB                734 MiB                       25.3x
    #
    # Same corpus shape, same N=500, same 100 MiB budget as the bash gate —
    # just over 3x the correct implementation's ~29 MiB, while the retained
    # implementation misses it by ~7x. $env:LUABOX_RETAINED_ENV_RSS_BUDGET_MIB
    # overrides the MiB ceiling; NOT scaled by -Factor, same reason as
    # $RssBudgetMib above. The wall-time budget IS scaled by -Factor, like
    # every timed leg — this is the Windows half of the bash gate's N37 fix:
    # previously this leg measured peak RSS only, so `check` could regress
    # arbitrarily here and stay green.
    $RetainedEnvCorpusFiles = 500
    $RetainedEnvRssBudgetMib = $(if ($env:LUABOX_RETAINED_ENV_RSS_BUDGET_MIB) { [int]$env:LUABOX_RETAINED_ENV_RSS_BUDGET_MIB } else { 100 })
    $RetainedEnvCheckBudgetBaseMs = 4000
    $retainedEnvCheckBudget = $RetainedEnvCheckBudgetBaseMs * $Factor

    Write-Host ""
    Write-Host "perf-gate: generating $RetainedEnvCorpusFiles-file retained-TypeEnv regression corpus..."
    $retainedEnvRoot = Join-Path $corpusDir "retained-env"
    $retainedEnvSrc = Join-Path $retainedEnvRoot "src"
    New-Item -ItemType Directory -Path $retainedEnvSrc -Force | Out-Null
    New-PerfManifest -Path (Join-Path $retainedEnvRoot "luabox.toml") -Name "perf-gate-retained-env" -Strict $true

    for ($i = 0; $i -lt $RetainedEnvCorpusFiles; $i++) {
        $modLines = [System.Collections.Generic.List[string]]::new()
        if ($i -gt 0) {
            $modLines.Add("local prev = require(`"mod_$($i - 1)`")")
            $modLines.Add("")
        }
        $modLines.Add("---@class Widget${i}A")
        $modLines.Add("---@field n number")
        $modLines.Add("local A = { n = $i }")
        $modLines.Add("")
        $modLines.Add("---@class Widget${i}B")
        $modLines.Add("---@field n number")
        $modLines.Add("local B = { n = $i }")
        $modLines.Add("")
        if ($i -gt 0) {
            $modLines.Add("return { a = A, b = B, prev = prev }")
        } else {
            $modLines.Add("return { a = A, b = B }")
        }
        Set-Content -Path (Join-Path $retainedEnvSrc "mod_$i.lua") -Value $modLines
    }

    # Windows counterpart of the bash gate's N38 fix: a PASS printed against
    # a corpus that was not actually generated the way the budgets below
    # assume is not evidence of anything.
    $actualLuaFiles = (Get-ChildItem -Path $retainedEnvSrc -Filter "*.lua" -File).Count
    if ($actualLuaFiles -ne $RetainedEnvCorpusFiles) {
        Write-Host ("FAIL retained-TypeEnv regression: corpus has {0} .lua file(s), expected {1} — the generation step did not run as this leg's budget assumes" -f $actualLuaFiles, $RetainedEnvCorpusFiles)
        $fail = $true
    } else {
        Write-Host ""
        Write-Host "perf-gate: peak RSS + wall time of check on the retained-TypeEnv regression corpus (warm)..."
        # RAYON_NUM_THREADS=4 pinned around both the warm-up and the measured
        # process — Windows counterpart of the bash gate's N39 fix. Peak RSS
        # on this corpus scales with rayon parallelism x ambient size
        # (luabox-types/src/lib.rs), so an unpinned thread count would make
        # the 100 MiB ceiling mean something different on every runner width;
        # pinning keeps the ceiling itself honest instead of loosening it.
        $prevRayonThreads = $env:RAYON_NUM_THREADS
        $env:RAYON_NUM_THREADS = "4"
        try {
            Push-Location $retainedEnvRoot
            try {
                & $luaboxBin check *> $null
            } finally {
                Pop-Location
            }

            $retainedInfo = [System.Diagnostics.ProcessStartInfo]::new()
            $retainedInfo.FileName = $luaboxBin
            $retainedInfo.Arguments = "check"
            $retainedInfo.WorkingDirectory = $retainedEnvRoot
            $retainedInfo.UseShellExecute = $false
            $retainedInfo.RedirectStandardOutput = $true
            $retainedInfo.RedirectStandardError = $true
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            $retainedProc = [System.Diagnostics.Process]::Start($retainedInfo)
            $retainedProc.StandardOutput.ReadToEnd() | Out-Null
            $retainedProc.StandardError.ReadToEnd() | Out-Null
            $retainedProc.WaitForExit()
            $sw.Stop()
            $retainedEnvMs = $sw.Elapsed.TotalMilliseconds
            $retainedEnvRssMib = [int]($retainedProc.PeakWorkingSet64 / 1MB)
            $retainedProc.Dispose()
        } finally {
            if ($prevRayonThreads) { $env:RAYON_NUM_THREADS = $prevRayonThreads } else { Remove-Item Env:\RAYON_NUM_THREADS -ErrorAction SilentlyContinue }
        }

        if ($retainedEnvRssMib -lt $RetainedEnvRssBudgetMib) {
            Write-Host ("PASS retained-TypeEnv regression, peak RSS: {0} MiB < {1} MiB" -f $retainedEnvRssMib, $RetainedEnvRssBudgetMib)
        } else {
            Write-Host ("FAIL retained-TypeEnv regression, peak RSS: {0} MiB >= {1} MiB" -f $retainedEnvRssMib, $RetainedEnvRssBudgetMib)
            Write-Host "     round 4 review R14 (reverted) measured ~734 MiB on this corpus (Linux); see check_cmd.rs's run_passes doc comment"
            $fail = $true
        }
        if ($retainedEnvMs -lt $retainedEnvCheckBudget) {
            Write-Host ("PASS retained-TypeEnv regression, wall time: {0:N1} ms < {1:N1} ms" -f $retainedEnvMs, $retainedEnvCheckBudget)
        } else {
            Write-Host ("FAIL retained-TypeEnv regression, wall time: {0:N1} ms >= {1:N1} ms" -f $retainedEnvMs, $retainedEnvCheckBudget)
            $fail = $true
        }
    }

    # --- DIAGNOSTICS-HEAVY GATE ---------------------------------------------
    # The corpus above is clean, so nothing so far times per-diagnostic
    # work. These four legs do: one file, $DiagCorpusFindings findings,
    # run twice over — once with every finding *suppressed*, once with
    # every finding *reported*.
    #
    # The suppressed pair keeps the renderer out of the measurement, so
    # what is left is exactly the per-finding bookkeeping; both commands
    # still compute every finding and resolve every suppression directive.
    # The rendered pair adds the renderer back and measures what a user
    # with a genuinely broken file pays: one source snippet, line and
    # column per label. Neither subsumes the other — a regression in
    # either half moves only its own pair — and stdout is discarded in
    # both, so the gate times the toolchain, not the terminal.
    $diagLintBudget = $DiagLintBudgetBaseMs * $Factor
    $diagCheckBudget = $DiagCheckBudgetBaseMs * $Factor
    $diagLintRenderedBudget = $DiagLintRenderedBudgetBaseMs * $Factor
    $diagCheckRenderedBudget = $DiagCheckRenderedBudgetBaseMs * $Factor

    $diagRoot = Join-Path $corpusDir "diagnostics-heavy"
    foreach ($project in @("lint", "check", "lint-rendered", "check-rendered")) {
        New-Item -ItemType Directory -Path (Join-Path $diagRoot "$project/src") -Force | Out-Null
        New-PerfManifest -Path (Join-Path $diagRoot "$project/luabox.toml") -Name "perf-gate-diagnostics" -Strict $false
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

    # The same two files with the suppressions removed, so every finding is
    # reported and rendered.
    $lintRenderedLines = [System.Collections.Generic.List[string]]::new()
    $lintRenderedLines.Add("local function main()")
    for ($i = 0; $i -lt $DiagCorpusFindings; $i++) {
        $lintRenderedLines.Add("    local unused_$i = $i")
    }
    $lintRenderedLines.Add("end")
    $lintRenderedLines.Add("return main")
    Set-Content -Path (Join-Path $diagRoot "lint-rendered/src/main.lua") -Value $lintRenderedLines

    $checkRenderedLines = [System.Collections.Generic.List[string]]::new()
    $checkRenderedLines.Add("---@class Point")
    $checkRenderedLines.Add("---@field x number")
    $checkRenderedLines.Add("local Point = { x = 1 }")
    $checkRenderedLines.Add("")
    $checkRenderedLines.Add("---@type Point")
    $checkRenderedLines.Add("local p = Point")
    $checkRenderedLines.Add("")
    for ($i = 0; $i -lt $DiagCorpusFindings; $i++) {
        $checkRenderedLines.Add("local v$i = p.nope$i")
        $checkRenderedLines.Add("print(v$i)")
    }
    $checkRenderedLines.Add("return p")
    Set-Content -Path (Join-Path $diagRoot "check-rendered/src/main.lua") -Value $checkRenderedLines

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
    Write-Host "perf-gate: lint on a diagnostics-heavy file, findings rendered (warm)..."
    Push-Location (Join-Path $diagRoot "lint-rendered")
    try {
        & $luaboxBin lint *> $null
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $luaboxBin lint *> $null
        $sw.Stop()
    } finally {
        Pop-Location
    }
    $diagLintRenderedMs = $sw.Elapsed.TotalMilliseconds
    if ($diagLintRenderedMs -lt $diagLintRenderedBudget) {
        Write-Host ("PASS lint (diagnostics-heavy, rendered): {0:N1} ms < {1:N1} ms" -f $diagLintRenderedMs, $diagLintRenderedBudget)
    } else {
        Write-Host ("FAIL lint (diagnostics-heavy, rendered): {0:N1} ms >= {1:N1} ms" -f $diagLintRenderedMs, $diagLintRenderedBudget)
        $fail = $true
    }

    Write-Host ""
    Write-Host "perf-gate: check on a diagnostics-heavy file, findings rendered (warm)..."
    Push-Location (Join-Path $diagRoot "check-rendered")
    try {
        & $luaboxBin check *> $null
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $luaboxBin check *> $null
        $sw.Stop()
    } finally {
        Pop-Location
    }
    $diagCheckRenderedMs = $sw.Elapsed.TotalMilliseconds
    if ($diagCheckRenderedMs -lt $diagCheckRenderedBudget) {
        Write-Host ("PASS check (diagnostics-heavy, rendered): {0:N1} ms < {1:N1} ms" -f $diagCheckRenderedMs, $diagCheckRenderedBudget)
    } else {
        Write-Host ("FAIL check (diagnostics-heavy, rendered): {0:N1} ms >= {1:N1} ms" -f $diagCheckRenderedMs, $diagCheckRenderedBudget)
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
