set shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"]

default:
    just --list

setup:
    pnpm.cmd install

frontend-check:
    .\node_modules\.bin\tsc.cmd

frontend-build:
    .\node_modules\.bin\tsc.cmd
    .\node_modules\.bin\vite.cmd build src --outDir ../dist --emptyOutDir

vite:
    .\scripts\start-vite.ps1

rust-check:
    cargo check --workspace

test:
    cargo test --workspace

check: frontend-check rust-check

dev:
    $vite = Start-Process -FilePath ".\node_modules\.bin\vite.cmd" -ArgumentList @("src", "--host", "127.0.0.1", "--port", "1420", "--strictPort") -PassThru; try { Start-Sleep -Seconds 2; .\node_modules\.bin\tauri.cmd dev } finally { if ($vite -and -not $vite.HasExited) { Stop-Process -Id $vite.Id -Force } }

build: frontend-build
    .\node_modules\.bin\tauri.cmd build

run: build
    .\src-tauri\target\release\backup-tool.exe

clean:
    Remove-Item -Recurse -Force dist, src-tauri/target, target -ErrorAction SilentlyContinue
