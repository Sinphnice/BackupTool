set shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"]

default:
    just --list

setup:
    pnpm.cmd install

frontend-check:
    .\node_modules\.bin\tsc.cmd

frontend-build:
    .\node_modules\.bin\tsc.cmd
    .\node_modules\.bin\vite.cmd build --config vite.app.config.ts --configLoader runner --outDir dist --emptyOutDir

vite:
    .\scripts\start-vite.ps1

rust-check:
    cargo check --workspace

test:
    pnpm.cmd test
    cargo test --workspace

check: frontend-check rust-check

dev:
    .\node_modules\.bin\tauri.cmd dev

build: frontend-build
    .\node_modules\.bin\tauri.cmd build

run: build
    .\target\release\backup-tool.exe

clean:
    Remove-Item -Recurse -Force dist, src-tauri/target, target -ErrorAction SilentlyContinue
