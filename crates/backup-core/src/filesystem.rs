use crate::{BackupCoreResult, BackupError};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Directory,
    File,
    Symlink,
    Other,
}

impl FileType {
    pub(crate) fn as_manifest_value(self) -> &'static str {
        match self {
            Self::Directory => "dir",
            Self::File => "file",
            Self::Symlink => "symlink",
            Self::Other => "other",
        }
    }

    pub(crate) fn from_manifest_value(value: &str) -> BackupCoreResult<Self> {
        match value {
            "dir" => Ok(Self::Directory),
            "file" => Ok(Self::File),
            "symlink" => Ok(Self::Symlink),
            "other" => Ok(Self::Other),
            _ => Err(BackupError::InvalidManifest(format!(
                "unknown file type: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub size: u64,
    pub modified_unix_seconds: Option<i64>,
    pub accessed_unix_seconds: Option<i64>,
    pub created_unix_seconds: Option<i64>,
    pub readonly: bool,
    pub platform: PlatformMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformMetadata {
    Basic,
    Windows(WindowsMetadata),
    Posix(PosixMetadata),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsMetadata {
    pub file_attributes: Option<u32>,
    pub is_symlink: bool,
    pub is_reparse_point: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PosixMetadata {
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub is_symlink: bool,
    pub is_fifo: bool,
    pub is_device: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub relative_path: PathBuf,
    pub file_type: FileType,
    pub metadata: Metadata,
}

pub trait FileSystemProvider {
    fn read_entry(&self, root: &Path, path: &Path) -> BackupCoreResult<FileEntry>;
    fn read_file(&self, path: &Path) -> BackupCoreResult<Vec<u8>>;
}

pub trait FileSystemWriter {
    fn create_directory(&self, path: &Path) -> BackupCoreResult<()>;
    fn write_file(&self, path: &Path, bytes: &[u8]) -> BackupCoreResult<()>;
    fn restore_metadata(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>>;
    fn handle_unsupported_entry(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreStrategy {
    BestEffort,
    Strict,
    DataOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestorePathStrategy {
    PreserveFullPath,
    PreserveRelativePath,
    Flatten,
}

impl Default for RestorePathStrategy {
    fn default() -> Self {
        Self::PreserveRelativePath
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlattenConflictStrategy {
    Error,
    Skip,
    Overwrite,
    Rename,
}

impl Default for FlattenConflictStrategy {
    fn default() -> Self {
        Self::Rename
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreOptions {
    pub strategy: RestoreStrategy,
    pub path_strategy: RestorePathStrategy,
    pub flatten_conflict_strategy: FlattenConflictStrategy,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            strategy: RestoreStrategy::BestEffort,
            path_strategy: RestorePathStrategy::PreserveRelativePath,
            flatten_conflict_strategy: FlattenConflictStrategy::Rename,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreWarning {
    pub relative_path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub warnings: Vec<RestoreWarning>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BasicFileSystemProvider;

impl FileSystemProvider for BasicFileSystemProvider {
    fn read_entry(&self, root: &Path, path: &Path) -> BackupCoreResult<FileEntry> {
        read_entry_with_platform(root, path, PlatformKind::Basic)
    }

    fn read_file(&self, path: &Path) -> BackupCoreResult<Vec<u8>> {
        Ok(fs::read(path)?)
    }
}

impl FileSystemWriter for BasicFileSystemProvider {
    fn create_directory(&self, path: &Path) -> BackupCoreResult<()> {
        fs::create_dir_all(path)?;
        Ok(())
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> BackupCoreResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
        Ok(())
    }

    fn restore_metadata(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>> {
        restore_basic_metadata(path, entry, strategy)
    }

    fn handle_unsupported_entry(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>> {
        handle_unsupported_entry(path, entry, strategy)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Basic,
    Windows,
    Posix,
}

#[derive(Debug, Clone, Copy)]
pub struct AutoFileSystemProvider {
    kind: ProviderKind,
}

impl AutoFileSystemProvider {
    pub fn for_path(path: &Path) -> Self {
        Self {
            kind: ProviderKind::from_path(path),
        }
    }

    pub fn kind(&self) -> ProviderKind {
        self.kind
    }
}

impl ProviderKind {
    pub fn from_path(path: &Path) -> Self {
        classify_path(path)
    }
}

impl FileSystemProvider for AutoFileSystemProvider {
    fn read_entry(&self, root: &Path, path: &Path) -> BackupCoreResult<FileEntry> {
        match self.kind {
            ProviderKind::Basic => BasicFileSystemProvider.read_entry(root, path),
            ProviderKind::Windows => WindowsFileSystemProvider.read_entry(root, path),
            ProviderKind::Posix => PosixFileSystemProvider.read_entry(root, path),
        }
    }

    fn read_file(&self, path: &Path) -> BackupCoreResult<Vec<u8>> {
        match self.kind {
            ProviderKind::Basic => BasicFileSystemProvider.read_file(path),
            ProviderKind::Windows => WindowsFileSystemProvider.read_file(path),
            ProviderKind::Posix => PosixFileSystemProvider.read_file(path),
        }
    }
}

impl FileSystemWriter for AutoFileSystemProvider {
    fn create_directory(&self, path: &Path) -> BackupCoreResult<()> {
        match self.kind {
            ProviderKind::Basic => BasicFileSystemProvider.create_directory(path),
            ProviderKind::Windows => WindowsFileSystemProvider.create_directory(path),
            ProviderKind::Posix => PosixFileSystemProvider.create_directory(path),
        }
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> BackupCoreResult<()> {
        match self.kind {
            ProviderKind::Basic => BasicFileSystemProvider.write_file(path, bytes),
            ProviderKind::Windows => WindowsFileSystemProvider.write_file(path, bytes),
            ProviderKind::Posix => PosixFileSystemProvider.write_file(path, bytes),
        }
    }

    fn restore_metadata(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>> {
        match self.kind {
            ProviderKind::Basic => BasicFileSystemProvider.restore_metadata(path, entry, strategy),
            ProviderKind::Windows => {
                WindowsFileSystemProvider.restore_metadata(path, entry, strategy)
            }
            ProviderKind::Posix => PosixFileSystemProvider.restore_metadata(path, entry, strategy),
        }
    }

    fn handle_unsupported_entry(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>> {
        match self.kind {
            ProviderKind::Basic => {
                BasicFileSystemProvider.handle_unsupported_entry(path, entry, strategy)
            }
            ProviderKind::Windows => {
                WindowsFileSystemProvider.handle_unsupported_entry(path, entry, strategy)
            }
            ProviderKind::Posix => {
                PosixFileSystemProvider.handle_unsupported_entry(path, entry, strategy)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsFileSystemProvider;

impl FileSystemProvider for WindowsFileSystemProvider {
    fn read_entry(&self, root: &Path, path: &Path) -> BackupCoreResult<FileEntry> {
        read_entry_with_platform(root, path, PlatformKind::Windows)
    }

    fn read_file(&self, path: &Path) -> BackupCoreResult<Vec<u8>> {
        Ok(fs::read(path)?)
    }
}

impl FileSystemWriter for WindowsFileSystemProvider {
    fn create_directory(&self, path: &Path) -> BackupCoreResult<()> {
        BasicFileSystemProvider.create_directory(path)
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> BackupCoreResult<()> {
        BasicFileSystemProvider.write_file(path, bytes)
    }

    fn restore_metadata(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>> {
        restore_basic_metadata(path, entry, strategy)
    }

    fn handle_unsupported_entry(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>> {
        handle_unsupported_entry(path, entry, strategy)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PosixFileSystemProvider;

impl FileSystemProvider for PosixFileSystemProvider {
    fn read_entry(&self, root: &Path, path: &Path) -> BackupCoreResult<FileEntry> {
        read_entry_with_platform(root, path, PlatformKind::Posix)
    }

    fn read_file(&self, path: &Path) -> BackupCoreResult<Vec<u8>> {
        Ok(fs::read(path)?)
    }
}

impl FileSystemWriter for PosixFileSystemProvider {
    fn create_directory(&self, path: &Path) -> BackupCoreResult<()> {
        BasicFileSystemProvider.create_directory(path)
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> BackupCoreResult<()> {
        BasicFileSystemProvider.write_file(path, bytes)
    }

    fn restore_metadata(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>> {
        restore_basic_metadata(path, entry, strategy)
    }

    fn handle_unsupported_entry(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>> {
        handle_unsupported_entry(path, entry, strategy)
    }
}

#[derive(Debug, Clone, Copy)]
enum PlatformKind {
    Basic,
    Windows,
    Posix,
}

fn read_entry_with_platform(
    root: &Path,
    path: &Path,
    platform_kind: PlatformKind,
) -> BackupCoreResult<FileEntry> {
    let metadata = fs::symlink_metadata(path)?;
    let relative_path = path
        .strip_prefix(root)
        .map_err(|_| BackupError::SourceDoesNotExist(root.to_path_buf()))?
        .to_path_buf();
    let file_type = detect_file_type(&metadata);
    let platform = match platform_kind {
        PlatformKind::Basic => PlatformMetadata::Basic,
        PlatformKind::Windows => PlatformMetadata::Windows(windows_metadata(&metadata)),
        PlatformKind::Posix => PlatformMetadata::Posix(posix_metadata(&metadata)),
    };

    Ok(FileEntry {
        relative_path,
        file_type,
        metadata: Metadata {
            size: metadata.len(),
            modified_unix_seconds: time_to_unix_seconds(metadata.modified().ok()),
            accessed_unix_seconds: time_to_unix_seconds(metadata.accessed().ok()),
            created_unix_seconds: time_to_unix_seconds(metadata.created().ok()),
            readonly: metadata.permissions().readonly(),
            platform,
        },
    })
}

#[cfg(windows)]
fn classify_path(path: &Path) -> ProviderKind {
    let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
    if text.starts_with("\\\\wsl$\\") || text.starts_with("\\\\wsl.localhost\\") {
        ProviderKind::Posix
    } else if has_windows_drive_prefix(&text) || text.starts_with("\\\\") {
        ProviderKind::Windows
    } else {
        ProviderKind::Basic
    }
}

#[cfg(windows)]
fn has_windows_drive_prefix(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

#[cfg(unix)]
fn classify_path(_path: &Path) -> ProviderKind {
    ProviderKind::Posix
}

#[cfg(not(any(windows, unix)))]
fn classify_path(_path: &Path) -> ProviderKind {
    ProviderKind::Basic
}

fn detect_file_type(metadata: &fs::Metadata) -> FileType {
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        FileType::Directory
    } else if file_type.is_file() {
        FileType::File
    } else if file_type.is_symlink() {
        FileType::Symlink
    } else {
        FileType::Other
    }
}

#[cfg(windows)]
fn windows_metadata(metadata: &fs::Metadata) -> WindowsMetadata {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let attributes = metadata.file_attributes();
    WindowsMetadata {
        file_attributes: Some(attributes),
        is_symlink: metadata.file_type().is_symlink(),
        is_reparse_point: attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
    }
}

#[cfg(not(windows))]
fn windows_metadata(metadata: &fs::Metadata) -> WindowsMetadata {
    WindowsMetadata {
        file_attributes: None,
        is_symlink: metadata.file_type().is_symlink(),
        is_reparse_point: false,
    }
}

#[cfg(unix)]
fn posix_metadata(metadata: &fs::Metadata) -> PosixMetadata {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let file_type = metadata.file_type();
    PosixMetadata {
        mode: Some(metadata.mode()),
        uid: Some(metadata.uid()),
        gid: Some(metadata.gid()),
        is_symlink: file_type.is_symlink(),
        is_fifo: file_type.is_fifo(),
        is_device: file_type.is_block_device() || file_type.is_char_device(),
    }
}

#[cfg(not(unix))]
fn posix_metadata(metadata: &fs::Metadata) -> PosixMetadata {
    PosixMetadata {
        mode: None,
        uid: None,
        gid: None,
        is_symlink: metadata.file_type().is_symlink(),
        is_fifo: false,
        is_device: false,
    }
}

fn restore_basic_metadata(
    path: &Path,
    entry: &FileEntry,
    strategy: RestoreStrategy,
) -> BackupCoreResult<Vec<RestoreWarning>> {
    if strategy == RestoreStrategy::DataOnly {
        return Ok(Vec::new());
    }

    let mut warnings = Vec::new();
    if entry.file_type == FileType::File {
        if let Some(modified) = entry.metadata.modified_unix_seconds {
            if let Err(error) = set_modified_time(path, modified) {
                handle_metadata_issue(
                    strategy,
                    entry,
                    format!("failed to restore modified time: {error}"),
                    &mut warnings,
                )?;
            }
        }

        if let Err(error) = set_readonly(path, entry.metadata.readonly) {
            handle_metadata_issue(
                strategy,
                entry,
                format!("failed to restore readonly attribute: {error}"),
                &mut warnings,
            )?;
        }
    }

    Ok(warnings)
}

fn handle_unsupported_entry(
    _path: &Path,
    entry: &FileEntry,
    strategy: RestoreStrategy,
) -> BackupCoreResult<Vec<RestoreWarning>> {
    match strategy {
        RestoreStrategy::DataOnly => Ok(Vec::new()),
        RestoreStrategy::BestEffort => Ok(vec![RestoreWarning {
            relative_path: entry.relative_path.clone(),
            message: format!("unsupported file type: {:?}", entry.file_type),
        }]),
        RestoreStrategy::Strict => Err(BackupError::UnsupportedFileType {
            path: entry.relative_path.clone(),
            file_type: format!("{:?}", entry.file_type),
        }),
    }
}

fn handle_metadata_issue(
    strategy: RestoreStrategy,
    entry: &FileEntry,
    message: String,
    warnings: &mut Vec<RestoreWarning>,
) -> BackupCoreResult<()> {
    match strategy {
        RestoreStrategy::BestEffort => {
            warnings.push(RestoreWarning {
                relative_path: entry.relative_path.clone(),
                message,
            });
            Ok(())
        }
        RestoreStrategy::Strict => Err(BackupError::MetadataRestore {
            path: entry.relative_path.clone(),
            message,
        }),
        RestoreStrategy::DataOnly => Ok(()),
    }
}

fn set_modified_time(path: &Path, modified_unix_seconds: i64) -> std::io::Result<()> {
    let modified = unix_seconds_to_system_time(modified_unix_seconds);
    let times = fs::FileTimes::new().set_modified(modified);
    fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_times(times)
}

fn set_readonly(path: &Path, readonly: bool) -> std::io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(readonly);
    fs::set_permissions(path, permissions)
}

fn time_to_unix_seconds(time: Option<SystemTime>) -> Option<i64> {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
}

fn unix_seconds_to_system_time(value: i64) -> SystemTime {
    if value >= 0 {
        UNIX_EPOCH + Duration::from_secs(value as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(value.unsigned_abs())
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderKind, RestoreOptions, RestoreStrategy};
    use std::path::Path;

    #[test]
    fn restore_options_default_to_best_effort() {
        assert_eq!(
            RestoreOptions::default().strategy,
            RestoreStrategy::BestEffort
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_select_windows_provider() {
        assert_eq!(
            ProviderKind::from_path(Path::new(r"C:\Users\l\data")),
            ProviderKind::Windows
        );
        assert_eq!(
            ProviderKind::from_path(Path::new(r"\\server\share\data")),
            ProviderKind::Windows
        );
    }

    #[cfg(windows)]
    #[test]
    fn wsl_unc_paths_select_posix_provider() {
        assert_eq!(
            ProviderKind::from_path(Path::new(r"\\wsl.localhost\Ubuntu\home\l\data")),
            ProviderKind::Posix
        );
        assert_eq!(
            ProviderKind::from_path(Path::new(r"\\wsl$\Ubuntu\home\l\data")),
            ProviderKind::Posix
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths_select_posix_provider() {
        assert_eq!(
            ProviderKind::from_path(Path::new("/tmp/data")),
            ProviderKind::Posix
        );
    }
}
