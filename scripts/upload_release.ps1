# ── GitHub Release Uploader для NanoClick v1.2.1 ──────────────────────────
# Завантажує три артефакти релізу на GitHub Release через REST API.
# Запуск: powershell -ExecutionPolicy Bypass -File scripts\upload_release.ps1
# Потрібен GitHub Personal Access Token з правами "repo".

param(
    [string]$Token = $env:GITHUB_TOKEN
)

$OWNER   = "Ar3ss12"
$REPO    = "nanoclick"
$TAG     = "v1.2.1"
$BUNDLE  = "$PSScriptRoot\..\target\release\bundle\nsis"

$FILES   = @(
    "$BUNDLE\NanoClick_1.2.1_x64-setup.exe",
    "$BUNDLE\NanoClick_1.2.1_x64-setup.exe.sig",
    "$BUNDLE\latest.json"
)

# ── Запит токена якщо не переданий ────────────────────────────────────────
if (-not $Token) {
    $secureToken = Read-Host -Prompt "GitHub Personal Access Token (repo scope)" -AsSecureString
    $bstr  = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($secureToken)
    $Token = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
    [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
}

$headers = @{
    Authorization = "token $Token"
    Accept        = "application/vnd.github+json"
    "X-GitHub-Api-Version" = "2022-11-28"
}

# ── 1. Отримати release_id по тегу ────────────────────────────────────────
Write-Host "`n[1/4] Шукаємо реліз $TAG ..." -ForegroundColor Cyan
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$OWNER/$REPO/releases/tags/$TAG" `
               -Headers $headers -Method Get
} catch {
    Write-Host "[ERROR] Реліз $TAG не знайдено або токен без прав: $_" -ForegroundColor Red
    exit 1
}

$releaseId = $release.id
$uploadUrl = $release.upload_url -replace '\{\?name,label\}', ''
Write-Host "  → release_id=$releaseId" -ForegroundColor Green

# ── 2. Видалити старі assets з тим самим ім'ям (щоб уникнути 422 Duplicate) ──
Write-Host "`n[2/4] Видаляємо старі assets ..." -ForegroundColor Cyan
$existingAssets = Invoke-RestMethod -Uri "https://api.github.com/repos/$OWNER/$REPO/releases/$releaseId/assets" `
                 -Headers $headers -Method Get

foreach ($asset in $existingAssets) {
    $targetNames = ($FILES | ForEach-Object { [System.IO.Path]::GetFileName($_) })
    if ($asset.name -in $targetNames) {
        Write-Host "  → Видаляємо старий: $($asset.name)" -ForegroundColor Yellow
        Invoke-RestMethod -Uri "https://api.github.com/repos/$OWNER/$REPO/releases/assets/$($asset.id)" `
                          -Headers $headers -Method Delete | Out-Null
    }
}

# ── 3. Завантажити нові файли ─────────────────────────────────────────────
Write-Host "`n[3/4] Завантажуємо нові артефакти ..." -ForegroundColor Cyan
foreach ($filePath in $FILES) {
    $fileName = [System.IO.Path]::GetFileName($filePath)
    if (-not (Test-Path $filePath)) {
        Write-Host "  [SKIP] Файл не знайдено: $filePath" -ForegroundColor Red
        continue
    }

    $contentType = switch -Wildcard ($fileName) {
        "*.exe"  { "application/octet-stream" }
        "*.sig"  { "text/plain" }
        "*.json" { "application/json" }
        default  { "application/octet-stream" }
    }

    $fileBytes = [System.IO.File]::ReadAllBytes($filePath)
    $uploadUri = "${uploadUrl}?name=${fileName}"

    Write-Host "  → $fileName ($([math]::Round($fileBytes.Length / 1KB, 1)) KB) ..." -NoNewline

    try {
        $uploadHeaders = $headers.Clone()
        $uploadHeaders["Content-Type"] = $contentType
        Invoke-RestMethod -Uri $uploadUri -Headers $uploadHeaders `
                          -Method Post -Body $fileBytes | Out-Null
        Write-Host " ✅" -ForegroundColor Green
    } catch {
        Write-Host " ❌ $_" -ForegroundColor Red
    }
}

# ── 4. Результат ─────────────────────────────────────────────────────────
Write-Host "`n[4/4] Готово!" -ForegroundColor Cyan
Write-Host "  Release: https://github.com/$OWNER/$REPO/releases/tag/$TAG" -ForegroundColor White
