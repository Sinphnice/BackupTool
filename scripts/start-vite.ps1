param(
    [string]$HostAddress = "127.0.0.1",
    [int]$Port = 1420,
    [switch]$SkipInstall
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

function Test-Command {
    param([string]$Name)
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

if (-not (Test-Command "pnpm.cmd")) {
    throw "pnpm.cmd is not available. Run scripts/bootstrap-windows.ps1 first, then reopen PowerShell."
}

$vite = Join-Path $repoRoot "node_modules\.bin\vite.cmd"
if ((-not (Test-Path $vite)) -and (-not $SkipInstall)) {
    Write-Host "Node dependencies are missing. Running pnpm.cmd install..."
    & pnpm.cmd install
}

if (-not (Test-Path $vite)) {
    throw "Vite is not installed. Run pnpm.cmd install or just setup first."
}

Write-Host "Starting Vite dev server at http://${HostAddress}:${Port}"
Write-Host "Keep this PowerShell window open while running the debug exe. Press Ctrl+C to stop."

& $vite "src" "--host" $HostAddress "--port" $Port "--strictPort"
