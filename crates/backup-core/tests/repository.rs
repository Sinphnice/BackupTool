use backup_core::{BackupFilter, FileKind, Repository, SnapshotId};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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
