<#
.SYNOPSIS
  Broad-tripwire JavaScript lint for the NanoClick frontend (oxlint, Rust binary).

.DESCRIPTION
  Runs `oxlint` over `src/` using the repository `.oxlintrc.json`. Unlike ESLint,
  oxlint needs no `package.json` and no `node_modules`, which is why it fits a
  project that deliberately ships without a bundler or an npm dependency tree.

  SCOPE / HONESTY: this is a SECOND line of defence, not the authority.
    * `scripts\check-js-syntax.ps1` (and the Rust test `test_frontend_js_syntax_is_valid`)
      stay authoritative for grammar, because they parse each file in the SAME
      mode the browser uses (`type="module"` → `.mjs`). A duplicate top-level
      declaration is fatal in a module and legal in a classic script — the linter
      is not trusted to reproduce that distinction.
    * oxlint catches breadth: unused/undefined identifiers, dead code, duplicate
      object keys, shady conditions, missing `await` on already-resolved chains.

  EXIT CODES
    0 - clean: no errors and no warnings (the gate passes --deny-warnings)
    1 - findings: oxlint reported errors or warnings
    2 - tooling unavailable (no node/npx) — not a code failure

  Use -AllowWarnings only while triaging a warning: the historical "errors only"
  mode. The 53-warning baseline was burned down to zero on 2026-09-21 (AGENTS.md),
  so a warning is a regression, not noise.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\check-js-lint.ps1
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\check-js-lint.ps1 -Fix
#>
[CmdletBinding()]
param(
  [string]$SrcDir,
  # Pin on purpose: a bare `latest` turns the release gate into a lottery.
  # Override per-run with -Version or the NANOCLICK_OXLINT_VERSION env var.
  [string]$Version = $env:NANOCLICK_OXLINT_VERSION,
  [string]$Config = ".oxlintrc.json",
  [switch]$Fix,
  # Emergency escape hatch for triaging: go back to "errors only".
  [switch]$AllowWarnings
)

$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($Version)) { $Version = '1.84.0' }

if ([string]::IsNullOrWhiteSpace($SrcDir)) {
  $scriptRoot = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
  $repoRoot = (Resolve-Path (Join-Path $scriptRoot '..')).Path
  $SrcDir = Join-Path $repoRoot 'src'
  if ([string]::IsNullOrWhiteSpace($Config) -or -not (Test-Path -LiteralPath (Join-Path $scriptRoot $Config))) {
    # keep the default config path relative to the repo root
    $Config = ".oxlintrc.json"
  }
  $runDir = $repoRoot
} else {
  $runDir = (Get-Location).Path
}

if (-not (Test-Path $SrcDir)) {
  Write-Host "Source directory not found: $SrcDir" -ForegroundColor Red
  exit 2
}

$npx = Get-Command npx -ErrorAction SilentlyContinue
if (-not $npx) {
  Write-Host 'npx not found - install Node.js to run the linter (the grammar gate needs it too).' -ForegroundColor Yellow
  exit 2
}

$oxlintArgs = @('-y', "oxlint@$Version", '--config', $Config, '-f', 'default')
if ($Fix) { $oxlintArgs += '--fix' }
# Warnings are treated as findings: the ~53-item baseline was burned down to zero
# on 2026-09-21 (see AGENTS.md), so anything reported here is new.
if (-not $AllowWarnings) { $oxlintArgs += '--deny-warnings' }

Push-Location $runDir
try {
  Write-Host "oxlint@$Version on $SrcDir" -ForegroundColor DarkCyan
  & npx @oxlintArgs $SrcDir
  $code = $LASTEXITCODE
} finally {
  Pop-Location
}

Write-Host ''
if ($code -eq 0) {
  # Exit 0 means "clean": no errors and no warnings (unless -AllowWarnings was used).
  Write-Host 'OK: oxlint reports no errors and no warnings.' -ForegroundColor Green
  exit 0
}
Write-Host "FAILED: oxlint reported findings (exit $code) - fix them, or pass -SkipChecks at release time." -ForegroundColor Red
exit 1
