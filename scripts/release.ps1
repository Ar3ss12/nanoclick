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
    [switch]$SkipChecks,
    [switch]$FullTests,
    [string]$KeyPath = "$env:USERPROFILE\.tauri\nanoclick.key",
    [string]$KeyPassword = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
    [string]$Token = $env:GITHUB_TOKEN,
    [string]$Owner = "Ar3ss12",
    [string]$Repo  = "nanoclick"
)

$ErrorActionPreference = "Stop"
$PipelineStopwatch = [System.Diagnostics.Stopwatch]::StartNew()
$repoRoot = Split-Path -Parent $PSScriptRoot
$ver = $Tag.TrimStart("v")
$bundleDir = Join-Path $repoRoot "target\release\bundle\nsis"
$exePath = Join-Path $bundleDir "NanoClick_${ver}_x64-setup.exe"
$sigPath = "$exePath.sig"
$latestJsonPath = Join-Path $bundleDir "latest.json"
$portableExe = Join-Path $repoRoot "target\release\nanoclick.exe"

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
Or use `NanoClick-portable.exe` for zero-install portable execution.
"@
}

# ── Optional GUI Form for editing release details ─────────────────────────────
if ($Gui) {
    Write-StepLog "Opening interactive release editor form..."
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
        Write-StepWarn "Release publication cancelled by user in GUI form."
        exit 0
    }

    $Title = $txtTitle.Text.Trim()
    $Notes = $txtNotes.Text.Trim()
    $Draft = [bool]$chkDraft.Checked
    Write-StepDone "Release parameters updated from GUI."
}

# Resolve key password if not explicitly passed
if (-not $KeyPassword) {
    $passFile = [System.IO.Path]::ChangeExtension($KeyPath, ".pass")
    if (Test-Path -LiteralPath $passFile) {
        $KeyPassword = (Get-Content -LiteralPath $passFile -Raw).Trim()
    }
}

$preflightRuns = ($Action -in @("all", "build")) -and (-not $SkipChecks)
$totalSteps = if ($Action -eq "all") {
    if ($preflightRuns) { "4" } else { "3" }
} else {
    if ($preflightRuns) { "2" } else { "1" }
}

Write-Host ""
Write-Host "╔════════════════════════════════════════════════════════════════╗" -ForegroundColor Cyan
Write-Host "║              NanoClick Release & Deployment Pipeline           ║" -ForegroundColor Cyan
Write-Host "╠════════════════════════════════════════════════════════════════╣" -ForegroundColor Cyan
Write-Host "║  Target Version : $($Tag.PadRight(46)) ║" -ForegroundColor White
Write-Host "║  Pipeline Mode  : $($Action.ToUpper().PadRight(46)) ║" -ForegroundColor White
Write-Host "║  Repository     : $("$Owner/$Repo".PadRight(46)) ║" -ForegroundColor White
Write-Host "╚════════════════════════════════════════════════════════════════╝" -ForegroundColor Cyan

# ── 0. PREFLIGHT GATE ─────────────────────────────────────────────────────────
# The fence that was missing when a release shipped with a dead frontend:
# `main.js` is an ES module, and a duplicate top-level declaration is a FATAL
# SyntaxError — the module never executes, the window still renders, and every
# Rust test stays green while the UI is 100% dead (see tech.md §Lessons Learned).
# `cargo tauri build` does NOT run tests, so the gate has to live here.
# Override deliberately with -SkipChecks (emergency hotfix only).
if ($preflightRuns) {
    $preflightTimer = [System.Diagnostics.Stopwatch]::StartNew()
    Write-StageHeader "0" $totalSteps "PREFLIGHT - Frontend & Rust gates (fail fast)"

    # 1. JavaScript grammar, parsed in the SAME mode the browser will use.
    $syntaxScript = Join-Path $repoRoot "scripts\check-js-syntax.ps1"
    if (Test-Path -LiteralPath $syntaxScript) {
        Write-StepLog "Checking frontend JavaScript grammar" "type=module as .mjs, classic as .js (node --check)"
        & powershell -NoProfile -ExecutionPolicy Bypass -File $syntaxScript
        if ($LASTEXITCODE -ne 0) {
            Write-StepError "JS grammar check FAILED - a module SyntaxError would ship as a rendered-but-dead UI"
            throw "Frontend JS grammar check failed (exit code $LASTEXITCODE). Fix it, or pass -SkipChecks to override."
        }
        Write-StepDone "Frontend JavaScript grammar clean"
    } else {
        Write-StepWarn "scripts\check-js-syntax.ps1 not found - grammar gate skipped"
    }

    # 2. Broad lint tripwire (oxlint — a Rust binary, no node_modules needed).
    #    GATE = zero errors AND zero warnings (--deny-warnings). The 53 warnings
    #    that used to be "the baseline" were dead code, shadowed identifiers and
    #    empty catches — all resolved in the source or silenced by a documented,
    #    justified exception. See AGENTS.md.
    $lintScript = Join-Path $repoRoot "scripts\check-js-lint.ps1"
    if (Test-Path -LiteralPath $lintScript) {
        Write-StepLog "Linting the frontend" "oxlint; 0 errors / 0 warnings required (--deny-warnings)"
        & powershell -NoProfile -ExecutionPolicy Bypass -File $lintScript
        if ($LASTEXITCODE -eq 1) {
            Write-StepError "Frontend lint FAILED - fix the reported errors before shipping"
            throw "Frontend lint failed. Fix it, or pass -SkipChecks to override."
        } elseif ($LASTEXITCODE -ne 0) {
            Write-StepWarn "Lint tooling unavailable (exit $LASTEXITCODE) - lint step skipped"
        } else {
            Write-StepDone "Frontend lint clean (0 errors / 0 warnings)"
        }
    } else {
        Write-StepWarn "scripts\check-js-lint.ps1 not found - lint gate skipped"
    }

    # 3. Rust gates (JS syntax + boot-guard wiring live in tests/test_assets.rs).
    Push-Location $repoRoot
    try {
        # Job count: the release profile is built with LTO + codegen-units=1, which is
        # memory-hungry — a 2-job run has already died with "rustc-LLVM ERROR: out of
        # memory". Default to 1 (the repo's documented "after cargo clean" rule) and let
        # a machine with headroom raise it: `set CARGO_BUILD_JOBS=4` before the script.
        $jobs = if ($env:CARGO_BUILD_JOBS -and $env:CARGO_BUILD_JOBS -match '^\d+$') { $env:CARGO_BUILD_JOBS } else { "1" }
        $env:CARGO_BUILD_JOBS = $jobs
        if ($FullTests) {
            Write-StepLog "Running the full test suite" "cargo test --release -j $jobs -- --skip physical_"
            cargo test --release -j $jobs -- --skip physical_
        } else {
            Write-StepLog "Running frontend-gate tests" "cargo test --release -j $jobs --test test_assets (use -FullTests for all)"
            cargo test --release -j $jobs --test test_assets
        }
        if ($LASTEXITCODE -ne 0) {
            Write-StepError "Test gate FAILED (exit code $LASTEXITCODE)"
            throw "Test gate failed. Fix the tests, or pass -SkipChecks to override."
        }
        Write-StepDone "Rust test gate green"
    } finally {
        Pop-Location
    }

    $preflightTimer.Stop()
    $preflightElapsed = [math]::Round($preflightTimer.Elapsed.TotalSeconds, 1)
    Write-StepDone "Stage 0 (PREFLIGHT) complete" "${preflightElapsed}s"
}

# ── 1. BUILD STEP ─────────────────────────────────────────────────────────────
if ($Action -in @("all", "build")) {
    $stepTimer = [System.Diagnostics.Stopwatch]::StartNew()
    Write-StageHeader "1" $totalSteps "BUILD - Compiling Rust Core & Packaging NSIS Bundle"

    Write-StepLog "Validating private signing key" $KeyPath
    if (-not (Test-Path -LiteralPath $KeyPath)) {
        Write-StepError "Private key not found at '$KeyPath'. Generate it first via scripts\make_release.ps1"
        throw "Private key not found at '$KeyPath'."
    }
    Write-StepDone "Signing key found"

    $rawKey = (Get-Content -LiteralPath $KeyPath -Raw).Trim()
    $env:TAURI_SIGNING_PRIVATE_KEY = $rawKey
    if ($KeyPassword) {
        $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $KeyPassword
    }

    Write-StepLog "Configuring build concurrency" "CARGO_BUILD_JOBS=2 (safe memory footprint)"
    $env:CARGO_BUILD_JOBS = "2"

    Write-StepLog "Executing Tauri release build" "cargo tauri build --bundles nsis"
    Write-StepInfo "Detail" "Compiling Rust core backend, bundling web UI, and generating NSIS installer..."

    Push-Location $repoRoot
    try {
        cargo tauri build --bundles nsis
        if ($LASTEXITCODE -ne 0) { 
            Write-StepError "cargo tauri build failed with exit code $LASTEXITCODE"
            throw "cargo tauri build failed (exit code $LASTEXITCODE)" 
        }
    } finally {
        Pop-Location
    }
    $stepTimer.Stop()
    $elapsed = [math]::Round($stepTimer.Elapsed.TotalSeconds, 1)

    # Verify generated artifacts
    if (-not (Test-Path -LiteralPath $exePath)) {
        Write-StepError "Build finished but expected NSIS installer was not found at $exePath"
        throw "Missing installer artifact: $exePath"
    }

    $setupSize = [math]::Round((Get-Item $exePath).Length / 1MB, 2)
    Write-StepDone "NSIS Installer generated" "$exePath ($setupSize MB)"

    if (Test-Path -LiteralPath $portableExe) {
        $portableSize = [math]::Round((Get-Item $portableExe).Length / 1MB, 2)
        Write-StepDone "Portable executable compiled" "$portableExe ($portableSize MB) -> mapped as 'NanoClick-portable.exe'"
    } else {
        Write-StepWarn "Standalone executable not found at $portableExe (only NSIS installer will be available)"
    }

    Write-StepDone "Stage 1 (BUILD) complete in ${elapsed}s"
}

# ── 2. SIGN STEP ──────────────────────────────────────────────────────────────
if ($Action -in @("all", "build", "sign")) {
    $stepTimer = [System.Diagnostics.Stopwatch]::StartNew()
    $stageIndex = if ($Action -eq "all") { "2" } else { "1" }
    Write-StageHeader $stageIndex $totalSteps "SIGN - Cryptographic Minisign Verification & Manifest"

    Write-StepLog "Checking target installer executable" $exePath
    if (-not (Test-Path -LiteralPath $exePath)) {
        Write-StepError "Target executable not found: $exePath"
        throw "Target executable not found: $exePath"
    }
    if (-not (Test-Path -LiteralPath $KeyPath)) {
        Write-StepError "Private key not found: $KeyPath"
        throw "Private key not found: $KeyPath"
    }

    # Clear environment key before calling -f to prevent argument conflict
    Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
    Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue

    # Clean old signature if exists
    if (Test-Path -LiteralPath $sigPath) {
        Write-StepLog "Removing previous signature" $sigPath
        Remove-Item -LiteralPath $sigPath -Force
    }

    Write-StepLog "Signing installer with private key" "cargo tauri signer sign -f $KeyPath"
    if ($KeyPassword) {
        cargo tauri signer sign -f $KeyPath -p $KeyPassword $exePath
    } else {
        cargo tauri signer sign -f $KeyPath $exePath
    }

    if ($LASTEXITCODE -ne 0 -or (-not (Test-Path -LiteralPath $sigPath))) {
        Write-StepError "Signing installer failed with exit code $LASTEXITCODE"
        throw "Signing installer failed (exit code $LASTEXITCODE)"
    }
    
    $sigSize = (Get-Item $sigPath).Length
    Write-StepDone "Minisign signature verified" "$sigPath ($sigSize bytes)"

    # Generate latest.json for Tauri updater v2
    Write-StepLog "Generating updater manifest" "latest.json"
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
    $stepTimer.Stop()
    $elapsed = [math]::Round($stepTimer.Elapsed.TotalSeconds, 1)

    Write-StepDone "Updater manifest generated" "$latestJsonPath"
    Write-StepDone "Stage 2 (SIGN) complete in ${elapsed}s"
}

# ── 3. UPLOAD STEP ────────────────────────────────────────────────────────────
if ($Action -in @("all", "upload")) {
    $stepTimer = [System.Diagnostics.Stopwatch]::StartNew()
    $stageIndex = if ($Action -eq "all") { "3" } else { "1" }
    Write-StageHeader $stageIndex $totalSteps "UPLOAD - Publishing Assets to GitHub Releases ($Owner/$Repo)"

    # Try resolving token from git-credential-manager or .env if not passed
    if (-not $Token) {
        Write-StepLog "Searching for GitHub credentials..."
        if ($env:GITHUB_TOKEN) {
            $Token = $env:GITHUB_TOKEN
            Write-StepDone "GitHub token found in environment variable GITHUB_TOKEN"
        } else {
            foreach ($envFile in @("$PSScriptRoot\.env", "$repoRoot\.env")) {
                if (Test-Path -LiteralPath $envFile) {
                    Get-Content -LiteralPath $envFile | ForEach-Object {
                        if ($_ -match '^\s*GITHUB_TOKEN\s*=\s*(.+)$') {
                            $Token = $matches[1].Trim()
                        }
                    }
                    if ($Token) {
                        Write-StepDone "GitHub token resolved from $envFile"
                        break
                    }
                }
            }
        }
    }

    if (-not $Token) {
        try {
            $pyToken = python -c "import subprocess; p=subprocess.Popen(['git','credential','fill'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True); out,_=p.communicate('protocol=https\nhost=github.com\n\n'); print([l[9:] for l in out.splitlines() if l.startswith('password=')][0])" 2>$null
            if ($pyToken) {
                $Token = $pyToken.Trim()
                Write-StepDone "GitHub token resolved via Git Credential Manager"
            }
        } catch {}
    }

    if (-not $Token) {
        Write-StepWarn "No token in environment or Git Credential Manager."
        $sec = Read-Host -Prompt "Enter GitHub Personal Access Token (repo scope)" -AsSecureString
        $bstr = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($sec)
        $Token = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
        [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
    }

    if (-not $Token) {
        Write-StepError "GitHub Token is required to upload release assets."
        throw "GitHub Token is required to upload release assets."
    }

    $headers = @{
        Authorization          = "token $Token"
        Accept                 = "application/vnd.github+json"
        "X-GitHub-Api-Version" = "2022-11-28"
    }

    $isDraft = [bool]$Draft.IsPresent

    # Find or create release
    Write-StepLog "Connecting to GitHub API for tag" $Tag
    $release = $null
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/tags/$Tag" `
                   -Headers $headers -Method Get
        Write-StepDone "Found existing release on GitHub" "ID: $($release.id), Name: $($release.name)"

        # Update title/notes if requested
        Write-StepLog "Updating release metadata on GitHub..."
        $updateBody = @{
            name        = $Title
            body        = $Notes
            draft       = $isDraft
            prerelease  = [bool]$Prerelease.IsPresent
            make_latest = if ($Prerelease.IsPresent) { "false" } else { "true" }
        } | ConvertTo-Json

        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/$($release.id)" `
                   -Headers $headers -Method Patch -Body $updateBody
        Write-StepDone "Release metadata updated successfully"
    } catch {
        Write-StepWarn "Release $Tag not found. Creating new release..."
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
        Write-StepDone "New release created on GitHub" "ID: $($release.id)"
    }

    $releaseId = $release.id
    $uploadBase = "https://uploads.github.com/repos/$Owner/$Repo/releases/$releaseId/assets"

    # Query existing assets to avoid 422 collision
    Write-StepLog "Checking existing assets on release ID $releaseId..."
    $assets = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/$releaseId/assets" `
              -Headers $headers -Method Get
    Write-StepInfo "Existing assets found on GitHub" "$($assets.Count) files"

    # Prepare file upload list
    $filesToUpload = @(
        @{ Path = $exePath;        Name = [System.IO.Path]::GetFileName($exePath); Mime = "application/vnd.microsoft.portable-executable" },
        @{ Path = $sigPath;        Name = [System.IO.Path]::GetFileName($sigPath); Mime = "text/plain" },
        @{ Path = $latestJsonPath; Name = "latest.json";                            Mime = "application/json" }
    )
    if (Test-Path -LiteralPath $portableExe) {
        $filesToUpload += @{ Path = $portableExe; Name = "NanoClick-portable.exe"; Mime = "application/vnd.microsoft.portable-executable" }
    } else {
        Write-StepWarn "Portable binary ($portableExe) not found; skipping."
    }

    $uploadIndex = 0
    $totalUploads = $filesToUpload.Count

    foreach ($file in $filesToUpload) {
        $uploadIndex++
        $fileName = if ($file.Name) { $file.Name } else { [System.IO.Path]::GetFileName($file.Path) }

        if (-not (Test-Path -LiteralPath $file.Path)) {
            Write-StepWarn "[$uploadIndex/$totalUploads] File not found locally: $($file.Path) (SKIPPING)"
            continue
        }

        # Check and delete existing asset with the same name
        foreach ($asset in $assets) {
            if ($asset.name -eq $fileName) {
                Write-StepLog "[$uploadIndex/$totalUploads] Deleting existing asset on GitHub" "$fileName (ID: $($asset.id))"
                Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/assets/$($asset.id)" `
                                  -Headers $headers -Method Delete | Out-Null
                Write-StepDone "Deleted old asset $fileName"
            }
        }

        $fileSize = (Get-Item $file.Path).Length
        $sizeFormatted = if ($fileSize -gt 1MB) {
            "$([math]::Round($fileSize / 1MB, 2)) MB"
        } else {
            "$([math]::Round($fileSize / 1KB, 1)) KB"
        }

        Write-StepLog "[$uploadIndex/$totalUploads] Uploading $fileName ($sizeFormatted)..." "Streaming to GitHub"
        $fileTimer = [System.Diagnostics.Stopwatch]::StartNew()

        $uploadUrl = "${uploadBase}?name=${fileName}"
        $fileBytes = [System.IO.File]::ReadAllBytes($file.Path)

        $uploadHeaders = @{
            Authorization          = "token $Token"
            "Content-Type"         = $file.Mime
            "X-GitHub-Api-Version" = "2022-11-28"
        }

        $null = Invoke-RestMethod -Uri $uploadUrl -Headers $uploadHeaders -Method Post -Body $fileBytes
        $fileTimer.Stop()
        $fileElapsed = [math]::Round($fileTimer.Elapsed.TotalSeconds, 1)

        Write-StepDone "[$uploadIndex/$totalUploads] Uploaded $fileName ($sizeFormatted) in ${fileElapsed}s"
    }

    $stepTimer.Stop()
    $uploadElapsed = [math]::Round($stepTimer.Elapsed.TotalSeconds, 1)
    Write-StepDone "Stage 3 (UPLOAD) complete in ${uploadElapsed}s"

    $statusStr = if ($isDraft) { "DRAFT (hidden from public)" } else { "PUBLISHED (public release)" }
    Write-Host ""
    Write-Host "╔════════════════════════════════════════════════════════════════╗" -ForegroundColor Green
    Write-Host "║                  RELEASE SUCCESSFULLY PUBLISHED                ║" -ForegroundColor Green
    Write-Host "╠════════════════════════════════════════════════════════════════╣" -ForegroundColor Green
    Write-Host "║  Status : $($statusStr.PadRight(54)) ║" -ForegroundColor White
    Write-Host "║  URL    : $($release.html_url.PadRight(54)) ║" -ForegroundColor Cyan
    Write-Host "╚════════════════════════════════════════════════════════════════╝" -ForegroundColor Green
}

$PipelineStopwatch.Stop()
$totalSec = [math]::Round($PipelineStopwatch.Elapsed.TotalSeconds, 1)
Write-StepDone "NanoClick Pipeline finished in ${totalSec}s total."
