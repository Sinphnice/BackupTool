# BackupTool 开发环境与构建说明

本文面向协作开发者，说明当前项目的技术组成、依赖工具链、环境配置和常用构建命令。

## 项目组成

BackupTool 当前采用 Tauri 2 桌面应用结构，顶层使用 `just` 组织构建任务，各语言部分仍由各自标准工具完成构建。

| 部分 | 目录 | 语言/框架 | 构建工具 | 作用 |
| --- | --- | --- | --- | --- |
| 前端界面 | `src/` | TypeScript, Vite | `pnpm`, `tsc`, `vite` | 构建 Web UI，并通过 Tauri API 调用后端命令 |
| 桌面壳与命令层 | `src-tauri/` | Rust, Tauri 2 | `cargo`, Tauri CLI | 提供桌面窗口、Tauri command、Rust 到 C ABI 的调用 |
| 核心库 | `core/` | C++17 | CMake, MSVC | 提供后续备份核心逻辑，目前暴露最小 C API |
| 顶层任务编排 | `justfile` | just recipe | `just` | 统一组织 setup/check/test/dev/build/run 等命令 |

当前第一阶段的最小调用链为：

```text
GUI -> TypeScript -> Tauri Rust command -> C ABI -> C++ Core
```

`just` 只负责任务编排，不替代各语言自己的构建系统：

- 前端依赖和前端构建由 `pnpm`、TypeScript、Vite 完成。
- C++ Core 配置和构建由 CMake/MSVC 完成。
- Rust/Tauri 检查、测试、打包由 Cargo 和 Tauri CLI 完成。
- `src-tauri/build.rs` 不调用 CMake，只负责把已经构建好的 `backup_core.lib` 链接进 Rust/Tauri 应用。

## Windows 环境要求

建议使用 Windows 10/11 x64，并安装以下工具：

| 工具 | 建议版本 | 用途 |
| --- | --- | --- |
| Git | 2.x | 获取源码、版本管理 |
| Node.js | 22 LTS 或更新 | 前端工具链运行时 |
| pnpm | 11.7.0 | 前端包管理器 |
| Rust/rustup | stable MSVC toolchain | Rust/Tauri 构建 |
| CMake | 3.28 或更新 | C++ Core 配置和生成 VS 工程 |
| Visual Studio 2022 Build Tools | MSVC v143 + Windows SDK | C++ 编译和链接 |
| just | 1.55 或更新 | 顶层构建任务入口 |

本仓库包含 `.cargo/config.toml`，固定 `x86_64-pc-windows-msvc` target 使用 `rust-lld.exe` 作为 linker，避免 Windows `PATH` 中其他同名 `link.exe` 干扰 Rust 链接。

## 一键检查与安装

Windows 开发者可以在仓库根目录运行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\bootstrap-windows.ps1
```

脚本会检查项目需要的工具，并尝试通过 `winget`、`rustup`、`corepack`、`cargo install` 安装缺失项。

只检查、不安装：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\bootstrap-windows.ps1 -CheckOnly
```

安装或更新系统级工具后，建议关闭并重新打开 PowerShell，使 `PATH` 生效。

## 首次配置

安装工具链后，在仓库根目录执行：

```powershell
just setup
```

该命令会执行：

```powershell
pnpm.cmd install
```

用于安装前端和 Tauri CLI 的 Node 依赖。

## 常用开发命令

列出所有可用任务：

```powershell
just
```

类型检查和 Rust 检查：

```powershell
just check
```

运行测试：

```powershell
just test
```

启动开发模式：

```powershell
just dev
```

构建 release 应用和安装包：

```powershell
just build
```

构建并运行 release 程序：

```powershell
just run
```

清理构建产物：

```powershell
just clean
```

## 构建流程

`just build` 的流程是：

1. `frontend-build`
   - `tsc.cmd`
   - `vite.cmd build src --outDir ../dist --emptyOutDir`
   - 输出前端静态文件到 `dist/`
2. `core-build-release`
   - CMake 配置 `core/build/msvc-release`
   - MSVC 编译生成 `core/build/msvc-release/core/Release/backup_core.lib`
3. Tauri release 构建
   - `tauri.cmd build`
   - Cargo 编译 Rust/Tauri 应用
   - `build.rs` 链接 C++ 静态库
   - 输出 `src-tauri/target/release/backup-tool.exe`
   - 同时生成 MSI/NSIS 安装包

`just dev` 的流程是：

1. 构建 Debug 版 C++ Core。
2. 启动 Vite dev server。
3. 启动 Tauri dev 应用。
4. Tauri 退出后关闭 Vite dev server。

## 常见问题

### PowerShell 无法执行 pnpm

如果 `pnpm` 被 PowerShell 执行策略拦截，可以使用：

```powershell
pnpm.cmd --version
```

本项目 `justfile` 已统一使用 `pnpm.cmd`，避免依赖 `pnpm.ps1`。

### MSBuild 读取 Windows SDK 失败

如果在受限沙箱或权限受限终端里看到类似 `C:\Users\<user>\AppData\Local\Microsoft SDKs` 访问被拒绝，通常不是项目代码问题，而是当前执行环境阻止 MSBuild 读取 Visual Studio/Windows SDK 配置。使用普通本机 PowerShell 或管理员 PowerShell 运行即可。

### release 程序为什么不显示终端

`src-tauri/src/main.rs` 使用：

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
```

这会让 release 构建使用 Windows GUI subsystem，启动时只显示 GUI，不额外创建控制台窗口。debug 构建仍保留控制台，便于查看日志。
