use backup_core::{
    BackupFilter, FileKind, FlattenConflictStrategy, Repository, RestoreOptions,
    RestorePathStrategy, RestoreStrategy, SnapshotId,
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
