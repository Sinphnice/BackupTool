use backup_core::{split_filter_list, BackupFilter};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupResultDto {
    pub(crate) file_count: u64,
    pub(crate) byte_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestoreResultDto {
    pub(crate) file_count: u64,
    pub(crate) byte_count: u64,
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
