<#
.SYNOPSIS
  Fast JavaScript grammar check for the NanoClick frontend.

.DESCRIPTION
  Checks every `<script src="...">` of the frontend with Node in the SAME parse
  mode the browser will use:
    * `type="module"` scripts are checked as `.mjs` (module mode)
    * classic scripts are checked as `.js` (script mode)

  WHY THIS EXISTS: `main.js` is loaded as an ES module, and in module mode a
  DUPLICATE top-level declaration is a fatal SyntaxError — the module never
  executes at all, so the window renders while absolutely nothing works (no
  handlers, no IPC, no hotkeys). The very same duplicate is legal in a classic
  script, which is why `node --check main.js` reports "OK" on a dead UI. This
  script and the Rust test `test_frontend_js_syntax_is_valid` are the check
  that was missing.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\check-js-syntax.ps1
#>
[CmdletBinding()]
param(
  [string]$SrcDir
)

$ErrorActionPreference = 'Stop'

# `$PSScriptRoot` is NOT reliably populated inside a `param()` default on
# Windows PowerShell 5.1 — resolve it in the body instead.
if ([string]::IsNullOrWhiteSpace($SrcDir)) {
  $scriptRoot = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
  $SrcDir = Join-Path $scriptRoot '..\src'
}

if (-not (Test-Path $SrcDir)) {
  Write-Host "Source directory not found: $SrcDir" -ForegroundColor Red
  exit 2
}

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
  Write-Host 'Node.js is required for this check (install Node.js, or rely on the Rust test).' -ForegroundColor Yellow
  exit 2
}

$htmlFiles = Get-ChildItem -Path $SrcDir -Filter *.html -ErrorAction SilentlyContinue
$checked = 0
$failed = @()

foreach ($html in $htmlFiles) {
  $raw = Get-Content -Path $html.FullName -Raw
  foreach ($match in [regex]::Matches($raw, '<script\b(?<attrs>[^>]*)>')) {
    $attrs = $match.Groups['attrs'].Value
    $srcMatch = [regex]::Match($attrs, 'src\s*=\s*(?<q>["''])(?<f>[^"'']+)(\k<q>)')
    if (-not $srcMatch.Success) { continue }
    $file = $srcMatch.Groups['f'].Value
    if (-not $file.EndsWith('.js')) { continue }

    $path = Join-Path $SrcDir $file
    if (-not (Test-Path $path)) {
      Write-Host "[MISSING] $file (referenced by $($html.Name))" -ForegroundColor Red
      $failed += $file
      continue
    }

    $isModule = $attrs -match 'type\s*=\s*["'']module["'']'
    # Copy with the extension that forces the right parse mode: .mjs = module.
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("nanoclick_syntax_" + [IO.Path]::GetFileNameWithoutExtension($file) + $(if ($isModule) { '.mjs' } else { '.js' }))
    Copy-Item -Path $path -Destination $tmp -Force

    # Windows PowerShell 5.1 quirk: with `$ErrorActionPreference = 'Stop'`, a
    # native command that writes to stderr is turned into a terminating
    # NativeCommandError, which SWALLOWS the very `SyntaxError` line this checker
    # exists to print (verified: a `2>&1` pipe or redirect does not help — the
    # error is raised before the pipeline sees the text). Routing the call
    # through `cmd /c` merges both streams into stdout for us, so PowerShell
    # never sees a native stderr write at all.
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = (& cmd /c "node --check `"$tmp`" 2>&1") -join "`n"
        $nodeExit = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prevEap
    }
    $ok = $nodeExit -eq 0
    Remove-Item $tmp -Force -ErrorAction SilentlyContinue

    $mode = if ($isModule) { 'module' } else { 'classic' }
    if ($ok) {
      Write-Host "[OK]      $file ($mode)" -ForegroundColor Green
    } else {
      Write-Host "[SYNTAX]  $file ($mode)" -ForegroundColor Red
      Write-Host ($out | Out-String).Trim() -ForegroundColor Red
      $failed += $file
    }
    $checked++
  }
}

Write-Host ''
if ($failed.Count -gt 0) {
  Write-Host "FAILED: $($failed.Count) of $checked script(s) would be REJECTED by the browser: $($failed -join ', ')" -ForegroundColor Red
  exit 1
}
Write-Host "OK: all $checked script(s) parse cleanly in their real (module/classic) mode." -ForegroundColor Green
exit 0
