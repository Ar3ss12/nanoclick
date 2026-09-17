# ── NanoClick All-in-One Release & Publishing Script ────────────────────────
# Usage:
#   Full release (build -> sign -> upload):
#     powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Tag "v1.0.0-beta"
#
#   Interactive release form (opens GUI window to edit notes & choose draft/publish):
#     powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Tag "v1.0.0-beta" -Gui
#
#   Upload only (already built & signed):
#     powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Tag "v1.0.0-beta" -Action "upload"
#
#   Upload as Draft (create on GitHub without publishing immediately):
#     powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Tag "v1.0.0-beta" -Action "upload" -Draft

param(
    [string]$Tag = "v1.0.0-beta",
    [ValidateSet("all", "build", "sign", "upload")][string]$Action = "all",
    [string]$Title,
    [string]$Notes,
    [string]$NotesFile,
    [switch]$Draft,
    [switch]$Prerelease,
    [switch]$Gui,
    [string]$KeyPath = "$env:USERPROFILE\.tauri\nanoclick.key",
    [string]$KeyPassword = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
    [string]$Token = $env:GITHUB_TOKEN,
    [string]$Owner = "Ar3ss12",
    [string]$Repo  = "nanoclick"
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$ver = $Tag.TrimStart("v")
$bundleDir = Join-Path $repoRoot "target\release\bundle\nsis"
$exePath = Join-Path $bundleDir "NanoClick_${ver}_x64-setup.exe"
$sigPath = "$exePath.sig"
$latestJsonPath = Join-Path $bundleDir "latest.json"

if (-not $Title) {
    $Title = "NanoClick $Tag"
}

# Default release notes template (clean, simple, professional)
if (-not $Notes -and $NotesFile -and (Test-Path -LiteralPath $NotesFile)) {
    $Notes = Get-Content -LiteralPath $NotesFile -Raw
}

if (-not $Notes) {
    $Notes = @"
## ⚡ NanoClick $Tag

Welcome to the first public beta release of **NanoClick**!

### Key Highlights
- **Ultra-lightweight & High Performance**: Built with Tauri 2 and pure Rust. Production setup is only ~3.7 MB and runs with ~15 MB RAM usage.
- **Smart Click Engine**: Single, double, and hold click modes with human jitter (timing & cursor randomization) and CPS telemetry up to 160 CPS.
- **Smart Guard & Typing Protection**: Automatically stops clicking when you type text and handles Windows UIPI elevation seamlessly.
- **Macro Recorder & Visual Editor**: Real-time recording with RDP (Ramer-Douglas-Peucker) curve compression and manual block sequencer.
- **Tri-Lingual Interface**: Full Ukrainian, English, and Russian localization.
- **Built-in Auto-Updater**: Cryptographically verified updates via Minisign.

### 📄 License
Released under the **PolyForm Noncommercial License 1.0.0** with NanoClick Additional Terms. Free for personal gaming, streaming, and research. See [LICENSE.md](https://github.com/Ar3ss12/nanoclick/blob/main/LICENSE.md) for complete details.

### 📦 Installation
Download and run `NanoClick_${ver}_x64-setup.exe` below. No administrative privileges required.
"@
}

# ── Optional GUI Form for editing release details ─────────────────────────────
if ($Gui) {
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing

    $form = New-Object System.Windows.Forms.Form
    $form.Text = "NanoClick Release Publisher - $Tag"
    $form.Size = New-Object System.Drawing.Size(640, 560)
    $form.StartPosition = "CenterScreen"
    $form.FormBorderStyle = "FixedDialog"
    $form.MaximizeBox = $false

    # Title label & box
    $lblTitle = New-Object System.Windows.Forms.Label
    $lblTitle.Text = "Release Title:"
    $lblTitle.Location = New-Object System.Drawing.Point(20, 15)
    $lblTitle.Size = New-Object System.Drawing.Size(120, 20)
    $form.Controls.Add($lblTitle)

    $txtTitle = New-Object System.Windows.Forms.TextBox
    $txtTitle.Text = $Title
    $txtTitle.Location = New-Object System.Drawing.Point(20, 38)
    $txtTitle.Size = New-Object System.Drawing.Size(585, 24)
    $form.Controls.Add($txtTitle)

    # Notes label & box
    $lblNotes = New-Object System.Windows.Forms.Label
    $lblNotes.Text = "Release Description (Markdown):"
    $lblNotes.Location = New-Object System.Drawing.Point(20, 72)
    $lblNotes.Size = New-Object System.Drawing.Size(250, 20)
    $form.Controls.Add($lblNotes)

    $txtNotes = New-Object System.Windows.Forms.TextBox
    $txtNotes.Multiline = $true
    $txtNotes.ScrollBars = "Vertical"
    $txtNotes.Text = $Notes
    $txtNotes.Font = New-Object System.Drawing.Font("Consolas", 9.5)
    $txtNotes.Location = New-Object System.Drawing.Point(20, 95)
    $txtNotes.Size = New-Object System.Drawing.Size(585, 330)
    $form.Controls.Add($txtNotes)

    # Draft Checkbox
    $chkDraft = New-Object System.Windows.Forms.CheckBox
    $chkDraft.Text = "Save as Draft (do not publish immediately to public)"
    $chkDraft.Checked = $Draft.IsPresent
    $chkDraft.Location = New-Object System.Drawing.Point(20, 435)
    $chkDraft.Size = New-Object System.Drawing.Size(380, 24)
    $form.Controls.Add($chkDraft)

    # Buttons
    $btnPublish = New-Object System.Windows.Forms.Button
    $btnPublish.Text = "🚀 Publish / Save Release"
    $btnPublish.DialogResult = [System.Windows.Forms.DialogResult]::OK
    $btnPublish.Location = New-Object System.Drawing.Point(400, 468)
    $btnPublish.Size = New-Object System.Drawing.Size(205, 34)
    $btnPublish.BackColor = [System.Drawing.Color]::FromArgb(6, 182, 212)
    $btnPublish.ForeColor = [System.Drawing.Color]::White
    $btnPublish.Font = New-Object System.Drawing.Font("Segoe UI", 9.5, [System.Drawing.FontStyle]::Bold)
    $form.AcceptButton = $btnPublish
    $form.Controls.Add($btnPublish)

    $btnCancel = New-Object System.Windows.Forms.Button
    $btnCancel.Text = "Cancel"
    $btnCancel.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
    $btnCancel.Location = New-Object System.Drawing.Point(300, 468)
    $btnCancel.Size = New-Object System.Drawing.Size(90, 34)
    $form.CancelButton = $btnCancel
    $form.Controls.Add($btnCancel)

    $dialogResult = $form.ShowDialog()
    if ($dialogResult -ne [System.Windows.Forms.DialogResult]::OK) {
        Write-Host "Release publication cancelled by user." -ForegroundColor Yellow
        exit 0
    }

    $Title = $txtTitle.Text.Trim()
    $Notes = $txtNotes.Text.Trim()
    if ($chkDraft.Checked) {
        $Draft = $true
    } else {
        $Draft = $false
    }
}

# Resolve key password if not explicitly passed
if (-not $KeyPassword) {
    $passFile = [System.IO.Path]::ChangeExtension($KeyPath, ".pass")
    if (Test-Path -LiteralPath $passFile) {
        $KeyPassword = (Get-Content -LiteralPath $passFile -Raw).Trim()
    }
}

Write-Host "======================================================" -ForegroundColor Cyan
Write-Host "  NanoClick Release Pipeline - $Tag ($Action)" -ForegroundColor Cyan
Write-Host "======================================================" -ForegroundColor Cyan

# ── 1. BUILD STEP ─────────────────────────────────────────────────────────────
if ($Action -in @("all", "build")) {
    Write-Host "`n[1/3] Building NSIS installer via Tauri..." -ForegroundColor Yellow
    if (-not (Test-Path -LiteralPath $KeyPath)) {
        throw "Private key not found at '$KeyPath'. Generate it first."
    }

    $rawKey = (Get-Content -LiteralPath $KeyPath -Raw).Trim()
    $env:TAURI_SIGNING_PRIVATE_KEY = $rawKey
    if ($KeyPassword) {
        $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $KeyPassword
    }

    Push-Location $repoRoot
    try {
        $env:CARGO_BUILD_JOBS = "2"
        cargo tauri build --bundles nsis
        if ($LASTEXITCODE -ne 0) { throw "cargo tauri build failed (exit code $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
    Write-Host "  -> Build completed successfully!" -ForegroundColor Green
}

# ── 2. SIGN STEP ──────────────────────────────────────────────────────────────
if ($Action -in @("all", "build", "sign")) {
    Write-Host "`n[2/3] Cryptographically signing installer..." -ForegroundColor Yellow
    if (-not (Test-Path -LiteralPath $exePath)) {
        throw "Target executable not found: $exePath"
    }
    if (-not (Test-Path -LiteralPath $KeyPath)) {
        throw "Private key not found: $KeyPath"
    }

    # Clear environment key before calling -f to prevent argument conflict
    Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
    Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue

    # Clean old signature if exists
    if (Test-Path -LiteralPath $sigPath) {
        Remove-Item -LiteralPath $sigPath -Force
    }

    # Execute signing with explicit password argument
    if ($KeyPassword) {
        cargo tauri signer sign -f $KeyPath -p $KeyPassword $exePath
    } else {
        cargo tauri signer sign -f $KeyPath $exePath
    }

    if ($LASTEXITCODE -ne 0 -or (-not (Test-Path -LiteralPath $sigPath))) {
        throw "Signing installer failed (exit code $LASTEXITCODE)"
    }
    Write-Host "  -> Signature generated: $sigPath" -ForegroundColor Green

    # Generate latest.json for Tauri updater v2
    $sigContent = (Get-Content -LiteralPath $sigPath -Raw).Trim()
    $pubDate = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    $downloadUrl = "https://github.com/$Owner/$Repo/releases/download/$Tag/NanoClick_${ver}_x64-setup.exe"

    $manifest = @{
        version   = $ver
        notes     = "NanoClick ${Tag} - high-performance desktop automation engine built with pure Rust."
        pub_date  = $pubDate
        platforms = @{
            "windows-x86_64" = @{
                signature = $sigContent
                url       = $downloadUrl
            }
        }
    }

    [IO.File]::WriteAllText(
        $latestJsonPath,
        ($manifest | ConvertTo-Json -Depth 6),
        [Text.UTF8Encoding]::new($false)
    )
    Write-Host "  -> Generated updater manifest: $latestJsonPath" -ForegroundColor Green
}

# ── 3. UPLOAD STEP ────────────────────────────────────────────────────────────
if ($Action -in @("all", "upload")) {
    Write-Host "`n[3/3] Publishing to GitHub Releases ($Owner/$Repo)..." -ForegroundColor Yellow

    # Try resolving token from git-credential-manager if not passed
    if (-not $Token) {
        if ($env:GITHUB_TOKEN) {
            $Token = $env:GITHUB_TOKEN
        } else {
            try {
                $pyToken = python -c "import subprocess; p=subprocess.Popen(['git','credential','fill'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True); out,_=p.communicate('protocol=https\nhost=github.com\n\n'); print([l[9:] for l in out.splitlines() if l.startswith('password=')][0])" 2>$null
                if ($pyToken) {
                    $Token = $pyToken.Trim()
                    Write-Host "  -> Resolved GitHub Token from Git Credential Manager." -ForegroundColor DarkGray
                }
            } catch {}
        }
    }

    if (-not $Token) {
        $sec = Read-Host -Prompt "Enter GitHub Personal Access Token (repo scope)" -AsSecureString
        $bstr = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($sec)
        $Token = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
        [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
    }

    if (-not $Token) {
        throw "GitHub Token is required to upload release assets."
    }

    $headers = @{
        Authorization          = "token $Token"
        Accept                 = "application/vnd.github+json"
        "X-GitHub-Api-Version" = "2022-11-28"
    }

    $isDraft = [bool]$Draft.IsPresent

    # Find or create release
    $release = $null
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/tags/$Tag" `
                   -Headers $headers -Method Get
        Write-Host "  -> Found existing release: $($release.name) (ID: $($release.id))" -ForegroundColor Cyan

        # Update title/notes if requested
        $updateBody = @{
            name        = $Title
            body        = $Notes
            draft       = $isDraft
            prerelease  = [bool]$Prerelease.IsPresent
            make_latest = if ($Prerelease.IsPresent) { "false" } else { "true" }
        } | ConvertTo-Json

        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/$($release.id)" `
                   -Headers $headers -Method Patch -Body $updateBody
        Write-Host "  -> Updated release details on GitHub." -ForegroundColor Green
    } catch {
        Write-Host "  -> Release $Tag not found on GitHub. Creating release..." -ForegroundColor Yellow
        $body = @{
            tag_name    = $Tag
            name        = $Title
            body        = $Notes
            draft       = $isDraft
            prerelease  = [bool]$Prerelease.IsPresent
            make_latest = if ($Prerelease.IsPresent) { "false" } else { "true" }
        } | ConvertTo-Json

        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases" `
                   -Headers $headers -Method Post -Body $body
        Write-Host "  -> Created release: $($release.name) (ID: $($release.id))" -ForegroundColor Green
    }

    $releaseId = $release.id
    $uploadBase = "https://uploads.github.com/repos/$Owner/$Repo/releases/$releaseId/assets"

    # Clean existing assets with same name to avoid 422 collision
    $assets = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/$releaseId/assets" `
              -Headers $headers -Method Get

    $portableExe = Join-Path $repoRoot "target\release\nanoclick.exe"
    $filesToUpload = @(
        @{ Path = $exePath;        Name = [System.IO.Path]::GetFileName($exePath); Mime = "application/vnd.microsoft.portable-executable" },
        @{ Path = $sigPath;        Name = [System.IO.Path]::GetFileName($sigPath); Mime = "text/plain" },
        @{ Path = $latestJsonPath; Name = "latest.json";                            Mime = "application/json" }
    )
    if (Test-Path -LiteralPath $portableExe) {
        $filesToUpload += @{ Path = $portableExe; Name = "NanoClick-portable.exe"; Mime = "application/vnd.microsoft.portable-executable" }
    }

    foreach ($file in $filesToUpload) {
        $fileName = if ($file.Name) { $file.Name } else { [System.IO.Path]::GetFileName($file.Path) }
        if (-not (Test-Path -LiteralPath $file.Path)) {
            Write-Host "  [SKIP] File missing: $($file.Path)" -ForegroundColor Red
            continue
        }

        foreach ($asset in $assets) {
            if ($asset.name -eq $fileName) {
                Write-Host "  -> Replacing existing asset: $fileName" -ForegroundColor DarkGray
                Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/assets/$($asset.id)" `
                                  -Headers $headers -Method Delete | Out-Null
            }
        }

        $sizeMb = [math]::Round((Get-Item $file.Path).Length / 1MB, 2)
        Write-Host "  -> Uploading $fileName ($sizeMb MB)..." -ForegroundColor Cyan
        $uploadUrl = "${uploadBase}?name=${fileName}"
        $fileBytes = [System.IO.File]::ReadAllBytes($file.Path)

        $uploadHeaders = @{
            Authorization          = "token $Token"
            "Content-Type"         = $file.Mime
            "X-GitHub-Api-Version" = "2022-11-28"
        }

        $null = Invoke-RestMethod -Uri $uploadUrl -Headers $uploadHeaders -Method Post -Body $fileBytes
        Write-Host "  [OK] Uploaded: $fileName" -ForegroundColor Green
    }

    Write-Host "`nRelease status: $(if ($isDraft) {'DRAFT (hidden)'} else {'PUBLISHED (public)'})" -ForegroundColor Green
    Write-Host "Release URL: $($release.html_url)" -ForegroundColor Green
}
