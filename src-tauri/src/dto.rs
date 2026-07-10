use backup_core::{split_filter_list, BackupFilter, FlattenConflictStrategy, RestorePathStrategy};
use serde::{Deserialize, Serialize};

/// 从 TypeScript 接收的筛选条件载荷。
///
/// 跨 Tauri 边界时字段名使用 camelCase；进入备份逻辑前会转换为 Rust core 的筛选模型。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupFilterDto {
    pub(crate) include_path_contains: Option<String>,
    pub(crate) exclude_path_contains: Option<String>,
    pub(crate) extensions: Option<String>,
    pub(crate) include_name_contains: Option<String>,
    pub(crate) exclude_name_contains: Option<String>,
    pub(crate) min_size: Option<u64>,
    pub(crate) max_size: Option<u64>,
    pub(crate) modified_after: Option<i64>,
    pub(crate) modified_before: Option<i64>,
}

/// 备份完成后返回给 GUI 的稳定命令响应结构。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupResultDto {
    pub(crate) file_count: u64,
    pub(crate) byte_count: u64,
    pub(crate) snapshot_id: String,
    pub(crate) snapshot_title: Option<String>,
    pub(crate) ignored_sources: Vec<String>,
}

/// 恢复完成后返回给 GUI 的稳定命令响应结构。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestoreResultDto {
    pub(crate) file_count: u64,
    pub(crate) byte_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveResultDto {
    pub(crate) algorithm: String,
    pub(crate) path: String,
    pub(crate) byte_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryInfoDto {
    pub(crate) path: String,
    pub(crate) name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotDeleteResultDto {
    pub(crate) snapshot_id: String,
    pub(crate) deleted_object_count: u64,
    pub(crate) reclaimed_bytes: u64,
    pub(crate) warnings: Vec<String>,
}

/// GUI 鍔犺浇 repository 鏃惰繑鍥炵殑 snapshot 鎽樿銆?
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotInfoDto {
    pub(crate) id: String,
    pub(crate) file_count: u64,
    pub(crate) byte_count: u64,
    pub(crate) created_unix_seconds: Option<i64>,
    pub(crate) created_nanoseconds: Option<u32>,
    pub(crate) sequence: Option<u16>,
    pub(crate) title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RestorePathStrategyDto {
    PreserveFullPath,
    PreserveRelativePath,
    Flatten,
}

impl From<RestorePathStrategyDto> for RestorePathStrategy {
    fn from(value: RestorePathStrategyDto) -> Self {
        match value {
            RestorePathStrategyDto::PreserveFullPath => Self::PreserveFullPath,
            RestorePathStrategyDto::PreserveRelativePath => Self::PreserveRelativePath,
            RestorePathStrategyDto::Flatten => Self::Flatten,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FlattenConflictStrategyDto {
    Error,
    Skip,
    Overwrite,
    Rename,
}

impl From<FlattenConflictStrategyDto> for FlattenConflictStrategy {
    fn from(value: FlattenConflictStrategyDto) -> Self {
        match value {
            FlattenConflictStrategyDto::Error => Self::Error,
            FlattenConflictStrategyDto::Skip => Self::Skip,
            FlattenConflictStrategyDto::Overwrite => Self::Overwrite,
            FlattenConflictStrategyDto::Rename => Self::Rename,
        }
    }
}

impl From<backup_core::SnapshotInfo> for SnapshotInfoDto {
    fn from(value: backup_core::SnapshotInfo) -> Self {
        Self {
            id: value.id.as_str().to_string(),
            file_count: value.file_count,
            byte_count: value.byte_count,
            created_unix_seconds: value.created_unix_seconds,
            created_nanoseconds: value.created_nanoseconds,
            sequence: value.sequence,
            title: value.title,
        }
    }
}

impl From<backup_core::ArchiveResult> for ArchiveResultDto {
    fn from(value: backup_core::ArchiveResult) -> Self {
        Self {
            algorithm: match value.algorithm {
                backup_core::ArchiveAlgorithm::Tar => "tar".to_string(),
            },
            path: value.path.display().to_string(),
            byte_count: value.byte_count,
        }
    }
}

impl From<backup_core::SnapshotDeleteResult> for SnapshotDeleteResultDto {
    fn from(value: backup_core::SnapshotDeleteResult) -> Self {
        Self {
            snapshot_id: value.snapshot_id.as_str().to_string(),
            deleted_object_count: value.deleted_object_count,
            reclaimed_bytes: value.reclaimed_bytes,
            warnings: value.warnings,
        }
    }
}

impl From<BackupFilterDto> for BackupFilter {
    fn from(value: BackupFilterDto) -> Self {
        Self {
            include_path_contains: split_filter_list(value.include_path_contains),
            exclude_path_contains: split_filter_list(value.exclude_path_contains),
            extensions: split_filter_list(value.extensions),
            include_name_contains: split_filter_list(value.include_name_contains),
            exclude_name_contains: split_filter_list(value.exclude_name_contains),
            min_size: value.min_size,
            max_size: value.max_size,
            modified_after: value.modified_after,
            modified_before: value.modified_before,
        }
    }
}
