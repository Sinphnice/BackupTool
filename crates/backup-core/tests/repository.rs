use backup_core::{
    ArchiveAlgorithm, BackupFilter, BackupOptions, CompressionAlgorithm, EncryptionAlgorithm,
    FileKind, FlattenConflictStrategy, Repository, RestoreOptions, RestorePathStrategy,
    RestoreStrategy, SnapshotId,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn creates_repository_layout_and_snapshot_file() {
    let root = TestDir::new("repo_layout");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    fs::create_dir_all(source.join("dir")).unwrap();
    fs::write(source.join("dir").join("a.txt"), "alpha").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let snapshot_file = repository.reader().read_snapshot(&snapshot.id).unwrap();

    assert!(repository_path.join("repo.meta").is_file());
    assert!(repository_path.join("snapshots").is_dir());
    assert!(repository_path.join("objects").is_dir());
    assert!(repository_path.join("indexes").is_dir());
    assert!(repository_path
        .join("snapshots")
        .join(format!("{}.snapshot", snapshot.id.as_str()))
        .is_file());
    assert!(!snapshot.id.as_str().starts_with("snapshot-"));
    assert_eq!(snapshot.id.as_str().split('-').count(), 3);
    assert_eq!(
        snapshot_file.created_unix_seconds,
        snapshot.created_unix_seconds
    );
    assert_eq!(
        snapshot_file.created_nanoseconds,
        snapshot.created_nanoseconds
    );
    assert_eq!(snapshot_file.sequence, snapshot.sequence);
    let text = fs::read_to_string(
        repository_path
            .join("snapshots")
            .join(format!("{}.snapshot", snapshot.id.as_str())),
    )
    .unwrap();
    assert!(text.starts_with("backup-tool snapshot v1\n"));
    assert!(text.contains(&format!(
        "created\t{}\t{}\t{}",
        snapshot.created_unix_seconds, snapshot.created_nanoseconds, snapshot.sequence
    )));
    assert!(snapshot_file
        .entries
        .iter()
        .any(|entry| entry.kind == FileKind::Directory
            && entry.relative_path == PathBuf::from("dir")));
    assert!(snapshot_file.entries.iter().any(|entry| {
        entry.kind == FileKind::File
            && entry.relative_path == PathBuf::from("dir").join("a.txt")
            && entry.size == 5
            && entry.object_id.is_some()
    }));
}

#[test]
fn snapshot_title_is_stored_and_validated() {
    let root = TestDir::new("repo_snapshot_title");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                snapshot_title: Some("  每日备份 title  ".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let snapshot_file = repository.reader().read_snapshot(&snapshot.id).unwrap();
    assert_eq!(snapshot.title.as_deref(), Some("每日备份 title"));
    assert_eq!(snapshot_file.title.as_deref(), Some("每日备份 title"));

    let empty_title = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                snapshot_title: Some("   ".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(empty_title.title, None);

    let error = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                snapshot_title: Some("x".repeat(121)),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("snapshot title must be at most 120 characters"));
}

#[test]
fn consecutive_backups_create_restorable_snapshots() {
    let root = TestDir::new("repo_snapshots");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source = root.path.join("source");
    let restore_first = root.path.join("restore_first");
    let restore_second = root.path.join("restore_second");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "first").unwrap();

    let first = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    fs::write(source.join("a.txt"), "second").unwrap();
    fs::write(source.join("b.txt"), "new").unwrap();
    let second = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();

    assert_ne!(first.id.as_str(), second.id.as_str());

    repository
        .reader()
        .restore(&first.id, &restore_first)
        .unwrap();
    repository
        .reader()
        .restore(&second.id, &restore_second)
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore_first.join("a.txt")).unwrap(),
        "first"
    );
    assert!(!restore_first.join("b.txt").exists());
    assert_eq!(
        fs::read_to_string(restore_second.join("a.txt")).unwrap(),
        "second"
    );
    assert_eq!(
        fs::read_to_string(restore_second.join("b.txt")).unwrap(),
        "new"
    );
}

#[test]
fn multi_source_backup_restores_under_source_root_names_by_default() {
    let root = TestDir::new("repo_multi_source_relative");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source_a = root.path.join("alpha");
    let source_b = root.path.join("beta");
    let restore = root.path.join("restore");
    fs::create_dir_all(source_a.join("docs")).unwrap();
    fs::create_dir_all(&source_b).unwrap();
    fs::write(source_a.join("docs").join("a.txt"), "alpha").unwrap();
    fs::write(source_b.join("b.txt"), "beta").unwrap();

    let snapshot = repository
        .writer()
        .backup_many([&source_a, &source_b], &BackupFilter::default())
        .unwrap();
    let snapshot_file = repository.reader().read_snapshot(&snapshot.id).unwrap();
    assert_eq!(snapshot_file.sources.len(), 2);

    repository.reader().restore(&snapshot.id, &restore).unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("alpha").join("docs").join("a.txt")).unwrap(),
        "alpha"
    );
    assert_eq!(
        fs::read_to_string(restore.join("beta").join("b.txt")).unwrap(),
        "beta"
    );
}

#[test]
fn relative_source_root_conflict_error_fails() {
    let (_root, repository, snapshot, restore) = duplicate_source_root_snapshot("repo_root_error");

    let error = repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                flatten_conflict_strategy: FlattenConflictStrategy::Error,
                ..RestoreOptions::default()
            },
        )
        .unwrap_err();

    assert!(matches!(error, backup_core::BackupError::PathConflict(_)));
}

#[test]
fn relative_source_root_conflict_skip_omits_later_source() {
    let (_root, repository, snapshot, restore) = duplicate_source_root_snapshot("repo_root_skip");

    repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                flatten_conflict_strategy: FlattenConflictStrategy::Skip,
                ..RestoreOptions::default()
            },
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("data").join("a.txt")).unwrap(),
        "alpha"
    );
    assert!(!restore.join("data").join("b.txt").exists());
}

#[test]
fn relative_source_root_conflict_overwrite_merges_same_root() {
    let (_root, repository, snapshot, restore) =
        duplicate_source_root_snapshot("repo_root_overwrite");

    repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                flatten_conflict_strategy: FlattenConflictStrategy::Overwrite,
                ..RestoreOptions::default()
            },
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("data").join("a.txt")).unwrap(),
        "alpha"
    );
    assert_eq!(
        fs::read_to_string(restore.join("data").join("b.txt")).unwrap(),
        "beta"
    );
}

#[test]
fn relative_source_root_conflict_rename_keeps_all_sources() {
    let (_root, repository, snapshot, restore) = duplicate_source_root_snapshot("repo_root_rename");

    repository.reader().restore(&snapshot.id, &restore).unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("data").join("a.txt")).unwrap(),
        "alpha"
    );
    assert_eq!(
        fs::read_to_string(restore.join("data (1)").join("b.txt")).unwrap(),
        "beta"
    );
}

#[test]
fn backup_many_deduplicates_child_sources() {
    let root = TestDir::new("repo_multi_source_dedup");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source = root.path.join("source");
    let child = source.join("child");
    fs::create_dir_all(&child).unwrap();
    fs::write(source.join("root.txt"), "root").unwrap();
    fs::write(child.join("child.txt"), "child").unwrap();

    let snapshot = repository
        .writer()
        .backup_many([&source, &child, &source], &BackupFilter::default())
        .unwrap();
    let snapshot_file = repository.reader().read_snapshot(&snapshot.id).unwrap();

    assert_eq!(snapshot_file.sources.len(), 1);
    assert_eq!(snapshot.ignored_sources.len(), 2);
    assert_eq!(
        snapshot_file
            .entries
            .iter()
            .filter(|entry| entry.kind == FileKind::File)
            .count(),
        2
    );
}

#[test]
fn preserve_full_path_restores_under_safe_absolute_path() {
    let root = TestDir::new("repo_full_path_restore");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();

    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                path_strategy: RestorePathStrategy::PreserveFullPath,
                ..RestoreOptions::default()
            },
        )
        .unwrap();

    let restored_files = collect_file_names(&restore);
    assert!(restored_files.iter().any(|path| path.ends_with("a.txt")));
    assert!(restored_files
        .iter()
        .any(|path| path.contains("source") && path.ends_with("a.txt")));
}

#[test]
fn flatten_conflict_error_fails_on_duplicate_file_names() {
    let (_root, repository, snapshot, restore) = duplicate_name_snapshot("repo_flatten_error");

    let error = repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                path_strategy: RestorePathStrategy::Flatten,
                flatten_conflict_strategy: FlattenConflictStrategy::Error,
                ..RestoreOptions::default()
            },
        )
        .unwrap_err();

    assert!(matches!(error, backup_core::BackupError::PathConflict(_)));
}

#[test]
fn flatten_conflict_skip_keeps_first_file() {
    let (_root, repository, snapshot, restore) = duplicate_name_snapshot("repo_flatten_skip");

    repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                path_strategy: RestorePathStrategy::Flatten,
                flatten_conflict_strategy: FlattenConflictStrategy::Skip,
                ..RestoreOptions::default()
            },
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("same.txt")).unwrap(),
        "alpha"
    );
}

#[test]
fn flatten_conflict_overwrite_uses_last_file() {
    let (_root, repository, snapshot, restore) = duplicate_name_snapshot("repo_flatten_overwrite");

    repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                path_strategy: RestorePathStrategy::Flatten,
                flatten_conflict_strategy: FlattenConflictStrategy::Overwrite,
                ..RestoreOptions::default()
            },
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("same.txt")).unwrap(),
        "beta"
    );
}

#[test]
fn flatten_conflict_rename_keeps_all_files() {
    let (_root, repository, snapshot, restore) = duplicate_name_snapshot("repo_flatten_rename");

    repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                path_strategy: RestorePathStrategy::Flatten,
                flatten_conflict_strategy: FlattenConflictStrategy::Rename,
                ..RestoreOptions::default()
            },
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("same.txt")).unwrap(),
        "alpha"
    );
    assert_eq!(
        fs::read_to_string(restore.join("same (1).txt")).unwrap(),
        "beta"
    );
}

#[test]
fn lists_snapshot_summary_from_repository_snapshot_files() {
    let root = TestDir::new("repo_list_snapshots");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "first").unwrap();

    let first = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    fs::write(source.join("b.txt"), "second").unwrap();
    let second = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();

    let snapshots = repository.reader().list_snapshots().unwrap();

    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].id.as_str(), second.id.as_str());
    assert_eq!(snapshots[0].file_count, 2);
    assert_eq!(snapshots[0].byte_count, 11);
    assert!(snapshots[0].created_unix_seconds.is_some());
    assert_eq!(snapshots[1].id.as_str(), first.id.as_str());
    assert_eq!(snapshots[1].file_count, 1);
    assert_eq!(snapshots[1].byte_count, 5);
}

#[test]
fn repository_backup_applies_filter_before_storing_objects() {
    let root = TestDir::new("repo_filter");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("keep.txt"), "keep").unwrap();
    fs::write(source.join("skip.bin"), "skip").unwrap();

    let snapshot = repository
        .writer()
        .backup(
            &source,
            &BackupFilter {
                path_regex: Some(r"\.txt$".to_string()),
                ..BackupFilter::default()
            },
        )
        .unwrap();

    repository.reader().restore(&snapshot.id, &restore).unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("keep.txt")).unwrap(),
        "keep"
    );
    assert!(!restore.join("skip.bin").exists());
}

#[test]
fn repository_backup_owner_filter_excludes_non_matching_owner() {
    let root = TestDir::new("repo_owner_filter");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("file.txt"), "owned").unwrap();

    let snapshot = repository
        .writer()
        .backup(
            &source,
            &BackupFilter {
                owner: Some("__backup_tool_owner_that_should_not_exist__".to_string()),
                ..BackupFilter::default()
            },
        )
        .unwrap();

    let snapshot_file = repository.reader().read_snapshot(&snapshot.id).unwrap();
    assert_eq!(
        snapshot_file
            .entries
            .iter()
            .filter(|entry| entry.object_id.is_some())
            .count(),
        0
    );
}

#[test]
fn missing_snapshot_returns_error() {
    let root = TestDir::new("missing_snapshot");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let error = repository
        .reader()
        .read_snapshot(&SnapshotId::from("missing".to_string()))
        .unwrap_err();

    assert!(error.is_snapshot_missing());
}

#[test]
fn delete_snapshot_removes_exclusive_objects() {
    let root = TestDir::new("repo_delete_snapshot_exclusive");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "exclusive content").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let object_id = first_file_object_id(&repository, &snapshot.id);
    let object_path = repository_path.join("objects").join(object_id.as_str());
    let object_size = fs::metadata(&object_path).unwrap().len();

    let result = repository.writer().delete_snapshot(&snapshot.id).unwrap();

    assert_eq!(result.snapshot_id, snapshot.id);
    assert_eq!(result.deleted_object_count, 1);
    assert_eq!(result.reclaimed_bytes, object_size);
    assert!(result.warnings.is_empty());
    assert!(!object_path.exists());
    assert!(repository.reader().list_snapshots().unwrap().is_empty());
}

#[test]
fn delete_snapshot_keeps_objects_referenced_by_another_snapshot() {
    let root = TestDir::new("repo_delete_snapshot_shared");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "shared content").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let first = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let second = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let object_id = first_file_object_id(&repository, &first.id);

    let result = repository.writer().delete_snapshot(&first.id).unwrap();

    assert_eq!(result.deleted_object_count, 0);
    assert!(repository_path
        .join("objects")
        .join(object_id.as_str())
        .exists());
    repository.reader().restore(&second.id, &restore).unwrap();
    assert_eq!(
        fs::read_to_string(restore.join("a.txt")).unwrap(),
        "shared content"
    );
}

#[test]
fn delete_snapshot_treats_plain_and_encrypted_objects_as_distinct_references() {
    let root = TestDir::new("repo_delete_snapshot_variants");
    let repository_path = root.path.join("repository");
    let plain_source = root.path.join("plain");
    let encrypted_source = root.path.join("encrypted");
    let restore = root.path.join("restore");
    fs::create_dir_all(&plain_source).unwrap();
    fs::create_dir_all(&encrypted_source).unwrap();
    fs::write(plain_source.join("plain.txt"), "same content").unwrap();
    fs::write(encrypted_source.join("encrypted.txt"), "same content").unwrap();

    let repository = encrypted_repository(&repository_path, "password");
    let plain = repository
        .writer()
        .backup(&plain_source, &BackupFilter::default())
        .unwrap();
    let encrypted = repository
        .writer()
        .backup_with_options(
            &encrypted_source,
            &BackupFilter::default(),
            BackupOptions {
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let plain_object = first_file_object_id(&repository, &plain.id);
    let encrypted_object = first_file_object_id(&repository, &encrypted.id);

    let result = repository
        .writer()
        .delete_snapshot_with_password(&encrypted.id, Some("password"))
        .unwrap();

    assert_eq!(result.deleted_object_count, 1);
    assert!(repository_path
        .join("objects")
        .join(plain_object.as_str())
        .exists());
    assert!(!repository_path
        .join("objects")
        .join(encrypted_object.as_str())
        .exists());
    repository.reader().restore(&plain.id, &restore).unwrap();
    assert_eq!(
        fs::read_to_string(restore.join("plain.txt")).unwrap(),
        "same content"
    );
}

#[test]
fn delete_snapshot_validates_other_snapshots_before_mutating_repository() {
    let root = TestDir::new("repo_delete_snapshot_corrupt_other");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "first content").unwrap();

    let repository = encrypted_repository(&repository_path, "correct horse battery staple");
    let first = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let object_id = first_file_object_id(&repository, &first.id);
    fs::write(source.join("a.txt"), "second content").unwrap();
    let second = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    fs::write(
        repository_path
            .join("snapshots")
            .join(format!("{}.snapshot", second.id.as_str())),
        "invalid snapshot",
    )
    .unwrap();

    repository.writer().delete_snapshot(&first.id).unwrap_err();

    assert!(repository_path
        .join("snapshots")
        .join(format!("{}.snapshot", first.id.as_str()))
        .exists());
    assert!(repository_path
        .join("objects")
        .join(object_id.as_str())
        .exists());
}

#[test]
fn delete_snapshot_reports_object_cleanup_failures_as_warnings() {
    let root = TestDir::new("repo_delete_snapshot_cleanup_warning");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "cleanup warning").unwrap();

    let repository = encrypted_repository(&repository_path, "password");
    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let object_id = first_file_object_id(&repository, &snapshot.id);
    let object_path = repository_path.join("objects").join(object_id.as_str());
    fs::remove_file(&object_path).unwrap();
    fs::create_dir(&object_path).unwrap();

    let result = repository.writer().delete_snapshot(&snapshot.id).unwrap();

    assert_eq!(result.deleted_object_count, 0);
    assert_eq!(result.warnings.len(), 1);
    assert!(!repository_path
        .join("snapshots")
        .join(format!("{}.snapshot", snapshot.id.as_str()))
        .exists());
}

#[test]
fn best_effort_restore_preserves_file_modified_time_and_readonly_attribute() {
    let root = TestDir::new("repo_metadata_best_effort");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    let source_file = source.join("readonly.txt");
    fs::create_dir_all(&source).unwrap();
    fs::write(&source_file, "metadata").unwrap();
    let expected_modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    set_modified_time(&source_file, expected_modified);
    set_readonly(&source_file, true);

    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let report = repository
        .reader()
        .restore_with_options(&snapshot.id, &restore, RestoreOptions::default())
        .unwrap();

    let restored_file = restore.join("readonly.txt");
    assert!(report.warnings.is_empty());
    assert_eq!(fs::read_to_string(&restored_file).unwrap(), "metadata");
    assert!(fs::metadata(&restored_file)
        .unwrap()
        .permissions()
        .readonly());
    let restored_modified = fs::metadata(&restored_file)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert_eq!(restored_modified, 1_700_000_000);

    set_readonly(&source_file, false);
    set_readonly(&restored_file, false);
}

#[test]
fn data_only_restore_skips_file_metadata() {
    let root = TestDir::new("repo_metadata_data_only");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    let source_file = source.join("readonly.txt");
    fs::create_dir_all(&source).unwrap();
    fs::write(&source_file, "metadata").unwrap();
    set_readonly(&source_file, true);

    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let report = repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                strategy: RestoreStrategy::DataOnly,
                ..RestoreOptions::default()
            },
        )
        .unwrap();

    let restored_file = restore.join("readonly.txt");
    assert!(report.warnings.is_empty());
    assert_eq!(fs::read_to_string(&restored_file).unwrap(), "metadata");
    assert!(!fs::metadata(&restored_file)
        .unwrap()
        .permissions()
        .readonly());

    set_readonly(&source_file, false);
}

#[test]
fn strict_restore_succeeds_when_recorded_metadata_is_supported() {
    let root = TestDir::new("repo_metadata_strict");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();

    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let report = repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                strategy: RestoreStrategy::Strict,
                ..RestoreOptions::default()
            },
        )
        .unwrap();

    assert!(report.warnings.is_empty());
    assert_eq!(fs::read_to_string(restore.join("a.txt")).unwrap(), "alpha");
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "backup_core_repository_{name}_{}",
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

fn encrypted_repository(path: impl AsRef<Path>, password: &str) -> Repository {
    Repository::init_with_options(
        path.as_ref(),
        None,
        EncryptionAlgorithm::Aes256Gcm,
        Some(password.to_string()),
    )
    .unwrap()
}

trait SnapshotDoesNotExistExt {
    fn is_snapshot_missing(&self) -> bool;
}

impl SnapshotDoesNotExistExt for backup_core::BackupError {
    fn is_snapshot_missing(&self) -> bool {
        matches!(self, backup_core::BackupError::SnapshotDoesNotExist(_))
    }
}

#[test]
fn repository_exports_and_imports_tar_archive() {
    let root = TestDir::new("repo_archive_round_trip");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let archive = root.path.join("repository.tar");
    let imported = root.path.join("imported");
    let restored = root.path.join("restored");
    fs::create_dir_all(source.join("docs")).unwrap();
    fs::write(source.join("docs").join("a.txt"), "alpha").unwrap();
    fs::write(source.join("b.txt"), "beta").unwrap();

    let repository = encrypted_repository(&repository_path, "password");
    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let exported = repository
        .export_archive(&archive, ArchiveAlgorithm::Tar)
        .unwrap();
    assert_eq!(exported.algorithm, ArchiveAlgorithm::Tar);
    assert!(exported.byte_count > 0);
    assert!(archive.is_file());

    let imported_repository =
        Repository::import_archive(&archive, &imported, ArchiveAlgorithm::Tar).unwrap();
    let snapshots = imported_repository.reader().list_snapshots().unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].id.as_str(), snapshot.id.as_str());

    imported_repository
        .reader()
        .restore(&snapshot.id, &restored)
        .unwrap();
    assert_eq!(
        fs::read_to_string(restored.join("docs").join("a.txt")).unwrap(),
        "alpha"
    );
    assert_eq!(fs::read_to_string(restored.join("b.txt")).unwrap(), "beta");
}

#[test]
fn zstd_backup_stores_compressed_object_and_restores_original_content() {
    let root = TestDir::new("repo_zstd_round_trip");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("text.txt"), "alpha alpha alpha alpha").unwrap();
    fs::write(source.join("binary.bin"), [0_u8, 1, 2, 3, 255]).unwrap();
    fs::write(source.join("empty.txt"), []).unwrap();

    let repository = encrypted_repository(&repository_path, "correct horse battery staple");
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                compression_algorithm: CompressionAlgorithm::Zstd,
                ..Default::default()
            },
        )
        .unwrap();
    let snapshot_file = repository.reader().read_snapshot(&snapshot.id).unwrap();
    let file_entries = snapshot_file
        .entries
        .iter()
        .filter(|entry| entry.kind == FileKind::File)
        .collect::<Vec<_>>();
    assert_eq!(file_entries.len(), 3);
    for entry in file_entries {
        let object_id = entry.object_id.as_ref().unwrap();
        let content_hash = object_id.as_str().strip_suffix("-plain").unwrap();
        assert_eq!(content_hash.len(), 64);
        assert!(content_hash
            .chars()
            .all(|value| value.is_ascii_digit() || ('a'..='f').contains(&value)));
        let object_path = repository_path.join("objects").join(object_id.as_str());
        assert!(object_path.exists());
        let header = object_header_text(&object_path);
        assert!(header.contains("compression\tzstd"));
        assert!(header.contains("crc32\t"));
        assert!(!repository_path
            .join("objects")
            .join(format!("{}.zst", object_id.as_str()))
            .exists());
    }

    repository.reader().restore(&snapshot.id, &restore).unwrap();
    assert_eq!(
        fs::read_to_string(restore.join("text.txt")).unwrap(),
        "alpha alpha alpha alpha"
    );
    assert_eq!(
        fs::read(restore.join("binary.bin")).unwrap(),
        [0, 1, 2, 3, 255]
    );
    assert_eq!(
        fs::read(restore.join("empty.txt")).unwrap(),
        Vec::<u8>::new()
    );
}

#[test]
fn object_id_uses_original_content_not_compressed_bytes() {
    let root = TestDir::new("repo_zstd_object_id");
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "same same same").unwrap();

    let none_repository_path = root.path.join("none_repository");
    let none_repository = Repository::init(&none_repository_path).unwrap();
    let none_snapshot = none_repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let none_snapshot_file = none_repository
        .reader()
        .read_snapshot(&none_snapshot.id)
        .unwrap();
    let none_object = none_snapshot_file
        .entries
        .iter()
        .find(|entry| entry.kind == FileKind::File)
        .and_then(|entry| entry.object_id.as_ref())
        .unwrap()
        .clone();

    let zstd_repository_path = root.path.join("zstd_repository");
    let zstd_repository = Repository::init(&zstd_repository_path).unwrap();
    let zstd_snapshot = zstd_repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                compression_algorithm: CompressionAlgorithm::Zstd,
                ..Default::default()
            },
        )
        .unwrap();
    let zstd_snapshot_file = zstd_repository
        .reader()
        .read_snapshot(&zstd_snapshot.id)
        .unwrap();
    let zstd_object = zstd_snapshot_file
        .entries
        .iter()
        .find(|entry| entry.kind == FileKind::File)
        .and_then(|entry| entry.object_id.as_ref())
        .unwrap()
        .clone();

    assert_eq!(none_object, zstd_object);
    assert!(none_repository_path
        .join("objects")
        .join(none_object.as_str())
        .exists());
    assert!(zstd_repository_path
        .join("objects")
        .join(zstd_object.as_str())
        .exists());
    assert!(object_header_text(
        &none_repository_path
            .join("objects")
            .join(none_object.as_str())
    )
    .contains("compression\tnone"));
    assert!(object_header_text(
        &zstd_repository_path
            .join("objects")
            .join(zstd_object.as_str())
    )
    .contains("compression\tzstd"));
}

#[test]
fn aes_encrypted_backup_restores_with_password() {
    let root = TestDir::new("repo_aes_round_trip");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("secret.txt"), "very secret content").unwrap();

    let repository = encrypted_repository(&repository_path, "correct horse battery staple");
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("correct horse battery staple".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let object_id = first_file_object_id(&repository, &snapshot.id);
    let object_path = repository_path.join("objects").join(object_id.as_str());
    let object_bytes = fs::read(&object_path).unwrap();
    let object_text = String::from_utf8_lossy(&object_bytes);
    let header = object_header_text(&object_path);
    assert!(header.contains("encryption\taes-256-gcm"));
    assert!(header.contains("key_id\t"));
    assert!(header.contains("nonce\t"));
    assert!(!object_text.contains("very secret content"));

    repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                decryption_password: Some("correct horse battery staple".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("secret.txt")).unwrap(),
        "very secret content"
    );
}

#[test]
fn encrypted_restore_requires_correct_password() {
    let root = TestDir::new("repo_aes_password_required");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("secret.txt"), "secret").unwrap();

    let repository = encrypted_repository(&repository_path, "right-password");
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("right-password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

    let missing = repository
        .reader()
        .restore(&snapshot.id, root.path.join("missing_password"))
        .unwrap_err();
    assert!(missing
        .to_string()
        .contains("encryption password must not be empty"));

    let wrong = repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            root.path.join("wrong_password"),
            RestoreOptions {
                decryption_password: Some("wrong-password".to_string()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(wrong.to_string().contains("failed to unlock repository"));
}

#[test]
fn encrypted_repository_can_reopen_and_unlock_master_key() {
    let root = TestDir::new("repo_reopen_unlock_master_key");
    let repository_path = root.path.join("repository");
    encrypted_repository(&repository_path, "password");

    let reopened = Repository::open(&repository_path).unwrap();
    reopened
        .verify_encryption_password(Some("password"))
        .unwrap();
    let error = reopened
        .verify_encryption_password(Some("wrong-password"))
        .unwrap_err();

    assert!(error.to_string().contains("failed to unlock repository"));
}

#[test]
fn change_repository_password_rewraps_master_key_without_rewriting_objects() {
    let root = TestDir::new("repo_change_password");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("secret.txt"), "secret").unwrap();

    let repository = encrypted_repository(&repository_path, "old-password");
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("old-password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let object_id = first_file_object_id(&repository, &snapshot.id);
    let object_path = repository_path.join("objects").join(object_id.as_str());
    let object_before = fs::read(&object_path).unwrap();

    repository
        .change_encryption_password("old-password", "new-password")
        .unwrap();

    assert!(repository
        .verify_encryption_password(Some("old-password"))
        .is_err());
    repository
        .verify_encryption_password(Some("new-password"))
        .unwrap();
    assert_eq!(fs::read(&object_path).unwrap(), object_before);
    assert_eq!(first_file_object_id(&repository, &snapshot.id), object_id);
    repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                decryption_password: Some("new-password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(restore.join("secret.txt")).unwrap(),
        "secret"
    );
}

#[test]
fn failed_password_change_keeps_old_password_valid() {
    let root = TestDir::new("repo_change_password_failure");
    let repository_path = root.path.join("repository");
    let repository = encrypted_repository(&repository_path, "old-password");
    fs::create_dir(repository_path.join("repo.meta.tmp")).unwrap();

    repository
        .change_encryption_password("old-password", "new-password")
        .unwrap_err();

    repository
        .verify_encryption_password(Some("old-password"))
        .unwrap();
}

#[test]
fn old_repository_metadata_format_returns_controlled_error() {
    let root = TestDir::new("repo_old_metadata_rejected");
    let repository_path = root.path.join("repository");
    fs::create_dir_all(repository_path.join("objects")).unwrap();
    fs::create_dir_all(repository_path.join("snapshots")).unwrap();
    fs::create_dir_all(repository_path.join("indexes")).unwrap();
    fs::write(
        repository_path.join("repo.meta"),
        "backup-tool repository v1\ndisplay_name\told\nencryption\tnone\n",
    )
    .unwrap();

    let error = Repository::open(&repository_path).unwrap_err();

    assert!(error
        .to_string()
        .contains("old repositories are not supported"));
}

#[test]
fn plain_and_encrypted_variants_of_same_content_coexist() {
    let root = TestDir::new("repo_file_level_encryption");
    let repository_path = root.path.join("repository");
    let plain_source = root.path.join("plain_source");
    let encrypted_source = root.path.join("encrypted_source");
    let restore_plain = root.path.join("restore_plain");
    fs::create_dir_all(&plain_source).unwrap();
    fs::create_dir_all(&encrypted_source).unwrap();
    fs::write(plain_source.join("plain.txt"), "shared content").unwrap();
    fs::write(encrypted_source.join("secret.txt"), "shared content").unwrap();

    let repository = encrypted_repository(&repository_path, "password");
    let plain_snapshot = repository
        .writer()
        .backup(&plain_source, &BackupFilter::default())
        .unwrap();
    let encrypted_snapshot = repository
        .writer()
        .backup_with_options(
            &encrypted_source,
            &BackupFilter::default(),
            BackupOptions {
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

    let plain_object = first_file_object_id(&repository, &plain_snapshot.id);
    let encrypted_object = first_file_object_id(&repository, &encrypted_snapshot.id);
    assert_ne!(plain_object, encrypted_object);
    assert_eq!(
        plain_object.as_str().strip_suffix("-plain").unwrap(),
        encrypted_object
            .as_str()
            .strip_suffix("-encrypted")
            .unwrap()
    );
    assert!(repository_path
        .join("objects")
        .join(plain_object.as_str())
        .exists());
    assert!(repository_path
        .join("objects")
        .join(encrypted_object.as_str())
        .exists());

    repository
        .reader()
        .restore(&plain_snapshot.id, &restore_plain)
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore_plain.join("plain.txt")).unwrap(),
        "shared content"
    );
}

#[test]
fn encrypted_files_with_same_content_share_one_object() {
    let root = TestDir::new("repo_encrypted_deduplication");
    let repository_path = root.path.join("repository");
    let first_source = root.path.join("first_source");
    let second_source = root.path.join("second_source");
    fs::create_dir_all(&first_source).unwrap();
    fs::create_dir_all(&second_source).unwrap();
    fs::write(first_source.join("first.txt"), "shared encrypted content").unwrap();
    fs::write(second_source.join("second.txt"), "shared encrypted content").unwrap();

    let repository = encrypted_repository(&repository_path, "password");
    let snapshot = repository
        .writer()
        .backup_many_with_options(
            [&first_source, &second_source],
            &BackupFilter::default(),
            BackupOptions {
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let snapshot_file = repository.reader().read_snapshot(&snapshot.id).unwrap();
    let object_ids = snapshot_file
        .entries
        .iter()
        .filter(|entry| entry.kind == FileKind::File)
        .map(|entry| entry.object_id.as_ref().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(object_ids.len(), 2);
    assert_eq!(object_ids[0], object_ids[1]);
    assert!(object_ids[0].as_str().ends_with("-encrypted"));
    assert_eq!(
        fs::read_dir(repository_path.join("objects"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn encrypted_object_rejects_a_different_password_without_overwriting() {
    let root = TestDir::new("repo_encrypted_password_conflict");
    let repository_path = root.path.join("repository");
    let first_source = root.path.join("first_source");
    let second_source = root.path.join("second_source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&first_source).unwrap();
    fs::create_dir_all(&second_source).unwrap();
    fs::write(first_source.join("first.txt"), "shared encrypted content").unwrap();
    fs::write(second_source.join("second.txt"), "shared encrypted content").unwrap();

    let repository = encrypted_repository(&repository_path, "first-password");
    let first_snapshot = repository
        .writer()
        .backup_with_options(
            &first_source,
            &BackupFilter::default(),
            BackupOptions {
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("first-password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let error = repository
        .writer()
        .backup_with_options(
            &second_source,
            &BackupFilter::default(),
            BackupOptions {
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("second-password".to_string()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("failed to unlock repository"));

    repository
        .reader()
        .restore_with_options(
            &first_snapshot.id,
            &restore,
            RestoreOptions {
                decryption_password: Some("first-password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(restore.join("first.txt")).unwrap(),
        "shared encrypted content"
    );
}

#[test]
fn zstd_and_aes_encrypted_backup_restores_original_content() {
    let root = TestDir::new("repo_zstd_aes_round_trip");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "compressed and encrypted text").unwrap();

    let repository = encrypted_repository(&repository_path, "password");
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                compression_algorithm: CompressionAlgorithm::Zstd,
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let object_id = first_file_object_id(&repository, &snapshot.id);
    let header = object_header_text(&repository_path.join("objects").join(object_id.as_str()));
    assert!(header.contains("compression\tzstd"));
    assert!(header.contains("encryption\taes-256-gcm"));

    repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                decryption_password: Some("password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("a.txt")).unwrap(),
        "compressed and encrypted text"
    );
}

#[test]
fn object_id_uses_content_hash_and_encryption_state() {
    let root = TestDir::new("repo_aes_object_id");
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "same content").unwrap();

    let plain_repository = Repository::init(root.path.join("plain_repository")).unwrap();
    let plain_snapshot = plain_repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let plain_object = first_file_object_id(&plain_repository, &plain_snapshot.id);

    let encrypted_repository =
        encrypted_repository(root.path.join("encrypted_repository"), "password");
    let encrypted_snapshot = encrypted_repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let encrypted_object = first_file_object_id(&encrypted_repository, &encrypted_snapshot.id);

    assert_ne!(plain_object, encrypted_object);
    assert_eq!(
        plain_object.as_str().strip_suffix("-plain").unwrap(),
        encrypted_object
            .as_str()
            .strip_suffix("-encrypted")
            .unwrap()
    );
}

#[test]
fn tar_export_import_preserves_encrypted_objects() {
    let root = TestDir::new("repo_aes_tar_round_trip");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let archive = root.path.join("repository.tar");
    let imported = root.path.join("imported");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "encrypted archive text").unwrap();

    let repository = encrypted_repository(&repository_path, "password");
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                encryption_algorithm: EncryptionAlgorithm::Aes256Gcm,
                encryption_password: Some("password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    repository
        .export_archive(&archive, ArchiveAlgorithm::Tar)
        .unwrap();
    let imported_repository =
        Repository::import_archive(&archive, &imported, ArchiveAlgorithm::Tar).unwrap();

    imported_repository
        .reader()
        .restore_with_options(
            &snapshot.id,
            &restore,
            RestoreOptions {
                decryption_password: Some("password".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("a.txt")).unwrap(),
        "encrypted archive text"
    );
}

#[test]
fn tar_export_import_preserves_zstd_objects() {
    let root = TestDir::new("repo_zstd_tar_round_trip");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let archive = root.path.join("repository.tar");
    let imported = root.path.join("imported");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "compressed text").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                compression_algorithm: CompressionAlgorithm::Zstd,
                ..Default::default()
            },
        )
        .unwrap();
    repository
        .export_archive(&archive, ArchiveAlgorithm::Tar)
        .unwrap();
    let imported_repository =
        Repository::import_archive(&archive, &imported, ArchiveAlgorithm::Tar).unwrap();

    imported_repository
        .reader()
        .restore(&snapshot.id, &restore)
        .unwrap();

    assert_eq!(
        fs::read_to_string(restore.join("a.txt")).unwrap(),
        "compressed text"
    );
}

#[test]
fn snapshot_entry_does_not_store_compression_algorithm() {
    let root = TestDir::new("repo_snapshot_no_compression");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "snapshot").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                compression_algorithm: CompressionAlgorithm::Zstd,
                ..Default::default()
            },
        )
        .unwrap();
    let snapshot_path = repository_path
        .join("snapshots")
        .join(format!("{}.snapshot", snapshot.id.as_str()));
    let text = fs::read_to_string(&snapshot_path).unwrap();

    for line in text.lines().filter(|line| line.starts_with("entry\t")) {
        assert_eq!(line.split('\t').count(), 13);
        assert!(!line.contains("\tzstd"));
        assert!(!line.contains("\tnone"));
    }
}

#[test]
fn same_object_id_is_rewritten_when_compression_algorithm_changes() {
    let root = TestDir::new("repo_object_rewrite_zstd");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore_first = root.path.join("restore_first");
    let restore_second = root.path.join("restore_second");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "same same same").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let first = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let first_object = first_file_object_id(&repository, &first.id);
    assert!(
        object_header_text(&repository_path.join("objects").join(first_object.as_str()))
            .contains("compression\tnone")
    );

    let second = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                compression_algorithm: CompressionAlgorithm::Zstd,
                ..Default::default()
            },
        )
        .unwrap();
    let second_object = first_file_object_id(&repository, &second.id);
    assert_eq!(first_object, second_object);
    assert!(
        object_header_text(&repository_path.join("objects").join(first_object.as_str()))
            .contains("compression\tzstd")
    );

    repository
        .reader()
        .restore(&first.id, &restore_first)
        .unwrap();
    repository
        .reader()
        .restore(&second.id, &restore_second)
        .unwrap();
    assert_eq!(
        fs::read_to_string(restore_first.join("a.txt")).unwrap(),
        "same same same"
    );
    assert_eq!(
        fs::read_to_string(restore_second.join("a.txt")).unwrap(),
        "same same same"
    );
}

#[test]
fn same_object_id_can_be_rewritten_back_to_uncompressed() {
    let root = TestDir::new("repo_object_rewrite_none");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "same same same").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let first = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                compression_algorithm: CompressionAlgorithm::Zstd,
                ..Default::default()
            },
        )
        .unwrap();
    let object_id = first_file_object_id(&repository, &first.id);
    assert!(
        object_header_text(&repository_path.join("objects").join(object_id.as_str()))
            .contains("compression\tzstd")
    );

    repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    assert!(
        object_header_text(&repository_path.join("objects").join(object_id.as_str()))
            .contains("compression\tnone")
    );

    repository.reader().restore(&first.id, &restore).unwrap();
    assert_eq!(
        fs::read_to_string(restore.join("a.txt")).unwrap(),
        "same same same"
    );
}

#[test]
fn invalid_object_magic_returns_error() {
    let root = TestDir::new("repo_object_bad_magic");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let object_id = first_file_object_id(&repository, &snapshot.id);
    fs::write(
        repository_path.join("objects").join(object_id.as_str()),
        "bad\n\npayload",
    )
    .unwrap();

    let error = repository
        .reader()
        .restore(&snapshot.id, &restore)
        .unwrap_err();

    assert!(error.to_string().contains("invalid object magic"));
}

#[test]
fn object_payload_size_mismatch_returns_error() {
    let root = TestDir::new("repo_object_bad_payload_size");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let object_id = first_file_object_id(&repository, &snapshot.id);
    fs::write(
        repository_path.join("objects").join(object_id.as_str()),
        "backup-tool object v1\ncompression\tnone\nencryption\tnone\nkey_id\t\nnonce\t\ncrc32\td0e0396a\noriginal_size\t5\npayload_size\t99\n\nalpha",
    )
    .unwrap();

    let error = repository
        .reader()
        .restore(&snapshot.id, &restore)
        .unwrap_err();

    assert!(error.to_string().contains("object payload size mismatch"));
}

#[test]
fn object_original_size_mismatch_returns_error() {
    let root = TestDir::new("repo_object_bad_original_size");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let object_id = first_file_object_id(&repository, &snapshot.id);
    fs::write(
        repository_path.join("objects").join(object_id.as_str()),
        "backup-tool object v1\ncompression\tnone\nencryption\tnone\nkey_id\t\nnonce\t\ncrc32\td0e0396a\noriginal_size\t99\npayload_size\t5\n\nalpha",
    )
    .unwrap();

    let error = repository
        .reader()
        .restore(&snapshot.id, &restore)
        .unwrap_err();

    assert!(error.to_string().contains("object original size mismatch"));
}

#[test]
fn object_crc32_mismatch_returns_error() {
    let root = TestDir::new("repo_object_bad_crc32");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let object_id = first_file_object_id(&repository, &snapshot.id);
    fs::write(
        repository_path.join("objects").join(object_id.as_str()),
        "backup-tool object v1\ncompression\tnone\nencryption\tnone\nkey_id\t\nnonce\t\ncrc32\td0e0396a\noriginal_size\t5\npayload_size\t5\n\nbravo",
    )
    .unwrap();

    let error = repository
        .reader()
        .restore(&snapshot.id, &restore)
        .unwrap_err();

    assert!(error.to_string().contains("object CRC32 mismatch"));
}

#[test]
fn export_rejects_non_repository_directory() {
    let root = TestDir::new("repo_archive_export_reject");
    let repository_path = root.path.join("repository");
    let repository = Repository::init(&repository_path).unwrap();
    fs::remove_file(repository_path.join("repo.meta")).unwrap();

    let error = repository
        .export_archive(root.path.join("out.tar"), ArchiveAlgorithm::Tar)
        .unwrap_err();

    assert!(matches!(
        error,
        backup_core::BackupError::InvalidRepository(_)
    ));
}

#[test]
fn import_rejects_non_empty_destination() {
    let root = TestDir::new("repo_archive_import_reject");
    let repository_path = root.path.join("repository");
    let archive = root.path.join("repository.tar");
    let destination = root.path.join("destination");
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "alpha").unwrap();
    let repository = Repository::init(&repository_path).unwrap();
    repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    repository
        .export_archive(&archive, ArchiveAlgorithm::Tar)
        .unwrap();
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("existing.txt"), "exists").unwrap();

    let error =
        Repository::import_archive(&archive, &destination, ArchiveAlgorithm::Tar).unwrap_err();

    assert!(matches!(
        error,
        backup_core::BackupError::InvalidRepository(_)
    ));
}

fn set_modified_time(path: &std::path::Path, modified: SystemTime) {
    let times = fs::FileTimes::new().set_modified(modified);
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(times)
        .unwrap();
}

fn set_readonly(path: &std::path::Path, readonly: bool) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(readonly);
    fs::set_permissions(path, permissions).unwrap();
}

fn duplicate_name_snapshot(name: &str) -> (TestDir, Repository, backup_core::Snapshot, PathBuf) {
    let root = TestDir::new(name);
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source_a = root.path.join("alpha");
    let source_b = root.path.join("beta");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source_a).unwrap();
    fs::create_dir_all(&source_b).unwrap();
    fs::write(source_a.join("same.txt"), "alpha").unwrap();
    fs::write(source_b.join("same.txt"), "beta").unwrap();

    let snapshot = repository
        .writer()
        .backup_many([&source_a, &source_b], &BackupFilter::default())
        .unwrap();
    (root, repository, snapshot, restore)
}

fn duplicate_source_root_snapshot(
    name: &str,
) -> (TestDir, Repository, backup_core::Snapshot, PathBuf) {
    let root = TestDir::new(name);
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source_a = root.path.join("alpha").join("data");
    let source_b = root.path.join("beta").join("data");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source_a).unwrap();
    fs::create_dir_all(&source_b).unwrap();
    fs::write(source_a.join("a.txt"), "alpha").unwrap();
    fs::write(source_b.join("b.txt"), "beta").unwrap();

    let snapshot = repository
        .writer()
        .backup_many([&source_a, &source_b], &BackupFilter::default())
        .unwrap();
    (root, repository, snapshot, restore)
}

fn collect_file_names(root: &std::path::Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_file_names_into(root, root, &mut files);
    files
}

fn object_header_text(path: &std::path::Path) -> String {
    let bytes = fs::read(path).unwrap();
    let end = bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .unwrap();
    String::from_utf8(bytes[..end].to_vec()).unwrap()
}

fn first_file_object_id(
    repository: &Repository,
    snapshot_id: &backup_core::SnapshotId,
) -> backup_core::ObjectId {
    repository
        .reader()
        .read_snapshot(snapshot_id)
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.kind == FileKind::File)
        .and_then(|entry| entry.object_id.as_ref())
        .unwrap()
        .clone()
}

fn collect_file_names_into(
    root: &std::path::Path,
    current: &std::path::Path,
    files: &mut Vec<String>,
) {
    for entry in fs::read_dir(current).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_file_names_into(root, &path, files);
        } else {
            files.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn backs_up_and_restores_symlink_and_fifo_nodes() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::FileTypeExt;

    let root = TestDir::new("repo_special_nodes");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("target.txt"), "alpha").unwrap();

    std::os::unix::fs::symlink("target.txt", source.join("link.txt")).unwrap();

    let fifo_path = source.join("pipe");
    let c_path = CString::new(fifo_path.as_os_str().as_bytes()).unwrap();
    let result = unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) };
    assert_eq!(
        result,
        0,
        "mkfifo failed: {}",
        std::io::Error::last_os_error()
    );

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let snapshot_file = repository.reader().read_snapshot(&snapshot.id).unwrap();

    let symlink_entry = snapshot_file
        .entries
        .iter()
        .find(|entry| entry.relative_path == PathBuf::from("link.txt"))
        .unwrap();
    assert_eq!(symlink_entry.kind, FileKind::Symlink);
    assert_eq!(
        symlink_entry.link_target.as_deref(),
        Some(Path::new("target.txt"))
    );
    assert!(symlink_entry.object_id.is_none());

    let fifo_entry = snapshot_file
        .entries
        .iter()
        .find(|entry| entry.relative_path == PathBuf::from("pipe"))
        .unwrap();
    assert_eq!(fifo_entry.kind, FileKind::Fifo);
    assert!(fifo_entry.object_id.is_none());

    repository
        .reader()
        .restore(&snapshot.id, &restore, RestoreOptions::default())
        .unwrap();

    assert_eq!(
        fs::read_link(restore.join("link.txt")).unwrap(),
        PathBuf::from("target.txt")
    );
    assert!(fs::symlink_metadata(restore.join("pipe"))
        .unwrap()
        .file_type()
        .is_fifo());
}

#[cfg(unix)]
#[test]
fn backs_up_and_restores_hard_link_relationships() {
    use std::os::unix::fs::MetadataExt;

    let root = TestDir::new("repo_hard_links");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    let restore = root.path.join("restore");
    let regular_dir = source.join("regular");
    let hardlinks_dir = source.join("hardlinks");
    fs::create_dir_all(&regular_dir).unwrap();
    fs::create_dir_all(&hardlinks_dir).unwrap();
    let original = regular_dir.join("file.txt");
    let linked = hardlinks_dir.join("file-hardlink.txt");
    fs::write(&original, "shared inode data").unwrap();
    fs::hard_link(&original, &linked).unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup(&source, &BackupFilter::default())
        .unwrap();
    let snapshot_file = repository.reader().read_snapshot(&snapshot.id).unwrap();
    let linked_entries = snapshot_file
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == FileKind::File
                && (entry.relative_path == PathBuf::from("regular").join("file.txt")
                    || entry.relative_path == PathBuf::from("hardlinks").join("file-hardlink.txt"))
        })
        .collect::<Vec<_>>();
    assert_eq!(linked_entries.len(), 2);
    assert_eq!(
        linked_entries
            .iter()
            .filter(|entry| entry.hard_link_target.is_some())
            .count(),
        1
    );

    repository.reader().restore(&snapshot.id, &restore).unwrap();

    let restored_original = restore.join("regular").join("file.txt");
    let restored_linked = restore.join("hardlinks").join("file-hardlink.txt");
    assert_eq!(
        fs::read_to_string(&restored_original).unwrap(),
        "shared inode data"
    );
    assert_eq!(
        fs::read_to_string(&restored_linked).unwrap(),
        "shared inode data"
    );
    let original_metadata = fs::metadata(&restored_original).unwrap();
    let linked_metadata = fs::metadata(&restored_linked).unwrap();
    assert_eq!(original_metadata.dev(), linked_metadata.dev());
    assert_eq!(original_metadata.ino(), linked_metadata.ino());
    assert_eq!(original_metadata.nlink(), 2);
    assert_eq!(linked_metadata.nlink(), 2);
}

#[cfg(unix)]
#[test]
fn path_regex_filters_symlink_and_fifo_nodes() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let root = TestDir::new("repo_special_node_filter");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("target.txt"), "alpha").unwrap();
    std::os::unix::fs::symlink("target.txt", source.join("link.txt")).unwrap();

    let fifo_path = source.join("pipe");
    let c_path = CString::new(fifo_path.as_os_str().as_bytes()).unwrap();
    let result = unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) };
    assert_eq!(
        result,
        0,
        "mkfifo failed: {}",
        std::io::Error::last_os_error()
    );

    let snapshot = repository
        .writer()
        .backup(
            &source,
            &BackupFilter {
                path_regex: Some(r"^(link\.txt|pipe)$".to_string()),
                ..BackupFilter::default()
            },
        )
        .unwrap();
    let snapshot_file = repository.reader().read_snapshot(&snapshot.id).unwrap();

    assert!(snapshot_file
        .entries
        .iter()
        .any(|entry| entry.relative_path == PathBuf::from("link.txt")
            && entry.kind == FileKind::Symlink));
    assert!(snapshot_file
        .entries
        .iter()
        .any(|entry| entry.relative_path == PathBuf::from("pipe") && entry.kind == FileKind::Fifo));
    assert!(!snapshot_file
        .entries
        .iter()
        .any(|entry| entry.relative_path == PathBuf::from("target.txt")));
}
