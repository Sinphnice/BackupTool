use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod filesystem;
pub mod repository;

pub use filesystem::{
    AutoFileSystemProvider, BasicFileSystemProvider, FileEntry, FileSystemProvider,
    FileSystemWriter, FileType, FlattenConflictStrategy, Metadata, PlatformMetadata,
    PosixFileSystemProvider, PosixMetadata, ProviderKind, RestoreOptions, RestorePathStrategy,
    RestoreReport, RestoreStrategy, RestoreWarning, WindowsFileSystemProvider, WindowsMetadata,
};
pub use repository::{
    ArchiveAlgorithm, ArchiveResult, ContentHasher, FileKind, Manifest, ManifestEntry, ObjectId,
    ObjectStore, Repository, RepositoryReader, RepositoryWriter, Snapshot, SnapshotId,
    SnapshotInfo, SourceInfo,
};

#[derive(Debug)]
pub enum BackupError {
    EmptyPath(&'static str),
    SourceDoesNotExist(PathBuf),
    SourceIsNotDirectory(PathBuf),
    Io(std::io::Error),
    InvalidModifiedTime(PathBuf),
    InvalidRepository(String),
    InvalidManifest(String),
    SnapshotDoesNotExist(String),
    EmptySources,
    PathConflict(PathBuf),
    SkipFile(PathBuf),
    InvalidArchive(String),
    UnsupportedFileType { path: PathBuf, file_type: String },
    MetadataRestore { path: PathBuf, message: String },
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
            Self::InvalidRepository(message) => write!(formatter, "invalid repository: {message}"),
            Self::InvalidManifest(message) => write!(formatter, "invalid manifest: {message}"),
            Self::SnapshotDoesNotExist(snapshot_id) => {
                write!(formatter, "snapshot does not exist: {snapshot_id}")
            }
            Self::EmptySources => write!(formatter, "at least one source path is required"),
            Self::PathConflict(path) => {
                write!(formatter, "restore path conflict: {}", path.display())
            }
            Self::SkipFile(path) => write!(formatter, "skip file: {}", path.display()),
            Self::InvalidArchive(message) => write!(formatter, "invalid archive: {message}"),
            Self::UnsupportedFileType { path, file_type } => {
                write!(
                    formatter,
                    "unsupported file type during strict restore: {} ({file_type})",
                    path.display()
                )
            }
            Self::MetadataRestore { path, message } => {
                write!(
                    formatter,
                    "failed to restore metadata for {}: {message}",
                    path.display()
                )
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

/// repository 备份流程使用的用户筛选条件。
///
/// include 列表为空表示不过滤；exclude 规则一旦匹配，优先排除对应文件。
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
    /// 判断一个普通文件是否应该被复制到备份输出目录。
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

fn normalize_path_text(path: &Path) -> String {
    // 统一使用平台无关的分隔符，使路径筛选在 Windows 和类 Unix 平台行为一致。
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
    // 前端筛选条件使用 Unix 时间戳；当前先拒绝 Unix epoch 之前的文件时间，
    // 避免在不兼容的时间基准之间做静默比较。
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).ok(),
        Err(_) => None,
    }
}

/// 将 GUI 使用的分号分隔列表转换为核心库使用的筛选片段。
pub fn split_filter_list(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
