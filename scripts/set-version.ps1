# ── NanoClick version bumper ────────────────────────────────────────────────
# Writes the app version into every file that STORES it, then verifies the result.
#
# Why this exists: `release.ps1 -Tag vX.Y.Z` and `make_release.ps1 -Tag vX.Y.Z`
# only READ the version (installer file name + latest.json). They do not edit
# anything, and the manifest that tauri embeds must already carry the tag — so
# the bump has to happen first. Nothing in the repo did it: the last bump
# (commit 5bcb91a) edited the files by hand.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\set-version.ps1 -Version 1.2.0
#   powershell -ExecutionPolicy Bypass -File scripts\set-version.ps1 -Version 1.2.0 -DryRun
#   powershell -ExecutionPolicy Bypass -File scripts\set-version.ps1 -Version 1.2.0 -SkipDocs
#
# After it runs:
#   cargo test --release -j 1 -- --skip physical_    # also refreshes Cargo.lock
#   git diff --stat
#   powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Tag vX.Y.Z

param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    # Show what would change, write nothing.
    [switch]$DryRun,

    # Touch only the two files tauri reads; skip the doc headers.
    [switch]$SkipDocs
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot

function Read-TextUtf8([string]$path) {
    [IO.File]::ReadAllText($path, [Text.UTF8Encoding]::new($false))
}

function Write-TextUtf8([string]$path, [string]$text) {
    # UTF-8 without BOM: the release notes and the JSON are read back by other
    # tools (and uploaded verbatim), and a BOM would leak into both.
    [IO.File]::WriteAllText($path, $text, [Text.UTF8Encoding]::new($false))
}

# Pattern + replacement per file. Every replacement keeps its $1/$3 prefix, so
# surrounding formatting (quotes, alignment, badge text) survives untouched.
$edits = @(
    @{ Path = 'src-tauri\Cargo.toml'
       Pattern = '(?m)^version = "(\d+\.\d+\.\d+)"'
       Replacement = ('version = "' + $Version + '"') },
    @{ Path = 'src-tauri\tauri.conf.json'
       Pattern = '("version"\s*:\s*")(\d+\.\d+\.\d+)(")'
       # `${1}` with braces, NOT `$1`: a group reference glued to a digit is
       # parsed as group 11 (which does not exist), and .NET then substitutes the
       # LITERAL text `$11` — the prefix disappears and the JSON breaks. That is
       # exactly how a bump once produced `$11.1.1",` and killed every build.
       Replacement = ('${1}' + $Version + '${3}') },
    @{ Path = 'README.md'
       Pattern = '(badge/version-)(\d+\.\d+\.\d+)(-)'
       Replacement = ('${1}' + $Version + '${3}') }
)

if (-not $SkipDocs) {
    # Internal docs carry a "Current version" header; historical version numbers
    # further down those files are evidence and must never be rewritten.
    foreach ($doc in @('UI_UX_SPECIFICATION.md', 'PRODUCT_ROADMAP_COMPARISON.md', 'docs\MACRO_ARCHITECTURE.md')) {
        $edits += @{ Path = $doc
                     Pattern = '(\*\*Current version:\*\* v)(\d+\.\d+\.\d+)'
                     Replacement = ('${1}' + $Version) }
    }
}

Write-Host ''
Write-Host "NanoClick version -> $Version$(if ($DryRun) { '  [DRY RUN: nothing is written]' })" -ForegroundColor Cyan
Write-Host ''

$changed = @()
foreach ($e in $edits) {
    $full = Join-Path $repoRoot $e.Path
    if (-not (Test-Path -LiteralPath $full)) {
        Write-Host ("  ----  {0} (missing, skipped)" -f $e.Path) -ForegroundColor DarkGray
        continue
    }

    $old = Read-TextUtf8 $full
    $new = [regex]::Replace($old, $e.Pattern, $e.Replacement)

    if ($new -eq $old) {
        if ($old.Contains($Version)) {
            Write-Host ("  ok    {0} (already {1})" -f $e.Path, $Version) -ForegroundColor DarkGray
        } else {
            Write-Host ("  WARN  {0}: pattern did not match - check the file by hand" -f $e.Path) -ForegroundColor DarkYellow
        }
        continue
    }

    # Never write something the next tool cannot read. A malformed
    # tauri.conf.json makes every cargo command die inside tauri-build
    # ("unable to parse JSON ... key must be a string"), which turns a bad bump
    # into a red release preflight. Checked in dry runs too, so the problem is
    # reported before anything is touched.
    if ($e.Path -like '*.json') {
        try { $null = $new | ConvertFrom-Json }
        catch { throw ("refusing to write invalid JSON to " + $e.Path + ": " + $_.Exception.Message) }
    }

    if (-not $DryRun) { Write-TextUtf8 $full $new }
    $verb = if ($DryRun) { 'would ' } else { 'wrote  ' }
    Write-Host ("  {0}{1}" -f $verb, $e.Path) -ForegroundColor Green
    $changed += $e.Path
}

# ── verify: the two files tauri reads must now carry the requested version ──
$mustMatch = @('src-tauri\Cargo.toml', 'src-tauri\tauri.conf.json')
$mismatch = @()
foreach ($rel in $mustMatch) {
    $full = Join-Path $repoRoot $rel
    if (-not (Test-Path -LiteralPath $full)) { continue }
    if (-not (Read-TextUtf8 $full).Contains($Version)) { $mismatch += $rel }
}

Write-Host ''
if ($mismatch.Count -gt 0 -and -not $DryRun) {
    throw ("version not applied to: " + ($mismatch -join ', '))
}

if ($changed.Count -eq 0) {
    Write-Host "Nothing to do: every file already says $Version." -ForegroundColor DarkGray
} elseif ($DryRun) {
    Write-Host ("Dry run: {0} file(s) would change." -f $changed.Count) -ForegroundColor Yellow
} else {
    Write-Host ("Done: {0} file(s) updated." -f $changed.Count) -ForegroundColor Green
    Write-Host ''
    Write-Host 'Next steps:' -ForegroundColor Cyan
    Write-Host "  1) cargo test --release -j 1 -- --skip physical_    # also refreshes Cargo.lock"
    Write-Host "  2) git diff --stat                                    # review, then commit"
    Write-Host "  3) powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Tag v$Version"
    Write-Host ''
    Write-Host 'Note: Cargo.lock still names the old version until the next cargo run;' -ForegroundColor DarkGray
    Write-Host 'that is expected and heals itself on step 1.' -ForegroundColor DarkGray
}
Write-Host ''
