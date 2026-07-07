use backup_core::{
    split_filter_list, BackupConfig, BackupFilter, BackupManager, RestoreConfig, RestoreManager,
    CORE_VERSION,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupFilterDto {
    include_path_contains: Option<String>,
    exclude_path_contains: Option<String>,
    extensions: Option<String>,
    include_name_contains: Option<String>,
    exclude_name_contains: Option<String>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    modified_after: Option<i64>,
    modified_before: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupResultDto {
    file_count: u64,
    byte_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoreResultDto {
    file_count: u64,
    byte_count: u64,
}

#[tauri::command]
fn core_version() -> Result<String, String> {
    Ok(CORE_VERSION.to_string())
}

#[tauri::command]
fn backup(
    source: String,
    destination: String,
    filter: Option<BackupFilterDto>,
) -> Result<BackupResultDto, String> {
    let result = BackupManager
        .run(&BackupConfig {
            source: path_from_input(source, "source")?,
            destination: path_from_input(destination, "destination")?,
            filter: filter.map(Into::into).unwrap_or_default(),
        })
        .map_err(|error| error.to_string())?;

    Ok(BackupResultDto {
        file_count: result.file_count,
        byte_count: result.byte_count,
    })
}

#[tauri::command]
fn restore(backup_path: String, destination: String) -> Result<RestoreResultDto, String> {
    let result = RestoreManager
        .run(&RestoreConfig {
            backup: path_from_input(backup_path, "backup")?,
            destination: path_from_input(destination, "destination")?,
        })
        .map_err(|error| error.to_string())?;

    Ok(RestoreResultDto {
        file_count: result.file_count,
        byte_count: result.byte_count,
    })
}

fn path_from_input(value: String, name: &'static str) -> Result<PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} path must not be empty"));
    }
    Ok(PathBuf::from(trimmed))
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![core_version, backup, restore])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}

#[cfg(test)]
mod tests {
    use super::{backup, core_version, restore, BackupFilterDto};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn core_version_returns_rust_core_value() {
        assert_eq!(core_version().unwrap(), "Backup Core 0.2.0 (Rust)");
    }

    #[test]
    fn backup_and_restore_round_trip_regular_files() {
        let root = TestDir::new("tauri_round_trip");
        let source = root.path.join("source");
        let backup_dir = root.path.join("backup");
        let restore_dir = root.path.join("restore");
        fs::create_dir_all(source.join("dir")).unwrap();
        fs::write(source.join("a.txt"), "alpha").unwrap();
        fs::write(source.join("dir").join("b.txt"), "beta").unwrap();
        fs::write(source.join("image.png"), [0_u8, 1, 2, 3]).unwrap();

        let backup_result = backup(
            source.to_string_lossy().into_owned(),
            backup_dir.to_string_lossy().into_owned(),
            None,
        )
        .unwrap();
        assert_eq!(backup_result.file_count, 3);

        let restore_result = restore(
            backup_dir.to_string_lossy().into_owned(),
            restore_dir.to_string_lossy().into_owned(),
        )
        .unwrap();
        assert_eq!(restore_result.file_count, 3);
        assert_eq!(
            fs::read_to_string(restore_dir.join("a.txt")).unwrap(),
            "alpha"
        );
        assert_eq!(
            fs::read_to_string(restore_dir.join("dir").join("b.txt")).unwrap(),
            "beta"
        );
        assert_eq!(
            fs::read(restore_dir.join("image.png")).unwrap(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn backup_applies_extension_filter() {
        let root = TestDir::new("tauri_filter");
        let source = root.path.join("source");
        let backup_dir = root.path.join("backup");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("keep.txt"), "keep").unwrap();
        fs::write(source.join("skip.png"), "skip").unwrap();

        let result = backup(
            source.to_string_lossy().into_owned(),
            backup_dir.to_string_lossy().into_owned(),
            Some(BackupFilterDto {
                include_path_contains: None,
                exclude_path_contains: None,
                extensions: Some("txt".to_string()),
                include_name_contains: None,
                exclude_name_contains: None,
                min_size: None,
                max_size: None,
                modified_after: None,
                modified_before: None,
            }),
        )
        .unwrap();

        assert_eq!(result.file_count, 1);
        assert!(backup_dir.join("keep.txt").exists());
        assert!(!backup_dir.join("skip.png").exists());
    }

    #[test]
    fn backup_returns_core_error_as_string() {
        let error = backup(
            "Z:\\definitely\\missing\\backup-tool-source".to_string(),
            "unused".to_string(),
            None,
        )
        .unwrap_err();

        assert!(error.contains("source path does not exist"));
    }

    struct TestDir {
        path: std::path::PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "backup_tool_{name}_{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
