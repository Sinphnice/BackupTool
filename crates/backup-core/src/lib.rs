use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub enum BackupError {
    EmptyPath(&'static str),
    SourceDoesNotExist(PathBuf),
    SourceIsNotDirectory(PathBuf),
    Io(std::io::Error),
    InvalidModifiedTime(PathBuf),
}

impl Display for BackupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPath(name) => write!(formatter, "{name} path must not be empty"),
            Self::SourceDoesNotExist(path) => {
                write!(formatter, "source path does not exist: {}", path.display())
            }
            Self::SourceIsNotDirectory(path) => {
                write!(
                    formatter,
                    "source path is not a directory: {}",
                    path.display()
                )
            }
            Self::Io(error) => write!(formatter, "{error}"),
            Self::InvalidModifiedTime(path) => {
                write!(formatter, "invalid modified time: {}", path.display())
            }
        }
    }
}

impl Error for BackupError {}

impl From<std::io::Error> for BackupError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub type BackupCoreResult<T> = Result<T, BackupError>;

#[derive(Debug, Clone, Default)]
pub struct BackupFilter {
    pub include_path_contains: Vec<String>,
    pub exclude_path_contains: Vec<String>,
    pub extensions: Vec<String>,
    pub include_name_contains: Vec<String>,
    pub exclude_name_contains: Vec<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_after: Option<i64>,
    pub modified_before: Option<i64>,
}

impl BackupFilter {
    pub fn allows(&self, relative_path: &Path, metadata: &fs::Metadata) -> BackupCoreResult<bool> {
        let path_text = normalize_path_text(relative_path);
        let name_text = relative_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_lowercase();

        if !contains_any_when_configured(&path_text, &self.include_path_contains) {
            return Ok(false);
        }
        if contains_any(&path_text, &self.exclude_path_contains) {
            return Ok(false);
        }
        if !contains_any_when_configured(&name_text, &self.include_name_contains) {
            return Ok(false);
        }
        if contains_any(&name_text, &self.exclude_name_contains) {
            return Ok(false);
        }
        if !self.extension_matches(relative_path) {
            return Ok(false);
        }

        let size = metadata.len();
        if self.min_size.is_some_and(|minimum| size < minimum) {
            return Ok(false);
        }
        if self.max_size.is_some_and(|maximum| size > maximum) {
            return Ok(false);
        }

        let modified = modified_unix_seconds(relative_path, metadata)?;
        if self
            .modified_after
            .is_some_and(|minimum| modified < minimum)
        {
            return Ok(false);
        }
        if self
            .modified_before
            .is_some_and(|maximum| modified > maximum)
        {
            return Ok(false);
        }

        Ok(true)
    }

    fn extension_matches(&self, relative_path: &Path) -> bool {
        if self.extensions.is_empty() {
            return true;
        }

        let extension = relative_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .trim_start_matches('.')
            .to_lowercase();

        self.extensions.iter().any(|configured| {
            configured
                .trim()
                .trim_start_matches('.')
                .eq_ignore_ascii_case(&extension)
        })
    }
}

#[derive(Debug, Clone)]
pub struct BackupConfig {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub filter: BackupFilter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupResult {
    pub file_count: u64,
    pub byte_count: u64,
}

#[derive(Debug, Clone)]
pub struct RestoreConfig {
    pub backup: PathBuf,
    pub destination: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreResult {
    pub file_count: u64,
    pub byte_count: u64,
}

pub struct BackupManager;

impl BackupManager {
    pub fn run(&self, config: &BackupConfig) -> BackupCoreResult<BackupResult> {
        copy_tree(
            &config.source,
            &config.destination,
            Some(&config.filter),
            "source",
        )
        .map(|copy| BackupResult {
            file_count: copy.file_count,
            byte_count: copy.byte_count,
        })
    }
}

pub struct RestoreManager;

impl RestoreManager {
    pub fn run(&self, config: &RestoreConfig) -> BackupCoreResult<RestoreResult> {
        copy_tree(&config.backup, &config.destination, None, "backup").map(|copy| RestoreResult {
            file_count: copy.file_count,
            byte_count: copy.byte_count,
        })
    }
}

#[derive(Default)]
struct CopyResult {
    file_count: u64,
    byte_count: u64,
}

fn copy_tree(
    source: &Path,
    destination: &Path,
    filter: Option<&BackupFilter>,
    source_name: &'static str,
) -> BackupCoreResult<CopyResult> {
    validate_source(source, source_name)?;
    if destination.as_os_str().is_empty() {
        return Err(BackupError::EmptyPath("destination"));
    }

    fs::create_dir_all(destination)?;
    let mut result = CopyResult::default();
    copy_children(source, source, destination, filter, &mut result)?;
    Ok(result)
}

fn validate_source(source: &Path, source_name: &'static str) -> BackupCoreResult<()> {
    if source.as_os_str().is_empty() {
        return Err(BackupError::EmptyPath(source_name));
    }
    if !source.exists() {
        return Err(BackupError::SourceDoesNotExist(source.to_path_buf()));
    }
    if !source.is_dir() {
        return Err(BackupError::SourceIsNotDirectory(source.to_path_buf()));
    }
    Ok(())
}

fn copy_children(
    root: &Path,
    current: &Path,
    destination: &Path,
    filter: Option<&BackupFilter>,
    result: &mut CopyResult,
) -> BackupCoreResult<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let source_path = entry.path();
        let metadata = entry.metadata()?;
        let relative = source_path
            .strip_prefix(root)
            .map_err(|_| BackupError::SourceDoesNotExist(root.to_path_buf()))?;
        let target = destination.join(relative);

        if metadata.is_dir() {
            fs::create_dir_all(&target)?;
            copy_children(root, &source_path, destination, filter, result)?;
            continue;
        }

        if !metadata.is_file() {
            continue;
        }

        if let Some(filter) = filter {
            if !filter.allows(relative, &metadata)? {
                continue;
            }
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&source_path, &target)?;
        result.file_count += 1;
        result.byte_count += metadata.len();
    }
    Ok(())
}

fn normalize_path_text(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
        .to_lowercase()
}

fn contains_any(value: &str, fragments: &[String]) -> bool {
    fragments.iter().any(|fragment| {
        let fragment = fragment.trim().to_lowercase();
        !fragment.is_empty() && value.contains(&fragment)
    })
}

fn contains_any_when_configured(value: &str, fragments: &[String]) -> bool {
    fragments.is_empty() || contains_any(value, fragments)
}

fn modified_unix_seconds(path: &Path, metadata: &fs::Metadata) -> BackupCoreResult<i64> {
    let modified = metadata
        .modified()
        .map_err(|_| BackupError::InvalidModifiedTime(path.to_path_buf()))?;
    system_time_to_unix_seconds(modified)
        .ok_or_else(|| BackupError::InvalidModifiedTime(path.to_path_buf()))
}

fn system_time_to_unix_seconds(time: SystemTime) -> Option<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).ok(),
        Err(_) => None,
    }
}

pub fn split_filter_list(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn backs_up_and_restores_regular_files() {
        let root = TestDir::new("round_trip");
        let source = root.path.join("source");
        let backup = root.path.join("backup");
        let restore = root.path.join("restore");
        fs::create_dir_all(source.join("dir")).unwrap();
        fs::write(source.join("a.txt"), "alpha").unwrap();
        fs::write(source.join("dir").join("b.txt"), "beta").unwrap();
        fs::write(source.join("image.png"), [0_u8, 1, 2, 3]).unwrap();

        let backup_result = BackupManager
            .run(&BackupConfig {
                source,
                destination: backup.clone(),
                filter: BackupFilter::default(),
            })
            .unwrap();
        assert_eq!(backup_result.file_count, 3);
        assert_eq!(backup_result.byte_count, 13);

        let restore_result = RestoreManager
            .run(&RestoreConfig {
                backup,
                destination: restore.clone(),
            })
            .unwrap();
        assert_eq!(restore_result.file_count, 3);
        assert_eq!(fs::read_to_string(restore.join("a.txt")).unwrap(), "alpha");
        assert_eq!(
            fs::read_to_string(restore.join("dir").join("b.txt")).unwrap(),
            "beta"
        );
        assert_eq!(fs::read(restore.join("image.png")).unwrap(), [0, 1, 2, 3]);
    }

    #[test]
    fn backs_up_empty_directory() {
        let root = TestDir::new("empty");
        let source = root.path.join("source");
        let backup = root.path.join("backup");
        fs::create_dir_all(&source).unwrap();

        let result = BackupManager
            .run(&BackupConfig {
                source,
                destination: backup.clone(),
                filter: BackupFilter::default(),
            })
            .unwrap();

        assert_eq!(result.file_count, 0);
        assert!(backup.exists());
    }

    #[test]
    fn supports_unicode_and_space_file_names() {
        let root = TestDir::new("unicode");
        let source = root.path.join("source");
        let backup = root.path.join("backup");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("含 空格.txt"), "text").unwrap();

        BackupManager
            .run(&BackupConfig {
                source,
                destination: backup.clone(),
                filter: BackupFilter::default(),
            })
            .unwrap();

        assert_eq!(
            fs::read_to_string(backup.join("含 空格.txt")).unwrap(),
            "text"
        );
    }

    #[test]
    fn applies_path_name_extension_and_size_filters() {
        let root = TestDir::new("filters");
        let source = root.path.join("source");
        let backup = root.path.join("backup");
        fs::create_dir_all(source.join("docs")).unwrap();
        fs::create_dir_all(source.join("tmp")).unwrap();
        fs::write(source.join("docs").join("keep-report.txt"), "12345").unwrap();
        fs::write(source.join("docs").join("skip.bin"), "12345").unwrap();
        fs::write(source.join("tmp").join("keep-report.txt"), "12345").unwrap();
        fs::write(source.join("docs").join("keep-small.txt"), "1").unwrap();

        let result = BackupManager
            .run(&BackupConfig {
                source,
                destination: backup.clone(),
                filter: BackupFilter {
                    include_path_contains: vec!["docs".to_string()],
                    exclude_path_contains: vec!["tmp".to_string()],
                    extensions: vec!["txt".to_string()],
                    include_name_contains: vec!["keep".to_string()],
                    exclude_name_contains: vec!["small".to_string()],
                    min_size: Some(2),
                    max_size: Some(10),
                    modified_after: None,
                    modified_before: None,
                },
            })
            .unwrap();

        assert_eq!(result.file_count, 1);
        assert!(backup.join("docs").join("keep-report.txt").exists());
        assert!(!backup.join("docs").join("skip.bin").exists());
        assert!(!backup.join("tmp").join("keep-report.txt").exists());
        assert!(!backup.join("docs").join("keep-small.txt").exists());
    }

    #[test]
    fn applies_modified_time_filter() {
        let root = TestDir::new("time_filter");
        let source = root.path.join("source");
        let backup = root.path.join("backup");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("now.txt"), "now").unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let result = BackupManager
            .run(&BackupConfig {
                source,
                destination: backup,
                filter: BackupFilter {
                    modified_after: Some(now - 60),
                    modified_before: Some(now + 60),
                    ..BackupFilter::default()
                },
            })
            .unwrap();

        assert_eq!(result.file_count, 1);
    }

    #[test]
    fn errors_when_source_does_not_exist() {
        let root = TestDir::new("missing");
        let error = BackupManager
            .run(&BackupConfig {
                source: root.path.join("missing"),
                destination: root.path.join("backup"),
                filter: BackupFilter::default(),
            })
            .unwrap_err();

        assert!(matches!(error, BackupError::SourceDoesNotExist(_)));
    }

    #[test]
    fn errors_when_source_is_not_directory() {
        let root = TestDir::new("file_source");
        let source = root.path.join("source.txt");
        fs::write(&source, "text").unwrap();

        let error = BackupManager
            .run(&BackupConfig {
                source,
                destination: root.path.join("backup"),
                filter: BackupFilter::default(),
            })
            .unwrap_err();

        assert!(matches!(error, BackupError::SourceIsNotDirectory(_)));
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "backup_core_{name}_{}",
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

    #[test]
    fn helper_splits_semicolon_lists() {
        assert_eq!(
            split_filter_list(Some("txt; png ; ;md".to_string())),
            vec!["txt", "png", "md"]
        );
    }

    #[test]
    fn restore_creates_missing_destination_parent_tree() {
        let root = TestDir::new("restore_parent");
        let backup = root.path.join("backup");
        let destination = root.path.join("nested").join("restore");
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("a.txt"), "a").unwrap();

        RestoreManager
            .run(&RestoreConfig {
                backup,
                destination: destination.clone(),
            })
            .unwrap();

        assert_eq!(fs::read_to_string(destination.join("a.txt")).unwrap(), "a");
    }

    #[test]
    fn backup_overwrites_existing_files() {
        let root = TestDir::new("overwrite");
        let source = root.path.join("source");
        let backup = root.path.join("backup");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(source.join("a.txt"), "new").unwrap();
        let mut existing = fs::File::create(backup.join("a.txt")).unwrap();
        existing.write_all(b"old").unwrap();
        drop(existing);

        BackupManager
            .run(&BackupConfig {
                source,
                destination: backup.clone(),
                filter: BackupFilter::default(),
            })
            .unwrap();

        assert_eq!(fs::read_to_string(backup.join("a.txt")).unwrap(), "new");
    }
}
