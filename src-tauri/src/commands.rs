use crate::dto::{
    ArchiveResultDto, BackupFilterDto, BackupResultDto, FlattenConflictStrategyDto,
    RepositoryInfoDto, RestorePathStrategyDto, RestoreResultDto, SnapshotDeleteResultDto,
    SnapshotInfoDto,
};
use backup_core::{
    ArchiveAlgorithm, BackupError, BackupOptions, CompressionAlgorithm, EncryptionAlgorithm,
    FileKind, Repository, RestoreOptions, SnapshotFile, SnapshotId,
};
use std::fs;
use std::path::{Path, PathBuf};

// Tauri command 层保持“薄适配”：校验 GUI 输入、转换 DTO，并把 core 错误映射成字符串。
/// 新建 repository，并把目录名作为第一版显示名写入仓库元数据。
#[tauri::command]
pub(crate) fn create_repository(
    parent_path: String,
    name: String,
    encryption_algorithm: Option<String>,
    encryption_password: Option<String>,
) -> Result<RepositoryInfoDto, String> {
    let parent = path_from_input(parent_path, "repository parent")?;
    if !parent.is_dir() {
        return Err(format!(
            "repository parent is not a directory: {}",
            parent.display()
        ));
    }
    let name = validate_repository_name(&name)?;
    let target = parent.join(name);
    if target.exists() {
        return Err(format!(
            "repository path already exists: {}",
            target.display()
        ));
    }
    let encryption_algorithm = encryption_algorithm_from_input(encryption_algorithm)?;
    validate_encryption_password(encryption_algorithm, encryption_password.as_deref())?;
    let repository = Repository::init_with_options(
        target,
        Some(name.to_string()),
        encryption_algorithm,
        encryption_password,
    )
    .map_err(|error| error.to_string())?;
    repository_info(&repository)
}

/// 打开已有 repository，返回前端侧栏需要的规范化路径和显示信息。
#[tauri::command]
pub(crate) fn open_repository(repository_path: String) -> Result<RepositoryInfoDto, String> {
    let repository = Repository::open(path_from_input(repository_path, "repository")?)
        .map_err(|error| error.to_string())?;
    repository_info(&repository)
}

/// 修改 repository 显示名；这里只改元数据，不重命名磁盘目录。
#[tauri::command]
pub(crate) fn rename_repository(
    repository_path: String,
    display_name: String,
) -> Result<RepositoryInfoDto, String> {
    let repository = Repository::open(path_from_input(repository_path, "repository")?)
        .map_err(|error| error.to_string())?;
    repository
        .set_display_name(display_name)
        .map_err(|error| error.to_string())?;
    repository_info(&repository)
}

/// 校验本次会话输入的仓库密码，密码本身只留在前端会话状态中。
#[tauri::command]
pub(crate) fn unlock_repository(
    repository_path: String,
    encryption_password: String,
) -> Result<RepositoryInfoDto, String> {
    let repository = Repository::open(path_from_input(repository_path, "repository")?)
        .map_err(|error| error.to_string())?;
    repository
        .verify_encryption_password(Some(encryption_password.as_str()))
        .map_err(|error| error.to_string())?;
    repository_info(&repository)
}

/// 使用旧密码解封主密钥后，再用新密码重新封装仓库主密钥。
#[tauri::command]
pub(crate) fn change_repository_password(
    repository_path: String,
    old_password: String,
    new_password: String,
) -> Result<RepositoryInfoDto, String> {
    let repository = Repository::open(path_from_input(repository_path, "repository")?)
        .map_err(|error| error.to_string())?;
    repository
        .change_encryption_password(old_password.as_str(), new_password.as_str())
        .map_err(|error| error.to_string())?;
    repository_info(&repository)
}

/// 删除整个 repository。命令层额外做根目录和仓库结构检查，避免误删普通目录。
#[tauri::command]
pub(crate) fn delete_repository(
    repository_path: String,
    encryption_password: Option<String>,
) -> Result<(), String> {
    let path = path_from_input(repository_path, "repository")?;
    let canonical = fs::canonicalize(&path).map_err(|error| error.to_string())?;
    if canonical.parent().is_none() {
        return Err("refusing to delete filesystem root".to_string());
    }
    let repository = Repository::open(&canonical).map_err(|error| error.to_string())?;
    let metadata = repository.metadata().map_err(|error| error.to_string())?;
    if metadata.encryption_algorithm != EncryptionAlgorithm::None {
        repository
            .verify_encryption_password(encryption_password.as_deref())
            .map_err(|error| error.to_string())?;
    }
    for required in ["repo.meta", "objects", "snapshots", "indexes"] {
        if !canonical.join(required).exists() {
            return Err(format!(
                "refusing to delete invalid repository: missing {}",
                required
            ));
        }
    }
    fs::remove_dir_all(canonical).map_err(|error| error.to_string())
}

/// 从 GUI 命令层启动一次同步 repository 备份。
///
/// 这一层只负责校验和转换 Tauri DTO，实际 repository 备份行为全部交给 `backup-core`。
#[tauri::command]
pub(crate) fn backup(
    sources: Vec<String>,
    destination: String,
    filter: Option<BackupFilterDto>,
    compression_algorithm: Option<String>,
    snapshot_title: Option<String>,
    encrypt_snapshot: Option<bool>,
    encryption_password: Option<String>,
) -> Result<BackupResultDto, String> {
    let compression_algorithm = compression_algorithm_from_input(compression_algorithm)?;
    let sources = paths_from_input(sources, "source")?;
    for source in &sources {
        ensure_source_directory(source)?;
    }
    let repository_path = path_from_input(destination, "repository")?;
    let repository = open_or_init_repository(repository_path)?;
    let repository_metadata = repository.metadata().map_err(|error| error.to_string())?;
    let encryption_algorithm = if encrypt_snapshot.unwrap_or(false) {
        if repository_metadata.encryption_algorithm == EncryptionAlgorithm::None {
            return Err("repository encryption is not configured".to_string());
        }
        repository
            .verify_encryption_password(encryption_password.as_deref())
            .map_err(|error| error.to_string())?;
        repository_metadata.encryption_algorithm
    } else {
        EncryptionAlgorithm::None
    };
    let filter = filter.map(Into::into).unwrap_or_default();
    let options = BackupOptions {
        compression_algorithm,
        encryption_algorithm,
        encryption_password,
        snapshot_title,
    };
    let snapshot = repository
        .writer()
        .backup_many_with_options(sources, &filter, options)
        .map_err(|error| error.to_string())?;
    let snapshot_file = repository
        .reader()
        .read_snapshot(&snapshot.id)
        .map_err(|error| error.to_string())?;
    let summary = summarize_snapshot_file(&snapshot_file);

    Ok(BackupResultDto {
        file_count: summary.file_count,
        byte_count: summary.byte_count,
        snapshot_id: snapshot.id.as_str().to_string(),
        snapshot_title: snapshot.title,
        ignored_sources: snapshot
            .ignored_sources
            .iter()
            .map(|source| source.display().to_string())
            .collect(),
    })
}

/// 将 repository 中的指定 snapshot 恢复到目标目录。
#[tauri::command]
pub(crate) fn restore(
    backup_path: String,
    snapshot_id: String,
    destination: String,
    path_strategy: Option<RestorePathStrategyDto>,
    flatten_conflict_strategy: Option<FlattenConflictStrategyDto>,
    decryption_password: Option<String>,
) -> Result<RestoreResultDto, String> {
    let snapshot_id = snapshot_id_from_input(snapshot_id)?;
    let repository = Repository::open(path_from_input(backup_path, "repository")?)
        .map_err(|error| error.to_string())?;
    let snapshot_file = repository
        .reader()
        .read_snapshot(&snapshot_id)
        .map_err(|error| error.to_string())?;
    let summary = summarize_snapshot_file(&snapshot_file);
    let mut options = RestoreOptions::default();
    if let Some(path_strategy) = path_strategy {
        options.path_strategy = path_strategy.into();
    }
    if let Some(flatten_conflict_strategy) = flatten_conflict_strategy {
        options.flatten_conflict_strategy = flatten_conflict_strategy.into();
    }
    options.decryption_password = normalize_optional_secret(decryption_password);
    let report = repository
        .reader()
        .restore_with_options(
            &snapshot_id,
            path_from_input(destination, "destination")?,
            options,
        )
        .map_err(|error| error.to_string())?;

    Ok(RestoreResultDto {
        file_count: summary.file_count,
        byte_count: summary.byte_count,
        warnings: report
            .warnings
            .into_iter()
            .map(|warning| format!("{}: {}", warning.relative_path.display(), warning.message))
            .collect(),
    })
}

/// 读取 repository 中可恢复的 snapshot 摘要，供 GUI 展示和选择。
#[tauri::command]
pub(crate) fn list_snapshots(repository_path: String) -> Result<Vec<SnapshotInfoDto>, String> {
    let repository = Repository::open(path_from_input(repository_path, "repository")?)
        .map_err(|error| error.to_string())?;
    repository
        .reader()
        .list_snapshots()
        .map(|snapshots| snapshots.into_iter().map(Into::into).collect())
        .map_err(|error| error.to_string())
}

/// 删除指定 snapshot，并由 core 扫描剩余引用后清理不再使用的 object。
#[tauri::command]
pub(crate) fn delete_snapshot(
    repository_path: String,
    snapshot_id: String,
    encryption_password: Option<String>,
) -> Result<SnapshotDeleteResultDto, String> {
    let repository = Repository::open(path_from_input(repository_path, "repository")?)
        .map_err(|error| error.to_string())?;
    repository
        .writer()
        .delete_snapshot_with_password(
            &snapshot_id_from_input(snapshot_id)?,
            encryption_password.as_deref(),
        )
        .map(Into::into)
        .map_err(|error| error.to_string())
}

/// 将完整 repository 导出为归档文件；当前算法只开放 tar。
#[tauri::command]
pub(crate) fn export_repository(
    repository_path: String,
    archive_path: String,
    algorithm: Option<String>,
) -> Result<ArchiveResultDto, String> {
    let algorithm = archive_algorithm_from_input(algorithm)?;
    let repository = Repository::open(path_from_input(repository_path, "repository")?)
        .map_err(|error| error.to_string())?;
    repository
        .export_archive(path_from_input(archive_path, "archive")?, algorithm)
        .map(Into::into)
        .map_err(|error| error.to_string())
}

/// 从归档文件导入 repository，导入后返回可直接加入侧栏的仓库路径。
#[tauri::command]
pub(crate) fn import_repository(
    archive_path: String,
    destination: String,
    algorithm: Option<String>,
) -> Result<ArchiveResultDto, String> {
    let algorithm = archive_algorithm_from_input(algorithm)?;
    let archive_path = path_from_input(archive_path, "archive")?;
    let destination = path_from_input(destination, "repository")?;
    let repository = Repository::import_archive(&archive_path, &destination, algorithm)
        .map_err(|error| error.to_string())?;
    let byte_count = fs::metadata(&archive_path)
        .map_err(|error| error.to_string())?
        .len();

    Ok(ArchiveResultDto {
        algorithm: archive_algorithm_name(algorithm).to_string(),
        path: repository.root().display().to_string(),
        byte_count,
    })
}

// 备份命令兼容“目标目录不存在则初始化仓库”的工作流，但拒绝混入普通非空目录。
fn open_or_init_repository(path: PathBuf) -> Result<Repository, String> {
    if path.join("repo.meta").is_file() {
        return Repository::open(path).map_err(|error| error.to_string());
    }

    if path.exists()
        && fs::read_dir(&path)
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
    {
        return Err(format!(
            "repository path exists but is not a BackupTool repository: {}",
            path.display()
        ));
    }

    Repository::init(path).map_err(|error| error.to_string())
}

// 前端以路径作为仓库唯一标识，因此这里统一返回规范化且适合展示的路径。
fn repository_info(repository: &Repository) -> Result<RepositoryInfoDto, String> {
    let path = clean_canonical_path(
        fs::canonicalize(repository.root()).map_err(|error| error.to_string())?,
    );
    let metadata = repository.metadata().map_err(|error| error.to_string())?;
    Ok(RepositoryInfoDto {
        path: path.display().to_string(),
        name: metadata.display_name,
        encryption_algorithm: encryption_algorithm_to_string(metadata.encryption_algorithm),
    })
}

fn encryption_algorithm_to_string(value: EncryptionAlgorithm) -> String {
    match value {
        EncryptionAlgorithm::None => "none",
        EncryptionAlgorithm::Aes256Gcm => "aes-256-gcm",
    }
    .to_string()
}

// Windows canonicalize 会产生 \\?\ 前缀；GUI 展示和状态去重使用普通路径更直观。
fn clean_canonical_path(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    let value = path.to_string_lossy().into_owned();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    value
        .strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or(path)
}

// 仓库名同时作为目录名使用，因此按 Windows 目录名限制做前置校验。
fn validate_repository_name(value: &str) -> Result<&str, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("repository name must not be empty".to_string());
    }
    if name == "." || name == ".." {
        return Err("repository name must not be '.' or '..'".to_string());
    }
    if name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|value| value.is_control() || "<>:\"/\\|?*".contains(value))
    {
        return Err(format!(
            "repository name contains invalid characters: {name}"
        ));
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        return Err(format!("repository name is reserved by Windows: {name}"));
    }
    Ok(name)
}

// command 层先保证源路径是目录，避免把 core 的扫描错误暴露成不明确的 GUI 反馈。
fn ensure_source_directory(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(BackupError::SourceDoesNotExist(path.to_path_buf()).to_string());
    }
    if !path.is_dir() {
        return Err(BackupError::SourceIsNotDirectory(path.to_path_buf()).to_string());
    }
    Ok(())
}

fn path_from_input(value: String, name: &'static str) -> Result<PathBuf, String> {
    // 路径输入校验放在命令层：`backup-core` 接收类型化路径，
    // 前端调用方仍然得到简单的字符串错误。
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} path must not be empty"));
    }
    Ok(PathBuf::from(trimmed))
}

// 多源备份必须至少有一个源；每个源仍复用单路径校验逻辑。
fn paths_from_input(values: Vec<String>, name: &'static str) -> Result<Vec<PathBuf>, String> {
    if values.is_empty() {
        return Err(format!("at least one {name} path is required"));
    }
    values
        .into_iter()
        .map(|value| path_from_input(value, name))
        .collect()
}

// snapshot id 来自前端选择项，不允许空白字符串继续进入 core。
fn snapshot_id_from_input(value: String) -> Result<SnapshotId, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("snapshot id must not be empty".to_string());
    }
    Ok(SnapshotId::from(trimmed.to_string()))
}

// 归档算法参数保留扩展点，当前只把空值和 tar 映射到 tar。
fn archive_algorithm_from_input(value: Option<String>) -> Result<ArchiveAlgorithm, String> {
    let value = value.unwrap_or_else(|| "tar".to_string());
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "tar" => Ok(ArchiveAlgorithm::Tar),
        other => Err(format!("unsupported archive algorithm: {other}")),
    }
}

fn archive_algorithm_name(value: ArchiveAlgorithm) -> &'static str {
    match value {
        ArchiveAlgorithm::Tar => "tar",
    }
}

// 压缩算法属于写入 object 时的选项；恢复时由 object header 自描述。
fn compression_algorithm_from_input(value: Option<String>) -> Result<CompressionAlgorithm, String> {
    let value = value.unwrap_or_else(|| "none".to_string());
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "none" => Ok(CompressionAlgorithm::None),
        "zstd" => Ok(CompressionAlgorithm::Zstd),
        other => Err(format!("unsupported compression algorithm: {other}")),
    }
}

// 加密算法由仓库和本次 snapshot 共同约束，命令层先处理字符串枚举。
fn encryption_algorithm_from_input(value: Option<String>) -> Result<EncryptionAlgorithm, String> {
    let value = value.unwrap_or_else(|| "none".to_string());
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "none" => Ok(EncryptionAlgorithm::None),
        "aes-256-gcm" => Ok(EncryptionAlgorithm::Aes256Gcm),
        other => Err(format!("unsupported encryption algorithm: {other}")),
    }
}

// 只有真正启用 AES 时才要求密码；未加密 snapshot 不应被空密码阻断。
fn validate_encryption_password(
    algorithm: EncryptionAlgorithm,
    password: Option<&str>,
) -> Result<(), String> {
    if algorithm == EncryptionAlgorithm::Aes256Gcm && password.unwrap_or_default().is_empty() {
        return Err("encryption password must not be empty".to_string());
    }
    Ok(())
}

// 空字符串等价于未提供密码，避免前端表单把空输入误传成有效 secret。
fn normalize_optional_secret(value: Option<String>) -> Option<String> {
    value.and_then(|value| if value.is_empty() { None } else { Some(value) })
}

#[derive(Default)]
struct OperationSummary {
    file_count: u64,
    byte_count: u64,
}

// command 返回的文件数和字节数按 snapshot 中的普通文件 entry 汇总。
fn summarize_snapshot_file(snapshot_file: &SnapshotFile) -> OperationSummary {
    let mut summary = OperationSummary::default();
    for entry in &snapshot_file.entries {
        if entry.kind == FileKind::File {
            summary.file_count += 1;
            summary.byte_count += entry.size;
        }
    }
    summary
}
