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
#   powershell -ExecutionPolicy Bypass -File scripts\make_release.ps1 -Steps rotate
#   powershell -ExecutionPolicy Bypass -File scripts\make_release.ps1 -Steps build,sign
#
# Inputs (NOT hardcoded here):
#   - $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD (optional — omit for a passwordless dev key)
#   - $env:GITHUB_TOKEN is NOT needed here; upload stays in upload_release.ps1.

param(
    [string]$Tag = "v1.0.0-beta",
    [string]$KeyPath = "$env:USERPROFILE\.tauri\nanoclick.key",
    [string[]]$Steps = @("rotate", "build", "sign")
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$pubPath = "$KeyPath.pub"

function Invoke-Step($name, [scriptblock]$body) {
    if ($Steps -notcontains $name) { return }
    Write-Host "`n=== [$name] ===" -ForegroundColor Cyan
    & $body
}

# ── rotate: backup old keypair (if any) + generate fresh passwordless keypair ──
Invoke-Step "rotate" {
    if (Test-Path -LiteralPath $KeyPath) {
        $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
        Copy-Item -LiteralPath $KeyPath "$KeyPath.bak-$stamp" -Force
        Write-Host "Backed up old private key -> $KeyPath.bak-$stamp" -ForegroundColor DarkGray
    }
    if (Test-Path -LiteralPath $pubPath) {
        $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
        Copy-Item -LiteralPath $pubPath "$pubPath.bak-$stamp" -Force
    }
    # Empty password arg = unencrypted key: `signer generate` still honors
    # --ci and writes the key without prompting. No secret is ever echoed.
    cargo tauri signer generate -w $KeyPath --ci -f -p ""
    if ($LASTEXITCODE -ne 0) { throw "signer generate failed (exit $LASTEXITCODE)" }

    $pub = (Get-Content -LiteralPath $pubPath -Raw).Trim()
    if (-not $pub) { throw "Public key file is empty: $pubPath" }

    # Patch tauri.conf.json pubkey (JSON round-trip, preserves the rest).
    $confPath = Join-Path $repoRoot "src-tauri\tauri.conf.json"
    $conf = Get-Content -LiteralPath $confPath -Raw | ConvertFrom-Json
    $conf.plugins.updater.pubkey = $pub
    [IO.File]::WriteAllText($confPath, ($conf | ConvertTo-Json -Depth 12), [Text.UTF8Encoding]::new($false))
    Write-Host "Patched plugins.updater.pubkey in src-tauri/tauri.conf.json" -ForegroundColor Green
    Write-Host "Public key: $pub" -ForegroundColor DarkGray

    Push-Location $repoRoot
    try {
        git add src-tauri/tauri.conf.json
        git commit -m "chore: rotate updater keypair for $Tag (passwordless dev key)"
    } finally { Pop-Location }
}

# ── build: NSIS installer (unsigned; signature comes from `sign` below) ──
Invoke-Step "build" {
    Push-Location $repoRoot
    try {
        # Ensure the keypair exists before spending minutes on the build.
        if (-not (Test-Path -LiteralPath $pubPath)) {
            throw "No public key at $pubPath — run the 'rotate' step first."
        }
        $env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content -LiteralPath $KeyPath -Raw).Trim()
        # Passwordless key: explicitly UNSET the password so the CLI never
        # waits on an interactive stdin prompt in automated shells.
        Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
        cargo tauri build --bundles nsis
        if ($LASTEXITCODE -ne 0) { throw "tauri build failed (exit $LASTEXITCODE)" }
    } finally { Pop-Location }
}

# ── sign: fast standalone .sig for the freshly built installer ──
Invoke-Step "sign" {
    $ver = $Tag.TrimStart("v")
    $exe = Join-Path $repoRoot "target\release\bundle\nsis\NanoClick_${ver}_x64-setup.exe"
    if (-not (Test-Path -LiteralPath $exe)) {
        throw "Installer not found: $exe — run the 'build' step first."
    }
    . "$PSScriptRoot\sign_release.ps1" -File $exe -KeyPath $KeyPath -KeyPassword ""
    $sig = "$exe.sig"
    if (-not (Test-Path -LiteralPath $sig)) { throw "Expected signature missing: $sig" }
    Write-Host "Release bundle ready:" -ForegroundColor Green
    Get-ChildItem (Join-Path $repoRoot "target\release\bundle\nsis") -File |
        Select-Object Name, @{n='KB'; e={[math]::Round($_.Length/1KB, 1)}} |
        Format-Table -AutoSize | Out-String | Write-Host
}