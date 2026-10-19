<#
.SYNOPSIS
  Fast JavaScript grammar check for the NanoClick frontend.

.DESCRIPTION
  Checks EVERY `*.js` file in `src/` with Node in the SAME parse mode the
  browser will use:
    * `type="module"` scripts (found via `<script src>` in `*.html`) are
      checked as `.mjs` (module mode)
    * everything else (classic scripts AND files not referenced by any html)
      is checked as `.js` (script mode)

  WHY THIS EXISTS: `main.js` is loaded as an ES module, and in module mode a
  DUPLICATE top-level declaration is a fatal SyntaxError — the module never
  executes at all, so the window renders while absolutely nothing works (no
  handlers, no IPC, no hotkeys). The very same duplicate is legal in a classic
  script, which is why `node --check main.js` reports "OK" on a dead UI. This
  script and the Rust test `test_frontend_js_syntax_is_valid` are the check
  that was missing.

  WHY EVERY FILE (not just html-referenced): an unreferenced-but-broken file
  is a landmine — the next html that includes it dies silently. Since
  2026-10-14 the gate walks the whole directory; files no html points at are
  reported as `[UNREFERENCED]` (warning, not failure) but still parsed.

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
# Normalize (resolves `..\src` to a full path) so relative-path math below
# (`Substring($SrcDir.Length)`) never overshoots.
$SrcDir = (Resolve-Path -LiteralPath $SrcDir).Path

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
  Write-Host 'Node.js is required for this check (install Node.js, or rely on the Rust test).' -ForegroundColor Yellow
  exit 2
}

# ── Pass 1: map every html-referenced script → parse mode ──────────────
$htmlFiles = Get-ChildItem -Path $SrcDir -Filter *.html -ErrorAction SilentlyContinue
$modeOf = @{}   # file name → 'module' | 'classic'
$referencedBy = @{}  # file name → html name (first referrer, for diagnostics)
foreach ($html in $htmlFiles) {
  $raw = Get-Content -Path $html.FullName -Raw
  foreach ($match in [regex]::Matches($raw, '<script\b(?<attrs>[^>]*)>')) {
    $attrs = $match.Groups['attrs'].Value
    $srcMatch = [regex]::Match($attrs, 'src\s*=\s*(?<q>["''])(?<f>[^"'']+)(\k<q>)')
    if (-not $srcMatch.Success) { continue }
    $file = $srcMatch.Groups['f'].Value
    if (-not $file.EndsWith('.js')) { continue }
    # A bare file name may carry a sub-path (e.g. `js/x.js`): normalize to the
    # leaf so it matches the walk below; sub-dirs are resolved in pass 2.
    $leaf = Split-Path -Leaf $file
    if ($attrs -match 'type\s*=\s*["'']module["'']') {
      $modeOf[$leaf] = 'module'
    } elseif (-not $modeOf.ContainsKey($leaf)) {
      $modeOf[$leaf] = 'classic'
    }
    if (-not $referencedBy.ContainsKey($leaf)) { $referencedBy[$leaf] = $html.Name }
  }
}

# ── Pass 2: walk EVERY *.js (recursive — sub-dirs included) ────────────
$jsFiles = Get-ChildItem -Path $SrcDir -Filter *.js -Recurse -File -ErrorAction SilentlyContinue
if (-not $jsFiles -or $jsFiles.Count -eq 0) {
  Write-Host "No .js files found under $SrcDir" -ForegroundColor Yellow
  exit 2
}

$checked = 0
$unreferenced = @()
$failed = @()

foreach ($js in $jsFiles) {
  $leaf = $js.Name
  $isModule = ($modeOf.ContainsKey($leaf) -and $modeOf[$leaf] -eq 'module')
  $isReferenced = $referencedBy.ContainsKey($leaf)

  # Copy with the extension that forces the right parse mode: .mjs = module.
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("nanoclick_syntax_" + [IO.Path]::GetFileNameWithoutExtension($leaf) + $(if ($isModule) { '.mjs' } else { '.js' }))
  Copy-Item -Path $js.FullName -Destination $tmp -Force

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
  # Relative path for readable output (sub-dir files included).
  $rel = $js.FullName.Substring($SrcDir.Length).TrimStart('\', '/')
  if (-not $isReferenced) { $unreferenced += $rel }
  $tag = if ($isReferenced) { '' } else { ' [UNREFERENCED]' }
  if ($ok) {
    $color = if ($isReferenced) { 'Green' } else { 'DarkCyan' }
    Write-Host "[OK]      $rel ($mode)$tag" -ForegroundColor $color
  } else {
    Write-Host "[SYNTAX]  $rel ($mode)$tag" -ForegroundColor Red
    Write-Host ($out | Out-String).Trim() -ForegroundColor Red
    $failed += $rel
  }
  $checked++
}

Write-Host ''
if ($unreferenced.Count -gt 0) {
  Write-Host "NOTE: $($unreferenced.Count) file(s) are parsed but referenced by no html (dead code or future include): $($unreferenced -join ', ')" -ForegroundColor DarkCyan
}
if ($failed.Count -gt 0) {
  Write-Host "FAILED: $($failed.Count) of $checked script(s) would be REJECTED by the browser: $($failed -join ', ')" -ForegroundColor Red
  exit 1
}
Write-Host "OK: all $checked script(s) parse cleanly in their real (module/classic) mode." -ForegroundColor Green
exit 0
