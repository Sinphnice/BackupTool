# BackupTool 项目概览

本文用于帮助开发者快速理解 BackupTool 的项目组成、架构设计、核心数据模型、构建方式和测试分布。环境安装细节见 [DEVELOPMENT.md](DEVELOPMENT.md)，课程要求见 [course.md](course.md)，后续开发规划见 [../.agents/PLAN.md](../.agents/PLAN.md)。

## 1. 项目定位

BackupTool 是一个基于 Tauri 2 的桌面备份工具。当前版本已经从早期 C++ Core + Rust FFI 方案迁移为纯 Rust 后端核心，整体调用链为：

```text
TypeScript GUI
    -> Tauri command
        -> Rust backup-core
```

当前备份格式采用目录型 repository，而不是简单镜像复制。每次备份生成一个 snapshot manifest，普通文件内容写入 object store；恢复时选择 snapshot，并按恢复路径策略将对象内容还原到目标目录。

当前已经支持：

- 多源目录备份。
- 源路径规范化、去重、父子目录去重。
- repository 初始化、打开、备份、恢复。
- snapshot 列表读取和 GUI 选择。
- 路径、扩展名、文件名、文件大小、修改时间筛选。
- 三种恢复路径策略：`PreserveFullPath`、`PreserveRelativePath`、`Flatten`。
- `Flatten` 冲突策略：`Error`、`Skip`、`Overwrite`、`Rename`。
- 基础文件元数据记录与恢复策略。
- repository 导出为 `.tar`，以及从 `.tar` 导入为 repository。
- core 层和 Tauri command 层自动化测试。

当前不再保留 legacy mirror backup，也就是单纯目录复制式备份。GUI 的备份、恢复、归档操作都围绕 repository 模型工作。

## 2. 项目结构

```text
BackupTool/
├── src/                    # 前端界面：TypeScript / HTML / CSS
├── src-tauri/              # Tauri 桌面应用层：command、DTO、权限、窗口配置
├── crates/
│   └── backup-core/        # 纯 Rust 备份核心库
├── scripts/                # Windows 环境检查和开发辅助脚本
├── docs/                   # 项目文档
├── .agents/                # 开发规划和 agent 工作资料，不进入远程仓库
├── Cargo.toml              # Rust workspace
├── package.json            # 前端与 Tauri CLI 依赖
├── pnpm-lock.yaml          # 前端依赖锁定文件
└── justfile                # 项目级任务入口
```

主要分层：

| 层次 | 路径 | 技术 | 职责 |
| --- | --- | --- | --- |
| GUI 层 | `src/` | TypeScript, HTML, CSS, Vite | 目录选择、参数输入、snapshot 选择、结果展示 |
| 桌面适配层 | `src-tauri/` | Rust, Tauri 2, serde | 暴露 Tauri command、DTO 转换、路径校验、错误转换 |
| 核心业务层 | `crates/backup-core/` | Rust | repository、snapshot、manifest、object store、恢复策略、tar 归档 |
| 构建编排 | `justfile` | just | 统一组织安装、检查、测试、开发运行和构建 |
| 环境脚本 | `scripts/` | PowerShell | 检查 Windows 开发依赖，或单独启动 Vite |

## 3. 依赖与构建工具

系统工具：

- Windows 10/11 x64。
- Git。
- Node.js 22 LTS 或更新版本。
- pnpm 11.7.0。
- Rust stable MSVC toolchain。
- just 1.55 或更新版本。

前端依赖由 `pnpm` 管理：

- `@tauri-apps/api`：前端调用 Tauri command。
- `@tauri-apps/plugin-dialog`：目录和文件选择对话框。
- `@tauri-apps/cli`：Tauri 开发运行和打包。
- `typescript`：类型检查。
- `vite`：前端开发服务和静态资源构建。

Rust 依赖由 Cargo workspace 管理：

- 根目录 `Cargo.toml` 声明 workspace。
- `crates/backup-core` 是核心库，当前依赖 `tar` crate 实现跨平台 tar 打包和解包。
- `src-tauri` 是 Tauri 应用 crate，依赖 `backup-core`、`serde`、`tauri`、`tauri-plugin-dialog`。

当前项目不再需要 CMake、MSBuild、C ABI 或 C++ 编译链。

## 4. 架构设计

系统分为三层：

```text
GUI 层
    负责输入、交互和展示

Tauri command 层
    负责桌面应用边界、DTO 转换和错误映射

backup-core 核心层
    负责备份、恢复、筛选、仓库、快照、归档等核心业务
```

实际调用路径：

```text
src/main.ts
    -> invoke("backup" / "restore" / "list_snapshots" / "export_repository" / "import_repository")
        -> src-tauri/src/commands.rs
            -> crates/backup-core
```

分层原则：

- GUI 不实现备份算法。
- Tauri command 不堆积复杂业务逻辑。
- `backup-core` 不依赖 Tauri、Node.js 或 GUI。
- 新业务能力优先在 `backup-core` 中形成可测试 API，再向 Tauri 和 GUI 暴露。
- 前端交互可以影响 DTO 设计，但不应反向污染核心业务模型。

## 5. Repository 设计

repository 是一个目录型备份仓库，磁盘结构为：

```text
repository/
├── repo.meta
├── objects/
├── snapshots/
└── indexes/
```

各部分职责：

- `repo.meta`：仓库标识文件，用于判断目录是否为合法 BackupTool repository。
- `objects/`：保存普通文件内容。对象名由内容 hash 和文件大小组成，相同内容可以复用。
- `snapshots/`：保存每次备份生成的 manifest，例如 `snapshot-xxxx.manifest`。
- `indexes/`：当前已创建，主要为后续索引能力预留。

一次备份的大致流程：

```text
用户选择一个或多个源目录
    -> 源路径规范化和去重
    -> 扫描目录树
    -> BackupFilter 判断普通文件是否进入备份
    -> 文件内容写入 objects/
    -> snapshots/<snapshot-id>.manifest 记录结构和对象引用
    -> 返回 snapshot id、文件数、字节数
```

一次恢复的大致流程：

```text
用户打开 repository
    -> list_snapshots 读取可用 snapshot
    -> 用户选择 snapshot 和恢复策略
    -> 读取 manifest
    -> 从 objects/ 读取对象内容
    -> 按路径策略写入恢复目标目录
```

## 6. 归档设计

当前支持将整个 repository 导出为单个 `.tar` 文件，也支持从 `.tar` 导入为可打开的 repository。

归档能力定位为 repository 的导出/导入格式，而不是替代 repository 本身：

```text
repository/
    -> export_repository
        -> repository.tar

repository.tar
    -> import_repository
        -> repository/
            -> Open Repository
            -> Restore snapshot
```

当前只支持未压缩 tar：

- 不实现 `.tar.gz`、`.zip`、压缩、加密或分卷。
- 使用 Rust `tar` crate，不调用系统 `tar` 命令。
- tar 内部只保存相对路径。
- 导出内容限定为 `repo.meta`、`objects/`、`snapshots/`、`indexes/`。
- 如果导出目标 `.tar` 放在 repository 根目录下，不会被作为仓库内容打入 tar。
- 导入目标目录必须不存在或为空，避免和已有文件混合。
- 导入时拒绝绝对路径、`..`、symlink 等非常规 tar entry，避免不安全解包。

对应 Tauri command：

```text
export_repository(repository_path, archive_path, algorithm?)
import_repository(archive_path, destination, algorithm?)
```

`algorithm` 当前默认为 `tar`，未知算法会返回错误。

## 7. 关键类型与关系

核心类型主要位于 `crates/backup-core/src/lib.rs` 和 `crates/backup-core/src/repository.rs`。

```text
Repository
├── init(root)
├── open(root)
├── writer() -> RepositoryWriter
├── reader() -> RepositoryReader
├── export_archive(output_file, ArchiveAlgorithm)
└── import_archive(archive_file, destination, ArchiveAlgorithm)

RepositoryWriter
└── backup_many(sources, filter) -> Snapshot

RepositoryReader
├── list_snapshots() -> Vec<SnapshotInfo>
├── read_manifest(snapshot_id) -> Manifest
└── restore_with_options(snapshot_id, destination, RestoreOptions)

Manifest
├── snapshot_id: SnapshotId
├── sources: Vec<SourceInfo>
└── entries: Vec<ManifestEntry>

ManifestEntry
├── source_index
├── relative_path
├── kind
├── size
├── modified_unix_seconds
├── object_id
└── metadata

ObjectStore
├── write_object(bytes) -> ObjectId
└── read_object(object_id) -> Vec<u8>
```

恢复相关策略：

```text
RestoreOptions
├── strategy: RestoreStrategy
├── path_strategy: RestorePathStrategy
└── flatten_conflict_strategy: FlattenConflictStrategy
```

Tauri DTO 位于 `src-tauri/src/dto.rs`：

- `BackupFilterDto`：前端筛选条件。
- `BackupResultDto`：备份结果。
- `RestoreResultDto`：恢复结果。
- `SnapshotInfoDto`：snapshot 列表展示数据。
- `ArchiveResultDto`：仓库导出/导入结果。

## 8. GUI 功能入口

当前 GUI 主要分为四块：

- `Backup`：添加一个或多个源目录，选择 repository 目录，执行备份。
- `Filters`：配置路径、扩展名、文件名、大小、修改时间筛选。
- `Restore`：打开 repository，自动加载 snapshot，选择恢复路径策略和目标目录。
- `Repository Archive`：导出 repository 为 `.tar`，或从 `.tar` 导入 repository。

GUI 只负责收集参数和展示结果，不直接操作 repository 文件结构。

## 9. 环境配置

首次配置：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\bootstrap-windows.ps1
just setup
```

只检查环境、不安装：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\bootstrap-windows.ps1 -CheckOnly
```

脚本会检查或准备：

- Git。
- Rust stable MSVC toolchain。
- Node.js。
- pnpm 11.7.0。
- just。
- 项目 Node 依赖。

如果 PowerShell 执行策略阻止 `pnpm.ps1`，使用 `pnpm.cmd`。本项目的 `justfile` 已统一调用 `.cmd` 入口。

## 10. 构建、运行与测试

列出所有任务：

```powershell
just
```

安装前端依赖：

```powershell
just setup
```

检查前端和 Rust workspace：

```powershell
just check
```

内部执行：

```text
tsc.cmd
cargo check --workspace
```

运行测试：

```powershell
just test
```

内部执行：

```text
cargo test --workspace
```

启动开发模式：

```powershell
just dev
```

`just dev` 会先启动 Vite，再启动 Tauri。Vite 服务地址为：

```text
http://127.0.0.1:1420
```

只启动 Vite：

```powershell
just vite
```

构建 release 应用和安装包：

```powershell
just build
```

构建并运行 release 可执行文件：

```powershell
just run
```

清理构建产物：

```powershell
just clean
```

release 可执行文件通常位于：

```text
target/release/backup-tool.exe
```

安装包通常位于：

```text
target/release/bundle/
```

## 11. 测试覆盖

当前测试分为两层。

`backup-core` 测试覆盖：

- repository 目录结构创建。
- snapshot manifest 写入和读取。
- 连续备份生成不同 snapshot。
- 多源备份、重复源去重、父子源去重。
- 按 snapshot 恢复历史版本。
- 路径筛选和元数据恢复策略。
- 三种恢复路径策略。
- `Flatten` 冲突策略。
- repository 导出 tar、导入 tar、导入后恢复。
- 非 repository 导出失败。
- 导入到非空目录失败。

Tauri command 层测试覆盖：

- `backup` command。
- `restore` command。
- `list_snapshots` command。
- `export_repository` command。
- `import_repository` command。
- DTO 转换和错误字符串返回。
- 未知归档算法错误。
- 非空导入目标错误。

提交前建议至少运行：

```powershell
just check
just test
```

最近一次验证结果：

```text
just check 通过
just test 通过
```

## 12. 开发方法

新增功能建议按以下顺序推进：

```text
1. 在 backup-core 中设计核心类型和业务逻辑
2. 为 backup-core 添加测试
3. 在 src-tauri 中添加 DTO 和 command
4. 为 command 层添加测试
5. 在 src/ 中做最小 GUI 集成
6. 运行 just check 和 just test
```

开发时应避免：

- 在 TypeScript 中实现核心备份逻辑。
- 在 Tauri command 中堆积复杂业务流程。
- 为临时界面需求破坏核心库模型。
- 在核心能力不稳定前过早做复杂 GUI。

## 13. Object 级压缩设计

当前压缩能力作用于 repository 的 `objects/`，粒度是单个 object，而不是整个仓库目录或整个 tar 包。备份时可以选择压缩算法：

- `none`：默认值，不压缩 payload。
- `zstd`：使用 zstd 压缩 payload。

`object-id` 始终由原始文件内容计算，不由 object header、压缩后的 payload 或后续可能加入的加密信息计算。这一点很重要：同一份原始文件内容即使采用不同压缩算法，逻辑 object id 也保持一致。

object 物理路径统一为：

```text
objects/<object-id>
```

object 文件内部使用文本 header + 二进制 payload：

```text
backup-tool object v1
compression    none|zstd
original_size  <u64>
payload_size   <u64>

<payload bytes>
```

压缩算法记录在 object header 中，不记录在 manifest entry 中。压缩和解压只处理空行之后的 payload bytes，不处理 header。若同一 `object-id` 已存在但 header 中的 compression 与本次备份选择不同，会用同一份原始数据按本次算法重新生成 object 并覆盖；由于 object id 对应的原始内容不变，旧 snapshot 仍可恢复出相同文件内容。

恢复流程中用户不需要选择压缩算法：

```text
manifest entry
    -> object_id
    -> ObjectStore 读取 objects/<object-id>
    -> 解析 object header 中的 compression
    -> 如果是 zstd 则解压 payload
    -> 写回原始文件内容
```

tar 导出/导入会保留 `objects/` 下的自描述 object 文件，因此压缩 repository 可以作为 tar 文件迁移到其他系统后再恢复。

后续规划以 `.agents/PLAN.md` 为准。当前主线应继续围绕 repository、snapshot、manifest、object store、archive、compression、encryption 等核心模型演进。压缩已经具备 object 级 zstd 第一版，后续可继续扩展压缩率统计、压缩等级配置和更多算法。
