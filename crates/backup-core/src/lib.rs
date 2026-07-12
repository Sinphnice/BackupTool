use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use regex::Regex;

pub mod filesystem;
pub mod repository;

pub use filesystem::{
    AutoFileSystemProvider, BasicFileSystemProvider, FileEntry, FileSystemProvider,
    FileSystemWriter, FileType, FlattenConflictStrategy, Metadata, PlatformMetadata,
    PosixFileSystemProvider, PosixMetadata, ProviderKind, RestoreOptions, RestorePathStrategy,
    RestoreReport, RestoreStrategy, RestoreWarning, WindowsFileSystemProvider, WindowsMetadata,
};
pub use repository::{
    ArchiveAlgorithm, ArchiveResult, BackupOptions, CompressionAlgorithm, ContentHasher,
    EncryptionAlgorithm, FileKind, ObjectId, ObjectStore, Repository, RepositoryMetadata,
    RepositoryReader, RepositoryWriter, Snapshot, SnapshotDeleteResult, SnapshotEntry,
    SnapshotFile, SnapshotId, SnapshotInfo, SourceInfo,
};

#[derive(Debug)]
pub enum BackupError {
    EmptyPath(&'static str),
    SourceDoesNotExist(PathBuf),
    SourceIsNotDirectory(PathBuf),
    Io(std::io::Error),
    InvalidModifiedTime(PathBuf),
    InvalidRepository(String),
    InvalidSnapshot(String),
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
            Self::InvalidSnapshot(message) => write!(formatter, "invalid snapshot: {message}"),
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
    pub path_regex: Option<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_after: Option<i64>,
    pub modified_before: Option<i64>,
}

impl BackupFilter {
    pub fn validate(&self) -> BackupCoreResult<()> {
        self.compiled_path_regex().map(|_| ())
    }

    /// 判断一个普通文件是否应该被复制到备份输出目录。
    pub fn allows(&self, relative_path: &Path, metadata: &fs::Metadata) -> BackupCoreResult<bool> {
        let path_text = normalize_path_text(relative_path);
        if let Some(regex) = self.compiled_path_regex()? {
            if !regex.is_match(&path_text) {
                return Ok(false);
            }
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

    fn compiled_path_regex(&self) -> BackupCoreResult<Option<Regex>> {
        let Some(pattern) = self
            .path_regex
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        Regex::new(pattern).map(Some).map_err(|error| {
            BackupError::InvalidRepository(format!("invalid path regex: {error}"))
        })
    }
}

fn normalize_path_text(path: &Path) -> String {
    // 统一使用平台无关的分隔符，使路径筛选在 Windows 和类 Unix 平台行为一致。
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
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
