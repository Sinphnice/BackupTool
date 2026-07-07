# BackupTool 后续开发计划

本文记录项目迁移为纯 Rust 后端后的开发路线。环境配置、依赖安装和构建命令见 [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md)；本文只描述功能开发顺序、架构边界、接口方向和验收标准。

## 当前状态

项目已经调整为纯 Rust 后端：

```text
GUI -> TypeScript -> Tauri command -> Rust backup-core
```

当前已经具备：

- Tauri 2 桌面应用骨架。
- TypeScript + HTML + CSS 前端。
- Rust/Tauri command 薄适配层。
- 独立 Rust 核心库 `crates/backup-core`。
- legacy mirror backup：普通目录、普通文件的目录镜像备份与恢复。
- 路径、扩展名、文件名、修改时间、文件大小筛选。
- `just` 顶层构建编排。

## 总体原则

- 后续主要业务逻辑放在 `crates/backup-core/`。
- `backup-core` 必须可以脱离 Tauri 和 GUI 独立测试。
- Rust/Tauri command 只负责参数转换、返回值转换、错误字符串转换和必要事件转发。
- TypeScript/GUI 只负责界面交互、参数收集、进度展示和结果展示。
- GUI 不直接实现备份、恢复、压缩、打包、加密等核心算法。
- Tauri command 不承载主要业务逻辑。
- 不提前实现完整异步任务系统，按“同步流程 -> 结果对象 -> 进度回调 -> 取消 -> 异步任务 -> 暂停/恢复”的顺序演进。
- 不进行与当前 Milestone 无关的 GUI 美化、框架扩展或依赖引入。
- 课程要求中的“独立实现”优先：核心备份格式、筛选、打包、压缩、加密逻辑不直接依赖第三方备份库完成。

## 架构边界

核心分层方向：

```text
src/ GUI
    -> src-tauri/ Tauri command DTO
        -> crates/backup-core/ Backup domain
```

`backup-core` 后续应逐步形成以下模块：

- `legacy_mirror`：保留当前目录镜像备份/恢复能力，作为基础验收和兼容基线。
- `repository`：仓库式备份入口，管理 snapshot、manifest 和 object store。
- `filesystem`：文件系统抽象，隔离 Windows/NTFS 与 POSIX/ext3/ext4 差异。
- `metadata`：通用元数据模型，记录时间、权限、属主、平台扩展信息。
- `archive`：单文件打包和解包。
- `compression`：压缩和解压。
- `encryption`：加密和解密。
- `task`：进度、取消、异步任务和后续暂停/恢复。

## Milestone 1：Rust 迁移与 legacy 功能稳定

目标：完成纯 Rust 后端迁移，并保证当前目录镜像备份、恢复、筛选功能稳定。

已完成方向：

- 移除 C++ Core、C ABI、CMake/MSBuild 构建链。
- 新增 `crates/backup-core`。
- Tauri command 调用 Rust core，不再通过 FFI。
- 保留 `backup`、`restore`、`core_version` command。
- 保留 GUI 最小备份/恢复/筛选界面。

验收要求：

- `just check` 通过。
- `just test` 通过。
- `just build` 通过。
- 构建链不再调用 CMake/MSBuild。
- 测试覆盖普通目录备份、恢复、筛选和错误传播。

## Milestone 2：仓库式备份设计

目标：从目录镜像复制演进为真正的备份仓库格式。

核心对象：

- `Repository`
- `Snapshot`
- `Manifest`
- `ManifestEntry`
- `ObjectStore`
- `ObjectId`
- `ContentHasher`
- `RepositoryWriter`
- `RepositoryReader`

仓库目录建议形态：

```text
repository/
├── repo.meta
├── snapshots/
│   └── <snapshot-id>.manifest
├── objects/
│   └── <object-id>
└── indexes/
```

第一版 repository 只要求：

- 支持普通目录和普通文件。
- 每次备份生成一个 snapshot。
- manifest 记录相对路径、文件类型、大小、修改时间、内容对象 ID。
- object store 保存文件内容。
- 恢复时从指定 snapshot 还原目录结构和文件内容。
- 不要求增量、压缩、加密和网络。

验收要求：

- 同一源目录连续备份生成多个 snapshot。
- 可以选择指定 snapshot 恢复。
- 恢复结果与源目录普通文件内容一致。
- legacy mirror backup 仍可作为单独模式保留。

## Milestone 3：文件系统抽象与元数据模型

目标：为 Windows/NTFS 和 POSIX/ext3/ext4 差异建立架构边界。

核心对象：

- `FileSystemProvider`
- `FileSystemWriter`
- `FileEntry`
- `FileType`
- `Metadata`
- `PlatformMetadata`

第一版能力：

- `BasicFileSystemProvider` 基于 Rust 标准库，支持普通文件和目录。
- `WindowsFileSystemProvider` 优先识别 NTFS 常见能力：创建时间、访问时间、修改时间、只读属性、符号链接或 reparse point。
- `PosixFileSystemProvider` 作为后续扩展，识别 mode、uid、gid、symlink、fifo、device 等。

恢复策略：

- 默认 `best_effort`：尽量恢复数据和可支持元数据，不支持的项目记录 warning。
- 后续可增加 `strict` 和 `data_only` 模式。

验收要求：

- manifest 能表达普通文件、目录、符号链接和平台元数据占位。
- Windows 下至少能保存和恢复普通文件修改时间与只读属性。
- 不支持的元数据不会导致普通文件恢复失败。

## Milestone 4：单文件打包与解包

目标：基于 repository manifest/object 格式实现单文件备份包。

核心对象：

- `ArchiveWriter`
- `ArchiveReader`
- `ArchiveHeader`
- `ArchiveIndex`

第一版能力：

- 将一个 snapshot 及其依赖 objects 打包为单个文件。
- 从单文件备份包读取 manifest 和对象数据并恢复。
- 文件格式预留压缩和加密标记位，但不提前实现算法。

验收要求：

- 打包后可以删除原 repository，再从单文件包恢复。
- 包格式能检测基本损坏，例如 magic/version 不匹配。

## Milestone 5：压缩与解压

目标：在仓库和打包格式稳定后增加压缩能力。

核心对象：

- `Compressor`
- `Decompressor`
- `CompressionAlgorithm`

第一版能力：

- 至少实现一种课程要求可接受的压缩算法。
- 明确压缩粒度：优先采用 object 级压缩，便于后续增量和随机恢复。
- manifest 或 object metadata 记录压缩算法和原始大小。

验收要求：

- 压缩备份可正确恢复。
- 测试覆盖文本文件压缩、二进制文件压缩和空文件。

## Milestone 6：加密与解密

目标：增加备份数据保护能力。

核心对象：

- `Encryptor`
- `Decryptor`
- `KeyDeriver`
- `EncryptionAlgorithm`

约束：

- 文档中明确区分教学实现和真实工程安全原则。
- 不把密码或密钥写入日志、manifest 明文敏感字段或错误信息。
- 解密失败返回明确但不泄露敏感信息的错误。

验收要求：

- 加密备份可用正确密码恢复。
- 错误密码恢复失败。
- 测试确认日志和错误信息不包含密码。

## Milestone 7：GUI 完善与任务状态

目标：在基础业务能力稳定后完善用户体验。

可逐步增加：

- 任务列表。
- 进度条。
- 当前文件。
- 处理速度。
- 已处理数量。
- 剩余量。
- 错误提示。
- 历史记录。
- 配置页面。

约束：

- GUI 完善不改变 `backup-core` 的业务边界。
- 复杂状态管理只在确有需要时引入。

## Milestone 8：定时备份与淘汰策略

目标：支持周期性备份和旧备份清理。

核心对象：

- `ScheduleConfig`
- `Scheduler`
- `RetentionPolicy`

第一版能力：

- 支持每日、每周、固定间隔备份。
- 计算下次执行时间。
- 触发 repository snapshot 备份。
- 支持保留最近 N 个 snapshot 或保留最近 N 天 snapshot。

## Milestone 9：实时备份

目标：监听文件变化并触发备份策略。

核心对象：

- `FileWatcher`
- `FileChangeEvent`
- `DebouncePolicy`

约束：

- Windows 和 Linux 的文件变化 API 不同，必须通过抽象接口隔离。
- 第一版可以优先支持 Windows。
- 实时备份需要考虑事件合并和防抖，避免频繁重复备份。

## Milestone 10：网络备份、增量备份和远程元数据

目标：将单机备份能力扩展为可选网络备份模式。

方向：

- 增量备份：基于 object hash 和 snapshot manifest 避免重复存储。
- 远程存储：抽象 `RemoteObjectStore`。
- 用户管理：仅在课程范围和时间允许时实现。
- 传输加密：优先复用加密模块和 TLS 能力。
- 远程元数据管理：同步 repository metadata、snapshot index 和 object index。

## 测试策略

`backup-core` 测试优先，不依赖 Tauri、Node.js 或 GUI。

第一批测试覆盖：

- 空目录扫描。
- 单文件目录扫描。
- 多级目录扫描。
- 深层嵌套目录。
- 文件名包含空格。
- Unicode 文件名。
- 普通目录完整备份。
- 空目录备份。
- 目标目录不存在。
- 源路径不存在。
- 源路径不是目录。
- 目标路径冲突。
- 备份后恢复。
- 恢复后比较目录结构、文件数量、文件大小和文件内容。
- 路径、扩展名、文件名、时间、尺寸筛选。

项目级验证继续使用现有入口：

```powershell
just check
just test
just build
```

## 当前默认假设

- 项目后端采用纯 Rust，不再恢复 C++ Core 或 C ABI。
- 当前目录镜像备份作为 legacy mirror backup 保留。
- 下一步优先设计 repository/snapshot/manifest/object store。
- 打包、压缩、加密、定时备份、实时备份、网络备份均不提前实现。
- Tauri command 和 TypeScript 不承载核心业务逻辑。
- `PLAN.md` 是开发路线文档，不替代环境配置文档。
