#requires -Version 5.1
<#
.SYNOPSIS
    Build Kiko's single binary: frontend assets -> src/resources/assets -> cargo release.

.DESCRIPTION
    This is the reviewed replacement for frontend/b.bat. It backs up the existing
    generated assets before replacing them and never commits or pushes anything.

.PARAMETER SkipFrontend
    Do not run pnpm install/build; use the existing frontend/dist.

.PARAMETER SkipReleaseBuild
    Only refresh embedded assets; do not run cargo build --release.

.EXAMPLE
    .\scripts\package.ps1 -SkipFrontend

.EXAMPLE
    .\scripts\package.ps1 -Confirm:$false
#>
[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [switch]$SkipFrontend,
    [switch]$SkipReleaseBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$frontend = Join-Path $root 'frontend'
$dist = Join-Path $frontend 'dist'
$assets = Join-Path $root 'src/resources/assets'

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory
    )
    Push-Location $WorkingDirectory
    try {
        & $Command @Arguments
        if ($LASTEXITCODE -ne 0) {
            throw "$Command $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

if (-not $SkipFrontend) {
    # Windows 默认 ExecutionPolicy 会拒绝 pnpm.ps1；优先用 .cmd shim。
    $pnpm = if (Get-Command 'pnpm.cmd' -ErrorAction SilentlyContinue) { 'pnpm.cmd' } else { 'pnpm' }
    Invoke-Checked -Command $pnpm -Arguments @('install', '--frozen-lockfile') -WorkingDirectory $frontend
    Invoke-Checked -Command $pnpm -Arguments @('build') -WorkingDirectory $frontend
}

$index = Join-Path $dist 'index.html'
if (-not (Test-Path -LiteralPath $index)) {
    throw "Frontend build output not found at $dist. Run without -SkipFrontend or build the frontend first."
}

if (-not (Test-Path -LiteralPath $assets)) {
    New-Item -ItemType Directory -Path $assets -Force | Out-Null
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$backup = Join-Path $root "data/assets-backup-$stamp"

if ($PSCmdlet.ShouldProcess($assets, "Back up to $backup and replace with $dist")) {
    New-Item -ItemType Directory -Path $backup -Force | Out-Null
    Get-ChildItem -LiteralPath $assets -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $backup -Recurse -Force
    }

    # The old hashed bundle names would otherwise be embedded as well. Back up first,
    # then replace the generated directory contents.
    Get-ChildItem -LiteralPath $assets -Force | Remove-Item -Recurse -Force
    Get-ChildItem -LiteralPath $dist -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $assets -Recurse -Force
    }
    Write-Host "Embedded frontend assets replaced; previous copy: $backup"
}

$sdk = Join-Path $frontend 'public/assets/KikoSDK.min.js'
$sdkTarget = Join-Path $root 'sdk/javascript/DialogFlowAiSDK.min.js'
if (Test-Path -LiteralPath $sdk) {
    Copy-Item -LiteralPath $sdk -Destination $sdkTarget -Force
    Write-Host "Updated $sdkTarget"
}

if (-not $SkipReleaseBuild) {
    Invoke-Checked -Command 'cargo' -Arguments @('build', '--release') -WorkingDirectory $root
    $suffix = if ($IsWindows -or $env:OS -eq 'Windows_NT') { '.exe' } else { '' }
    Write-Host "Built $(Join-Path $root "target/release/kiko$suffix")"
}
else {
    Write-Host "Skipped cargo build --release"
}
