<#
.SYNOPSIS
  Builds a signed Windows release and publishes it to the Sidekick backend, so every installed
  copy offers the update the next time it opens.

.DESCRIPTION
  The version comes from client/src-tauri/tauri.conf.json: bump it there (and in Cargo.toml),
  commit, then run this. The build is signed with the release key, and the app refuses any update
  that isn't, so the private key is the one thing that must never leak.

.EXAMPLE
  ./scripts/release.ps1 -BackendUrl https://sidekick.example.com -Email you@company.com -Notes "Faster hints"
#>
param(
  [Parameter(Mandatory = $true)][string]$BackendUrl,
  [Parameter(Mandatory = $true)][string]$Email,
  [string]$Notes = "",
  [string]$KeyDir = "$env:USERPROFILE\.sidekick-release",
  [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$repo = Split-Path $PSScriptRoot -Parent
$conf = Get-Content "$repo\client\src-tauri\tauri.conf.json" -Raw | ConvertFrom-Json
$version = $conf.version
Write-Host "Releasing Sidekick $version" -ForegroundColor Cyan

if (-not (Test-Path "$KeyDir\sidekick.key")) {
  throw "No signing key at $KeyDir\sidekick.key. Generate one with: npx @tauri-apps/cli signer generate -w $KeyDir\sidekick.key"
}

if (-not $SkipBuild) {
  # Compiled in as the installed app's server address.
  $env:SIDEKICK_BACKEND_URL = $BackendUrl
  $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content "$KeyDir\sidekick.key" -Raw
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = (Get-Content "$KeyDir\password.txt" -Raw).Trim()
  Push-Location "$repo\client"
  try { npm run tauri build } finally { Pop-Location }
}

$bundle = "$repo\client\src-tauri\target\release\bundle\nsis"
$installer = Get-ChildItem $bundle -Filter "*_${version}_*-setup.exe" | Select-Object -First 1
if (-not $installer) { throw "No installer for $version in $bundle. Did the build run?" }
$signatureFile = "$($installer.FullName).sig"
if (-not (Test-Path $signatureFile)) { throw "No signature beside $($installer.Name): is createUpdaterArtifacts on?" }

$password = Read-Host "Password for $Email" -AsSecureString
$plain = [Runtime.InteropServices.Marshal]::PtrToStringAuto(
  [Runtime.InteropServices.Marshal]::SecureStringToBSTR($password))
$token = (Invoke-RestMethod -Method Post -Uri "$BackendUrl/auth/login" -ContentType "application/json" `
    -Body (@{ email = $Email; password = $plain } | ConvertTo-Json)).access_token

$form = @{
  version   = $version
  notes     = $Notes
  signature = (Get-Content $signatureFile -Raw).Trim()
  target    = "windows-x86_64"
  installer = Get-Item $installer.FullName
}
$published = Invoke-RestMethod -Method Post -Uri "$BackendUrl/updates" -Form $form `
  -Headers @{ Authorization = "Bearer $token" }

Write-Host "Published $($published.version) ($($published.file))" -ForegroundColor Green
Write-Host "Every signed-in copy will offer it on next launch."
