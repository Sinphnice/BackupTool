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

core-config:
    just core-config-debug

core-config-debug:
    cmake -S . -B core/build/msvc-debug -G "Visual Studio 17 2022" -A x64 -DBACKUP_CORE_BUILD_TESTS=OFF -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL

core-config-release:
    cmake -S . -B core/build/msvc-release -G "Visual Studio 17 2022" -A x64 -DBACKUP_CORE_BUILD_TESTS=OFF -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL

core-build:
    just core-build-debug

core-build-debug: core-config-debug
    cmake --build core/build/msvc-debug --config Debug

core-build-release: core-config-release
    cmake --build core/build/msvc-release --config Release

rust-check: core-build-debug
    cargo check --manifest-path src-tauri/Cargo.toml

test: core-build-debug
    cargo test --manifest-path src-tauri/Cargo.toml

check: frontend-check rust-check

dev: core-build-debug
    $vite = Start-Process -FilePath ".\node_modules\.bin\vite.cmd" -ArgumentList @("src", "--host", "127.0.0.1", "--port", "1420", "--strictPort") -PassThru; try { Start-Sleep -Seconds 2; .\node_modules\.bin\tauri.cmd dev } finally { if ($vite -and -not $vite.HasExited) { Stop-Process -Id $vite.Id -Force } }

build: frontend-build core-build-release
    .\node_modules\.bin\tauri.cmd build

run: build
    .\src-tauri\target\release\backup-tool.exe

clean:
    Remove-Item -Recurse -Force dist, src-tauri/target, core/build -ErrorAction SilentlyContinue
