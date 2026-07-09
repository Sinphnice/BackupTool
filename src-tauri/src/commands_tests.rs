use crate::commands::{backup, export_repository, import_repository, list_snapshots, restore};
use crate::dto::{BackupFilterDto, FlattenConflictStrategyDto, RestorePathStrategyDto};
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
        vec![source.to_string_lossy().into_owned()],
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
        None,
        None,
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
        vec![source.to_string_lossy().into_owned()],
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
        None,
        None,
    )
    .unwrap();
    assert!(restore_dir.join("keep.txt").exists());
    assert!(!restore_dir.join("skip.png").exists());
}

#[test]
fn list_snapshots_returns_repository_snapshot_summaries() {
    let root = TestDir::new("tauri_list_snapshots");
    let source = root.path.join("source");
    let repository_dir = root.path.join("repository");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();

    let first = backup(
        vec![source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
    )
    .unwrap();
    fs::write(source.join("b.txt"), "beta").unwrap();
    let second = backup(
        vec![source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
    )
    .unwrap();

    let snapshots = list_snapshots(repository_dir.to_string_lossy().into_owned()).unwrap();

    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].id, second.snapshot_id);
    assert_eq!(snapshots[0].file_count, 2);
    assert_eq!(snapshots[0].byte_count, 9);
    assert!(snapshots[0].created_unix_seconds.is_some());
    assert_eq!(snapshots[1].id, first.snapshot_id);
}

#[test]
fn backup_returns_core_error_as_string() {
    let error = backup(
        vec!["Z:\\definitely\\missing\\backup-tool-source".to_string()],
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
        vec![source.to_string_lossy().into_owned()],
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
    let error = restore(
        "unused".to_string(),
        " ".to_string(),
        "unused".to_string(),
        None,
        None,
    )
    .unwrap_err();

    assert!(error.contains("snapshot id must not be empty"));
}

#[test]
fn backup_requires_at_least_one_source() {
    let error = backup(Vec::new(), "unused".to_string(), None).unwrap_err();

    assert!(error.contains("at least one source path is required"));
}

#[test]
fn backup_accepts_multiple_sources_and_restore_uses_path_options() {
    let root = TestDir::new("tauri_multi_source");
    let source_a = root.path.join("alpha");
    let source_b = root.path.join("beta");
    let repository_dir = root.path.join("repository");
    let restore_dir = root.path.join("restore");
    fs::create_dir_all(&source_a).unwrap();
    fs::create_dir_all(&source_b).unwrap();
    fs::write(source_a.join("same.txt"), "alpha").unwrap();
    fs::write(source_b.join("same.txt"), "beta").unwrap();

    let backup_result = backup(
        vec![
            source_a.to_string_lossy().into_owned(),
            source_b.to_string_lossy().into_owned(),
        ],
        repository_dir.to_string_lossy().into_owned(),
        None,
    )
    .unwrap();
    assert_eq!(backup_result.file_count, 2);

    restore(
        repository_dir.to_string_lossy().into_owned(),
        backup_result.snapshot_id,
        restore_dir.to_string_lossy().into_owned(),
        Some(RestorePathStrategyDto::Flatten),
        Some(FlattenConflictStrategyDto::Rename),
    )
    .unwrap();

    assert!(restore_dir.join("same.txt").exists());
    assert!(restore_dir.join("same (1).txt").exists());
}

#[test]
fn export_and_import_repository_commands_round_trip_tar() {
    let root = TestDir::new("tauri_archive_round_trip");
    let source = root.path.join("source");
    let repository_dir = root.path.join("repository");
    let archive = root.path.join("repository.tar");
    let imported = root.path.join("imported");
    let restored = root.path.join("restored");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();

    let backup_result = backup(
        vec![source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
    )
    .unwrap();
    let export_result = export_repository(
        repository_dir.to_string_lossy().into_owned(),
        archive.to_string_lossy().into_owned(),
        None,
    )
    .unwrap();
    assert_eq!(export_result.algorithm, "tar");
    assert!(export_result.byte_count > 0);

    let import_result = import_repository(
        archive.to_string_lossy().into_owned(),
        imported.to_string_lossy().into_owned(),
        Some("tar".to_string()),
    )
    .unwrap();
    assert_eq!(import_result.algorithm, "tar");
    assert_eq!(import_result.path, imported.display().to_string());

    let snapshots = list_snapshots(imported.to_string_lossy().into_owned()).unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].id, backup_result.snapshot_id);
    restore(
        imported.to_string_lossy().into_owned(),
        backup_result.snapshot_id,
        restored.to_string_lossy().into_owned(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(fs::read_to_string(restored.join("a.txt")).unwrap(), "alpha");
}

#[test]
fn archive_commands_reject_unknown_algorithm() {
    let error = export_repository(
        "unused".to_string(),
        "unused".to_string(),
        Some("zip".to_string()),
    )
    .unwrap_err();

    assert!(error.contains("unsupported archive algorithm: zip"));
}

#[test]
fn import_repository_command_rejects_non_empty_destination() {
    let root = TestDir::new("tauri_archive_non_empty");
    let source = root.path.join("source");
    let repository_dir = root.path.join("repository");
    let archive = root.path.join("repository.tar");
    let destination = root.path.join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();
    backup(
        vec![source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
    )
    .unwrap();
    export_repository(
        repository_dir.to_string_lossy().into_owned(),
        archive.to_string_lossy().into_owned(),
        Some("tar".to_string()),
    )
    .unwrap();
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("existing.txt"), "exists").unwrap();

    let error = import_repository(
        archive.to_string_lossy().into_owned(),
        destination.to_string_lossy().into_owned(),
        None,
    )
    .unwrap_err();

    assert!(error.contains("import destination exists and is not empty"));
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
