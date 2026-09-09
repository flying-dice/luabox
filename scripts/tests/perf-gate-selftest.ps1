<#
.SYNOPSIS
    Self-test for scripts/perf-gate.ps1 and scripts/perf-gate-lib.ps1 — the
    Windows counterpart of scripts/tests/perf-gate-selftest.sh.

.DESCRIPTION
    #58 review round 6, M51: perf-gate.ps1 had NO self-test at all, and
    New-PerfManifest was a hand-reimplementation of perf-gate-lib.sh's
    write_perf_manifest with nothing proving the two stay equivalent.

    RUN STATUS. Earlier revisions of this header said this file was
    unverified because no `pwsh` existed on the Linux box it was written on.
    That is no longer true: #58 review round 8 ran this suite under
    PowerShell 7.4.6 on Linux, in full, and every case here — including the
    round-8 additions — has actually executed. What a Linux pwsh run does
    NOT cover is the Windows-only half of perf-gate.ps1: Process.Start's
    FileName semantics, PeakWorkingSet64 and the working-set accounting the
    three RSS-measuring legs read. ci.yml's `perf-gate-selftest-ps1` job
    (windows-latest, `shell: pwsh`) is the platform leg for that, and stays
    the gate for it. If that job is red and this file has not changed, the
    failure is a Windows-specific behaviour this suite cannot see from
    Linux — fix the .ps1 side to match the bash semantics it mirrors, not
    the test.

    Two parts, mirroring perf-gate-selftest.sh:

    Part 1 exercises perf-gate-lib.ps1's pure functions directly (no real
    binary, no corpus) — Read-PerfBudgets, New-PerfManifest,
    ConvertTo-ScaledBudgetMs, Test-LuaFileCount.

    Part 2 runs perf-gate.ps1 ITSELF against a stub `luabox`/`gen-corpus`
    (here, plain .ps1 scripts — PowerShell's `&` call operator runs a .ps1
    file directly, so no compiled shim is needed) via the $env:LUABOX_BIN /
    $env:GEN_CORPUS_BIN seam perf-gate.ps1 now carries (mirroring
    perf-gate.sh's own LUABOX_BIN/GEN_CORPUS_BIN). It covers the SEVEN of
    perf-gate.ps1's ten threshold comparisons that are measured through the
    `& $luaboxBin ...` + Stopwatch pattern: cold start, fmt --check (warm),
    check (warm), and the four diagnostics-heavy timing legs — plus, since
    round 8, the two judgements that are not threshold comparisons at all:
    the abort-mid-legs catch (F11) and a ceiling actually binding at a high
    -Factor (F15b). It does NOT
    cover the check-peak-RSS leg or the retained-TypeEnv leg (RSS AND wall
    time both) — those three go through raw
    [System.Diagnostics.Process]::Start() with $info.FileName set directly
    to the binary path, which Windows can only launch as a true executable,
    not a .ps1 script; stubbing that would need a compiled shim (a tiny
    .exe) this round did not build. That gap is real and disclosed, not
    silently absent — #58 review round 6, M28's rework of the bash
    self-test could stub every leg because bash always launches
    `"$luabox_bin" ...` through a shell, which does not care whether the
    target is a script or a real binary.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
# Two levels up from scripts/tests — mirrors the bash selftest's
# `repo="$here/../.."`; one level (an earlier revision's mistake) lands on
# scripts/ and doubles the path of everything joined below it.
$repo = Split-Path -Parent (Split-Path -Parent $here)
$lib = Join-Path $here "../perf-gate-lib.ps1"
$gate = Join-Path $here "../perf-gate.ps1"

. $lib

$script:pass = 0
$script:fail = 0

function Test-Equal {
    param([string]$Name, $Got, $Want)
    if ("$Got" -eq "$Want") {
        Write-Host "PASS  $Name"
        $script:pass++
    } else {
        Write-Host "FAIL  ${Name}: got [$Got], want [$Want]"
        $script:fail++
    }
}

function Test-True {
    param([string]$Name, [scriptblock]$Block)
    $result = & $Block
    if ($result) {
        Write-Host "PASS  $Name"
        $script:pass++
    } else {
        Write-Host "FAIL  ${Name}: expected truthy, got [$result]"
        $script:fail++
    }
}

function Test-False {
    param([string]$Name, [scriptblock]$Block)
    $result = & $Block
    if (-not $result) {
        Write-Host "PASS  $Name"
        $script:pass++
    } else {
        Write-Host "FAIL  ${Name}: expected falsy, got [$result]"
        $script:fail++
    }
}

# ============================================================================
# Part 1 — perf-gate-lib.ps1's pure functions.
# ============================================================================

# --- ConvertTo-ScaledBudgetMs ------------------------------------------------
Test-Equal "ConvertTo-ScaledBudgetMs default factor" (ConvertTo-ScaledBudgetMs 1000 1.0) 1000
Test-Equal "ConvertTo-ScaledBudgetMs CI factor (4.0)" (ConvertTo-ScaledBudgetMs 1000 4.0) 4000
# [int] truncates toward zero for a positive double in PowerShell — the same
# "truncates, does not round" contract scale_budget_ms's `%d` printf format
# gives on the bash side (perf-gate-selftest.sh pins the identical property
# there). A case whose fractional part clears .5 by a comfortable margin, so
# neither IEEE-754 imprecision nor a Round()-based mutation can pass by
# accident: 100 * 1.678 = 167.8 (truncate -> 167, round -> 168).
Test-Equal "ConvertTo-ScaledBudgetMs truncates, does not round" (ConvertTo-ScaledBudgetMs 100 1.678) 167

# The optional ceiling, mirroring perf-gate-selftest.sh's four scale_budget_ms
# ceiling cases one for one (same base/factor/ceiling triples, same expected
# values). -Factor multiplies a base that already carries ~3x headroom, so
# CI's effective ceiling for the check leg had reached ~12x its measured
# baseline before the cap existed. The cases without a ceiling above are the
# control: a leg passing none must be unchanged.
Test-Equal "ConvertTo-ScaledBudgetMs caps a scaled budget at its ceiling" `
    (ConvertTo-ScaledBudgetMs 7500 4.0 -CeilingMs 8000) 8000
Test-Equal "ConvertTo-ScaledBudgetMs leaves a scaled budget under its ceiling alone" `
    (ConvertTo-ScaledBudgetMs 7500 1.0 -CeilingMs 8000) 7500
Test-Equal "ConvertTo-ScaledBudgetMs ceiling equal to the scaled value does not cap" `
    (ConvertTo-ScaledBudgetMs 2000 2.0 -CeilingMs 4000) 4000
# The bash side spells "no ceiling" as an empty third argument; PowerShell's
# is the default 0, asserted both ways so a future change to the sentinel
# cannot quietly start capping every uncapped leg to zero.
Test-Equal "ConvertTo-ScaledBudgetMs with no ceiling behaves as if uncapped" `
    (ConvertTo-ScaledBudgetMs 1000 4.0) 4000
Test-Equal "ConvertTo-ScaledBudgetMs with a zero ceiling behaves as if uncapped" `
    (ConvertTo-ScaledBudgetMs 1000 4.0 -CeilingMs 0) 4000

# --- Test-LuaFileCount -------------------------------------------------------
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("perf-gate-selftest-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $work -Force | Out-Null
try {
    $full = Join-Path $work "full"
    New-Item -ItemType Directory -Path $full -Force | Out-Null
    1..3 | ForEach-Object { New-Item -ItemType File -Path (Join-Path $full "mod_$_.lua") -Force | Out-Null }
    Test-True "Test-LuaFileCount passes on a matching corpus" { Test-LuaFileCount $full 3 }

    $empty = Join-Path $work "empty"
    New-Item -ItemType Directory -Path $empty -Force | Out-Null
    Test-False "Test-LuaFileCount fails on an empty corpus (N38/M30's exact shape)" { Test-LuaFileCount $empty 500 }

    $short = Join-Path $work "short"
    New-Item -ItemType Directory -Path $short -Force | Out-Null
    1..2 | ForEach-Object { New-Item -ItemType File -Path (Join-Path $short "mod_$_.lua") -Force | Out-Null }
    Test-False "Test-LuaFileCount fails on a truncated corpus" { Test-LuaFileCount $short 3 }

    # Non-.lua files in the same directory must not be counted.
    $mixed = Join-Path $work "mixed"
    New-Item -ItemType Directory -Path $mixed -Force | Out-Null
    1..3 | ForEach-Object { New-Item -ItemType File -Path (Join-Path $mixed "mod_$_.lua") -Force | Out-Null }
    New-Item -ItemType File -Path (Join-Path $mixed "luabox.toml") -Force | Out-Null
    New-Item -ItemType File -Path (Join-Path $mixed "README.md") -Force | Out-Null
    Test-True "Test-LuaFileCount ignores non-.lua files in the same directory" { Test-LuaFileCount $mixed 3 }

    # --- New-PerfManifest ----------------------------------------------------
    $strictOut = Join-Path $work "strict.toml"
    New-PerfManifest -Path $strictOut -Name "perf-gate-corpus" -Strict $true
    $strictContent = Get-Content -Path $strictOut -Raw
    if ($strictContent -match 'name = "perf-gate-corpus"' -and
        $strictContent -match '\[types\]' -and
        $strictContent -match 'strict = true' -and
        $strictContent.TrimEnd() -match '\[dependencies\]$') {
        Write-Host "PASS  New-PerfManifest (strict) has the right name, [types] block and trailing [dependencies]"
        $script:pass++
    } else {
        Write-Host "FAIL  New-PerfManifest (strict): unexpected content"
        Write-Host $strictContent
        $script:fail++
    }

    $lenientOut = Join-Path $work "lenient.toml"
    New-PerfManifest -Path $lenientOut -Name "perf-gate-diagnostics" -Strict $false
    $lenientContent = Get-Content -Path $lenientOut -Raw
    if ($lenientContent -match 'name = "perf-gate-diagnostics"' -and
        $lenientContent -notmatch '\[types\]' -and
        $lenientContent -notmatch 'strict = true' -and
        $lenientContent.TrimEnd() -match '\[dependencies\]$') {
        Write-Host "PASS  New-PerfManifest (non-strict) omits [types] entirely"
        $script:pass++
    } else {
        Write-Host "FAIL  New-PerfManifest (non-strict): unexpected content"
        Write-Host $lenientContent
        $script:fail++
    }

    # --- Read-PerfBudgets ------------------------------------------------------
    $budgetsPath = Join-Path $repo "scripts/perf-gate-budgets.env"
    $budgets = Read-PerfBudgets $budgetsPath
    if ($budgets["CHECK_BUDGET_BASE_MS"] -gt 0 -and $budgets["RSS_BUDGET_MIB"] -gt 0) {
        Write-Host "PASS  Read-PerfBudgets parses the real perf-gate-budgets.env"
        $script:pass++
    } else {
        Write-Host "FAIL  Read-PerfBudgets: expected positive CHECK_BUDGET_BASE_MS and RSS_BUDGET_MIB"
        $script:fail++
    }

    # The two ceiling keys specifically. They were added to the shared
    # budgets file by the round 6 rebase and read by perf-gate.sh alone —
    # perf-gate.ps1 ignored them for a full round, which is the M50 drift in
    # its quietest form (one file, two readers, one of them partial). Pinned
    # by name so a key that lands in perf-gate-budgets.env and never reaches
    # this side again fails here rather than in production.
    if ($budgets["CHECK_CEILING_MS"] -gt 0 -and $budgets["DIAG_CHECK_CEILING_MS"] -gt 0) {
        Write-Host "PASS  Read-PerfBudgets exposes the ceiling keys perf-gate.ps1 applies"
        $script:pass++
    } else {
        Write-Host "FAIL  Read-PerfBudgets: expected positive CHECK_CEILING_MS and DIAG_CHECK_CEILING_MS"
        $script:fail++
    }

    # #58 review round 8, F15b: the round 6 rebase capped two of the EIGHT
    # scaled legs, so the other six ran unbounded at CI's factor 4.0 — the
    # "a 12x ceiling is not a gate" condition perf-gate-lib.sh's own comment
    # names, left live on three quarters of the gate. Pinned by name on both
    # sides (perf-gate-selftest.sh carries the identical list) because a leg
    # that LOSES its ceiling looks exactly like a leg that never had one.
    foreach ($key in @(
        "COLD_START_CEILING_MS", "FMT_CEILING_MS", "CHECK_CEILING_MS",
        "RETAINED_ENV_CHECK_CEILING_MS", "DIAG_LINT_CEILING_MS",
        "DIAG_CHECK_CEILING_MS", "DIAG_LINT_RENDERED_CEILING_MS",
        "DIAG_CHECK_RENDERED_CEILING_MS")) {
        if ($budgets[$key] -gt 0) {
            Write-Host "PASS  perf-gate-budgets.env defines a positive $key (F15b: all 8 scaled legs are capped)"
            $script:pass++
        } else {
            Write-Host "FAIL  perf-gate-budgets.env has no positive $key"
            $script:fail++
        }
    }

    # #58 review round 8, F16: the RETAINED-TYPEENV REGRESSION GATE's three
    # constants were bare literals in perf-gate.ps1 AND perf-gate.sh — the
    # one leg guarding a ~25x peak-RSS regression was the one whose budget
    # could drift between Linux and Windows with nothing to notice, contra
    # M50. They live in the shared file now; pinned by name so a key that
    # never reaches it fails here instead of on a Windows runner later.
    foreach ($key in @(
        "RETAINED_ENV_CORPUS_FILES", "RETAINED_ENV_RSS_BUDGET_MIB",
        "RETAINED_ENV_CHECK_BUDGET_BASE_MS")) {
        if ($budgets[$key] -gt 0) {
            Write-Host "PASS  perf-gate-budgets.env defines $key (F16: no longer hand-carried in both gates)"
            $script:pass++
        } else {
            Write-Host "FAIL  perf-gate-budgets.env does not define $key"
            $script:fail++
        }
    }

    # A ceiling BELOW its own base would silently re-tighten the FACTOR=1.0
    # budget every base in that file was calibrated at — the cap bounds the
    # FACTOR, never the calibration. Checked as a relation between the
    # committed numbers, on both readers.
    $inverted = @()
    foreach ($leg in @("COLD_START", "FMT", "CHECK", "RETAINED_ENV_CHECK",
        "DIAG_LINT", "DIAG_CHECK", "DIAG_LINT_RENDERED", "DIAG_CHECK_RENDERED")) {
        if ($budgets["${leg}_CEILING_MS"] -lt $budgets["${leg}_BUDGET_BASE_MS"]) {
            $inverted += $leg
        }
    }
    Test-Equal "every ceiling in perf-gate-budgets.env is >= its own base" ($inverted -join ",") ""

    $malformed = Join-Path $work "malformed-budgets.env"
    Set-Content -Path $malformed -Value @("GOOD_KEY=1", "not a key=value line at all")
    $threw = $false
    try { Read-PerfBudgets $malformed | Out-Null } catch { $threw = $true }
    if ($threw) {
        Write-Host "PASS  Read-PerfBudgets throws on a malformed line rather than half-parsing"
        $script:pass++
    } else {
        Write-Host "FAIL  Read-PerfBudgets: expected a throw on a malformed line"
        $script:fail++
    }
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

# ============================================================================
# Part 2 — perf-gate.ps1, run for real against a stub luabox/gen-corpus.
# ============================================================================
# See the file header for what this covers (7 of 10 threshold comparisons)
# and what it structurally cannot (the three RSS-measuring legs, which use
# raw Process.Start() rather than the `&` call operator).

# PERF_GATE_SELFTEST_SKIP_BEHAVIORAL drops Part 2 — every case that runs
# perf-gate.ps1 for real. It exists for local iteration on Part 1 alone.
#
# It does NOT produce a pass (decisions/12 section 3, local merge-gate
# finding): the earlier version of this switch dropped a third of the suite
# and still exited 0 with a clean "N passed, 0 failed" line — a green verdict
# over a suite whose behavioural half never ran. The final verdict below is
# PARTIAL and the exit code nonzero whenever this variable is set, so the
# only way to get a green perf-gate-selftest is to run all of it.
# perf-gate-selftest.sh carries the identical rule for the identical
# variable; the two must not diverge.
$skipBehavioral = [bool]$env:PERF_GATE_SELFTEST_SKIP_BEHAVIORAL
if ($skipBehavioral) {
    Write-Host "SKIP  Part 2 (behavioural perf-gate.ps1 runs) - PERF_GATE_SELFTEST_SKIP_BEHAVIORAL set"
} else {

$gwork = Join-Path ([System.IO.Path]::GetTempPath()) ("perf-gate-selftest-gate-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $gwork -Force | Out-Null

# The stub `luabox`: a .ps1 script invoked as `& $luaboxBin <args>`. Which
# leg is asking is decided by $PWD, the same discriminator the bash stub
# uses (perf-gate.ps1 always Push-Location's into a leg-specific directory
# first) — the corpus root, or one of the four diagnostics-heavy/*
# subdirectories. Every knob is an env var so a case can drive exactly one
# leg slow while every other stays fast.
$stubLuabox = Join-Path $gwork "luabox.ps1"
@'
param()
$cmd = $args[0]
$cwd = (Get-Location).Path
function Wait-Ms($ms) {
    if ($ms -and [double]$ms -gt 0) { Start-Sleep -Milliseconds ([int][double]$ms) }
}
switch ($cmd) {
    "--version" { Wait-Ms $env:STUB_VERSION_MS; exit 0 }
    "fmt" { Wait-Ms $env:STUB_FMT_MS; exit 1 }
    "lint" {
        if ($cwd -like "*diagnostics-heavy*lint-rendered") { Wait-Ms $env:STUB_DIAG_LINT_RENDERED_MS }
        elseif ($cwd -like "*diagnostics-heavy*lint") { Wait-Ms $env:STUB_DIAG_LINT_MS }
        exit 1
    }
    "check" {
        if ($cwd -like "*diagnostics-heavy*check-rendered") { Wait-Ms $env:STUB_DIAG_CHECK_RENDERED_MS; exit 1 }
        elseif ($cwd -like "*diagnostics-heavy*check") { Wait-Ms $env:STUB_DIAG_CHECK_MS; exit 1 }
        else {
            # STUB_THROW_ON_CHECK_100K: raise a TERMINATING error from inside
            # the stub, mid-run, on the 100-kLOC corpus's `check (warm)` leg —
            # i.e. after cold start and fmt have already reported PASS and
            # before any diagnostics-heavy leg has run. `throw` propagates out
            # of a `&`-invoked .ps1 into the caller's scope regardless of
            # $ErrorActionPreference, which is exactly the shape perf-gate.ps1's
            # own catch exists for: a binary that fails to launch, a corpus
            # write error, the RSS leg's Process.Start blowing up. Drives the
            # abort-mid-legs case below (#58 review round 8, F11).
            if ($env:STUB_THROW_ON_CHECK_100K) {
                throw "stub-luabox: simulated mid-run failure on the 100-kLOC check leg"
            }
            Wait-Ms $env:STUB_CHECK_100K_MS; exit 0
        }
    }
    default {
        Write-Error "stub-luabox: unsupported invocation: $($args -join ' ')"
        exit 2
    }
}
'@ | Set-Content -Path $stubLuabox

$stubGenCorpus = Join-Path $gwork "gen-corpus.ps1"
@'
param()
$out = ""
$files = 0
for ($i = 0; $i -lt $args.Length; $i++) {
    if ($args[$i] -eq "--out") { $out = $args[$i + 1] }
    if ($args[$i] -eq "--files") { $files = [int]$args[$i + 1] }
}
New-Item -ItemType Directory -Path $out -Force | Out-Null
for ($i = 0; $i -lt $files; $i++) {
    New-Item -ItemType File -Path (Join-Path $out "mod_$i.lua") -Force | Out-Null
}
# Explicit: a `&`-invoked .ps1 that ends without `exit` leaves $LASTEXITCODE
# unset in the caller, which the gate's post-invocation check reads as failure.
exit 0
'@ | Set-Content -Path $stubGenCorpus

# The full set of FAIL lines a run of every leg could print. Every member
# except the one a case targets is asserted ABSENT, so a case cannot pass
# because it got the right exit code and the right FAIL line while some other
# comparison also (wrongly) fired. This list must name all TEN legs, not just
# the seven Part 2 can drive: the three RSS-measuring legs are ones this file
# cannot make fail on purpose (see the header — Process.Start() cannot launch
# a .ps1 stub), but they can still fail by ACCIDENT, and leaving them out of
# the absence sweep is exactly the "asserted the right thing while something
# else broke" gap the sweep exists for. Mirrors perf-gate-selftest.sh's
# all_fail_needles, which has carried all ten from the start.
$allFailNeedles = @(
    "FAIL cold start:",
    "FAIL fmt --check (warm):",
    "FAIL check (warm):",
    "FAIL check peak RSS:",
    "FAIL retained-TypeEnv regression, peak RSS:",
    "FAIL retained-TypeEnv regression, wall time:",
    "FAIL lint (diagnostics-heavy):",
    "FAIL check (diagnostics-heavy):",
    "FAIL lint (diagnostics-heavy, rendered):",
    "FAIL check (diagnostics-heavy, rendered):",
    # #58 review round 8, F11: perf-gate.ps1's abort-mid-legs catch — the
    # guard against "half a gate reading as green", and the only one of the
    # five defects that round found which had NO self-test at all — was
    # absent from this sweep, so no case asserted it either present OR
    # absent. Deleting the catch outright left this suite fully green. It is
    # a FAIL line like any other now: every case asserts it absent except
    # abort_mid_legs_fails_loudly, which asserts it present.
    "FAIL perf-gate: aborted before all legs ran:"
)

function Invoke-Gate {
    param(
        [string]$Name,
        [int]$WantExit,
        [string]$WantFailNeedle = "",
        [string[]]$AlsoPresent = @(),
        # The `!needle` half of perf-gate-selftest.sh's vocabulary
        # (selftest-lib.sh's assert_exit): a case that can only prove a
        # string PRESENT cannot prove the right code path fired. $allFailNeedles
        # covers the ten leg FAILs automatically; this is for everything else
        # a case needs gone — a verdict line, another leg's PASS.
        [string[]]$AlsoAbsent = @())
    $log = Join-Path $gwork "$Name.log"
    $env:LUABOX_BIN = $stubLuabox
    $env:GEN_CORPUS_BIN = $stubGenCorpus
    pwsh -File $gate *> $log
    $got = $LASTEXITCODE
    Remove-Item Env:\LUABOX_BIN -ErrorAction SilentlyContinue
    Remove-Item Env:\GEN_CORPUS_BIN -ErrorAction SilentlyContinue
    $logText = Get-Content -Path $log -Raw
    $ok = $true
    $report = ""
    if ($got -ne $WantExit) { $ok = $false; $report += " expected exit $WantExit, got $got" }
    foreach ($needle in $allFailNeedles) {
        $shouldBePresent = ($WantFailNeedle -ne "" -and $needle -eq $WantFailNeedle)
        $isPresent = $logText.Contains($needle)
        if ($shouldBePresent -and -not $isPresent) { $ok = $false; $report += " missing:[$needle]" }
        if (-not $shouldBePresent -and $isPresent) { $ok = $false; $report += " present-but-should-be-absent:[$needle]" }
    }
    foreach ($needle in $AlsoPresent) {
        if (-not $logText.Contains($needle)) { $ok = $false; $report += " missing:[$needle]" }
    }
    foreach ($needle in $AlsoAbsent) {
        if ($logText.Contains($needle)) { $ok = $false; $report += " present-but-should-be-absent:[$needle]" }
    }
    if ($ok) {
        Write-Host "PASS  $Name (exit $got)"
        $script:pass++
    } else {
        Write-Host "FAIL  ${Name}: $report"
        Write-Host $logText
        $script:fail++
    }
}

# Baseline: every leg fast -> ALL GATES PASSED, exit 0.
$env:STUB_VERSION_MS = "0"; $env:STUB_FMT_MS = "0"; $env:STUB_CHECK_100K_MS = "0"
$env:STUB_DIAG_LINT_MS = "0"; $env:STUB_DIAG_CHECK_MS = "0"
$env:STUB_DIAG_LINT_RENDERED_MS = "0"; $env:STUB_DIAG_CHECK_RENDERED_MS = "0"
# The two AlsoPresent needles pin the stub-mode SKIP lines for the
# Process-launched legs (see the file header's disclosed gap): if the gate
# ever stops printing them — the skip silently widening, or the legs
# silently running against a stub — this case goes red.
Invoke-Gate "all_legs_pass_when_fast" 0 -AlsoPresent @(
    "SKIP check peak RSS: stub binaries cannot be process-launched",
    "SKIP retained-TypeEnv regression: stub binaries cannot be process-launched"
)

# One case per comparison this stub can drive: exactly that leg's FAIL,
# nothing else. Budgets are the REAL ones from perf-gate-budgets.env (no
# override), so a widened constant would make these cases go green just
# like the bash audit's equivalent cases.
$env:STUB_VERSION_MS = "200"
Invoke-Gate "cold_start_threshold_is_live" 1 "FAIL cold start:"
$env:STUB_VERSION_MS = "0"

$env:STUB_FMT_MS = "3000"
Invoke-Gate "fmt_threshold_is_live" 1 "FAIL fmt --check (warm):"
$env:STUB_FMT_MS = "0"

$env:STUB_CHECK_100K_MS = "8000"
Invoke-Gate "check_warm_threshold_is_live" 1 "FAIL check (warm):"
$env:STUB_CHECK_100K_MS = "0"

$env:STUB_DIAG_LINT_MS = "1500"
Invoke-Gate "diag_lint_threshold_is_live" 1 "FAIL lint (diagnostics-heavy):"
$env:STUB_DIAG_LINT_MS = "0"

$env:STUB_DIAG_CHECK_MS = "4500"
Invoke-Gate "diag_check_threshold_is_live" 1 "FAIL check (diagnostics-heavy):"
$env:STUB_DIAG_CHECK_MS = "0"

$env:STUB_DIAG_LINT_RENDERED_MS = "1000"
Invoke-Gate "diag_lint_rendered_threshold_is_live" 1 "FAIL lint (diagnostics-heavy, rendered):"
$env:STUB_DIAG_LINT_RENDERED_MS = "0"

$env:STUB_DIAG_CHECK_RENDERED_MS = "3000"
Invoke-Gate "diag_check_rendered_threshold_is_live" 1 "FAIL check (diagnostics-heavy, rendered):"
$env:STUB_DIAG_CHECK_RENDERED_MS = "0"

# --- F11: the abort-mid-legs catch ------------------------------------------
# perf-gate.ps1's catch is the guard against a gate that measured half of
# what it claims and called it green: $ErrorActionPreference is "Continue"
# through the legs (the fmt/lint-stderr note), so without the catch an
# exception thrown mid-run aborts every REMAINING leg, runs the finally, and
# falls through to `exit 0` with $fail still $false. Round 6 recorded that as
# MEASURED, not hypothetical — the .ps1 selftest's first real execution hit
# it through the RSS leg — and then shipped the fix with no case behind it
# (#58 review round 8, F11: delete the `catch` block and this suite stayed
# fully green, the one silently-green defect of the five being the one with
# no test).
#
# The stub raises a terminating error on the 100-kLOC `check` leg, so the
# abort lands strictly BETWEEN legs: cold start and fmt have already printed
# PASS, and check (warm) and everything after it never run. That ordering is
# asserted both ways — the two earlier PASSes present, the aborted leg's own
# PASS absent — so the case cannot pass on an exception thrown before the
# gate did anything, which would prove nothing about "aborted BEFORE ALL LEGS
# ran". Restore the catch's absence and this goes RED on the missing needle;
# no other case in the file changes.
$env:STUB_THROW_ON_CHECK_100K = "1"
Invoke-Gate "abort_mid_legs_fails_loudly" 1 "FAIL perf-gate: aborted before all legs ran:" `
    -AlsoPresent @("PASS cold start:", "PASS fmt --check (warm):") `
    -AlsoAbsent @("PASS check (warm):", "perf-gate: ALL GATES PASSED")
Remove-Item Env:\STUB_THROW_ON_CHECK_100K -ErrorAction SilentlyContinue

# --- F15b: a newly-capped leg's ceiling is actually WIRED --------------------
# Part 1 proves the eight ceiling keys exist and that ConvertTo-ScaledBudgetMs
# honours -CeilingMs. Neither proves perf-gate.ps1 PASSES one at a given leg —
# which is the exact gap F15b names, and the exact shape of the drift the
# round 6 local merge-gate found (CHECK_CEILING_MS/DIAG_CHECK_CEILING_MS sat
# in the shared file honoured by the bash reader and silently ignored here).
# Every case above runs at the default factor 1.0, where scaled == base and
# no ceiling can bind.
#
# Same leg and same numbers as perf-gate-selftest.sh's twin case: at
# LUABOX_PERF_FACTOR=100 the uncapped cold-start budget is 50 * 100 = 5000 ms
# and the capped one is COLD_START_CEILING_MS = 150 ms; a 200 ms stub
# `--version` sits between them. Drop -CeilingMs from perf-gate.ps1's
# $coldStartBudget line and the 5000 ms budget swallows it: the FAIL needle
# vanishes, exit goes 1 -> 0, RED.
$env:LUABOX_PERF_FACTOR = "100"
$env:STUB_VERSION_MS = "200"
Invoke-Gate "cold_start_ceiling_is_wired_at_a_high_factor" 1 "FAIL cold start:"
$env:STUB_VERSION_MS = "0"
Remove-Item Env:\LUABOX_PERF_FACTOR -ErrorAction SilentlyContinue

Remove-Item -Recurse -Force $gwork -ErrorAction SilentlyContinue

} # PERF_GATE_SELFTEST_SKIP_BEHAVIORAL

# The gate on the skip switch itself — mirrors
# perf-gate-selftest.sh's skip_behavioral_refuses_to_report_a_full_pass.
# Re-invokes THIS file with the variable set and requires the child to refuse
# to call itself green: nonzero exit and a PARTIAL verdict, with the counts
# still printed (what is withheld is the VERDICT, not the data). Only run
# when the variable is not already set, or the child would spawn a
# grandchild without bound.
if (-not $skipBehavioral) {
    $partialLog = Join-Path ([System.IO.Path]::GetTempPath()) ("perf-gate-selftest-partial-" + [System.Guid]::NewGuid().ToString("N") + ".log")
    $env:PERF_GATE_SELFTEST_SKIP_BEHAVIORAL = "1"
    pwsh -File $MyInvocation.MyCommand.Path *> $partialLog
    $partialExit = $LASTEXITCODE
    Remove-Item Env:\PERF_GATE_SELFTEST_SKIP_BEHAVIORAL -ErrorAction SilentlyContinue
    $partialText = Get-Content -Path $partialLog -Raw
    Remove-Item -Force $partialLog -ErrorAction SilentlyContinue
    if ($partialExit -ne 0 -and
        $partialText.Contains("PARTIAL - behavioural cases skipped (local-iteration mode)") -and
        $partialText.Contains("SKIP  Part 2") -and
        $partialText.Contains("0 failed") -and
        -not $partialText.Contains("ALL GATES PASSED")) {
        Write-Host "PASS  skip_behavioral_refuses_to_report_a_full_pass (exit $partialExit)"
        $script:pass++
    } else {
        Write-Host "FAIL  skip_behavioral_refuses_to_report_a_full_pass: expected a nonzero exit and a PARTIAL verdict, got exit $partialExit"
        Write-Host $partialText
        $script:fail++
    }
}

Write-Host ""
Write-Host "perf-gate-selftest (PowerShell): $script:pass passed, $script:fail failed"
if ($skipBehavioral) {
    Write-Host "perf-gate-selftest: PARTIAL - behavioural cases skipped (local-iteration mode)"
    Write-Host "perf-gate-selftest:   PERF_GATE_SELFTEST_SKIP_BEHAVIORAL was set, so every case that runs"
    Write-Host "perf-gate-selftest:   perf-gate.ps1 for real was dropped - the counts above are Part 1 only"
    Write-Host "perf-gate-selftest:   and are not evidence that the gate's threshold comparisons are"
    Write-Host "perf-gate-selftest:   load-bearing. Re-run without the variable before treating this green."
    exit 1
}
if ($script:fail -eq 0 -and $script:pass -gt 0) {
    exit 0
} else {
    exit 1
}
