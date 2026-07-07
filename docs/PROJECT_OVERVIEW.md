# BackupTool 项目说明

本文用于帮助开发者快速理解 BackupTool 的项目构成、架构设计、核心类型、构建方法和后续开发边界，也可作为后续系统设计文档的基础材料。环境安装细节见 [DEVELOPMENT.md](DEVELOPMENT.md)，课程要求见 [course.md](course.md)，后续路线见 [../.agents/PLAN.md](../.agents/PLAN.md)。

## 1. 项目概述

BackupTool 是《软件开发综合实验》课程项目，目标是实现一款桌面端数据备份软件。当前版本已经从早期 C++ Core + Rust FFI 方案调整为纯 Rust 后端方案，整体调用链为：

```text
TypeScript GUI
    -> Tauri command
        -> Rust backup-core
```

当前采用 repository backup：每次备份生成一个 snapshot manifest，普通文件内容保存到 object store，恢复时按用户指定的 snapshot id 还原目录结构和文件内容。

早期 legacy mirror backup，也就是单纯目录复制形式的备份，已经从核心库、Tauri command 和测试中移除。当前 GUI 的备份/恢复按钮调用的是 repository backup / restore。

当前已支持：

- 普通目录备份。
- 普通目录恢复。
- 保留相对目录结构。
- 自动创建目标目录。
- 路径、扩展名、文件名、修改时间、文件大小筛选。
- 仓库目录初始化。
- snapshot manifest 写入和读取。
- object store 保存普通文件内容。
- 按指定 snapshot 恢复普通文件和目录结构。
- 备份后返回 snapshot id。
- GUI 触发备份和恢复。
- 核心库和 Tauri command 层自动化测试。

尚未实现：

- snapshot 列表和选择界面。
- 特殊文件处理和完整元数据恢复。
- 打包、压缩、加密。
- 定时备份、实时备份、网络备份。

## 2. 项目结构

```text
BackupTool/
├── src/                    # 前端界面：TypeScript / HTML / CSS
├── src-tauri/              # Tauri 桌面应用层：命令、DTO、窗口配置
├── crates/
│   └── backup-core/        # 纯 Rust 备份核心库
├── scripts/                # Windows 环境检查和开发辅助脚本
├── docs/                   # 面向开发和课程提交的公开文档
├── .agents/                # 开发规划和 agent 工作资料，不提交远程
├── Cargo.toml              # Rust workspace
├── package.json            # 前端和 Tauri CLI 依赖
├── pnpm-lock.yaml          # 前端依赖锁定文件
└── justfile                # 顶层任务入口
```

主要组成如下：

| 层次 | 路径 | 技术 | 职责 |
| --- | --- | --- | --- |
| GUI 层 | `src/` | TypeScript, HTML, CSS, Vite | 收集用户输入、调用 Tauri command、展示执行结果。 |
| 桌面应用层 | `src-tauri/` | Rust, Tauri 2, serde | 提供桌面窗口、命令入口、DTO 转换、路径输入校验。 |
| 核心业务层 | `crates/backup-core/` | Rust | 实现 repository、snapshot、manifest、object store、筛选和错误模型。 |
| 构建编排 | `justfile` | just | 统一组织安装、检查、测试、开发运行和打包。 |
| 环境脚本 | `scripts/` | PowerShell | 检查并准备 Windows 开发依赖，或单独启动 Vite。 |

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
- `@tauri-apps/cli`：Tauri 开发运行和打包。
- `typescript`：类型检查。
- `vite`：前端开发服务和静态资源构建。

Rust 依赖由 Cargo workspace 管理：

- 根目录 `Cargo.toml` 声明 workspace。
- `crates/backup-core` 是独立核心库，目前不依赖第三方 crate。
- `src-tauri` 是 Tauri 应用 crate，依赖 `backup-core`、`serde`、`tauri`。

当前项目不再需要 CMake、MSBuild、C ABI 或 C++ 编译链。

## 4. 架构设计

系统分为三层：

```text
GUI 层
    负责输入和展示

Tauri 命令层
    负责桌面应用入口、DTO 转换和错误映射

backup-core 核心层
    负责备份、恢复、筛选和后续核心业务模型
```

实际调用路径：

```text
src/main.ts
    -> invoke("backup" / "restore")
        -> src-tauri/src/commands.rs
            -> crates/backup-core/src/lib.rs
```

分层原则：

- GUI 不实现备份算法。
- Tauri command 不堆积业务逻辑。
- `backup-core` 不依赖 Tauri、Node.js 或 GUI。
- 新业务能力先在 `backup-core` 中形成可测试 API，再向 Tauri 和 GUI 暴露。
- 桌面交互可以影响 DTO 设计，但不应反向污染核心业务模型。

## 5. 关键类型与关系

当前核心业务集中在 `crates/backup-core/src/lib.rs` 和 `crates/backup-core/src/repository.rs`。

筛选条件：

```text
BackupFilter
├── include_path_contains
├── exclude_path_contains
├── extensions
├── include_name_contains
├── exclude_name_contains
├── min_size
├── max_size
├── modified_after
└── modified_before
```

错误模型：

```text
BackupError
├── EmptyPath
├── SourceDoesNotExist
├── SourceIsNotDirectory
├── Io
└── InvalidModifiedTime
```

Tauri 命令层的 DTO 位于 `src-tauri/src/dto.rs`：

- `BackupFilterDto`：接收前端 camelCase 筛选字段。
- `BackupResultDto`：返回备份结果。
- `RestoreResultDto`：返回恢复结果。
- `impl From<BackupFilterDto> for BackupFilter`：把界面输入转换为核心库筛选模型。

重要关系：

```text
TypeScript BackupFilter
    -> BackupFilterDto
        -> BackupFilter
            -> RepositoryWriter::backup
```

仓库式备份类型：

```text
Repository
├── init(root)
├── open(root)
├── writer() -> RepositoryWriter
└── reader() -> RepositoryReader

RepositoryWriter
└── backup(source, filter) -> Snapshot

RepositoryReader
├── read_manifest(snapshot_id) -> Manifest
└── restore(snapshot_id, destination)

Manifest
├── snapshot_id: SnapshotId
└── entries: Vec<ManifestEntry>

ManifestEntry
├── relative_path: PathBuf
├── kind: FileKind
├── size: u64
├── modified_unix_seconds: Option<i64>
└── object_id: Option<ObjectId>

ObjectStore
├── write_object(bytes) -> ObjectId
└── read_object(object_id) -> Vec<u8>
```

## 6. 当前业务流程

备份流程：

```text
用户填写源目录、repository 目录、筛选条件
    -> 前端收集表单
    -> invoke("backup")
    -> Tauri 反序列化 BackupFilterDto
    -> commands::backup 打开或初始化 Repository
    -> RepositoryWriter::backup
    -> 遍历源目录
    -> BackupFilter::allows 判断普通文件是否进入备份
    -> 普通文件内容写入 object store
    -> snapshots/<snapshot-id>.manifest 记录快照
    -> 返回 BackupResultDto
    -> GUI 显示文件数、字节数和 snapshot id
```

恢复流程：

```text
用户填写 repository 目录、snapshot id 和恢复目录
    -> 前端调用 invoke("restore")
    -> commands::restore 打开 Repository
    -> RepositoryReader::read_manifest 读取指定 snapshot
    -> RepositoryReader::restore
    -> 按 manifest 创建目录并从 object store 读取文件内容
    -> 返回 RestoreResultDto
    -> GUI 显示文件数和字节数
```

## 7. 环境配置

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

如果 PowerShell 执行策略阻止 `pnpm.ps1`，使用 `pnpm.cmd`。本项目的 `justfile` 已经统一调用 `.cmd` 入口。

## 8. 构建、运行与测试

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

`just dev` 会启动 Vite，再启动 Tauri。Vite 服务地址为：

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

## 9. 测试覆盖

当前测试分为两层。

`backup-core` 测试覆盖：

- 分号分隔筛选列表解析。
- repository 目录结构创建。
- snapshot manifest 写入和读取。
- 连续备份生成不同 snapshot。
- 按指定 snapshot 恢复不同历史版本。
- repository 备份筛选。
- 缺失 snapshot 错误。

Tauri command 层测试覆盖：

- `backup` command 调用核心库。
- `restore` command 调用核心库。
- 筛选 DTO 转换。
- 备份结果返回 snapshot id。
- 核心库错误转换为字符串。

提交前建议至少运行：

```powershell
just check
just test
```

最近一次验证结果：

```text
just check 通过
just test 通过，8 个测试全部通过
```

## 10. 开发方法

新增功能时，推荐按以下顺序推进：

```text
1. 在 backup-core 中设计类型和业务逻辑
2. 为 backup-core 添加单元测试
3. 在 src-tauri 中添加 DTO 和 command
4. 为 command 层添加测试
5. 在 src/ 中做最小 GUI 集成
6. 运行 just check 和 just test
```

开发时应避免：

- 在 TypeScript 中实现核心备份逻辑。
- 在 Tauri command 中实现复杂业务流程。
- 为临时界面需求破坏核心库模型。
- 在核心能力不稳定前过早做复杂 GUI。
- 在当前阶段提前实现打包、压缩、加密、定时、实时和网络备份。

## 11. 后续设计方向

后续规划以 `.agents/PLAN.md` 为准。当前已经完成第一版 repository/snapshot/manifest/object store，并已接入 Tauri command 和 GUI。下一步重点是围绕该模型继续扩展元数据、打包、压缩和加密：

```text
Repository
    -> Snapshot
        -> Manifest
            -> ObjectStore
```

预期模块方向：

- `repository`：管理仓库、快照、manifest 和对象存储。
- `filesystem`：隔离不同平台的文件系统差异。
- `metadata`：记录时间、权限、属主和平台扩展元数据。
- `archive`：单文件打包和解包。
- `compression`：压缩和解压。
- `encryption`：加密和解密。
- `task`：进度、取消、异步任务和后续暂停/恢复。

近期不应直接进入 GUI 美化或高级功能，而应优先稳定 repository、snapshot、manifest、object store 这些核心模型，并补充 snapshot 列表、选择与错误提示等必要操作能力。
