<#
.SYNOPSIS
  Total RAM footprint of an app AND its WebView2 children.

.DESCRIPTION
  A fair comparison needs the WHOLE tree. A Tauri/WebView2 app is one small
  host process plus several `msedgewebview2.exe` children (browser, GPU,
  renderers, utility). Quoting only the host process makes any WebView2 app
  look ~10 MB small - that is exactly how "27 MB" myths are born.

  Use the SAME script on every product you compare, e.g.:
      powershell -ExecutionPolicy Bypass -File scripts\measure-ram.ps1 -Name nanoclick
      powershell -ExecutionPolicy Bypass -File scripts\measure-ram.ps1 -Name BlurAutoClicker

.PARAMETER Name
  Process name with or without .exe (case-insensitive).

.PARAMETER Samples
  How many measurements to take (default 1).

.PARAMETER IntervalMs
  Delay between samples in milliseconds.
#>
param(
  [Parameter(Mandatory = $true)][string]$Name,
  [int]$Samples = 1,
  [int]$IntervalMs = 0
)

function Get-ProcessTree {
  param([string]$rootName)
  $all = Get-CimInstance Win32_Process |
         Select-Object ProcessId, ParentProcessId, Name, WorkingSetSize
  $roots = $all | Where-Object { $_.Name -ieq "$rootName.exe" -or $_.Name -ieq $rootName }
  if (-not $roots) { return @() }

  $ids = New-Object 'System.Collections.Generic.HashSet[int]'
  foreach ($r in $roots) { [void]$ids.Add([int]$r.ProcessId) }

  # Walk down the parent chain until no new descendants appear.
  $changed = $true
  while ($changed) {
    $changed = $false
    foreach ($p in $all) {
      if ($ids.Contains([int]$p.ParentProcessId) -and -not $ids.Contains([int]$p.ProcessId)) {
        [void]$ids.Add([int]$p.ProcessId)
        $changed = $true
      }
    }
  }
  return $all | Where-Object { $ids.Contains([int]$_.ProcessId) }
}

for ($s = 1; $s -le $Samples; $s++) {
  $tree = @(Get-ProcessTree -rootName $Name)
  if ($tree.Count -eq 0) {
    Write-Host "No running process matches '$Name'." -ForegroundColor Yellow
    exit 1
  }

  $total = 0L
  Write-Host ("Sample {0}: process tree for '{1}'" -f $s, $Name)
  foreach ($p in ($tree | Sort-Object WorkingSetSize -Descending)) {
    $mb = [math]::Round($p.WorkingSetSize / 1MB, 2)
    $total += [int64]$p.WorkingSetSize
    Write-Host ("  {0,-22} PID {1,-7} {2,10:N2} MB" -f $p.Name, $p.ProcessId, $mb)
  }
  Write-Host ("  TOTAL: {0} processes, {1:N2} MB" -f $tree.Count, [math]::Round($total / 1MB, 2)) `
    -ForegroundColor Green
  Write-Host ""

  if ($s -lt $Samples -and $IntervalMs -gt 0) { Start-Sleep -Milliseconds $IntervalMs }
}
