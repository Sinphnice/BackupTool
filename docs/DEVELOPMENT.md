# BackupTool 开发环境与构建说明

本文面向协作开发者，说明当前项目的技术组成、依赖工具链、环境配置和常用构建命令。

## 项目组成

BackupTool 当前采用 Tauri 2 桌面应用结构，顶层使用 `just` 组织构建任务，各部分仍由各自标准工具完成构建。

| 部分 | 目录 | 语言/框架 | 构建工具 | 作用 |
| --- | --- | --- | --- | --- |
| 前端界面 | `src/` | TypeScript, Vite | `pnpm`, `tsc`, `vite` | 构建 Web UI，并通过 Tauri API 调用后端命令 |
| 桌面壳与命令层 | `src-tauri/` | Rust, Tauri 2 | `cargo`, Tauri CLI | 提供桌面窗口、Tauri command 和 DTO 转换 |
| 备份核心库 | `crates/backup-core/` | Rust | `cargo` | 实现备份、恢复、筛选和后续仓库式备份核心逻辑 |
| 顶层任务编排 | `justfile` | just recipe | `just` | 统一组织 setup/check/test/dev/build/run 等命令 |

当前调用链为：

```text
GUI -> TypeScript -> Tauri Rust command -> Rust backup-core
```

`src-tauri` 不承载核心备份业务，只负责把 GUI 参数转换为 `backup-core` 的配置对象，并把结果转换回 Tauri command 返回值。

## Windows 环境要求

建议使用 Windows 10/11 x64，并安装以下工具：

| 工具 | 建议版本 | 用途 |
| --- | --- | --- |
| Git | 2.x | 获取源码、版本管理 |
| Node.js | 22 LTS 或更新 | 前端工具链运行时 |
| pnpm | 11.7.0 | 前端包管理器 |
| Rust/rustup | stable MSVC toolchain | Rust/Tauri 构建 |
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

只启动 Vite dev server：

```powershell
just vite
```

该命令等价于：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\start-vite.ps1
```

它会在 `http://127.0.0.1:1420` 启动前端开发服务。只有在需要手动运行 debug 版 Tauri 程序时才需要单独启动 Vite；正常开发建议直接使用 `just dev`。

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
2. Tauri release 构建
   - `tauri.cmd build`
   - Cargo 编译 Rust/Tauri 应用和 `backup-core`
   - 输出 `src-tauri/target/release/backup-tool.exe`
   - 同时生成 MSI/NSIS 安装包

`just dev` 的流程是：

1. 启动 Vite dev server。
2. 启动 Tauri dev 应用。
3. Tauri 退出后关闭 Vite dev server。

如果直接双击或手动运行 debug 版 `src-tauri/target/debug/backup-tool.exe`，程序仍会尝试访问 `http://127.0.0.1:1420`。此时必须先运行 `just vite` 并保持该 PowerShell 窗口打开，否则界面会显示 `ERR_CONNECTION_REFUSED`。

## 常见问题

### PowerShell 无法执行 pnpm

如果 `pnpm` 被 PowerShell 执行策略拦截，可以使用：

```powershell
pnpm.cmd --version
```

本项目 `justfile` 已统一使用 `pnpm.cmd`，避免依赖 `pnpm.ps1`。

### release 程序为什么不显示终端

`src-tauri/src/main.rs` 使用：

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
```

这会让 release 构建使用 Windows GUI subsystem，启动时只显示 GUI，不额外创建控制台窗口。debug 构建仍保留控制台，便于查看日志。
