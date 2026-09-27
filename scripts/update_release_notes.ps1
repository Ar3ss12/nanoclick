# ── GitHub release-notes updater for NanoClick ──────────────────────────────
# Updates the BODY (and optionally the title) of an EXISTING release, without
# rebuilding or re-uploading a single asset. Use it when the shipped binaries did
# not change but their description must (typo, missing section, extra context).
#
# Run: powershell -ExecutionPolicy Bypass -File scripts\update_release_notes.ps1 `
#          -Tag "v1.2.0" -NotesFile "docs\RELEASE_NOTES_v1.2.0.md"
#
# Token resolution order (same chain as release.ps1): -Token, $env:GITHUB_TOKEN,
# scripts\.env, .env, Git Credential Manager, then an interactive prompt.

param(
    [Parameter(Mandatory = $true)][string]$Tag,
    [string]$NotesFile,
    [string]$Notes,
    [string]$Title,
    [string]$Token = $env:GITHUB_TOKEN,
    [string]$Owner = "Ar3ss12",
    [string]$Repo = "nanoclick"
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

function Write-Step([string]$msg, [string]$colour = "Gray") {
    Write-Host (" [{0}] {1}" -f (Get-Date -Format "HH:mm:ss"), $msg) -ForegroundColor $colour
}

# PS 5.1 encodes a string body as ISO-8859-1, which turns every non-ASCII character
# into '?' — send raw UTF-8 bytes instead (the same cure the release pipeline uses).
function Send-GitHubJson {
    param(
        [string]$Uri,
        [hashtable]$Headers,
        [string]$Method,
        [hashtable]$Payload
    )
    $json = $Payload | ConvertTo-Json -Depth 6 -Compress
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    return Invoke-RestMethod -Uri $Uri -Headers $Headers -Method $Method `
        -ContentType "application/json; charset=utf-8" -Body $bytes
}

Write-Host ""
Write-Host "==============================================================" -ForegroundColor Cyan
Write-Host " NanoClick release-notes updater   $Owner/$Repo  tag=$Tag" -ForegroundColor Cyan
Write-Host "==============================================================" -ForegroundColor Cyan

# ── 1. Body text ──────────────────────────────────────────────────────────────
if ($NotesFile) {
    $path = if ([System.IO.Path]::IsPathRooted($NotesFile)) { $NotesFile } else { Join-Path $repoRoot $NotesFile }
    if (-not (Test-Path -LiteralPath $path)) {
        Write-Step "Notes file not found: $path" "Red"
        exit 1
    }
    $Notes = [System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8)
    Write-Step "Body read from $path ($($Notes.Length) characters, UTF-8)" "Green"
}
if (-not $Notes) {
    Write-Step "Pass -NotesFile <path> or -Notes <text>" "Red"
    exit 1
}

# ── 2. Token ──────────────────────────────────────────────────────────────────
if (-not $Token) {
    foreach ($envFile in @("$PSScriptRoot\.env", "$repoRoot\.env")) {
        if (Test-Path -LiteralPath $envFile) {
            Get-Content -LiteralPath $envFile | ForEach-Object {
                if ($_ -match '^\s*GITHUB_TOKEN\s*=\s*(.+)$') { $Token = $matches[1].Trim() }
            }
            if ($Token) {
                Write-Step "Token resolved from $envFile" "Green"
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
            Write-Step "Token resolved via Git Credential Manager" "Green"
        }
    } catch {}
}
if (-not $Token) {
    Write-Step "No token in the environment, .env or Git Credential Manager." "DarkYellow"
    $sec = Read-Host -Prompt "Enter GitHub Personal Access Token (repo scope)" -AsSecureString
    $bstr = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($sec)
    $Token = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
    [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
}
if (-not $Token) {
    Write-Step "A GitHub token with repo scope is required." "Red"
    exit 1
}

$headers = @{
    Authorization          = "token $Token"
    Accept                 = "application/vnd.github+json"
    "X-GitHub-Api-Version" = "2022-11-28"
}

# ── 3. Find the release and patch it ──────────────────────────────────────────
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/tags/$Tag" `
        -Headers $headers -Method Get
} catch {
    Write-Step "Release $Tag not found on $Owner/$Repo (nothing was changed)." "Red"
    exit 1
}
Write-Step "Found release id=$($release.id) name=`"$($release.name)`" (assets untouched)" "Green"

$payload = @{ body = $Notes }
if ($Title) { $payload.name = $Title }

try {
    $updated = Send-GitHubJson -Uri "https://api.github.com/repos/$Owner/$Repo/releases/$($release.id)" `
        -Headers $headers -Method Patch -Payload $payload
} catch {
    Write-Step "GitHub rejected the update: $($_.Exception.Message)" "Red"
    exit 1
}

Write-Step "Release body updated: $($updated.body.Length) characters" "Green"
Write-Step "Title on GitHub: $($updated.name)" "Green"
Write-Host ""
Write-Host " Open: https://github.com/$Owner/$Repo/releases/tag/$Tag" -ForegroundColor Cyan
