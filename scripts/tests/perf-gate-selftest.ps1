<#
.SYNOPSIS
    Self-test for scripts/perf-gate.ps1 and scripts/perf-gate-lib.ps1 — the
    Windows counterpart of scripts/tests/perf-gate-selftest.sh.

.DESCRIPTION
    #58 review round 6, M51: perf-gate.ps1 had NO self-test at all, and
    New-PerfManifest was a hand-reimplementation of perf-gate-lib.sh's
    write_perf_manifest with nothing proving the two stay equivalent. This
    file is unverified — there is no `pwsh` on the Linux box this round's
    fixes were made and reviewed on (checked directly: `which pwsh` finds
    nothing), so nothing here has actually been RUN. It is written to the
    same structure and against the same properties as the bash self-test it
    mirrors, using the same PowerShell idioms perf-gate.ps1 and
    perf-gate-lib.ps1 already use elsewhere in this repo, but it has not
    been exercised even once. scripts/tests/perf-gate-selftest.ps1 needs a
    real `pwsh` run — a CI leg for that (Windows runner, `pwsh -File
    scripts/tests/perf-gate-selftest.ps1`) is added alongside this file so
    the first real execution happens in CI, not silently deferred forever.

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
    check (warm), and the four diagnostics-heavy timing legs. It does NOT
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
$repo = Split-Path -Parent $here
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

if ($env:PERF_GATE_SELFTEST_SKIP_BEHAVIORAL) {
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
        else { Wait-Ms $env:STUB_CHECK_100K_MS; exit 0 }
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
'@ | Set-Content -Path $stubGenCorpus

$allFailNeedles = @(
    "FAIL cold start:",
    "FAIL fmt --check (warm):",
    "FAIL check (warm):",
    "FAIL lint (diagnostics-heavy):",
    "FAIL check (diagnostics-heavy):",
    "FAIL lint (diagnostics-heavy, rendered):",
    "FAIL check (diagnostics-heavy, rendered):"
)

function Invoke-Gate {
    param([string]$Name, [int]$WantExit, [string]$WantFailNeedle = "")
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
Invoke-Gate "all_legs_pass_when_fast" 0

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

Remove-Item -Recurse -Force $gwork -ErrorAction SilentlyContinue

} # PERF_GATE_SELFTEST_SKIP_BEHAVIORAL

Write-Host ""
Write-Host "perf-gate-selftest (PowerShell): $script:pass passed, $script:fail failed"
if ($script:fail -eq 0 -and $script:pass -gt 0) {
    exit 0
} else {
    exit 1
}
