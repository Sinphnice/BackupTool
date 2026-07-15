use crate::{BackupCoreResult, BackupError};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Directory,
    File,
    Symlink,
    Fifo,
    Device,
    Other,
}

impl FileType {
    pub(crate) fn as_snapshot_value(self) -> &'static str {
        match self {
            Self::Directory => "dir",
            Self::File => "file",
            Self::Symlink => "symlink",
            Self::Fifo => "fifo",
            Self::Device => "device",
            Self::Other => "other",
        }
    }

    pub(crate) fn from_snapshot_value(value: &str) -> BackupCoreResult<Self> {
        match value {
            "dir" => Ok(Self::Directory),
            "file" => Ok(Self::File),
            "symlink" => Ok(Self::Symlink),
            "fifo" => Ok(Self::Fifo),
            "device" => Ok(Self::Device),
            "other" => Ok(Self::Other),
            _ => Err(BackupError::InvalidSnapshot(format!(
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
    pub device_major: Option<u64>,
    pub device_minor: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub relative_path: PathBuf,
    pub file_type: FileType,
    pub metadata: Metadata,
}

pub trait FileSystemProvider {
    fn read_entry(&self, root: &Path, path: &Path) -> BackupCoreResult<FileEntry>;
    fn read_link(&self, path: &Path) -> BackupCoreResult<PathBuf>;
    fn read_file(&self, path: &Path) -> BackupCoreResult<Vec<u8>>;
}

pub trait FileSystemWriter {
    fn create_directory(&self, path: &Path) -> BackupCoreResult<()>;
    fn create_symlink(&self, path: &Path, target: &Path) -> BackupCoreResult<Vec<RestoreWarning>>;
    fn create_fifo(&self, path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>>;
    fn create_device(&self, path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>>;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreOptions {
    pub strategy: RestoreStrategy,
    pub path_strategy: RestorePathStrategy,
    pub flatten_conflict_strategy: FlattenConflictStrategy,
    pub decryption_password: Option<String>,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            strategy: RestoreStrategy::BestEffort,
            path_strategy: RestorePathStrategy::PreserveRelativePath,
            flatten_conflict_strategy: FlattenConflictStrategy::Rename,
            decryption_password: None,
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

    fn read_link(&self, path: &Path) -> BackupCoreResult<PathBuf> {
        Ok(fs::read_link(path)?)
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

    fn create_symlink(&self, path: &Path, target: &Path) -> BackupCoreResult<Vec<RestoreWarning>> {
        create_symlink(path, target)
    }

    fn create_fifo(&self, path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
        create_fifo(path, entry)
    }

    fn create_device(&self, path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
        create_device(path, entry)
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

    fn read_link(&self, path: &Path) -> BackupCoreResult<PathBuf> {
        match self.kind {
            ProviderKind::Basic => BasicFileSystemProvider.read_link(path),
            ProviderKind::Windows => WindowsFileSystemProvider.read_link(path),
            ProviderKind::Posix => PosixFileSystemProvider.read_link(path),
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

    fn create_symlink(&self, path: &Path, target: &Path) -> BackupCoreResult<Vec<RestoreWarning>> {
        match self.kind {
            ProviderKind::Basic => BasicFileSystemProvider.create_symlink(path, target),
            ProviderKind::Windows => WindowsFileSystemProvider.create_symlink(path, target),
            ProviderKind::Posix => PosixFileSystemProvider.create_symlink(path, target),
        }
    }

    fn create_fifo(&self, path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
        match self.kind {
            ProviderKind::Basic => BasicFileSystemProvider.create_fifo(path, entry),
            ProviderKind::Windows => WindowsFileSystemProvider.create_fifo(path, entry),
            ProviderKind::Posix => PosixFileSystemProvider.create_fifo(path, entry),
        }
    }

    fn create_device(&self, path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
        match self.kind {
            ProviderKind::Basic => BasicFileSystemProvider.create_device(path, entry),
            ProviderKind::Windows => WindowsFileSystemProvider.create_device(path, entry),
            ProviderKind::Posix => PosixFileSystemProvider.create_device(path, entry),
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

    fn read_link(&self, path: &Path) -> BackupCoreResult<PathBuf> {
        Ok(fs::read_link(path)?)
    }
}

impl FileSystemWriter for WindowsFileSystemProvider {
    fn create_directory(&self, path: &Path) -> BackupCoreResult<()> {
        BasicFileSystemProvider.create_directory(path)
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> BackupCoreResult<()> {
        BasicFileSystemProvider.write_file(path, bytes)
    }

    fn create_symlink(&self, path: &Path, target: &Path) -> BackupCoreResult<Vec<RestoreWarning>> {
        create_symlink(path, target)
    }

    fn create_fifo(&self, path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
        create_fifo(path, entry)
    }

    fn create_device(&self, path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
        create_device(path, entry)
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
        #[cfg(windows)]
        if wsl_unc_path(path).is_some() {
            return read_wsl_entry(root, path);
        }
        read_entry_with_platform(root, path, PlatformKind::Posix)
    }

    fn read_file(&self, path: &Path) -> BackupCoreResult<Vec<u8>> {
        Ok(fs::read(path)?)
    }

    fn read_link(&self, path: &Path) -> BackupCoreResult<PathBuf> {
        #[cfg(windows)]
        if let Some((distribution, linux_path)) = wsl_unc_path(path) {
            return read_wsl_link(&distribution, &linux_path);
        }
        Ok(fs::read_link(path)?)
    }
}

impl FileSystemWriter for PosixFileSystemProvider {
    fn create_directory(&self, path: &Path) -> BackupCoreResult<()> {
        BasicFileSystemProvider.create_directory(path)
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> BackupCoreResult<()> {
        BasicFileSystemProvider.write_file(path, bytes)
    }

    fn create_symlink(&self, path: &Path, target: &Path) -> BackupCoreResult<Vec<RestoreWarning>> {
        #[cfg(windows)]
        if let Some((distribution, linux_path)) = wsl_unc_path(path) {
            return create_wsl_symlink(&distribution, &linux_path, target);
        }
        create_symlink(path, target)
    }

    fn create_fifo(&self, path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
        #[cfg(windows)]
        if let Some((distribution, linux_path)) = wsl_unc_path(path) {
            return create_wsl_fifo(&distribution, &linux_path, entry);
        }
        create_fifo(path, entry)
    }

    fn create_device(&self, path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
        #[cfg(windows)]
        if let Some((distribution, linux_path)) = wsl_unc_path(path) {
            return create_wsl_device(&distribution, &linux_path, entry);
        }
        create_device(path, entry)
    }

    fn restore_metadata(
        &self,
        path: &Path,
        entry: &FileEntry,
        strategy: RestoreStrategy,
    ) -> BackupCoreResult<Vec<RestoreWarning>> {
        #[cfg(windows)]
        if let Some((distribution, linux_path)) = wsl_unc_path(path) {
            return restore_wsl_metadata(&distribution, &linux_path, entry, strategy);
        }
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
    if text.starts_with("\\\\wsl$\\")
        || text.starts_with("\\\\wsl.localhost\\")
        || text.starts_with("\\\\?\\unc\\wsl$\\")
        || text.starts_with("\\\\?\\unc\\wsl.localhost\\")
    {
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
    } else if is_fifo_file_type(&file_type) {
        FileType::Fifo
    } else if is_device_file_type(&file_type) {
        FileType::Device
    } else {
        FileType::Other
    }
}

#[cfg(windows)]
fn read_wsl_entry(root: &Path, path: &Path) -> BackupCoreResult<FileEntry> {
    let (distribution, linux_path) = wsl_unc_path(path)
        .ok_or_else(|| BackupError::SourceDoesNotExist(path.to_path_buf()))?;
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            &distribution,
            "--",
            "stat",
            "-c",
            "%f\t%s\t%Y\t%X\t%W\t%u\t%g\t%t\t%T",
            "--",
            &linux_path,
        ])
        .output()?;
    if !output.status.success() {
        return Err(BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "failed to stat WSL path {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )));
    }

    let text = String::from_utf8(output.stdout).map_err(|error| {
        BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid WSL stat output: {error}"),
        ))
    })?;
    let parts = text.trim_end().split('\t').collect::<Vec<_>>();
    if parts.len() != 9 {
        return Err(BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid WSL stat field count: {}", parts.len()),
        )));
    }

    let mode = u32::from_str_radix(parts[0], 16).map_err(invalid_wsl_stat_field)?;
    let size = parts[1].parse::<u64>().map_err(invalid_wsl_stat_field)?;
    let modified_unix_seconds = parse_wsl_time(parts[2])?;
    let accessed_unix_seconds = parse_wsl_time(parts[3])?;
    let created_unix_seconds = parse_wsl_time(parts[4])?;
    let uid = parts[5].parse::<u32>().map_err(invalid_wsl_stat_field)?;
    let gid = parts[6].parse::<u32>().map_err(invalid_wsl_stat_field)?;
    let raw_device_major = parse_wsl_device_number(parts[7])?;
    let raw_device_minor = parse_wsl_device_number(parts[8])?;
    let relative_path = path
        .strip_prefix(root)
        .map_err(|_| BackupError::SourceDoesNotExist(root.to_path_buf()))?
        .to_path_buf();

    let file_kind_bits = mode & 0o170000;
    let file_type = match file_kind_bits {
        0o040000 => FileType::Directory,
        0o100000 => FileType::File,
        0o120000 => FileType::Symlink,
        0o010000 => FileType::Fifo,
        0o060000 | 0o020000 => FileType::Device,
        _ => FileType::Other,
    };
    let is_device = matches!(file_kind_bits, 0o060000 | 0o020000);
    let device_major = is_device.then_some(raw_device_major);
    let device_minor = is_device.then_some(raw_device_minor);

    Ok(FileEntry {
        relative_path,
        file_type,
        metadata: Metadata {
            size,
            modified_unix_seconds,
            accessed_unix_seconds,
            created_unix_seconds,
            readonly: mode & 0o200 == 0,
            platform: PlatformMetadata::Posix(PosixMetadata {
                mode: Some(mode),
                uid: Some(uid),
                gid: Some(gid),
                is_symlink: file_type == FileType::Symlink,
                is_fifo: file_type == FileType::Fifo,
                is_device,
                device_major,
                device_minor,
            }),
        },
    })
}

#[cfg(windows)]
fn read_wsl_link(distribution: &str, linux_path: &str) -> BackupCoreResult<PathBuf> {
    let output = Command::new("wsl.exe")
        .args(["-d", distribution, "--", "readlink", "--", linux_path])
        .output()?;
    if !output.status.success() {
        return Err(BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "failed to read WSL symlink {}: {}",
                linux_path,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )));
    }
    let target = String::from_utf8(output.stdout).map_err(|error| {
        BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid WSL readlink output: {error}"),
        ))
    })?;
    Ok(PathBuf::from(target.trim_end_matches(['\r', '\n'])))
}

#[cfg(windows)]
fn create_wsl_symlink(
    distribution: &str,
    linux_path: &str,
    target: &Path,
) -> BackupCoreResult<Vec<RestoreWarning>> {
    let target = target.to_string_lossy().replace('\\', "/");
    let output = Command::new("wsl.exe")
        .args(["-d", distribution, "--", "ln", "-s", "--", &target, linux_path])
        .output()?;
    if output.status.success() {
        Ok(Vec::new())
    } else {
        Err(BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "failed to create WSL symlink {}: {}",
                linux_path,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )))
    }
}

#[cfg(windows)]
fn create_wsl_fifo(
    distribution: &str,
    linux_path: &str,
    entry: &FileEntry,
) -> BackupCoreResult<Vec<RestoreWarning>> {
    let mode = match &entry.metadata.platform {
        PlatformMetadata::Posix(metadata) => metadata.mode.unwrap_or(0o644) & 0o777,
        _ => 0o644,
    };
    let mode_text = format!("{mode:o}");
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            distribution,
            "--",
            "mkfifo",
            "-m",
            &mode_text,
            "--",
            linux_path,
        ])
        .output()?;
    if output.status.success() {
        Ok(Vec::new())
    } else {
        Err(BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "failed to create WSL fifo {}: {}",
                linux_path,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )))
    }
}

#[cfg(windows)]
fn create_wsl_device(
    distribution: &str,
    linux_path: &str,
    entry: &FileEntry,
) -> BackupCoreResult<Vec<RestoreWarning>> {
    let metadata = match &entry.metadata.platform {
        PlatformMetadata::Posix(metadata) => metadata,
        _ => return create_device(linux_path.as_ref(), entry),
    };
    let mode = metadata.mode.unwrap_or(0);
    let kind = match mode & 0o170000 {
        0o020000 => "c",
        0o060000 => "b",
        _ => return create_device(linux_path.as_ref(), entry),
    };
    let Some(major) = metadata.device_major else {
        return create_device(linux_path.as_ref(), entry);
    };
    let Some(minor) = metadata.device_minor else {
        return create_device(linux_path.as_ref(), entry);
    };
    let permissions = format!("{:o}", mode & 0o777);
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            distribution,
            "-u",
            "root",
            "--",
            "mknod",
            "-m",
            &permissions,
            "--",
            linux_path,
            kind,
            &major.to_string(),
            &minor.to_string(),
        ])
        .output()?;
    if output.status.success() {
        if let (Some(uid), Some(gid)) = (metadata.uid, metadata.gid) {
            let owner = format!("{uid}:{gid}");
            let chown = Command::new("wsl.exe")
                .args([
                    "-d",
                    distribution,
                    "-u",
                    "root",
                    "--",
                    "chown",
                    "--",
                    &owner,
                    linux_path,
                ])
                .output()?;
            if !chown.status.success() {
                return Err(BackupError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!(
                        "failed to chown WSL device {}: {}",
                        linux_path,
                        String::from_utf8_lossy(&chown.stderr).trim()
                    ),
                )));
            }
        }
        Ok(Vec::new())
    } else {
        Err(BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "failed to create WSL device {}: {}",
                linux_path,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )))
    }
}

#[cfg(windows)]
fn restore_wsl_metadata(
    distribution: &str,
    linux_path: &str,
    entry: &FileEntry,
    strategy: RestoreStrategy,
) -> BackupCoreResult<Vec<RestoreWarning>> {
    if strategy == RestoreStrategy::DataOnly {
        return Ok(Vec::new());
    }

    let PlatformMetadata::Posix(metadata) = &entry.metadata.platform else {
        return Ok(Vec::new());
    };
    let mut warnings = Vec::new();
    let is_symlink = entry.file_type == FileType::Symlink;

    if let Some(modified) = entry.metadata.modified_unix_seconds {
        let timestamp = format!("@{modified}");
        let mut arguments = vec!["-m", "-d", timestamp.as_str(), "--", linux_path];
        if is_symlink {
            arguments.insert(0, "-h");
        }
        if let Err(error) = run_wsl_as_root(distribution, "touch", &arguments) {
            handle_metadata_issue(
                strategy,
                entry,
                format!("failed to restore modified time: {error}"),
                &mut warnings,
            )?;
        }
    }

    if !is_symlink {
        if let Some(mode) = metadata.mode {
            let mode = format!("{:o}", mode & 0o7777);
            if let Err(error) = run_wsl_as_root(distribution, "chmod", &["--", &mode, linux_path]) {
                handle_metadata_issue(
                    strategy,
                    entry,
                    format!("failed to restore POSIX permissions: {error}"),
                    &mut warnings,
                )?;
            }
        }
    }

    if let (Some(uid), Some(gid)) = (metadata.uid, metadata.gid) {
        let owner = format!("{uid}:{gid}");
        let mut arguments = vec!["--", owner.as_str(), linux_path];
        if is_symlink {
            arguments.insert(0, "-h");
        }
        if let Err(error) = run_wsl_as_root(distribution, "chown", &arguments) {
            handle_metadata_issue(
                strategy,
                entry,
                format!("failed to restore POSIX owner: {error}"),
                &mut warnings,
            )?;
        }
    }

    Ok(warnings)
}

#[cfg(windows)]
fn run_wsl_as_root(distribution: &str, program: &str, arguments: &[&str]) -> std::io::Result<()> {
    let output = Command::new("wsl.exe")
        .args(["-d", distribution, "-u", "root", "--", program])
        .args(arguments)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "WSL {program} failed for {distribution}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ))
    }
}

#[cfg(windows)]
fn wsl_unc_path(path: &Path) -> Option<(String, String)> {
    let text = path.to_string_lossy().replace('/', "\\");
    let lower = text.to_ascii_lowercase();
    let prefix = if lower.starts_with(r"\\wsl.localhost\") {
        r"\\wsl.localhost\"
    } else if lower.starts_with(r"\\wsl$\") {
        r"\\wsl$\"
    } else if lower.starts_with(r"\\?\unc\wsl.localhost\") {
        r"\\?\UNC\wsl.localhost\"
    } else if lower.starts_with(r"\\?\unc\wsl$\") {
        r"\\?\UNC\wsl$\"
    } else {
        return None;
    };
    let rest = text.strip_prefix(prefix)?;
    let mut parts = rest.split('\\').filter(|part| !part.is_empty());
    let distribution = parts.next()?.to_string();
    let linux_path = format!("/{}", parts.collect::<Vec<_>>().join("/"));
    Some((distribution, linux_path))
}

#[cfg(windows)]
pub(crate) fn wsl_file_owner_values(path: &Path) -> BackupCoreResult<Option<Vec<String>>> {
    let Some((distribution, linux_path)) = wsl_unc_path(path) else {
        return Ok(None);
    };
    let output = Command::new("wsl.exe")
        .args([
            "-d",
            &distribution,
            "--",
            "stat",
            "-c",
            "%U\t%u",
            "--",
            &linux_path,
        ])
        .output()?;
    if !output.status.success() {
        return Err(BackupError::InvalidRepository(format!(
            "failed to read WSL file owner for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let output = String::from_utf8(output.stdout).map_err(|error| {
        BackupError::InvalidRepository(format!(
            "invalid WSL file owner output for {}: {error}",
            path.display()
        ))
    })?;
    let mut parts = output.trim_end().split('\t');
    let account = parts.next().unwrap_or_default().trim();
    let uid = parts.next().unwrap_or_default().trim();
    let mut owners = Vec::new();
    if !account.is_empty() && account != "UNKNOWN" {
        owners.push(account.to_string());
    }
    if !uid.is_empty() && !owners.iter().any(|owner| owner == uid) {
        owners.push(uid.to_string());
    }
    Ok(Some(owners))
}

#[cfg(windows)]
fn parse_wsl_time(value: &str) -> BackupCoreResult<Option<i64>> {
    let parsed = value.parse::<i64>().map_err(invalid_wsl_stat_field)?;
    if parsed < 0 {
        Ok(None)
    } else {
        Ok(Some(parsed))
    }
}

#[cfg(windows)]
fn parse_wsl_device_number(value: &str) -> BackupCoreResult<u64> {
    if value.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(value, 16)
        .map_err(invalid_wsl_stat_field)
}

#[cfg(windows)]
fn invalid_wsl_stat_field(error: impl std::error::Error) -> BackupError {
    BackupError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("invalid WSL stat field: {error}"),
    ))
}

#[cfg(unix)]
fn is_fifo_file_type(file_type: &fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    file_type.is_fifo()
}

#[cfg(not(unix))]
fn is_fifo_file_type(_file_type: &fs::FileType) -> bool {
    false
}

#[cfg(unix)]
fn is_device_file_type(file_type: &fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    file_type.is_block_device() || file_type.is_char_device()
}

#[cfg(not(unix))]
fn is_device_file_type(_file_type: &fs::FileType) -> bool {
    false
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
        device_major: None,
        device_minor: None,
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
        device_major: None,
        device_minor: None,
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

#[cfg(unix)]
fn create_symlink(path: &Path, target: &Path) -> BackupCoreResult<Vec<RestoreWarning>> {
    use std::os::unix::fs::symlink;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    symlink(target, path)?;
    Ok(Vec::new())
}

#[cfg(windows)]
fn create_symlink(path: &Path, target: &Path) -> BackupCoreResult<Vec<RestoreWarning>> {
    use std::os::windows::fs::{symlink_dir, symlink_file};
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if target.is_dir() {
        symlink_dir(target, path)?;
    } else {
        symlink_file(target, path)?;
    }
    Ok(Vec::new())
}

#[cfg(not(any(unix, windows)))]
fn create_symlink(path: &Path, _target: &Path) -> BackupCoreResult<Vec<RestoreWarning>> {
    Ok(vec![RestoreWarning {
        relative_path: path.to_path_buf(),
        message: "symlink restore is not supported on this platform".to_string(),
    }])
}

#[cfg(unix)]
fn create_fifo(path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        BackupError::MetadataRestore {
            path: entry.relative_path.clone(),
            message: "fifo path contains interior NUL byte".to_string(),
        }
    })?;
    let mode = match &entry.metadata.platform {
        PlatformMetadata::Posix(metadata) => metadata.mode.unwrap_or(0o644) & 0o777,
        _ => 0o644,
    };
    let result = unsafe { libc::mkfifo(c_path.as_ptr(), mode as libc::mode_t) };
    if result == 0 {
        Ok(Vec::new())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

#[cfg(not(unix))]
fn create_fifo(_path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
    Ok(vec![RestoreWarning {
        relative_path: entry.relative_path.clone(),
        message: "fifo restore is not supported on this platform".to_string(),
    }])
}

fn create_device(_path: &Path, entry: &FileEntry) -> BackupCoreResult<Vec<RestoreWarning>> {
    Ok(vec![RestoreWarning {
        relative_path: entry.relative_path.clone(),
        message: "device node restore requires platform-specific privileges".to_string(),
    }])
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
