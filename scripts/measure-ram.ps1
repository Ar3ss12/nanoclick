<#
.SYNOPSIS
  RAM footprint of an app AND its WebView2 children - measured HONESTLY.

.DESCRIPTION
  Two different numbers exist for the same processes, and mixing them up is how
  "the same app weighs 115 MB" and "weighs 380 MB" appear in one sentence:

  1. TOTAL WORKING SET (`Win32_Process.WorkingSetSize` / `WorkingSet64`) counts
     every resident page - including SHARED pages (system DLLs, GPU buffers and
     Chromium's own `msedge.dll`, ~150 MB of code). Windows loads a shared page
     into physical RAM ONCE and maps it into EVERY process that uses it, so
     summing total working sets across 6 `msedgewebview2.exe` processes counts
     that page 6 times. It is an upper bound, never a comparable figure.
  2. PRIVATE WORKING SET (`Working Set - Private`) counts only pages owned by
     that process. Private pages are disjoint by definition, so the sum over the
     tree is a real cost - and it is exactly what Task Manager's "Memory" column
     on the Processes tab shows.

  This script prints BOTH columns per process and headlines the PRIVATE total,
  because that is the number a human can verify in Task Manager. Use the same
  script on every product you compare:

      powershell -ExecutionPolicy Bypass -File scripts\measure-ram.ps1 -Name nanoclick
      powershell -ExecutionPolicy Bypass -File scripts\measure-ram.ps1 -Name BlurAutoClicker

  Why the whole tree matters: a Tauri/WebView2 app is one small host process plus
  several `msedgewebview2.exe` children (browser, GPU, renderer, network, ...
  ). Quoting only the host makes any WebView2 app look tiny; quoting only the
  inflated total makes it look monstrous.

.PARAMETER Name
  Process name with or without .exe (case-insensitive).

.PARAMETER Samples
  How many measurements to take (default 1). With more than one, a min/avg/max
  summary for the private total is printed at the end.

.PARAMETER IntervalMs
  Delay between samples in milliseconds.

.PARAMETER SortByTotal
  Sort the table by the inflated total working set instead of the private one
  (only changes the order of the rows).
#>
param(
  [Parameter(Mandatory = $true)][string]$Name,
  [int]$Samples = 1,
  [int]$IntervalMs = 0,
  [switch]$SortByTotal
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

# Private Working Set per PID. Primary source is the performance counter class
# (`WorkingSetPrivate`, the value Task Manager shows); the fallback is the
# process object's private COMMIT, which is a different (larger) metric and is
# labelled as such instead of silently pretending to be the same thing.
function Get-PrivateWorkingSet {
  param([int[]]$ProcessIds)
  $map = @{}
  $fromCounter = $false
  try {
    $perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -ErrorAction Stop |
            Select-Object IDProcess, WorkingSetPrivate
    foreach ($row in $perf) {
      $pid_ = [int]$row.IDProcess
      if ($ProcessIds -contains $pid_) {
        $map[$pid_] = [int64]$row.WorkingSetPrivate
        $fromCounter = $true
      }
    }
  } catch {
    $fromCounter = $false
  }
  if (-not $fromCounter) {
    Write-Host "  [warn] perf counters unavailable; showing private COMMIT instead of private working set" -ForegroundColor Yellow
    foreach ($id in $ProcessIds) {
      try {
        $proc = Get-Process -Id $id -ErrorAction Stop
        $map[$id] = [int64]$proc.PrivateMemorySize64
      } catch { }
    }
    return @{ Map = $map; Exact = $false }
  }
  return @{ Map = $map; Exact = $true }
}

# ── Reporting ────────────────────────────────────────────────────────────
# Headline = PRIVATE total (comparable with Task Manager, no page counted
# twice). The sum of total working sets is printed next to it, labelled as an
# upper bound, so nobody mistakes it for the app's real cost.
$privateSamples = @()

for ($s = 1; $s -le $Samples; $s++) {
  $tree = @(Get-ProcessTree -rootName $Name)
  if ($tree.Count -eq 0) {
    Write-Host "No running process matches '$Name'." -ForegroundColor Yellow
    exit 1
  }

  $pids = @($tree | ForEach-Object { [int]$_.ProcessId })
  $private = Get-PrivateWorkingSet -ProcessIds $pids
  $exact = [bool]$private.Exact
  $map = $private.Map

  # @(...) around both: a single-element `foreach`/pipeline result is unwrapped
  # by PowerShell, and a bare object has no .Count (it printed "  processes, ...").
  $rows = @(foreach ($p in $tree) {
    $id = [int]$p.ProcessId
    [pscustomobject]@{
      Name    = $p.Name
      Pid     = $id
      Private = [int64]$(if ($map.ContainsKey($id)) { $map[$id] } else { 0 })
      Total   = [int64]$p.WorkingSetSize
    }
  })
  if ($SortByTotal) { $rows = @($rows | Sort-Object Total -Descending) }
  else { $rows = @($rows | Sort-Object Private -Descending) }

  $privTotal = [int64]($rows | Measure-Object Private -Sum).Sum
  $allTotal = [int64]($rows | Measure-Object Total -Sum).Sum
  $privateSamples += $privTotal

  Write-Host ("Sample {0}: process tree for '{1}'" -f $s, $Name)
  Write-Host ("  {0,-28} {1,-8} {2,12} {3,12}" -f "Process", "PID", "Private WS", "Total WS")
  foreach ($r in $rows) {
    Write-Host ("  {0,-28} {1,-8} {2,9:N2} MB {3,9:N2} MB" -f `
      $r.Name, $r.Pid, [math]::Round($r.Private / 1MB, 2), [math]::Round($r.Total / 1MB, 2))
  }
  $label = if ($exact) { "PRIVATE total (Task Manager 'Memory')" } else { "PRIVATE COMMIT total (fallback)" }
  Write-Host ("  {0}: {1} processes, {2:N2} MB" -f $label, $rows.Count, [math]::Round($privTotal / 1MB, 2)) `
    -ForegroundColor Green
  Write-Host ("  sum of TOTAL working sets: {0:N2} MB - upper bound only, shared pages (msedge.dll, system DLLs) are counted once per process" `
    -f [math]::Round($allTotal / 1MB, 2)) -ForegroundColor DarkGray
  Write-Host ""

  if ($s -lt $Samples -and $IntervalMs -gt 0) { Start-Sleep -Milliseconds $IntervalMs }
}

if ($Samples -gt 1) {
  $m = $privateSamples | Measure-Object -Minimum -Maximum -Average
  Write-Host ("PRIVATE total over {0} samples: min {1:N2} MB / avg {2:N2} MB / max {3:N2} MB" -f `
    $Samples, ($m.Minimum / 1MB), ($m.Average / 1MB), ($m.Maximum / 1MB)) -ForegroundColor Cyan
}
