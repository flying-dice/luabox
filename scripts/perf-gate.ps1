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

# perf-gate-lib.ps1 holds every piece of judgement that does not need a real
# binary (New-PerfManifest, Read-PerfBudgets, Test-LuaFileCount,
# ConvertTo-ScaledBudgetMs) — mirrors scripts/perf-gate-lib.sh's split, and
# is what scripts/tests/perf-gate-selftest.ps1 dot-sources to test this
# file's judgement in milliseconds (#58 review round 6, M51).
. (Join-Path $PSScriptRoot "perf-gate-lib.ps1")

# The budget constants below used to be a second, hand-carried copy of
# scripts/perf-gate.sh's own — one rule, two copies, free to drift (#58
# review round 6, M50). They now live once, with their calibration
# rationale, in perf-gate-budgets.env; Read-PerfBudgets parses the same
# KEY=VALUE file scripts/perf-gate.sh `source`s directly. Read every key
# that file defines: a key honoured by one reader and ignored by the other
# is the same drift M50 closed, just quieter — which is exactly what
# happened to CHECK_CEILING_MS/DIAG_CHECK_CEILING_MS between the round 6
# rebase and the local merge-gate that found them unread here.
$PerfBudgets = Read-PerfBudgets (Join-Path $PSScriptRoot "perf-gate-budgets.env")
$ColdStartBudgetBaseMs = $PerfBudgets["COLD_START_BUDGET_BASE_MS"]
$FmtBudgetBaseMs = $PerfBudgets["FMT_BUDGET_BASE_MS"]
$CheckBudgetBaseMs = $PerfBudgets["CHECK_BUDGET_BASE_MS"]
$CheckCeilingMs = $PerfBudgets["CHECK_CEILING_MS"]
$DiagLintBudgetBaseMs = $PerfBudgets["DIAG_LINT_BUDGET_BASE_MS"]
$DiagCheckBudgetBaseMs = $PerfBudgets["DIAG_CHECK_BUDGET_BASE_MS"]
$DiagCheckCeilingMs = $PerfBudgets["DIAG_CHECK_CEILING_MS"]
$DiagLintRenderedBudgetBaseMs = $PerfBudgets["DIAG_LINT_RENDERED_BUDGET_BASE_MS"]
$DiagCheckRenderedBudgetBaseMs = $PerfBudgets["DIAG_CHECK_RENDERED_BUDGET_BASE_MS"]
$DiagCorpusFindings = $PerfBudgets["DIAG_CORPUS_FINDINGS"]
# NOT scaled by -Factor: a slow or loaded machine runs the same allocations,
# it just takes longer over them, so a CPU multiplier has no business
# loosening a memory ceiling. $env:LUABOX_RSS_BUDGET_MIB overrides it, for
# when the budget itself is being renegotiated.
$RssBudgetMib = $(if ($env:LUABOX_RSS_BUDGET_MIB) { [int]$env:LUABOX_RSS_BUDGET_MIB } else { $PerfBudgets["RSS_BUDGET_MIB"] })

$coldStartBudget = ConvertTo-ScaledBudgetMs $ColdStartBudgetBaseMs $Factor
$fmtBudget = ConvertTo-ScaledBudgetMs $FmtBudgetBaseMs $Factor

Write-Host "perf-gate: LUABOX_PERF_FACTOR=$Factor (cold-start budget $([math]::Round($coldStartBudget)) ms, fmt budget $([math]::Round($fmtBudget)) ms)"

# $env:LUABOX_BIN / $env:GEN_CORPUS_BIN: override the two binaries this gate
# measures and skip the `cargo build` entirely when BOTH are set — the
# PowerShell counterpart of scripts/perf-gate.sh's own LUABOX_BIN/
# GEN_CORPUS_BIN seam (#58 review round 6, M28/M51). Unset in every real
# use; this exists for scripts/tests/perf-gate-selftest.ps1 to run this
# file for real against a stub.
$useStubBinaries = $env:LUABOX_BIN -and $env:GEN_CORPUS_BIN
if (-not $useStubBinaries) {
    Write-Host "perf-gate: building release binaries..."
    cargo build --release -p luabox-cli
    if ($LASTEXITCODE -ne 0) { throw "cargo build -p luabox-cli failed (exit $LASTEXITCODE)" }
    cargo build --release --manifest-path tools/gen-corpus/Cargo.toml --target-dir target/gen-corpus
    if ($LASTEXITCODE -ne 0) { throw "cargo build gen-corpus failed (exit $LASTEXITCODE)" }
}

# From here on, native calls are redirected with `*>` so their output can
# be suppressed/measured cleanly. PowerShell wraps a redirected native
# command's stderr text into terminating ErrorRecords under EAP "Stop"
# (this is independent of $PSNativeCommandUseErrorActionPreference, which
# only governs exit codes) — `luabox fmt --check` legitimately writes to
# stderr on a nonzero exit, which isn't a script failure. Switch to
# "Continue" and check $LASTEXITCODE explicitly where it matters instead.
$ErrorActionPreference = "Continue"

if ($useStubBinaries) {
    $luaboxBin = $env:LUABOX_BIN
    $genCorpusBin = $env:GEN_CORPUS_BIN
} else {
    $luaboxBin = Join-Path $repoRoot "target/release/luabox.exe"
    if (-not (Test-Path $luaboxBin)) { $luaboxBin = Join-Path $repoRoot "target/release/luabox" }
    $genCorpusBin = Join-Path $repoRoot "target/gen-corpus/release/gen-corpus.exe"
    if (-not (Test-Path $genCorpusBin)) { $genCorpusBin = Join-Path $repoRoot "target/gen-corpus/release/gen-corpus" }
}

$corpusDir = Join-Path ([System.IO.Path]::GetTempPath()) ("luabox-perf-corpus-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $corpusDir -Force | Out-Null

$fail = $false

try {
    Write-Host "perf-gate: generating ~100 kLOC corpus into $corpusDir ..."
    & $genCorpusBin --out (Join-Path $corpusDir "src") --seed 42 --files 50 --lines-per-file 2000
    if ($LASTEXITCODE -ne 0) { throw "gen-corpus failed (exit $LASTEXITCODE)" }

    New-PerfManifest -Path (Join-Path $corpusDir "luabox.toml") -Name "perf-gate-corpus" -Strict $true

    # #58 review round 6, M30: the retained-TypeEnv corpus below was the
    # ONLY one of three generated corpora wired to a file-count assertion —
    # this one (the CHECK GATE / PEAK-RSS GATE legs' corpus) and the four
    # diagnostics-heavy corpora further down could print PASS against an
    # empty directory with nothing catching it. Fails the whole gate rather
    # than skipping just this leg — matching perf-gate.sh's own `|| fail=1`
    # wiring for the same three corpora, and decisions/07's "the ceiling
    # side of the bargain" no longer resting on an unaudited assumption
    # that generation actually ran.
    if (-not (Test-LuaFileCount (Join-Path $corpusDir "src") 50)) {
        Write-Host "FAIL 100-kLOC corpus generation: see the error above — the CHECK GATE and PEAK-RSS GATE legs below would not be measuring what their PASS/FAIL lines claim"
        $fail = $true
    }

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
    # SPEC.md §16.1 named `check` on the 100-kLOC corpus < 1 s warm as the
    # original acceptance target. CHECK_BUDGET_BASE_MS no longer equals that
    # figure — #58 review round 6, M37 found it FAILING on real hardware at
    # FACTOR=1.0; see perf-gate-budgets.env's own comment for the real
    # numbers that session measured and the headroom the rebased budget
    # carries. Live since GL#6; the fmt --check gate above stays as the wider
    # safety net. -CheckCeilingMs caps the SCALED budget, mirroring
    # perf-gate.sh's third argument at the same leg.
    $checkBudget = ConvertTo-ScaledBudgetMs $CheckBudgetBaseMs $Factor -CeilingMs $CheckCeilingMs
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
    $retainedEnvCheckBudget = ConvertTo-ScaledBudgetMs $RetainedEnvCheckBudgetBaseMs $Factor

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
    # assume is not evidence of anything. Routed through Test-LuaFileCount
    # (perf-gate-lib.ps1) rather than a bespoke Get-ChildItem check here, so
    # this and the two wirings above/below are one function, not three
    # hand-rolled counts free to drift (#58 review round 6, M30/M50).
    if (-not (Test-LuaFileCount $retainedEnvSrc $RetainedEnvCorpusFiles)) {
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
    $diagLintBudget = ConvertTo-ScaledBudgetMs $DiagLintBudgetBaseMs $Factor
    $diagCheckBudget = ConvertTo-ScaledBudgetMs $DiagCheckBudgetBaseMs $Factor -CeilingMs $DiagCheckCeilingMs
    $diagLintRenderedBudget = ConvertTo-ScaledBudgetMs $DiagLintRenderedBudgetBaseMs $Factor
    $diagCheckRenderedBudget = ConvertTo-ScaledBudgetMs $DiagCheckRenderedBudgetBaseMs $Factor

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

    # #58 review round 6, M30: each diagnostics-heavy corpus is one file: a
    # truncated Set-Content or a wrong path would leave one of the four legs
    # below timing an empty directory instead of $DiagCorpusFindings
    # findings, exactly the "PASS on nothing" shape the CHECK GATE and
    # RETAINED-TYPEENV REGRESSION GATE legs above are now wired against too.
    foreach ($project in @("lint", "check", "lint-rendered", "check-rendered")) {
        if (-not (Test-LuaFileCount (Join-Path $diagRoot "$project/src") 1)) {
            Write-Host "FAIL diagnostics-heavy corpus generation ($project): see the error above"
            $fail = $true
        }
    }

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
