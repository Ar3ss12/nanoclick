# ── GitHub Release Uploader для NanoClick ───────────────────────────────────
# Завантажує артефакти релізу на GitHub Release через REST API.
# Запуск: powershell -ExecutionPolicy Bypass -File scripts\upload_release.ps1 -Tag "v1.0.0-beta"
# Потрібен GitHub Personal Access Token з правами "repo".

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

# Збираємо список файлів для завантаження
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
    Write-StepWarn "Портативний бінарник не знайдено ($PORTABLE); завантажується лише NSIS інсталятор."
}

# ── 1. Авторизація та пошук релізу ──────────────────────────────────────────
Write-StageHeader "1" "4" "AUTH & QUERY - Пошук та валідація релізу $TAG"

if (-not $Token) {
    Write-StepLog "Шукаємо токен у файлах конфігурації (.env)..."
    foreach ($envPath in @("$PSScriptRoot\.env", "$repoRoot\.env")) {
        if (Test-Path -LiteralPath $envPath) {
            Get-Content -LiteralPath $envPath | ForEach-Object {
                if ($_ -match '^\s*GITHUB_TOKEN\s*=\s*(.+)$') {
                    $Token = $matches[1].Trim()
                }
            }
            if ($Token) { 
                Write-StepDone "Токен зчитано з $envPath"
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
            Write-StepDone "Токен знайдено у Git Credential Manager"
        }
    } catch {}
}

if (-not $Token) {
    Write-StepWarn "Токен не знайдено автоматично. Введіть токен вручну:"
    $secureToken = Read-Host -Prompt "GitHub Personal Access Token (repo scope)" -AsSecureString
    $bstr  = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($secureToken)
    $Token = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
    [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
}

if (-not $Token) {
    Write-StepError "Токен GitHub обов'язковий для виконання операції."
    throw "GitHub Token is required to upload release assets."
}

$headers = @{
    Authorization          = "token $Token"
    Accept                 = "application/vnd.github+json"
    "X-GitHub-Api-Version" = "2022-11-28"
}

Write-StepLog "Перевіряємо існування релізу $TAG на GitHub..."
$release = $null
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$OWNER/$REPO/releases/tags/$TAG" `
               -Headers $headers -Method Get
    Write-StepDone "Знайдено існуючий реліз" "ID: $($release.id), Назва: '$($release.name)'"
} catch {
    Write-StepWarn "Реліз $TAG ще не існує. Створюємо новий реліз на GitHub..."
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
        Write-StepDone "Створено новий публічний реліз" "ID: $($release.id)"
    } catch {
        Write-StepError "Помилка при створенні релізу: $($_.Exception.Message)"
        throw $_
    }
}

$releaseId = $release.id
$uploadBase = "https://uploads.github.com/repos/$OWNER/$REPO/releases/$releaseId/assets"

# ── 2. Очищення старих ассетів (запобігання колізії 422 Unprocessable) ────────
Write-StageHeader "2" "4" "CLEANUP - Перевірка та видалення дублікатів ассетів"

Write-StepLog "Отримуємо список уже завантажених ассетів для release_id=$releaseId..."
$existingAssets = Invoke-RestMethod -Uri "https://api.github.com/repos/$OWNER/$REPO/releases/$releaseId/assets" `
                 -Headers $headers -Method Get
Write-StepInfo "Знайдено існуючих ассетів" "$($existingAssets.Count) шт."

$targetNames = ($FILES | ForEach-Object { $_.Name })
$cleanedCount = 0

foreach ($asset in $existingAssets) {
    if ($asset.name -in $targetNames) {
        Write-StepLog "Видалення застарілого файлу з GitHub" "$($asset.name) (Asset ID: $($asset.id))"
        Invoke-RestMethod -Uri "https://api.github.com/repos/$OWNER/$REPO/releases/assets/$($asset.id)" `
                          -Headers $headers -Method Delete | Out-Null
        Write-StepDone "Видалено" "$($asset.name)"
        $cleanedCount++
    }
}
if ($cleanedCount -eq 0) {
    Write-StepInfo "Очищення" "Дублікатів не виявлено. Усі слоти готові до чистого завантаження."
}

# ── 3. Завантаження нових файлів ─────────────────────────────────────────────
Write-StageHeader "3" "4" "UPLOAD - Потокове завантаження артефактів"

$uploadIndex = 0
$totalUploads = $FILES.Count

foreach ($item in $FILES) {
    $uploadIndex++
    $filePath = $item.Path
    $fileName = $item.Name

    if (-not (Test-Path -LiteralPath $filePath)) {
        Write-StepWarn "[$uploadIndex/$totalUploads] Файл не знайдено локально: $filePath (ПРОПУСК)"
        continue
    }

    $fileSize = (Get-Item $filePath).Length
    $sizeFormatted = if ($fileSize -gt 1MB) {
        "$([math]::Round($fileSize / 1MB, 2)) MB"
    } else {
        "$([math]::Round($fileSize / 1KB, 1)) KB"
    }

    Write-StepLog "[$uploadIndex/$totalUploads] Завантажуємо $fileName ($sizeFormatted)..." "Передача даних на GitHub"
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
        Write-StepDone "[$uploadIndex/$totalUploads] Завантажено $fileName ($sizeFormatted)" "Час: ${fileElapsed}с"
    } catch {
        $fileTimer.Stop()
        Write-StepError "[$uploadIndex/$totalUploads] Помилка завантаження $fileName : $($_.Exception.Message)"
        throw $_
    }
}

# ── 4. Підсумок релізу ──────────────────────────────────────────────────────
$Stopwatch.Stop()
$totalElapsed = [math]::Round($Stopwatch.Elapsed.TotalSeconds, 1)

Write-StageHeader "4" "4" "COMPLETE - Реліз успішно опубліковано!"

Write-Host ""
Write-Host "╔════════════════════════════════════════════════════════════════╗" -ForegroundColor Green
Write-Host "║              РЕЗУЛЬТАТ ЗАВАНТАЖЕННЯ АРТЕФАКТІВ                 ║" -ForegroundColor Green
Write-Host "╠════════════════════════════════════════════════════════════════╣" -ForegroundColor Green
Write-Host "║  Тег версії  : $($TAG.PadRight(47)) ║" -ForegroundColor White
Write-Host "║  Загальний час: $(("${totalElapsed} с").PadRight(46)) ║" -ForegroundColor White
Write-Host "║  Посилання   : $($release.html_url.PadRight(47)) ║" -ForegroundColor Cyan
Write-Host "╚════════════════════════════════════════════════════════════════╝" -ForegroundColor Green
