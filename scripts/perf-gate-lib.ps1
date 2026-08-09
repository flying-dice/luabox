<#
.SYNOPSIS
    Pure helper functions shared by scripts/perf-gate.ps1 and its self-test
    (scripts/tests/perf-gate-selftest.ps1). PowerShell counterpart of
    scripts/perf-gate-lib.sh — same split, same reason: kept free of any
    real `luabox`/`cargo`/corpus-generation call so the self-test can dot-
    source this file directly and exercise the logic in milliseconds
    (#58 review round 6, M51). Dot-sourcing this file only defines
    functions — nothing here has a side effect on its own.
#>

# Read-PerfBudgets <path> — parses the KEY=VALUE file both perf-gate.sh
# (via a plain bash `source`) and this script read as the one owner of the
# budget constants (#58 review round 6, M50: these used to be a second,
# hand-carried copy in this file, free to drift from perf-gate.sh's own).
# Blank lines and lines starting with `#` are comments. Fails loudly (throws)
# on a missing file or a line that is not `KEY=INTEGER` — a budgets file
# that half-parses is not a fact about the budgets, the same "fail when it
# measures nothing" rule every other gate in this repo follows.
function Read-PerfBudgets {
    param(
        [Parameter(Mandatory)][string]$Path
    )
    if (-not (Test-Path $Path)) {
        throw "Read-PerfBudgets: no budgets file at $Path"
    }
    $budgets = @{}
    foreach ($line in Get-Content -Path $Path) {
        $trimmed = $line.Trim()
        if ($trimmed -eq "" -or $trimmed.StartsWith("#")) { continue }
        if ($trimmed -notmatch '^([A-Z_][A-Z0-9_]*)=(-?[0-9]+)$') {
            throw "Read-PerfBudgets: malformed line in ${Path}: $line"
        }
        $budgets[$Matches[1]] = [int]$Matches[2]
    }
    return $budgets
}

# New-PerfManifest -Path -Name -Strict — every generated corpus needs a
# luabox.toml differing only in `name` and whether `[types] strict = true`
# is present; two near-identical here-strings (N42) is exactly the kind of
# duplication that lets one copy drift from the other silently. One writer,
# one place to read the shape from — mirrors perf-gate-lib.sh's
# write_perf_manifest.
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

# ConvertTo-ScaledBudgetMs <BaseMs> <Factor> [-CeilingMs] — the -Factor float
# multiply every timed leg's budget goes through, extracted so the self-test
# can pin the truncate-vs-round behaviour directly, the same property
# perf-gate-lib.sh's scale_budget_ms self-test pins on the bash side.
# `[int]` truncates toward zero in PowerShell for a positive double, which
# is the same "truncates, does not round" contract scale_budget_ms's `%d`
# printf format gives on the bash side.
#
# -CeilingMs mirrors scale_budget_ms's optional third argument, added on the
# bash side by the round 6 budget rebase and left off this one — the parity
# drift a local merge-gate found: perf-gate.ps1 reads the SAME
# perf-gate-budgets.env, so CHECK_CEILING_MS/DIAG_CHECK_CEILING_MS were sitting
# in the shared file being honoured by one reader and silently ignored by the
# other. -Factor absorbs slow shared hardware by multiplying the WHOLE budget,
# but a base budget already carries ~3x headroom over a measured baseline by
# convention, and multiplying headroom by headroom compounds: at -Factor 4.0
# the 7500 ms `check` base becomes a 30000 ms ceiling, ~12x the ~2.5 s
# baseline. Capping the SCALED value keeps CI's ceiling within a stated
# multiple of what the leg actually costs. 0 (the default) means "no cap",
# matching the bash side's empty third argument — a leg passing no ceiling is
# unchanged.
function ConvertTo-ScaledBudgetMs {
    param(
        [Parameter(Mandatory)][double]$BaseMs,
        [Parameter(Mandatory)][double]$Factor,
        [double]$CeilingMs = 0
    )
    # [Math]::Floor, not a bare [int] cast: PowerShell's [int] rounds
    # (167.5 -> 168) where bash's $(( )) integer arithmetic truncates —
    # measured drift between the two readers of the same budgets file.
    $scaled = [int][Math]::Floor($BaseMs * $Factor)
    if ($CeilingMs -gt 0 -and $scaled -gt $CeilingMs) {
        return [int][Math]::Floor($CeilingMs)
    }
    return $scaled
}

# Test-LuaFileCount <Dir> <Expected> — N38/M30's fix: a peak-RSS (or
# wall-time) PASS printed against a corpus that was not actually generated
# the way the budget assumes (an empty directory, a truncated loop) is not
# evidence of anything. Counts top-level *.lua files only, same contract as
# perf-gate-lib.sh's assert_lua_file_count. Returns $true/$false rather than
# throwing — the caller decides whether a mismatch is fatal, same as the
# bash function's own return-code contract.
function Test-LuaFileCount {
    param(
        [Parameter(Mandatory)][string]$Dir,
        [Parameter(Mandatory)][int]$Expected
    )
    if (-not (Test-Path $Dir)) {
        Write-Host "error: corpus directory does not exist: $Dir"
        return $false
    }
    $got = (Get-ChildItem -Path $Dir -Filter "*.lua" -File -ErrorAction SilentlyContinue).Count
    if ($got -ne $Expected) {
        Write-Host "error: corpus at $Dir has $got .lua file(s), expected $Expected — the generation step"
        Write-Host "error:   did not run the way this leg's budget assumes; the measurement below would not"
        Write-Host "error:   be about the corpus its own PASS/FAIL line claims"
        return $false
    }
    return $true
}
