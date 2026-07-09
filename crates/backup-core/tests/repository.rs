use backup_core::{
    ArchiveAlgorithm, BackupFilter, BackupOptions, CompressionAlgorithm, FileKind,
    FlattenConflictStrategy, Repository, RestoreOptions, RestorePathStrategy, RestoreStrategy,
    SnapshotId,
};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn creates_repository_layout_and_snapshot_manifest() {
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
    let manifest = repository.reader().read_manifest(&snapshot.id).unwrap();

    assert!(repository_path.join("repo.meta").is_file());
    assert!(repository_path.join("snapshots").is_dir());
    assert!(repository_path.join("objects").is_dir());
    assert!(repository_path.join("indexes").is_dir());
    assert!(repository_path
        .join("snapshots")
        .join(format!("{}.manifest", snapshot.id.as_str()))
        .is_file());
    assert!(manifest
        .entries
        .iter()
        .any(|entry| entry.kind == FileKind::Directory
            && entry.relative_path == PathBuf::from("dir")));
    assert!(manifest.entries.iter().any(|entry| {
        entry.kind == FileKind::File
            && entry.relative_path == PathBuf::from("dir").join("a.txt")
            && entry.size == 5
            && entry.object_id.is_some()
    }));
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
    let manifest = repository.reader().read_manifest(&snapshot.id).unwrap();
    assert_eq!(manifest.sources.len(), 2);

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
    let manifest = repository.reader().read_manifest(&snapshot.id).unwrap();

    assert_eq!(manifest.sources.len(), 1);
    assert_eq!(snapshot.ignored_sources.len(), 2);
    assert_eq!(
        manifest
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
fn lists_snapshot_summary_from_repository_manifests() {
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
                extensions: vec!["txt".to_string()],
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
fn missing_snapshot_returns_error() {
    let root = TestDir::new("missing_snapshot");
    let repository = Repository::init(root.path.join("repository")).unwrap();
    let error = repository
        .reader()
        .read_manifest(&SnapshotId::from("missing".to_string()))
        .unwrap_err();

    assert!(error.is_snapshot_missing());
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

    let repository = Repository::init(&repository_path).unwrap();
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

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                compression_algorithm: CompressionAlgorithm::Zstd,
            },
        )
        .unwrap();
    let manifest = repository.reader().read_manifest(&snapshot.id).unwrap();
    let file_entries = manifest
        .entries
        .iter()
        .filter(|entry| entry.kind == FileKind::File)
        .collect::<Vec<_>>();
    assert_eq!(file_entries.len(), 3);
    for entry in file_entries {
        let object_id = entry.object_id.as_ref().unwrap();
        let object_path = repository_path.join("objects").join(object_id.as_str());
        assert!(object_path.exists());
        assert!(object_header_text(&object_path).contains("compression\tzstd"));
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
    let none_manifest = none_repository
        .reader()
        .read_manifest(&none_snapshot.id)
        .unwrap();
    let none_object = none_manifest
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
            },
        )
        .unwrap();
    let zstd_manifest = zstd_repository
        .reader()
        .read_manifest(&zstd_snapshot.id)
        .unwrap();
    let zstd_object = zstd_manifest
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
fn manifest_entry_does_not_store_compression_algorithm() {
    let root = TestDir::new("repo_manifest_no_compression");
    let repository_path = root.path.join("repository");
    let source = root.path.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "manifest").unwrap();

    let repository = Repository::init(&repository_path).unwrap();
    let snapshot = repository
        .writer()
        .backup_with_options(
            &source,
            &BackupFilter::default(),
            BackupOptions {
                compression_algorithm: CompressionAlgorithm::Zstd,
            },
        )
        .unwrap();
    let manifest_path = repository_path
        .join("snapshots")
        .join(format!("{}.manifest", snapshot.id.as_str()));
    let text = fs::read_to_string(&manifest_path).unwrap();

    for line in text.lines().filter(|line| line.starts_with("entry\t")) {
        assert_eq!(line.split('\t').count(), 11);
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
        "backup-tool object v1\ncompression\tnone\noriginal_size\t5\npayload_size\t99\n\nalpha",
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
        "backup-tool object v1\ncompression\tnone\noriginal_size\t99\npayload_size\t5\n\nalpha",
    )
    .unwrap();

    let error = repository
        .reader()
        .restore(&snapshot.id, &restore)
        .unwrap_err();

    assert!(error.to_string().contains("object original size mismatch"));
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
        .read_manifest(snapshot_id)
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
