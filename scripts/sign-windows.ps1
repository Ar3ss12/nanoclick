param(
  [Parameter(Mandatory = $true)]
  [string]$Artifact
)

$ErrorActionPreference = "Stop"
$signtool = $env:SIGNTOOL_PATH
$thumbprint = $env:WINDOWS_CERTIFICATE_THUMBPRINT
$timestamp = $env:WINDOWS_TIMESTAMP_URL

if ([string]::IsNullOrWhiteSpace($signtool)) { throw "Set SIGNTOOL_PATH to signtool.exe" }
if ([string]::IsNullOrWhiteSpace($thumbprint)) { throw "Set WINDOWS_CERTIFICATE_THUMBPRINT" }
if ([string]::IsNullOrWhiteSpace($timestamp)) { throw "Set WINDOWS_TIMESTAMP_URL" }
if (-not (Test-Path -LiteralPath $Artifact)) { throw "Artifact not found: $Artifact" }

& $signtool sign /sha1 $thumbprint /fd sha256 /tr $timestamp /td sha256 /a $Artifact
if ($LASTEXITCODE -ne 0) { throw "signtool failed with exit code $LASTEXITCODE" }
& $signtool verify /pa /all $Artifact
if ($LASTEXITCODE -ne 0) { throw "signature verification failed" }

