param(
    [string]$HostAddress = "127.0.0.1",
    [int]$Port = 5173,
    [switch]$SkipInstall
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

function Test-Command {
    param([string]$Name)
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Test-DevServer {
    param(
        [string]$HostAddress,
        [int]$Port
    )

    try {
        $response = Invoke-WebRequest -Uri "http://${HostAddress}:${Port}" -UseBasicParsing -TimeoutSec 2
        return [int]$response.StatusCode -ge 200 -and [int]$response.StatusCode -lt 500
    }
    catch {
        return $false
    }
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

$connection = Get-NetTCPConnection -LocalPort $Port -ErrorAction SilentlyContinue |
    Where-Object { $_.State -eq "Listen" } |
    Select-Object -First 1

if ($connection) {
    if (Test-DevServer -HostAddress $HostAddress -Port $Port) {
        Write-Host "Reusing existing Vite dev server at http://${HostAddress}:${Port}"
        exit 0
    }

    $process = Get-Process -Id $connection.OwningProcess -ErrorAction SilentlyContinue
    $processName = if ($process) { $process.ProcessName } else { "unknown" }
    throw "Port ${Port} is already in use by PID $($connection.OwningProcess) ($processName), but http://${HostAddress}:${Port} is not a usable Vite dev server. Stop that process or choose another dev port."
}

Write-Host "Starting Vite dev server at http://${HostAddress}:${Port}"
Write-Host "Keep this PowerShell window open while running the debug exe. Press Ctrl+C to stop."

& $vite "--config" "vite.app.config.ts" "--configLoader" "runner" "--host" $HostAddress "--port" $Port "--strictPort"
