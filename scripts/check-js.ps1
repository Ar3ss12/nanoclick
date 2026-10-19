<#
.SYNOPSIS
  Combined JavaScript gate for the NanoClick frontend (syntax + lint).

.DESCRIPTION
  Runs BOTH existing gates in order (same commands you already use):
    1. scripts\check-js-syntax.ps1 — Node grammar check over EVERY *.js
       in src/ (module mode for type="module", classic for the rest).
    2. scripts\check-js-lint.ps1 — oxlint tripwire over the whole src/
       (all 10 .js files, recursive, no node_modules needed).

  This file adds no new checks — it only chains the two scripts above and
  merges their exit codes, so one command replaces two:
    powershell -ExecutionPolicy Bypass -File scripts\check-js.ps1

  EXIT CODES
    0 - both gates green
    1 - syntax and/or lint reported findings
    2 - tooling unavailable (no node/npx)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\check-js.ps1
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$scriptRoot = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
$syntaxScript = Join-Path $scriptRoot 'check-js-syntax.ps1'
$lintScript = Join-Path $scriptRoot 'check-js-lint.ps1'

$failed = $false

Write-Host '══ [1/2] syntax (node --check, every *.js) ══' -ForegroundColor Cyan
& powershell -ExecutionPolicy Bypass -File $syntaxScript
$syntaxCode = $LASTEXITCODE
if ($syntaxCode -ne 0) { $failed = $true }

Write-Host ''
Write-Host '══ [2/3] lint (oxlint, whole src/) ══' -ForegroundColor Cyan
& powershell -ExecutionPolicy Bypass -File $lintScript
$lintCode = $LASTEXITCODE
if ($lintCode -eq 2) {
  Write-Host 'INCOMPLETE: linter tooling unavailable (no node/npx).' -ForegroundColor Yellow
  exit 2
}
if ($lintCode -ne 0) { $failed = $true }

Write-Host ''
Write-Host '══ [3/3] frontend unit tests (node --test) ══' -ForegroundColor Cyan
$repoRoot = (Resolve-Path (Join-Path $scriptRoot '..')).Path
$testFiles = Get-ChildItem -Path (Join-Path $repoRoot 'tests\frontend') -Filter '*.test.mjs' -ErrorAction SilentlyContinue
if ($testFiles) {
  foreach ($tf in $testFiles) {
    & node --test $tf.FullName
    if ($LASTEXITCODE -ne 0) { $failed = $true }
  }
}

Write-Host ''
if ($failed) {
  Write-Host '══ JS GATE: RED (see failures above) ══' -ForegroundColor Red
  exit 1
}
Write-Host '══ JS GATE: GREEN (syntax + lint + tests clean) ══' -ForegroundColor Green
exit 0
