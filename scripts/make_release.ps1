# ── NanoClick release maker: keypair rotation → pubkey patch → build → sign ──
# Runs end-to-end (or step by step with -Steps) and NEVER commits secrets:
# keypair stays in %USERPROFILE%\.tauri\ and your shell only.
# The generated key is PASSWORDLESS (dev/beta flow): no password prompts at all.
#
# Typical full run (fresh passwordless key, no password prompts at all):
#   powershell -ExecutionPolicy Bypass -File scripts\make_release.ps1
#
# Step-by-step (resume at any point):
#   powershell -ExecutionPolicy Bypass -File scripts\make_release.ps1 -Steps rotate
#   powershell -ExecutionPolicy Bypass -File scripts\make_release.ps1 -Steps build,sign

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
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    if (Test-Path -LiteralPath $KeyPath) {
        Copy-Item -LiteralPath $KeyPath "$KeyPath.bak-$stamp" -Force
        Write-Host "Backed up old private key -> $KeyPath.bak-$stamp" -ForegroundColor DarkGray
    }
    if (Test-Path -LiteralPath $pubPath) {
        Copy-Item -LiteralPath $pubPath "$pubPath.bak-$stamp" -Force
    }
    # --ci without -p = passwordless key: the CLI skips all prompts and writes
    # the key unencrypted. No secret is ever echoed.
    cargo tauri signer generate -w $KeyPath --ci -f
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

# ── build: NSIS installer (signature comes from `sign` below) ──
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
