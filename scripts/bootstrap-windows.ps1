param(
    [switch]$CheckOnly
)

$ErrorActionPreference = "Continue"

function Write-Section {
    param([string]$Text)
    Write-Host ""
    Write-Host "== $Text ==" -ForegroundColor Cyan
}

function Test-Command {
    param([string]$Name)
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Get-CommandVersionText {
    param(
        [string]$Command,
        [string[]]$Arguments = @("--version")
    )

    if (-not (Test-Command $Command)) {
        return $null
    }

    try {
        $output = & $Command @Arguments 2>$null
        return ($output | Select-Object -First 1)
    } catch {
        return $null
    }
}

function Install-WingetPackage {
    param(
        [string]$PackageId,
        [string]$Name,
        [string[]]$ExtraArgs = @()
    )

    if ($CheckOnly) {
        Write-Warning "$Name is missing. Install with: winget install --id $PackageId --exact"
        return
    }

    if (-not (Test-Command "winget")) {
        throw "winget is required to install $Name automatically. Install App Installer from Microsoft Store first."
    }

    Write-Host "Installing $Name with winget..."
    $args = @(
        "install",
        "--id", $PackageId,
        "--exact",
        "--accept-package-agreements",
        "--accept-source-agreements"
    ) + $ExtraArgs
    & winget @args
}

function Ensure-CommandWithWinget {
    param(
        [string]$Command,
        [string]$Name,
        [string]$PackageId,
        [string[]]$VersionArgs = @("--version"),
        [string[]]$InstallExtraArgs = @()
    )

    $version = Get-CommandVersionText -Command $Command -Arguments $VersionArgs
    if ($version) {
        Write-Host "$Name found: $version" -ForegroundColor Green
        return
    }

    Install-WingetPackage -PackageId $PackageId -Name $Name -ExtraArgs $InstallExtraArgs
}

function Ensure-Rust {
    if (-not (Test-Command "rustup")) {
        Install-WingetPackage -PackageId "Rustlang.Rustup" -Name "rustup"
    } else {
        Write-Host "rustup found: $(Get-CommandVersionText rustup)" -ForegroundColor Green
    }

    if ($CheckOnly -and -not (Test-Command "rustup")) {
        return
    }

    if (-not (Test-Command "rustup")) {
        Write-Warning "rustup was installed, but is not visible in this shell yet. Reopen PowerShell and run this script again."
        return
    }

    $installedToolchains = (& rustup toolchain list 2>$null) -join "`n"
    if ($installedToolchains -match "stable-x86_64-pc-windows-msvc") {
        Write-Host "Rust stable MSVC toolchain found." -ForegroundColor Green
    } elseif ($CheckOnly) {
        Write-Warning "Rust stable MSVC toolchain is missing. Install with: rustup toolchain install stable-x86_64-pc-windows-msvc"
    } else {
        & rustup toolchain install stable-x86_64-pc-windows-msvc
    }

    if (-not $CheckOnly) {
        & rustup default stable-x86_64-pc-windows-msvc
        & rustup target add x86_64-pc-windows-msvc
    }
}

function Ensure-Pnpm {
    if (-not (Test-Command "node")) {
        Install-WingetPackage -PackageId "OpenJS.NodeJS.LTS" -Name "Node.js LTS"
    } else {
        Write-Host "Node.js found: $(Get-CommandVersionText node)" -ForegroundColor Green
    }

    if (-not (Test-Command "corepack")) {
        if ($CheckOnly) {
            Write-Warning "corepack is missing. It is normally installed with Node.js."
            return
        }
        Write-Warning "corepack is not visible yet. Reopen PowerShell after Node.js installation, then rerun this script."
        return
    }

    if ($CheckOnly) {
        $pnpmVersion = Get-CommandVersionText -Command "pnpm.cmd" -Arguments @("--version")
        if ($pnpmVersion) {
            Write-Host "pnpm found: $pnpmVersion" -ForegroundColor Green
        } else {
            Write-Warning "pnpm 11.7.0 is missing. Install with: corepack enable; corepack prepare pnpm@11.7.0 --activate"
        }
        return
    }

    & corepack enable
    & corepack prepare pnpm@11.7.0 --activate

    $pnpmVersion = Get-CommandVersionText -Command "pnpm.cmd" -Arguments @("--version")
    if ($pnpmVersion) {
        Write-Host "pnpm ready: $pnpmVersion" -ForegroundColor Green
    } else {
        Write-Warning "pnpm was prepared by corepack, but is not visible in this shell yet. Reopen PowerShell and rerun this script if needed."
    }
}

function Ensure-Just {
    $version = Get-CommandVersionText -Command "just" -Arguments @("--version")
    if ($version) {
        Write-Host "just found: $version" -ForegroundColor Green
        return
    }

    if ($CheckOnly) {
        Write-Warning "just is missing. Install with: cargo install just"
        return
    }

    if (-not (Test-Command "cargo")) {
        Write-Warning "cargo is not available yet, skipping just installation. Reopen PowerShell after Rust installation and rerun this script."
        return
    }

    & cargo install just
}

Write-Section "System tools"
Ensure-CommandWithWinget -Command "git" -Name "Git" -PackageId "Git.Git"

Write-Section "Rust toolchain"
Ensure-Rust
Ensure-Just

Write-Section "Node toolchain"
Ensure-Pnpm

Write-Section "Project dependencies"
if (Test-Command "pnpm.cmd") {
    if ($CheckOnly) {
        Write-Host "Run project dependency install with: just setup"
    } else {
        & pnpm.cmd install
    }
} else {
    Write-Warning "pnpm.cmd is not available, skipping project dependency installation."
}

Write-Host ""
Write-Host "Done. If tools were installed or PATH changed, reopen PowerShell before building." -ForegroundColor Cyan
