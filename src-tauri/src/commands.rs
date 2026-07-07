use crate::dto::{BackupFilterDto, BackupResultDto, RestoreResultDto};
use backup_core::{BackupError, FileKind, Manifest, Repository, SnapshotId};
use std::fs;
use std::path::{Path, PathBuf};

/// 从 GUI 命令层启动一次同步 repository 备份。
///
/// 这一层只负责校验和转换 Tauri DTO，实际 repository 备份行为全部交给 `backup-core`。
#[tauri::command]
pub(crate) fn backup(
    source: String,
    destination: String,
    filter: Option<BackupFilterDto>,
) -> Result<BackupResultDto, String> {
    let source = path_from_input(source, "source")?;
    ensure_source_directory(&source)?;
    let repository_path = path_from_input(destination, "repository")?;
    let repository = open_or_init_repository(repository_path)?;
    let filter = filter.map(Into::into).unwrap_or_default();
    let snapshot = repository
        .writer()
        .backup(source, &filter)
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
    })
}

/// 将 repository 中的指定 snapshot 恢复到目标目录。
#[tauri::command]
pub(crate) fn restore(
    backup_path: String,
    snapshot_id: String,
    destination: String,
) -> Result<RestoreResultDto, String> {
    let snapshot_id = snapshot_id_from_input(snapshot_id)?;
    let repository = Repository::open(path_from_input(backup_path, "repository")?)
        .map_err(|error| error.to_string())?;
    let manifest = repository
        .reader()
        .read_manifest(&snapshot_id)
        .map_err(|error| error.to_string())?;
    let summary = summarize_manifest(&manifest);
    repository
        .reader()
        .restore(&snapshot_id, path_from_input(destination, "destination")?)
        .map_err(|error| error.to_string())?;

    Ok(RestoreResultDto {
        file_count: summary.file_count,
        byte_count: summary.byte_count,
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

fn snapshot_id_from_input(value: String) -> Result<SnapshotId, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("snapshot id must not be empty".to_string());
    }
    Ok(SnapshotId::from(trimmed.to_string()))
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
