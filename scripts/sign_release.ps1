# ── NanoClick release signer ─────────────────────────────────────────────
# Signs a packaged artifact with the Tauri updater (minisign) private key.
# Produces "<artifact>.sig" next to the input file.
#
# Usage (encrypted key — password resolved param → env → .env, else one masked prompt):
#   powershell -ExecutionPolicy Bypass -File scripts\sign_release.ps1 `
#     -File "target\release\bundle\nsis\NanoClick_1.0.0-beta_x64-setup.exe"
#
# Usage (passwordless key, e.g. after make_release.ps1 rotation):
#   powershell -ExecutionPolicy Bypass -File scripts\sign_release.ps1 `
#     -File "target\release\bundle\nsis\NanoClick_1.0.0-beta_x64-setup.exe" -KeyPassword ""
#
# The private key NEVER lives in the repository: it is read from
# %USERPROFILE%\.tauri\ (or -KeyPath) and injected via the
# TAURI_SIGNING_PRIVATE_KEY environment variable, because the Tauri CLI
# expects the key *content*, not a file path.

param(
    [Parameter(Mandatory = $true)][string]$File,
    [string]$KeyPath = "$env:USERPROFILE\.tauri\nanoclick.key",
    [string]$KeyPassword = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
)

# ── Resolve key path & password: param → process env → .env file ──
# Same convention as upload_release.ps1: TAURI_SIGNING_PRIVATE_KEY_PASSWORD
# (and optionally TAURI_SIGNING_PRIVATE_KEY_PATH) can live in
# scripts\.env or the repo-root .env. Both are gitignored and never committed,
# so the secret lives on your disk, not in the repo.
# The CURRENT flow uses a PASSWORDLESS key (make_release.ps1): pass
# -KeyPassword "" (or nothing at all) and the CLI signs without -p.
$passwordFromEnvOrFile = $false
foreach ($envPath in @("$PSScriptRoot\.env", "$PSScriptRoot\..\.env")) {
    if (Test-Path -LiteralPath $envPath) {
        Get-Content -LiteralPath $envPath | ForEach-Object {
            if ($_ -match '^\s*TAURI_SIGNING_PRIVATE_KEY_PATH\s*=\s*(.+)$') {
                $KeyPath = $matches[1].Trim()
            }
            if (-not $passwordFromEnvOrFile -and $_ -match '^\s*TAURI_SIGNING_PRIVATE_KEY_PASSWORD\s*=\s*(.+)$') {
                $KeyPassword = $matches[1].Trim()
                $passwordFromEnvOrFile = $true
            }
        }
    }
}

if (-not (Test-Path -LiteralPath $File)) {
    throw "Artifact not found: $File"
}

if (-not (Test-Path -LiteralPath $KeyPath)) {
    throw "Updater private key not found: $KeyPath (keep it OUTSIDE the repository)"
}

$rawKey = (Get-Content -LiteralPath $KeyPath -Raw).Trim()
if (-not $rawKey) {
    throw "Updater private key file is empty: $KeyPath"
}

# Decide whether to pass -p:
#   * explicit -KeyPassword ""  → passwordless key (make_release.ps1 flow)
#   * explicit -KeyPassword xyz → encrypted key
#   * password from env/.env    → encrypted key
#   * nothing provided          → try passwordless (the CLI refuses clearly if
#                                 the key is actually encrypted → we fail fast)
$usePassword = ""
if ($PSBoundParameters.ContainsKey("KeyPassword")) {
    $usePassword = $KeyPassword
} elseif ($KeyPassword) {
    $usePassword = $KeyPassword
}

# Export the key into this session for the documented one-shot build; mirror
# the password decision in the env so `cargo tauri build` behaves identically.
$env:TAURI_SIGNING_PRIVATE_KEY = $rawKey
if ($usePassword -ne "") {
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $usePassword
} else {
    Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
}
Write-Host "Exported TAURI_SIGNING_PRIVATE_KEY into this session (password $($(if ($usePassword -eq '') {'omitted'} else {'set'})) )." -ForegroundColor DarkGray

Write-Host "Signing: $File" -ForegroundColor Cyan

# Mirrors the documented workflow in UPDATER_RELEASE.md: the Tauri CLI wants the
# key *content* via -k (passing the file path fails with
# "failed to decode base64 secret key ... Invalid symbol").
if ($usePassword -ne "") {
    cargo tauri signer sign -k $rawKey -p $usePassword $File
} else {
    cargo tauri signer sign -k $rawKey $File
}
if ($LASTEXITCODE -ne 0) {
    throw "Signing failed with exit code $LASTEXITCODE (wrong password for an encrypted key, or corrupted key file)"
}

$sig = "$File.sig"
if (Test-Path -LiteralPath $sig) {
    Write-Host "Signature written: $sig" -ForegroundColor Green
} else {
    throw "Signing reported success but $sig was not created"
}
#
# Typical full run (fresh key, no password on it):
#   powershell -ExecutionPolicy Bypass -File scripts\make_release.ps1
#
# Step-by-step (resume at any point):
