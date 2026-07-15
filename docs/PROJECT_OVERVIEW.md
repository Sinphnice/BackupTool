# BackupTool 项目概览

本文用于帮助开发者快速理解 BackupTool 的项目组成、架构设计、核心数据模型、构建方式和测试分布。环境安装细节见 [DEVELOPMENT.md](DEVELOPMENT.md)，课程要求见 [course.md](course.md)。

## 1. 项目定位

BackupTool 是一个基于 Tauri 2 的桌面备份工具。当前版本已经完成从早期实验性实现到仓库式备份模型的迁移，后端边界为纯 Rust：

```text
React TypeScript GUI
    -> Tauri command
        -> backup-core Rust library
            -> filesystem repository
```

当前备份不是简单的目录镜像复制。程序会把备份数据写入 repository，每次备份生成一个 snapshot，普通文件内容进入 object store。恢复时选择 repository 中的 snapshot，再按恢复路径策略写入目标目录。

当前主要能力：

- 新建、打开、重命名、删除 repository。
- 仓库级加密配置和密码修改。
- 多源目录备份。
- 源路径规范化、去重和父子包含路径去重。
- snapshot 标题、创建时间、源路径和 entry 元数据记录。
- snapshot 列表读取、恢复、删除。
- 删除 snapshot 后清理不再被引用的 object。
- 路径正则、文件大小和修改时间筛选。
- `PreserveRelativePath`、`PreserveFullPath`、`Flatten` 三种恢复路径策略。
- `Error`、`Skip`、`Overwrite`、`Rename` 四种冲突策略。
- object 级 `none` / `zstd` 压缩。
- object 级 AES-256-GCM 加密。
- object 级 CRC32 完整性校验。
- repository 导出为 `.tar`，以及从 `.tar` 安全导入。
- 仓库中心化 React GUI。
- Rust core、Tauri command 和前端状态测试。

## 2. 项目结构

```text
BackupTool/
├── src/                    # React / TypeScript 前端
├── src-tauri/              # Tauri 2 桌面壳与 command 层
├── crates/
│   └── backup-core/        # 纯 Rust 备份核心库
├── scripts/                # Windows 环境检查和辅助脚本
├── docs/                   # 项目文档
├── Cargo.toml              # Rust workspace
├── package.json            # 前端和 Tauri CLI 依赖
├── pnpm-lock.yaml          # 前端依赖锁定文件
└── justfile                # 项目级任务入口
```

主要分层：

| 层次 | 路径 | 技术 | 职责 |
| --- | --- | --- | --- |
| GUI 层 | `src/` | React, TypeScript, Vite, CSS | 仓库侧栏、工作区、表单、状态和结果展示 |
| 桌面适配层 | `src-tauri/` | Rust, Tauri 2, serde | Tauri command、DTO 转换、路径校验、错误映射 |
| 核心业务层 | `crates/backup-core/` | Rust | repository、snapshot、object、筛选、恢复、归档 |
| 构建编排 | `justfile` | just | 统一组织依赖安装、检查、测试、开发运行和构建 |
| 环境脚本 | `scripts/` | PowerShell | Windows 依赖检查和 Vite 辅助启动 |

当前不再需要 CMake、C++ core、C ABI、MSBuild 或手写 linker 配置。

## 3. 依赖与构建工具

系统侧建议：

- Windows 10/11 x64。
- Git。
- Rust stable MSVC toolchain。
- Node.js。
- pnpm。
- just。
- WebView2 Runtime。

前端依赖由 `pnpm` 管理：

- `react` / `react-dom`：GUI 组件和渲染。
- `@tauri-apps/api`：前端调用 Tauri command。
- `@tauri-apps/plugin-dialog`：文件和目录选择对话框。
- `@tauri-apps/plugin-store`：保存本机 UI 状态。
- `lucide-react`：基础图标。
- `vite` / `@vitejs/plugin-react`：开发服务器和前端构建。
- `typescript`：类型检查。
- `vitest`：前端状态测试。
- `@tauri-apps/cli`：Tauri 开发运行和打包。

Rust 依赖由 Cargo workspace 管理：

- 根目录 `Cargo.toml` 声明 workspace。
- `crates/backup-core` 是核心库。
- `src-tauri` 是 Tauri 应用 crate，依赖 `backup-core`。

`backup-core` 当前主要外部 crate：

- `regex`：路径筛选。
- `sha2`：计算 SHA-256 内容 hash。
- `zstd`：object payload 压缩和解压。
- `aes-gcm`：AES-256-GCM payload 加密。
- `argon2`：从用户密码派生密钥加密仓库主密钥。
- `rand`：生成 salt、nonce 和仓库主密钥。
- `hex`：编码二进制密钥材料元数据。
- `tar`：repository 导出和导入。

CRC32 当前由 `backup-core` 内部实现，用于 object 级完整性校验，没有引入额外 crate。

## 4. 架构原则

系统按三层组织：

```text
GUI 层
    负责交互、表单、状态、展示

Tauri command 层
    负责桌面边界、DTO 转换、路径校验、错误字符串化

backup-core 核心层
    负责备份、恢复、仓库、快照、对象、压缩、加密和归档
```

设计约束：

- GUI 不实现备份算法。
- Tauri command 不承载核心业务，只做边界适配。
- `backup-core` 不依赖 Tauri、React、Node.js 或 WebView。
- 新能力优先在 `backup-core` 中形成可测试 API，再暴露到 command 和 GUI。
- 密码不写入前端持久化状态。
- repository 是磁盘数据格式，前端 Store 只是本机 UI 状态。

典型调用路径：

```text
src/App.tsx
    -> src/api.ts
        -> invoke("backup" / "restore" / "list_snapshots" / ...)
            -> src-tauri/src/commands.rs
                -> crates/backup-core/src/repository.rs
```

## 5. Repository 磁盘格式

repository 是一个目录：

```text
repository/
├── repo.meta
├── objects/
├── snapshots/
└── indexes/
```

各部分职责：

- `repo.meta`：仓库元数据。保存显示名、仓库加密算法、KDF 参数、wrapped repository master key 等信息。仓库显示名不要求与磁盘目录名一致。
- `objects/`：文件内容对象。对象文件名是 `<sha256>-plain` 或 `<sha256>-encrypted`。
- `snapshots/`：每次备份生成的 snapshot 文件，格式为 `<snapshot-id>.snapshot`。
- `indexes/`：预留目录，当前主要用于保持仓库结构稳定。

repository 的合法性通过 `repo.meta` 和必要目录判断。命令层在备份时不会把数据写入“已有但不是 repository 的非空目录”，避免和普通文件夹混合。

## 6. Snapshot 模型

snapshot id 格式：

```text
<unix_seconds>-<nanoseconds_9_digits>-<sequence_3_digits>
```

snapshot 文件路径：

```text
snapshots/<snapshot-id>.snapshot
```

snapshot 文件头：

```text
backup-tool snapshot v1
```

snapshot 文件记录：

- snapshot id。
- 创建时间的秒、纳秒和 sequence。
- 可选标题。
- 每个源目录的绝对路径和恢复根名称。
- 每个 entry 的类型、相对路径、大小、修改时间、object id 和元数据。

entry 类型包括：

- `Directory`
- `File`
- `Symlink`
- `Other`

当前重点支持普通目录和普通文件。元数据保存包括大小、访问时间、创建时间、修改时间、只读状态，以及 Windows / POSIX 平台相关基础字段。恢复时默认使用 `BestEffort`，即尽量恢复数据和可支持元数据；不支持的元数据以警告方式处理。

## 7. 多源备份

`backup` command 接收多个源目录：

```text
backup(sources, destination, filter, compressionAlgorithm, snapshotTitle, encryptSnapshot, encryptionPassword)
```

核心层会先规范化源路径：

1. 转为绝对路径。
2. 尽量使用可访问路径的 canonical form。
3. 规范化 `.` 和 `..`。
4. 排序。
5. 去重。
6. 如果父目录和子目录同时存在，保留父目录，移除多余子目录。

被移除的重复或子路径会进入 `ignored_sources`，由 GUI 显示给用户。

多源恢复时，`PreserveRelativePath` 默认使用每个源目录的恢复根名称隔离不同源。恢复根名称优先来自源目录顶层名；如果发生冲突，复用 `Flatten` 的冲突策略处理名称。

## 8. Object 模型

object id 当前格式：

```text
<content_sha256>-plain
<content_sha256>-encrypted
```

其中：

- `content_sha256` 只由原始文件数据计算。
- `plain` 表示 object payload 未加密。
- `encrypted` 表示 object payload 使用仓库主密钥加密。

这样可以保证：

- 同一明文内容在不同压缩算法下仍有相同内容 hash。
- 同一明文内容的明文 object 和加密 object 可以同时存在。
- 一个加密快照不会把其他引用同内容明文 object 的快照变成需要密码。

object 文件是自描述格式，头部为文本，payload 为二进制：

```text
backup-tool object v1
compression    none|zstd
encryption     none|aes-256-gcm
key_id         <repository key id or empty>
nonce          <hex nonce or empty>
crc32          <8 hex digits>
original_size  <u64>
payload_size   <u64>

<payload bytes>
```

写入流程：

```text
raw bytes
    -> CRC32 over raw bytes
    -> optional zstd compression
        -> optional AES-256-GCM encryption
            -> object header + payload
```

读取流程：

```text
object header + payload
    -> optional AES-256-GCM decryption
        -> optional zstd decompression
            -> CRC32 verification over raw bytes
            -> raw bytes
```

`crc32` 记录的是原始文件数据的 CRC32，不是压缩后或加密后的 payload CRC。恢复时先完成解密和解压，再对得到的原始 bytes 重新计算 CRC32；如果不一致，恢复会返回错误并停止写出该 object 对应的文件。

压缩、加密和 CRC 校验都围绕 object payload 工作；object header 本身保持明文自描述格式。

## 9. 加密模型

当前加密是 object payload 级别，不是整个 repository 或 tar 文件级别。

仓库加密配置位于 `repo.meta`：

- `EncryptionAlgorithm::None`：仓库不配置加密。
- `EncryptionAlgorithm::Aes256Gcm`：仓库配置 AES-256-GCM object 加密能力。

加密仓库创建时会生成随机 repository master key。用户密码通过 Argon2id 派生 key encryption key，用于封装 master key。`repo.meta` 保存 salt、KDF 参数、nonce、wrapped master key 和 key id，但不保存明文密码或明文 master key。

添加 snapshot 时：

- 未勾选加密：写入 `-plain` object，不需要密码。
- 勾选加密：需要仓库已经配置加密，并且需要用户密码解封装 master key，写入 `-encrypted` object。

恢复 snapshot 时：

- 如果 snapshot 不引用加密 object，不需要密码。
- 如果 snapshot 引用加密 object，需要提供正确密码。

修改仓库密码时，只重新封装 repository master key，不重写 object，不改变 snapshot，不改变 object id。

## 10. 筛选模型

`BackupFilter` 支持：

- `path_regex`：匹配规范化后的相对路径，路径分隔符统一为 `/`。
- `min_size` / `max_size`：文件大小范围。
- `modified_after` / `modified_before`：修改时间范围，使用 Unix seconds。

筛选只决定普通文件是否进入 snapshot。目录 entry 仍可能被记录，用于恢复目录结构。非法正则会在备份开始前返回错误。

## 11. 恢复模型

恢复路径策略：

- `PreserveRelativePath`：默认策略。单源时恢复源目录内相对结构；多源时用源恢复根名称隔离。
- `PreserveFullPath`：把原绝对路径编码进目标目录，例如 Windows 盘符会转换为安全路径组件。
- `Flatten`：只把文件恢复到目标根目录，不恢复原始目录层级。

冲突策略：

- `Error`：遇到目标路径已存在即失败。
- `Skip`：跳过冲突文件。
- `Overwrite`：覆盖目标文件。
- `Rename`：自动改名，例如 `config.json`、`config (1).json`。

恢复元数据策略当前默认 `BestEffort`。底层还保留 `Strict` 和 `DataOnly` 模型，便于后续扩展 GUI 或命令层选项。

## 12. Tar 导入导出

当前支持 repository 级 tar 归档：

```text
repository/
    -> export_repository(...)
        -> repository.tar

repository.tar
    -> import_repository(...)
        -> repository/
```

设计点：

- 归档对象是完整 repository，不是单个 snapshot，也不是用户原始源目录。
- 当前唯一算法是 `tar`，接口保留 algorithm 参数。
- 使用 Rust `tar` crate，不依赖系统 tar 命令。
- tar 内部路径全部是相对路径。
- 导出内容包含 `repo.meta`、`objects/`、`snapshots/`、`indexes/`。
- 如果输出 tar 文件位于 repository 内部，导出时会跳过它，避免递归打包自身。
- 导入目标目录必须不存在或为空。
- 导入时拒绝绝对路径、`..`、symlink 等非常规 entry，避免不安全解包。

tar 导出会保留加密 object；迁移后恢复加密 snapshot 仍需要正确密码。

## 13. GUI 设计

前端位于 `src/`，当前使用 React 和原生 CSS。

主要界面结构：

- 自定义标题栏：保留窗口拖动和窗口控制按钮，并提供 sidebar 折叠按钮。
- 左侧 sidebar：新建、打开、导入仓库；显示置顶仓库和普通仓库列表。
- 仓库列表：支持置顶、取消置顶、归档隐藏、拖拽排序。
- 右侧 session/workspace：按当前选中 repository 展示快照工作区。
- 仓库工作区：显示仓库名、快照列表、刷新、添加快照、导出仓库。
- 功能弹窗：添加快照、导出仓库、恢复快照使用居中 modal。
- 设置页面：支持仓库显示名修改、仓库密码相关操作和删除仓库。

前端状态：

- 使用 `@tauri-apps/plugin-store` 保存本机 UI 状态。
- 保存内容包括侧栏宽度、仓库列表、置顶状态、归档状态、顺序、当前仓库、各仓库草稿。
- 密码和解密口令不写入 Store。
- repository 数据本身不依赖前端 Store；Store 损坏不应破坏磁盘仓库。

## 14. Tauri Command 接口

主要 command 位于 `src-tauri/src/commands.rs`：

```text
create_repository(parent_path, name, encryption_algorithm?, encryption_password?)
open_repository(repository_path)
rename_repository(repository_path, display_name)
unlock_repository(repository_path, encryption_password)
change_repository_password(repository_path, old_password, new_password)
delete_repository(repository_path, encryption_password?)

backup(sources, destination, filter?, compression_algorithm?, snapshot_title?, encrypt_snapshot?, encryption_password?)
restore(backup_path, snapshot_id, destination, path_strategy?, flatten_conflict_strategy?, decryption_password?)
list_snapshots(repository_path)
delete_snapshot(repository_path, snapshot_id, encryption_password?)

export_repository(repository_path, archive_path, algorithm?)
import_repository(archive_path, destination, algorithm?)
```

DTO 位于 `src-tauri/src/dto.rs`。跨 Tauri 边界字段使用 `camelCase`，进入 core 前转换为 Rust 内部类型。

## 15. 测试分布

当前测试主要分三类：

```text
crates/backup-core/tests/repository.rs
    core 层 repository、snapshot、object、压缩、加密、恢复、tar 测试

src-tauri/src/commands_tests.rs
    Tauri command 层 DTO、路径校验、命令行为和错误映射测试

src/state.test.ts
    前端仓库状态、排序、置顶、归档、草稿和路由回退测试
```

常用验证命令：

```powershell
just check
just test
```

`just check` 包含：

```powershell
.\node_modules\.bin\tsc.cmd
cargo check --workspace
```

`just test` 包含：

```powershell
pnpm.cmd test
cargo test --workspace
```

## 16. 构建和运行

首次安装依赖：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\bootstrap-windows.ps1
just setup
```

开发运行：

```powershell
just dev
```

构建 release：

```powershell
just build
```

构建后可执行文件：

```text
target/release/backup-tool.exe
```

完整环境配置见 [DEVELOPMENT.md](DEVELOPMENT.md)。

## 17. WSL 与 POSIX 文件系统

Windows 应用访问 WSL 文件时使用 UNC 路径，例如：

```text
\\wsl.localhost\Ubuntu-24.04\home\user\demo
```

`AutoFileSystemProvider` 会为这类路径选择 POSIX provider。扫描阶段通过 `wsl.exe stat` 获取真实 Linux 文件类型、大小、修改时间、模式、UID、GID 和设备主次编号；因此筛选条件不会混用 Windows UNC 元数据：

- `path_regex` 匹配规范化后的相对路径。
- `owner` 同时接受 Linux 用户名或 UID，例如 `root` / `0`、`sinphnice` / `1000`。
- `min_size`、`max_size`、`modified_after`、`modified_before` 使用同一份 Linux `stat` 元数据。
- 目录始终继续遍历；普通文件、符号链接、FIFO、设备节点和其他非目录节点会应用筛选条件。

快照 entry 当前的文件类型包括：

- `Directory`
- `File`
- `Symlink`
- `Fifo`
- `Device`
- `Other`

恢复到 WSL 时，符号链接、FIFO 和字符/块设备分别通过 Linux `ln -s`、`mkfifo`、`mknod` 创建；随后按恢复策略尽力回写 POSIX `mode`、`uid/gid` 和修改时间。目录元数据必须延后到子项写入后处理，避免先恢复 restrictive 权限导致子项无法创建。Unix socket 当前归为 `Other`，不支持恢复为可工作的 socket。

密码属于前端会话敏感状态，不写入 Store。GUI 为所有密码输入框提供应用自绘的显示/隐藏控件，而不是依赖 WebView2 的原生密码揭示按钮。

## 18. 后续扩展方向

当前架构为后续能力预留了几个方向：

- 更完整的文件系统 provider，包括特殊文件、符号链接和权限恢复策略。
- 更丰富的 snapshot 管理，例如标签、搜索、差异比较和保留策略。
- object 垃圾回收和仓库一致性检查。
- 更多归档算法，例如自定义 pack、zip、压缩 tar。
- 更细粒度的压缩参数和压缩率统计。
- 加密元数据保护，例如加密路径和 snapshot 文件。
- 定时备份和实时监听备份。
- 更完善的 GUI 主题、图标、交互和无障碍支持。

这些扩展应继续遵循当前分层：先在 `backup-core` 形成可测试模型，再通过 Tauri command 暴露，最后接入 GUI。
