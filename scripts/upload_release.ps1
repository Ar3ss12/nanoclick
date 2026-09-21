# ── GitHub Release Uploader for NanoClick ──────────────────────────────────
# Uploads release artifacts to GitHub Release via REST API.
# Run: powershell -ExecutionPolicy Bypass -File scripts\upload_release.ps1 -Tag "v1.0.0-beta"
# Requires a GitHub Personal Access Token with "repo" scope.

param(
    [string]$Token = $env:GITHUB_TOKEN,
    [string]$Tag = "v1.0.0-beta"
)

$ErrorActionPreference = "Stop"
$Stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

$OWNER    = "Ar3ss12"
$REPO     = "nanoclick"
$TAG      = $Tag
$VER      = $Tag.TrimStart("v")
$repoRoot = Split-Path -Parent $PSScriptRoot
$BUNDLE   = Join-Path $repoRoot "target\release\bundle\nsis"
$PORTABLE = Join-Path $repoRoot "target\release\nanoclick.exe"

# ── Structured Logging Helpers ────────────────────────────────────────────────
function Get-Timestamp {
    return (Get-Date -Format "HH:mm:ss")
}

function Write-StageHeader([string]$stageNum, [string]$totalStages, [string]$stageName) {
    $time = Get-Timestamp
    Write-Host ""
    Write-Host "==================================================================" -ForegroundColor Cyan
    Write-Host " [$time] [STAGE $stageNum/$totalStages] $stageName" -ForegroundColor Cyan
    Write-Host "==================================================================" -ForegroundColor Cyan
}

function Write-StepLog([string]$action, [string]$detail = "") {
    $time = Get-Timestamp
    if ($detail) {
        Write-Host " [$time] ⏳ ${action}: " -NoNewline -ForegroundColor Yellow
        Write-Host $detail -ForegroundColor Gray
    } else {
        Write-Host " [$time] ⏳ $action" -ForegroundColor Yellow
    }
}

function Write-StepDone([string]$msg, [string]$detail = "") {
    $time = Get-Timestamp
    if ($detail) {
        Write-Host " [$time] ✅ $msg " -NoNewline -ForegroundColor Green
        Write-Host "($detail)" -ForegroundColor DarkGray
    } else {
        Write-Host " [$time] ✅ $msg" -ForegroundColor Green
    }
}

function Write-StepInfo([string]$label, [string]$info) {
    $time = Get-Timestamp
    Write-Host " [$time]    ℹ️  ${label}: " -NoNewline -ForegroundColor DarkCyan
    Write-Host $info -ForegroundColor White
}

function Write-StepWarn([string]$msg) {
    $time = Get-Timestamp
    Write-Host " [$time] ⚠️  $msg" -ForegroundColor DarkYellow
}

function Write-StepError([string]$msg) {
    $time = Get-Timestamp
    Write-Host " [$time] ❌ $msg" -ForegroundColor Red
}

Write-Host ""
Write-Host "╔════════════════════════════════════════════════════════════════╗" -ForegroundColor Cyan
Write-Host "║              NanoClick Asset Release Uploader                  ║" -ForegroundColor Cyan
Write-Host "╠════════════════════════════════════════════════════════════════╣" -ForegroundColor Cyan
Write-Host "║  Target Version : $($TAG.PadRight(46)) ║" -ForegroundColor White
Write-Host "║  Repository     : $("$OWNER/$REPO".PadRight(46)) ║" -ForegroundColor White
Write-Host "╚════════════════════════════════════════════════════════════════╝" -ForegroundColor Cyan

# Collect the files to upload
$setupExe = Join-Path $BUNDLE "NanoClick_${VER}_x64-setup.exe"
$setupSig = Join-Path $BUNDLE "NanoClick_${VER}_x64-setup.exe.sig"
$latestJson = Join-Path $BUNDLE "latest.json"

$FILES = @(
    @{ Path = $setupExe;   Name = "NanoClick_${VER}_x64-setup.exe"; Mime = "application/vnd.microsoft.portable-executable" },
    @{ Path = $setupSig;   Name = "NanoClick_${VER}_x64-setup.exe.sig"; Mime = "text/plain" },
    @{ Path = $latestJson; Name = "latest.json"; Mime = "application/json" }
)

if (Test-Path -LiteralPath $PORTABLE) {
    $FILES += @{ Path = $PORTABLE; Name = "NanoClick-portable.exe"; Mime = "application/vnd.microsoft.portable-executable" }
} else {
    Write-StepWarn "Portable binary not found ($PORTABLE); uploading the NSIS installer only."
}

# ── 1. Auth and release lookup ──────────────────────────────────────────
Write-StageHeader "1" "4" "AUTH & QUERY - Finding and validating release $TAG"

if (-not $Token) {
    Write-StepLog "Looking for a token in the configuration files (.env)..."
    foreach ($envPath in @("$PSScriptRoot\.env", "$repoRoot\.env")) {
        if (Test-Path -LiteralPath $envPath) {
            Get-Content -LiteralPath $envPath | ForEach-Object {
                if ($_ -match '^\s*GITHUB_TOKEN\s*=\s*(.+)$') {
                    $Token = $matches[1].Trim()
                }
            }
            if ($Token) { 
                Write-StepDone "Token read from $envPath"
                break 
            }
        }
    }
}

if (-not $Token) {
    try {
        $pyToken = python -c "import subprocess; p=subprocess.Popen(['git','credential','fill'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True); out,_=p.communicate('protocol=https\nhost=github.com\n\n'); print([l[9:] for l in out.splitlines() if l.startswith('password=')][0])" 2>$null
        if ($pyToken) {
            $Token = $pyToken.Trim()
            Write-StepDone "Token found in Windows Credential Manager"
        }
    } catch {}
}

if (-not $Token) {
    Write-StepWarn "No token found automatically. Enter the token manually:"
    $secureToken = Read-Host -Prompt "GitHub Personal Access Token (repo scope)" -AsSecureString
    $bstr  = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($secureToken)
    $Token = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
    [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
}

if (-not $Token) {
    Write-StepError "A GitHub token is mandatory for this operation."
    throw "GitHub Token is required to upload release assets."
}

$headers = @{
    Authorization          = "token $Token"
    Accept                 = "application/vnd.github+json"
    "X-GitHub-Api-Version" = "2022-11-28"
}

Write-StepLog "Checking whether release $TAG exists on GitHub..."
$release = $null
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$OWNER/$REPO/releases/tags/$TAG" `
               -Headers $headers -Method Get
    Write-StepDone "Found the existing release" "ID: $($release.id), Name: '$($release.name)'"
} catch {
    Write-StepWarn "Release $TAG does not exist yet. Creating a new release on GitHub..."
    $createBody = @{
        tag_name   = $TAG
        name       = "NanoClick $TAG"
        draft       = $false
        prerelease  = $false
        make_latest = "true"
    } | ConvertTo-Json
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$OWNER/$REPO/releases" `
                   -Headers $headers -Method Post -Body $createBody
        Write-StepDone "Created a new public release" "ID: $($release.id)"
    } catch {
        Write-StepError "Failed to create the release: $($_.Exception.Message)"
        throw $_
    }
}

$releaseId = $release.id
$uploadBase = "https://uploads.github.com/repos/$OWNER/$REPO/releases/$releaseId/assets"

# ── 2. Cleaning up old assets (preventing 422 Unprocessable collisions) ────────
Write-StageHeader "2" "4" "CLEANUP - Check and delete duplicate assets"

Write-StepLog "Fetching the already uploaded assets for release_id=$releaseId..."
$existingAssets = Invoke-RestMethod -Uri "https://api.github.com/repos/$OWNER/$REPO/releases/$releaseId/assets" `
                 -Headers $headers -Method Get
Write-StepInfo "Existing assets found" "$($existingAssets.Count) items."

$targetNames = ($FILES | ForEach-Object { $_.Name })
$cleanedCount = 0

foreach ($asset in $existingAssets) {
    if ($asset.name -in $targetNames) {
        Write-StepLog "Deleting the stale file from GitHub" "$($asset.name) (Asset ID: $($asset.id))"
        Invoke-RestMethod -Uri "https://api.github.com/repos/$OWNER/$REPO/releases/assets/$($asset.id)" `
                          -Headers $headers -Method Delete | Out-Null
        Write-StepDone "Deleted" "$($asset.name)"
        $cleanedCount++
    }
}
if ($cleanedCount -eq 0) {
    Write-StepInfo "Cleanup" "No duplicates detected. All slots are ready for a clean upload."
}

# ── 3. Uploading the new files ─────────────────────────────────────────────
Write-StageHeader "3" "4" "UPLOAD - Streaming artifact upload"

$uploadIndex = 0
$totalUploads = $FILES.Count

foreach ($item in $FILES) {
    $uploadIndex++
    $filePath = $item.Path
    $fileName = $item.Name

    if (-not (Test-Path -LiteralPath $filePath)) {
        Write-StepWarn "[$uploadIndex/$totalUploads] File not found locally: $filePath (SKIP)"
        continue
    }

    $fileSize = (Get-Item $filePath).Length
    $sizeFormatted = if ($fileSize -gt 1MB) {
        "$([math]::Round($fileSize / 1MB, 2)) MB"
    } else {
        "$([math]::Round($fileSize / 1KB, 1)) KB"
    }

    Write-StepLog "[$uploadIndex/$totalUploads] Uploading $fileName ($sizeFormatted)..." "Streaming to GitHub"
    $fileTimer = [System.Diagnostics.Stopwatch]::StartNew()

    $fileBytes = [System.IO.File]::ReadAllBytes($filePath)
    $uploadUri = "${uploadBase}?name=${fileName}"

    try {
        $uploadHeaders = @{
            Authorization          = "token $Token"
            "Content-Type"         = $item.Mime
            "X-GitHub-Api-Version" = "2022-11-28"
        }
        $null = Invoke-RestMethod -Uri $uploadUri -Headers $uploadHeaders `
                                  -Method Post -Body $fileBytes
        $fileTimer.Stop()
        $fileElapsed = [math]::Round($fileTimer.Elapsed.TotalSeconds, 1)
        Write-StepDone "[$uploadIndex/$totalUploads] Uploaded $fileName ($sizeFormatted)" "Time: ${fileElapsed}s"
    } catch {
        $fileTimer.Stop()
        Write-StepError "[$uploadIndex/$totalUploads] Upload failed $fileName : $($_.Exception.Message)"
        throw $_
    }
}

# ── 4. Release summary ──────────────────────────────────────────────────────
$Stopwatch.Stop()
$totalElapsed = [math]::Round($Stopwatch.Elapsed.TotalSeconds, 1)

Write-StageHeader "4" "4" "COMPLETE - Release published successfully!"

Write-Host ""
Write-Host "╔════════════════════════════════════════════════════════════════╗" -ForegroundColor Green
Write-Host "║              ARTIFACT UPLOAD RESULT                 ║" -ForegroundColor Green
Write-Host "╠════════════════════════════════════════════════════════════════╣" -ForegroundColor Green
Write-Host "║  Version tag  : $($TAG.PadRight(47)) ║" -ForegroundColor White
Write-Host "║  Total time: $(("${totalElapsed} s").PadRight(46)) ║" -ForegroundColor White
Write-Host "║  URL   : $($release.html_url.PadRight(47)) ║" -ForegroundColor Cyan
Write-Host "╚════════════════════════════════════════════════════════════════╝" -ForegroundColor Green
