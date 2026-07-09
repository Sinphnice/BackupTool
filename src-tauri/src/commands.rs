use crate::dto::{
    ArchiveResultDto, BackupFilterDto, BackupResultDto, FlattenConflictStrategyDto,
    RestorePathStrategyDto, RestoreResultDto, SnapshotInfoDto,
};
use backup_core::{
    ArchiveAlgorithm, BackupError, FileKind, Manifest, Repository, RestoreOptions, SnapshotId,
};
use std::fs;
use std::path::{Path, PathBuf};

/// 从 GUI 命令层启动一次同步 repository 备份。
///
/// 这一层只负责校验和转换 Tauri DTO，实际 repository 备份行为全部交给 `backup-core`。
#[tauri::command]
pub(crate) fn backup(
    sources: Vec<String>,
    destination: String,
    filter: Option<BackupFilterDto>,
) -> Result<BackupResultDto, String> {
    let sources = paths_from_input(sources, "source")?;
    for source in &sources {
        ensure_source_directory(source)?;
    }
    let repository_path = path_from_input(destination, "repository")?;
    let repository = open_or_init_repository(repository_path)?;
    let filter = filter.map(Into::into).unwrap_or_default();
    let snapshot = repository
        .writer()
        .backup_many(sources, &filter)
        .map_err(|error| error.to_string())?;
    let manifest = repository
        .reader()
        .read_manifest(&snapshot.id)
        .map_err(|error| error.to_string())?;
    let summary = summarize_manifest(&manifest);

    Ok(BackupResultDto {
        file_count: summary.file_count,
        byte_count: summary.byte_count,
        snapshot_id: snapshot.id.as_str().to_string(),
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
) -> Result<RestoreResultDto, String> {
    let snapshot_id = snapshot_id_from_input(snapshot_id)?;
    let repository = Repository::open(path_from_input(backup_path, "repository")?)
        .map_err(|error| error.to_string())?;
    let manifest = repository
        .reader()
        .read_manifest(&snapshot_id)
        .map_err(|error| error.to_string())?;
    let summary = summarize_manifest(&manifest);
    let mut options = RestoreOptions::default();
    if let Some(path_strategy) = path_strategy {
        options.path_strategy = path_strategy.into();
    }
    if let Some(flatten_conflict_strategy) = flatten_conflict_strategy {
        options.flatten_conflict_strategy = flatten_conflict_strategy.into();
    }
    repository
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

fn paths_from_input(values: Vec<String>, name: &'static str) -> Result<Vec<PathBuf>, String> {
    if values.is_empty() {
        return Err(format!("at least one {name} path is required"));
    }
    values
        .into_iter()
        .map(|value| path_from_input(value, name))
        .collect()
}

fn snapshot_id_from_input(value: String) -> Result<SnapshotId, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("snapshot id must not be empty".to_string());
    }
    Ok(SnapshotId::from(trimmed.to_string()))
}

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

#[derive(Default)]
struct OperationSummary {
    file_count: u64,
    byte_count: u64,
}

fn summarize_manifest(manifest: &Manifest) -> OperationSummary {
    let mut summary = OperationSummary::default();
    for entry in &manifest.entries {
        if entry.kind == FileKind::File {
            summary.file_count += 1;
            summary.byte_count += entry.size;
        }
    }
    summary
}
