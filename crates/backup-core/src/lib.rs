use regex::Regex;
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
    pub owner: Option<String>,
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
    pub fn allows(
        &self,
        relative_path: &Path,
        path: &Path,
        metadata: &fs::Metadata,
    ) -> BackupCoreResult<bool> {
        let path_text = normalize_path_text(relative_path);
        if let Some(regex) = self.compiled_path_regex()? {
            if !regex.is_match(&path_text) {
                return Ok(false);
            }
        }

        if let Some(owner_filter) = normalized_filter_text(self.owner.as_deref()) {
            let Some(owner) = file_owner_text(path, metadata)? else {
                return Ok(false);
            };
            if !owner_matches(&owner, owner_filter) {
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
        Regex::new(pattern)
            .map(Some)
            .map_err(|error| BackupError::InvalidRepository(format!("invalid path regex: {error}")))
    }
}

fn normalize_path_text(path: &Path) -> String {
    // 统一使用平台无关的分隔符，使路径筛选在 Windows 和类 Unix 平台行为一致。
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalized_filter_text(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn owner_matches(owner: &str, filter: &str) -> bool {
    let owner = owner.trim();
    let filter = filter.trim();
    if owner.eq_ignore_ascii_case(filter) {
        return true;
    }
    owner
        .rsplit(['\\', '/'])
        .next()
        .is_some_and(|account| account.eq_ignore_ascii_case(filter))
}

#[cfg(windows)]
fn file_owner_text(path: &Path, _metadata: &fs::Metadata) -> BackupCoreResult<Option<String>> {
    windows_file_owner_text(path)
}

#[cfg(unix)]
fn file_owner_text(_path: &Path, metadata: &fs::Metadata) -> BackupCoreResult<Option<String>> {
    use std::os::unix::fs::MetadataExt;
    Ok(Some(metadata.uid().to_string()))
}

#[cfg(not(any(windows, unix)))]
fn file_owner_text(_path: &Path, _metadata: &fs::Metadata) -> BackupCoreResult<Option<String>> {
    Ok(None)
}

#[cfg(windows)]
fn windows_file_owner_text(path: &Path) -> BackupCoreResult<Option<String>> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID};

    let mut path_wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut owner_sid: PSID = null_mut();
    let mut security_descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let result = unsafe {
        GetNamedSecurityInfoW(
            path_wide.as_mut_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner_sid,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut security_descriptor,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(BackupError::InvalidRepository(format!(
            "failed to read file owner for {}: Windows error {result}",
            path.display()
        )));
    }

    let owner = lookup_windows_account(owner_sid);
    if !security_descriptor.is_null() {
        unsafe {
            let _ = LocalFree(security_descriptor.cast());
        }
    }
    owner
}

#[cfg(windows)]
fn lookup_windows_account(
    sid: windows_sys::Win32::Security::PSID,
) -> BackupCoreResult<Option<String>> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Security::{LookupAccountSidW, SID_NAME_USE};

    if sid.is_null() {
        return Ok(None);
    }

    let mut name_len = 0;
    let mut domain_len = 0;
    let mut sid_type: SID_NAME_USE = 0;
    unsafe {
        LookupAccountSidW(
            null_mut(),
            sid,
            null_mut(),
            &mut name_len,
            null_mut(),
            &mut domain_len,
            &mut sid_type,
        );
    }
    if name_len == 0 {
        return Err(BackupError::InvalidRepository(
            "failed to read owner account name".into(),
        ));
    }

    let mut name = vec![0u16; name_len as usize];
    let mut domain = vec![0u16; domain_len as usize];
    let result = unsafe {
        LookupAccountSidW(
            null_mut(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut sid_type,
        )
    };
    if result == 0 {
        return Err(BackupError::InvalidRepository(
            "failed to read owner account name".into(),
        ));
    }
    name.truncate(name_len as usize);
    domain.truncate(domain_len as usize);
    let account = String::from_utf16_lossy(&name)
        .trim_end_matches('\0')
        .to_string();
    let domain = String::from_utf16_lossy(&domain)
        .trim_end_matches('\0')
        .to_string();
    if domain.is_empty() {
        Ok(Some(account))
    } else {
        Ok(Some(format!("{domain}\\{account}")))
    }
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
