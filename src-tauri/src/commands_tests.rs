use crate::commands::{backup, restore};
use crate::dto::BackupFilterDto;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn backup_and_restore_round_trip_regular_files() {
    let root = TestDir::new("tauri_round_trip");
    let source = root.path.join("source");
    let repository_dir = root.path.join("repository");
    let restore_dir = root.path.join("restore");
    fs::create_dir_all(source.join("dir")).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();
    fs::write(source.join("dir").join("b.txt"), "beta").unwrap();
    fs::write(source.join("image.png"), [0_u8, 1, 2, 3]).unwrap();

    let backup_result = backup(
        source.to_string_lossy().into_owned(),
        repository_dir.to_string_lossy().into_owned(),
        None,
    )
    .unwrap();
    assert_eq!(backup_result.file_count, 3);
    assert!(backup_result.snapshot_id.starts_with("snapshot-"));

    let restore_result = restore(
        repository_dir.to_string_lossy().into_owned(),
        backup_result.snapshot_id,
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
    let repository_dir = root.path.join("repository");
    let restore_dir = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("keep.txt"), "keep").unwrap();
    fs::write(source.join("skip.png"), "skip").unwrap();

    let result = backup(
        source.to_string_lossy().into_owned(),
        repository_dir.to_string_lossy().into_owned(),
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
    restore(
        repository_dir.to_string_lossy().into_owned(),
        result.snapshot_id,
        restore_dir.to_string_lossy().into_owned(),
    )
    .unwrap();
    assert!(restore_dir.join("keep.txt").exists());
    assert!(!restore_dir.join("skip.png").exists());
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

#[test]
fn backup_rejects_non_empty_non_repository_destination() {
    let root = TestDir::new("tauri_reject_non_repo");
    let source = root.path.join("source");
    let destination = root.path.join("old_backup");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("legacy.txt"), "legacy").unwrap();

    let error = backup(
        source.to_string_lossy().into_owned(),
        destination.to_string_lossy().into_owned(),
        None,
    )
    .unwrap_err();

    assert!(error.contains("exists but is not a BackupTool repository"));
    assert!(!destination.join("repo.meta").exists());
    assert!(!destination.join("objects").exists());
    assert!(!destination.join("snapshots").exists());
}

#[test]
fn restore_requires_snapshot_id() {
    let error = restore("unused".to_string(), " ".to_string(), "unused".to_string()).unwrap_err();

    assert!(error.contains("snapshot id must not be empty"));
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
