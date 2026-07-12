use crate::commands::{
    backup, change_repository_password, create_repository, delete_repository, delete_snapshot,
    export_repository, import_repository, list_snapshots, open_repository, rename_repository, restore,
    unlock_repository,
};
use crate::dto::{BackupFilterDto, FlattenConflictStrategyDto, RestorePathStrategyDto};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn create_and_open_repository_return_canonical_repository_info() {
    let root = TestDir::new("tauri_create_repository");

    let created = create_repository(
        root.path.to_string_lossy().into_owned(),
        "Project Backup".to_string(),
        None,
        None,
    )
    .unwrap();

    assert_eq!(created.name, "Project Backup");
    assert!(std::path::Path::new(&created.path).is_absolute());
    assert!(!created.path.starts_with(r"\\?\"));
    assert!(std::path::Path::new(&created.path)
        .join("repo.meta")
        .is_file());
    let opened = open_repository(created.path.clone()).unwrap();
    assert_eq!(opened.path, created.path);
    assert_eq!(opened.name, created.name);
}

#[test]
fn create_repository_rejects_invalid_names_and_existing_targets() {
    let root = TestDir::new("tauri_create_repository_invalid");

    for name in ["", "..", "bad/name", "CON", "trailing."] {
        assert!(
            create_repository(
                root.path.to_string_lossy().into_owned(),
                name.to_string(),
                None,
                None
            )
            .is_err()
        );
    }

    create_repository(
        root.path.to_string_lossy().into_owned(),
        "existing".to_string(),
        None,
        None,
    )
    .unwrap();
    let error = create_repository(
        root.path.to_string_lossy().into_owned(),
        "existing".to_string(),
        None,
        None,
    )
    .unwrap_err();
    assert!(error.contains("already exists"));
}

#[test]
fn rename_repository_updates_display_name_without_renaming_directory() {
    let root = TestDir::new("tauri_rename_repository");
    let created = create_repository(
        root.path.to_string_lossy().into_owned(),
        "physical-name".to_string(),
        None,
        None,
    )
    .unwrap();

    let renamed = rename_repository(created.path.clone(), "Display Name".to_string()).unwrap();

    assert_eq!(renamed.name, "Display Name");
    assert_eq!(renamed.path, created.path);
    assert!(std::path::Path::new(&created.path).exists());
}

#[test]
fn encrypted_repository_can_be_unlocked_after_reopen() {
    let root = TestDir::new("tauri_unlock_repository");
    let created = create_repository(
        root.path.to_string_lossy().into_owned(),
        "encrypted".to_string(),
        Some("aes-256-gcm".to_string()),
        Some("password".to_string()),
    )
    .unwrap();

    let opened = open_repository(created.path.clone()).unwrap();
    assert_eq!(opened.encryption_algorithm, "aes-256-gcm");
    assert!(unlock_repository(created.path.clone(), "wrong".to_string()).is_err());
    let unlocked = unlock_repository(created.path, "password".to_string()).unwrap();
    assert_eq!(unlocked.name, "encrypted");
}

#[test]
fn delete_repository_removes_valid_repository_and_rejects_non_repository() {
    let root = TestDir::new("tauri_delete_repository");
    let created = create_repository(
        root.path.to_string_lossy().into_owned(),
        "repository".to_string(),
        None,
        None,
    )
    .unwrap();
    let non_repository = root.path.join("plain-dir");
    fs::create_dir_all(&non_repository).unwrap();

    assert!(delete_repository(non_repository.to_string_lossy().into_owned(), None).is_err());
    delete_repository(created.path.clone(), None).unwrap();
    assert!(!std::path::Path::new(&created.path).exists());
}

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
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(backup_result.file_count, 3);
    assert!(!backup_result.snapshot_id.starts_with("snapshot-"));
    assert_eq!(backup_result.snapshot_id.split('-').count(), 3);

    let restore_result = restore(
        repository_dir.to_string_lossy().into_owned(),
        backup_result.snapshot_id,
        restore_dir.to_string_lossy().into_owned(),
        None,
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
fn backup_applies_path_regex_filter() {
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
            path_regex: Some(r"\.txt$".to_string()),
            min_size: None,
            max_size: None,
            modified_after: None,
            modified_before: None,
        }),
        None,
        None,
        None,
        None,
    )
    .unwrap();

    assert_eq!(result.file_count, 1);
    restore(
        repository_dir.to_string_lossy().into_owned(),
        result.snapshot_id,
        restore_dir.to_string_lossy().into_owned(),
        None,
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
        None,
        Some("first title".to_string()),
        None,
        None,
    )
    .unwrap();
    fs::write(source.join("b.txt"), "beta").unwrap();
    let second = backup(
        vec![source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();

    let snapshots = list_snapshots(repository_dir.to_string_lossy().into_owned()).unwrap();

    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].id, second.snapshot_id);
    assert_eq!(snapshots[0].file_count, 2);
    assert_eq!(snapshots[0].byte_count, 9);
    assert!(snapshots[0].created_unix_seconds.is_some());
    assert!(snapshots[0].created_nanoseconds.is_some());
    assert!(snapshots[0].sequence.is_some());
    assert_eq!(snapshots[1].id, first.snapshot_id);
    assert_eq!(snapshots[1].title.as_deref(), Some("first title"));
}

#[test]
fn delete_snapshot_command_returns_cleanup_summary() {
    let root = TestDir::new("tauri_delete_snapshot");
    let source = root.path.join("source");
    let repository_dir = root.path.join("repository");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();
    let snapshot = backup(
        vec![source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();

    let result = delete_snapshot(
        repository_dir.to_string_lossy().into_owned(),
        snapshot.snapshot_id.clone(),
        None,
    )
    .unwrap();

    assert_eq!(result.snapshot_id, snapshot.snapshot_id);
    assert_eq!(result.deleted_object_count, 1);
    assert!(result.reclaimed_bytes > 0);
    assert!(result.warnings.is_empty());
    assert!(
        list_snapshots(repository_dir.to_string_lossy().into_owned())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn backup_returns_core_error_as_string() {
    let error = backup(
        vec!["Z:\\definitely\\missing\\backup-tool-source".to_string()],
        "unused".to_string(),
        None,
        None,
        None,
        None,
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
        None,
        None,
        None,
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
        None,
    )
    .unwrap_err();

    assert!(error.contains("snapshot id must not be empty"));
}

#[test]
fn backup_requires_at_least_one_source() {
    let error = backup(
        Vec::new(),
        "unused".to_string(),
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap_err();

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
        None,
        None,
        None,
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
        None,
    )
    .unwrap();

    assert!(restore_dir.join("same.txt").exists());
    assert!(restore_dir.join("same (1).txt").exists());
}

#[test]
fn backup_command_accepts_zstd_compression_and_restore_decompresses() {
    let root = TestDir::new("tauri_zstd_backup");
    let source = root.path.join("source");
    let repository_dir = root.path.join("repository");
    let restore_dir = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha alpha alpha").unwrap();

    let backup_result = backup(
        vec![source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
        Some("zstd".to_string()),
        None,
        None,
        None,
    )
    .unwrap();
    let object_path = fs::read_dir(repository_dir.join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(object_header_text(&object_path).contains("compression\tzstd"));
    assert_ne!(
        object_path.extension().and_then(|value| value.to_str()),
        Some("zst")
    );

    restore(
        repository_dir.to_string_lossy().into_owned(),
        backup_result.snapshot_id,
        restore_dir.to_string_lossy().into_owned(),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(restore_dir.join("a.txt")).unwrap(),
        "alpha alpha alpha"
    );
}

#[test]
fn backup_command_rejects_unknown_compression_algorithm() {
    let error = backup(
        vec!["unused".to_string()],
        "unused".to_string(),
        None,
        Some("brotli".to_string()),
        None,
        None,
        None,
    )
    .unwrap_err();

    assert!(error.contains("unsupported compression algorithm: brotli"));
}

#[test]
fn backup_command_accepts_aes_encryption_and_restore_decrypts() {
    let root = TestDir::new("tauri_aes_backup");
    let source = root.path.join("source");
    let repository_dir = root.path.join("repository");
    let restore_dir = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "secret text").unwrap();
    create_repository(
        root.path.to_string_lossy().into_owned(),
        "repository".to_string(),
        Some("aes-256-gcm".to_string()),
        Some("password".to_string()),
    )
    .unwrap();

    let backup_result = backup(
        vec![source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
        None,
        None,
        Some(true),
        Some("password".to_string()),
    )
    .unwrap();
    let object_path = fs::read_dir(repository_dir.join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let object_bytes = fs::read(&object_path).unwrap();
    assert!(object_header_text(&object_path).contains("encryption\taes-256-gcm"));
    assert!(!String::from_utf8_lossy(&object_bytes).contains("secret text"));

    restore(
        repository_dir.to_string_lossy().into_owned(),
        backup_result.snapshot_id,
        restore_dir.to_string_lossy().into_owned(),
        None,
        None,
        Some("password".to_string()),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(restore_dir.join("a.txt")).unwrap(),
        "secret text"
    );
}

#[test]
fn backup_command_rejects_invalid_encryption_options() {
    let root = TestDir::new("tauri_invalid_repository_encryption");
    let missing_password = create_repository(
        root.path.to_string_lossy().into_owned(),
        "missing-password".to_string(),
        Some("aes-256-gcm".to_string()),
        None,
    )
    .unwrap_err();
    assert!(missing_password.contains("encryption password must not be empty"));

    let unknown_algorithm = create_repository(
        root.path.to_string_lossy().into_owned(),
        "unknown-algorithm".to_string(),
        Some("rot13".to_string()),
        Some("password".to_string()),
    )
    .unwrap_err();
    assert!(unknown_algorithm.contains("unsupported encryption algorithm: rot13"));
}

#[test]
fn restore_command_rejects_missing_or_wrong_decryption_password() {
    let root = TestDir::new("tauri_aes_restore_password");
    let source = root.path.join("source");
    let repository_dir = root.path.join("repository");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "secret text").unwrap();
    create_repository(
        root.path.to_string_lossy().into_owned(),
        "repository".to_string(),
        Some("aes-256-gcm".to_string()),
        Some("password".to_string()),
    )
    .unwrap();

    let backup_result = backup(
        vec![source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
        None,
        None,
        Some(true),
        Some("password".to_string()),
    )
    .unwrap();

    let missing_password = restore(
        repository_dir.to_string_lossy().into_owned(),
        backup_result.snapshot_id.clone(),
        root.path.join("missing").to_string_lossy().into_owned(),
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(missing_password.contains("encryption password must not be empty"));

    let wrong_password = restore(
        repository_dir.to_string_lossy().into_owned(),
        backup_result.snapshot_id,
        root.path.join("wrong").to_string_lossy().into_owned(),
        None,
        None,
        Some("wrong".to_string()),
    )
    .unwrap_err();
    assert!(wrong_password.contains("failed to unlock repository"));
}

#[test]
fn change_repository_password_command_rewraps_key() {
    let root = TestDir::new("tauri_change_repository_password");
    let source = root.path.join("source");
    let repository_dir = root.path.join("repository");
    let restore_dir = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "secret text").unwrap();
    create_repository(
        root.path.to_string_lossy().into_owned(),
        "repository".to_string(),
        Some("aes-256-gcm".to_string()),
        Some("old-password".to_string()),
    )
    .unwrap();
    let backup_result = backup(
        vec![source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
        None,
        None,
        Some(true),
        Some("old-password".to_string()),
    )
    .unwrap();

    change_repository_password(
        repository_dir.to_string_lossy().into_owned(),
        "old-password".to_string(),
        "new-password".to_string(),
    )
    .unwrap();

    assert!(unlock_repository(
        repository_dir.to_string_lossy().into_owned(),
        "old-password".to_string(),
    )
    .is_err());
    unlock_repository(
        repository_dir.to_string_lossy().into_owned(),
        "new-password".to_string(),
    )
    .unwrap();
    restore(
        repository_dir.to_string_lossy().into_owned(),
        backup_result.snapshot_id,
        restore_dir.to_string_lossy().into_owned(),
        None,
        None,
        Some("new-password".to_string()),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(restore_dir.join("a.txt")).unwrap(),
        "secret text"
    );
}

#[test]
fn delete_encrypted_snapshot_requires_password_but_plain_snapshot_does_not() {
    let root = TestDir::new("tauri_delete_encrypted_snapshot");
    let plain_source = root.path.join("plain");
    let encrypted_source = root.path.join("encrypted");
    let repository_dir = root.path.join("repository");
    fs::create_dir_all(&plain_source).unwrap();
    fs::create_dir_all(&encrypted_source).unwrap();
    fs::write(plain_source.join("plain.txt"), "plain").unwrap();
    fs::write(encrypted_source.join("secret.txt"), "secret").unwrap();
    create_repository(
        root.path.to_string_lossy().into_owned(),
        "repository".to_string(),
        Some("aes-256-gcm".to_string()),
        Some("password".to_string()),
    )
    .unwrap();
    let plain = backup(
        vec![plain_source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
        None,
        None,
        Some(false),
        None,
    )
    .unwrap();
    let encrypted = backup(
        vec![encrypted_source.to_string_lossy().into_owned()],
        repository_dir.to_string_lossy().into_owned(),
        None,
        None,
        None,
        Some(true),
        Some("password".to_string()),
    )
    .unwrap();

    delete_snapshot(
        repository_dir.to_string_lossy().into_owned(),
        plain.snapshot_id,
        None,
    )
    .unwrap();
    assert!(delete_snapshot(
        repository_dir.to_string_lossy().into_owned(),
        encrypted.snapshot_id.clone(),
        Some("wrong".to_string()),
    )
    .is_err());
    delete_snapshot(
        repository_dir.to_string_lossy().into_owned(),
        encrypted.snapshot_id,
        Some("password".to_string()),
    )
    .unwrap();
}

#[test]
fn delete_encrypted_repository_requires_password() {
    let root = TestDir::new("tauri_delete_encrypted_repository");
    let created = create_repository(
        root.path.to_string_lossy().into_owned(),
        "repository".to_string(),
        Some("aes-256-gcm".to_string()),
        Some("password".to_string()),
    )
    .unwrap();

    assert!(delete_repository(created.path.clone(), Some("wrong".to_string())).is_err());
    assert!(std::path::Path::new(&created.path).exists());
    delete_repository(created.path.clone(), Some("password".to_string())).unwrap();
    assert!(!std::path::Path::new(&created.path).exists());
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
        None,
        None,
        None,
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
        None,
        None,
        None,
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

fn object_header_text(path: &std::path::Path) -> String {
    let bytes = fs::read(path).unwrap();
    let end = bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .unwrap();
    String::from_utf8(bytes[..end].to_vec()).unwrap()
}
