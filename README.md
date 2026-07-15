# BackupTool

BackupTool 是一个基于 Tauri 2、React 和 Rust 的桌面备份工具。项目当前采用仓库式备份设计：用户把多个源目录添加为一次快照，文件内容进入对象存储，恢复时从仓库中选择快照并按指定路径策略还原。

这个项目最初服务于《软件开发综合实验》课程，但当前 README 按普通开源项目组织，重点说明可用功能、设计思路、构建方式和开发入口。

## 功能特性

- 桌面 GUI：基于 Tauri 2 + React，提供仓库侧栏、快照列表、添加快照、恢复快照、导入和导出等操作。
- 仓库式备份：一个 repository 可以保存多次 snapshot，文件内容统一放入 object store。
- 多源备份：一次快照可以包含多个源目录；重复路径和父子包含路径会被规范化并去重。
- 快照标题：每次备份可以写入一个短标题，便于在 GUI 中识别。
- 筛选条件：支持路径正则、Owner、文件大小范围、修改时间范围。
- WSL / POSIX 文件：支持 WSL 路径下普通文件、目录、符号链接、FIFO 和字符/块设备节点的备份与恢复，并尽力保留 POSIX 元数据。
- 恢复路径策略：
  - `PreserveRelativePath`：按源目录内相对路径恢复，默认策略。
  - `PreserveFullPath`：保留原始绝对路径结构。
  - `Flatten`：扁平恢复到目标目录。
- 冲突处理策略：`Error`、`Skip`、`Overwrite`、`Rename`，默认 `Rename`。
- 对象级压缩：支持 `none` 和 `zstd`，压缩只作用于 object payload。
- 对象级加密：支持未加密 object 和 AES-256-GCM 加密 object 共存。
- 仓库密码管理：加密仓库使用 Argon2id 从用户密码派生密钥，用于解封装仓库主密钥；支持修改仓库密码。
- 安全导入导出：支持将完整 repository 导出为 `.tar`，也可以从 `.tar` 导入为 repository。
- 快照删除：删除 snapshot 文件，并清理不再被其他 snapshot 引用的 object。
- 自动化测试：包含 Rust core 测试、Tauri command 测试和前端状态测试。

## 设计概览

BackupTool 的核心不是简单复制目录，而是把备份数据组织为一个目录型仓库：

```text
repository/
├── repo.meta
├── objects/
├── snapshots/
└── indexes/
```

- `repo.meta` 保存仓库元数据，包括显示名和仓库加密配置。
- `snapshots/` 保存每次备份生成的 `<snapshot-id>.snapshot` 文件。
- `objects/` 保存文件内容对象。
- `indexes/` 当前预留给后续索引能力。

一次备份的大致流程：

```text
选择源目录
  -> 规范化和去重源路径
  -> 扫描目录树并应用筛选条件
  -> 将普通文件内容写入 objects/
  -> 写入 snapshots/<snapshot-id>.snapshot
  -> GUI 刷新快照列表
```

一次恢复的大致流程：

```text
打开 repository
  -> 选择 snapshot
  -> 选择恢复目录和路径策略
  -> 读取 snapshot 中的 object 引用
  -> 从 objects/ 读取、解密、解压 payload
  -> 写入目标目录
```

object id 由原始文件内容的 SHA-256 hash 和加密状态组成：

```text
<sha256>-plain
<sha256>-encrypted
```

这意味着压缩算法、nonce、payload 存储格式不会改变内容 hash；但明文 object 和加密 object 可以同时存在，避免同内容文件因其中一个加密快照而影响其他未加密快照。

## 技术栈

| 部分 | 路径 | 技术 | 职责 |
| --- | --- | --- | --- |
| 前端 GUI | `src/` | React, TypeScript, Vite, CSS | 界面、状态、表单、仓库工作区 |
| 桌面应用层 | `src-tauri/` | Tauri 2, Rust, serde | 窗口、权限、command、DTO 转换 |
| 备份核心库 | `crates/backup-core/` | Rust | repository、snapshot、object、恢复策略、tar |
| 构建编排 | `justfile` | just | 统一开发、检查、测试和构建命令 |
| 辅助脚本 | `scripts/` | PowerShell | Windows 环境检查和 Vite 辅助启动 |

后端当前是纯 Rust 实现，不再依赖旧的 C++ core、C ABI、CMake 或 MSBuild。

## 快速开始

推荐环境：

- Windows 10/11 x64
- Git
- Rust stable MSVC toolchain
- Node.js
- pnpm
- just
- WebView2 Runtime

首次安装依赖：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\bootstrap-windows.ps1
just setup
```

启动开发版：

```powershell
just dev
```

运行检查和测试：

```powershell
just check
just test
```

构建 release：

```powershell
just build
```

构建后可执行文件位于：

```text
target/release/backup-tool.exe
```

更多开发环境说明见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 常用命令

```powershell
just setup          # 安装前端依赖
just check          # TypeScript + Rust 检查
just test           # 前端状态测试 + Rust 测试
just dev            # 启动 Tauri 开发模式
just build          # 构建 release 应用
just run            # 构建并运行 release exe
just clean          # 清理构建产物
```

## 使用方式

1. 在左侧侧栏中新建或打开 repository。
2. 进入仓库工作区后点击 `Add Snapshot`。
3. 添加一个或多个源目录，按需设置标题、筛选、压缩和加密选项。
4. 备份完成后，snapshot 会出现在快照列表中。
5. 点击快照可以恢复到指定目录，并选择恢复路径策略和冲突策略。
6. 需要迁移仓库时，可以导出 `.tar`，在另一台机器上导入后继续打开和恢复。

## 安全说明

当前加密作用于 object payload，不加密 snapshot 元数据、路径、文件大小、仓库名称或 tar 文件结构。也就是说，加密快照可以保护文件内容，但不能隐藏目录结构和文件名等元数据。

加密仓库使用用户密码解封装仓库主密钥，object payload 使用 AES-256-GCM 加密。密码不会以明文写入仓库。导出 tar 时会保留加密 object，迁移到其他机器后恢复加密快照仍需要正确密码。

项目仍处于课程和实验性质的开发阶段。在用于重要数据前，应先用测试目录验证备份、导出、导入和恢复结果。

## 项目文档

- [项目概览](docs/PROJECT_OVERVIEW.md)：更完整的架构、数据模型和测试分布。
- [开发环境与构建说明](docs/DEVELOPMENT.md)：依赖安装、环境检查和构建步骤。
- [课程要求](docs/course.md)：原始课程任务说明。

## License

当前仓库尚未声明开源许可证。正式公开发布前应补充 `LICENSE` 文件。
