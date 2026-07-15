use crate::filesystem::{
    AutoFileSystemProvider, FileEntry, FileSystemProvider, FileSystemWriter,
    FlattenConflictStrategy, Metadata, RestoreOptions, RestorePathStrategy, RestoreReport,
};
use crate::{BackupCoreResult, BackupError, BackupFilter};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const REPOSITORY_META: &str = "backup-tool repository v1\n";
const SNAPSHOT_HEADER: &str = "backup-tool snapshot v1";
const SNAPSHOT_TITLE_MAX_CHARS: usize = 120;
const REPOSITORY_DISPLAY_NAME_MAX_CHARS: usize = 120;
const REPOSITORY_KEY_FORMAT_VERSION: u16 = 1;
// tar 导入导出与安全解包逻辑独立维护，避免主仓库流程混入归档细节。
mod archive;
// 密码派生、仓库主密钥封装和 object payload 加解密集中在这里。
mod crypto;
// object store 负责文件内容、压缩、加密、CRC 和内容 hash。
mod object_store;
// snapshot 文件格式读写集中在独立模块，便于后续升级磁盘格式。
mod snapshot_file;

use crypto::{
    create_wrapped_master_key, parse_optional_hex, required_password, unlock_wrapped_master_key,
    validate_encryption_password, wrap_master_key, RepositoryMasterKey,
};
pub use object_store::{ContentHasher, ObjectStore, StoredObject};
use snapshot_file::{
    escape_field, normalize_snapshot_title, read_snapshot_file, unescape_field, write_snapshot_file,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// snapshot 的稳定标识，对应 `snapshots/<id>.snapshot` 文件名。
pub struct SnapshotId(String);

impl SnapshotId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for SnapshotId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// object 的稳定标识，由原始内容 SHA-256 和加密状态后缀组成。
pub struct ObjectId(String);

impl ObjectId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn encryption_algorithm(&self) -> BackupCoreResult<EncryptionAlgorithm> {
        let (content_hash, algorithm) = if let Some(hash) = self.0.strip_suffix("-plain") {
            (hash, EncryptionAlgorithm::None)
        } else if let Some(hash) = self.0.strip_suffix("-encrypted") {
            (hash, EncryptionAlgorithm::Aes256Gcm)
        } else {
            return Err(BackupError::InvalidRepository(format!(
                "invalid object id: {}",
                self.0
            )));
        };
        if content_hash.len() != 64
            || !content_hash
                .bytes()
                .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
        {
            return Err(BackupError::InvalidRepository(format!(
                "invalid object content hash: {}",
                self.0
            )));
        }
        Ok(algorithm)
    }
}

impl From<String> for ObjectId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 创建 snapshot 后返回给调用方的摘要信息。
pub struct Snapshot {
    pub id: SnapshotId,
    pub created_unix_seconds: i64,
    pub created_nanoseconds: u32,
    pub sequence: u16,
    pub title: Option<String>,
    pub ignored_sources: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 用于列表展示的 snapshot 元信息，不包含完整 entry 列表。
pub struct SnapshotInfo {
    pub id: SnapshotId,
    pub file_count: u64,
    pub byte_count: u64,
    pub created_unix_seconds: Option<i64>,
    pub created_nanoseconds: Option<u32>,
    pub sequence: Option<u16>,
    pub title: Option<String>,
    pub has_encrypted_objects: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 删除 snapshot 的结果，包含已回收 object 和清理警告。
pub struct SnapshotDeleteResult {
    pub snapshot_id: SnapshotId,
    pub deleted_object_count: u64,
    pub reclaimed_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// repository 的磁盘元数据，对应 `repo.meta`。
pub struct RepositoryMetadata {
    pub display_name: String,
    pub encryption_algorithm: EncryptionAlgorithm,
    format_version: u16,
    kdf: String,
    argon2_parameters: String,
    salt: Option<Vec<u8>>,
    wrapping_algorithm: String,
    nonce: Option<Vec<u8>>,
    wrapped_master_key: Option<Vec<u8>>,
    key_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// snapshot 中记录的一个源目录。
pub struct SourceInfo {
    pub index: usize,
    pub absolute_path: PathBuf,
    pub restore_root: PathBuf,
}

pub use crate::filesystem::FileType as FileKind;

#[derive(Debug, Clone, PartialEq, Eq)]
/// snapshot 中的一个文件系统 entry，包括路径、类型、元数据和 object 引用。
pub struct SnapshotEntry {
    pub source_index: usize,
    pub relative_path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    pub modified_unix_seconds: Option<i64>,
    pub object_id: Option<ObjectId>,
    pub hard_link_target: Option<HardLinkTarget>,
    pub link_target: Option<PathBuf>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 后续 hard link entry 指向的首个已记录文件。
pub struct HardLinkTarget {
    pub source_index: usize,
    pub relative_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// `.snapshot` 文件在内存中的完整结构。
pub struct SnapshotFile {
    pub snapshot_id: SnapshotId,
    pub created_unix_seconds: i64,
    pub created_nanoseconds: u32,
    pub sequence: u16,
    pub title: Option<String>,
    pub sources: Vec<SourceInfo>,
    pub entries: Vec<SnapshotEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// repository 归档算法。当前只实现未压缩 tar。
pub enum ArchiveAlgorithm {
    Tar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// repository 导入/导出结果。
pub struct ArchiveResult {
    pub algorithm: ArchiveAlgorithm,
    pub path: PathBuf,
    pub byte_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// object payload 压缩算法。
pub enum CompressionAlgorithm {
    None,
    Zstd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// object payload 加密算法。
pub enum EncryptionAlgorithm {
    None,
    Aes256Gcm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 添加 snapshot 时的可选行为。
pub struct BackupOptions {
    pub compression_algorithm: CompressionAlgorithm,
    pub encryption_algorithm: EncryptionAlgorithm,
    pub encryption_password: Option<String>,
    pub snapshot_title: Option<String>,
}

impl Default for BackupOptions {
    fn default() -> Self {
        Self {
            compression_algorithm: CompressionAlgorithm::None,
            encryption_algorithm: EncryptionAlgorithm::None,
            encryption_password: None,
            snapshot_title: None,
        }
    }
}

impl CompressionAlgorithm {
    fn as_object_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Zstd => "zstd",
        }
    }

    fn from_object_value(value: &str) -> BackupCoreResult<Self> {
        match value {
            "none" => Ok(Self::None),
            "zstd" => Ok(Self::Zstd),
            _ => Err(BackupError::InvalidRepository(format!(
                "invalid compression algorithm: {value}"
            ))),
        }
    }
}

impl EncryptionAlgorithm {
    fn as_object_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Aes256Gcm => "aes-256-gcm",
        }
    }

    fn from_object_value(value: &str) -> BackupCoreResult<Self> {
        match value {
            "none" => Ok(Self::None),
            "aes-256-gcm" => Ok(Self::Aes256Gcm),
            _ => Err(BackupError::InvalidRepository(format!(
                "invalid encryption algorithm: {value}"
            ))),
        }
    }

    fn object_id_suffix(self) -> &'static str {
        match self {
            Self::None => "plain",
            Self::Aes256Gcm => "encrypted",
        }
    }
}

#[derive(Debug, Clone)]
/// repository 根对象，负责打开磁盘仓库并创建 reader/writer。
pub struct Repository {
    root: PathBuf,
}

impl Repository {
    /// 初始化一个未加密 repository。
    pub fn init(root: impl Into<PathBuf>) -> BackupCoreResult<Self> {
        Self::init_with_options(root, None, EncryptionAlgorithm::None, None)
    }

    /// 初始化 repository，并可指定显示名和仓库加密能力。
    ///
    /// 加密配置只决定该仓库是否能写入 encrypted object；未加密 snapshot 仍可存在。
    pub fn init_with_options(
        root: impl Into<PathBuf>,
        display_name: Option<String>,
        encryption_algorithm: EncryptionAlgorithm,
        encryption_password: Option<String>,
    ) -> BackupCoreResult<Self> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(BackupError::EmptyPath("repository"));
        }
        validate_encryption_password(encryption_algorithm, encryption_password.as_deref())?;

        fs::create_dir_all(root.join("snapshots"))?;
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("indexes"))?;
        let display_name = normalize_repository_display_name(display_name)
            .or_else(|| {
                root.file_name()
                    .and_then(|value| value.to_str())
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| root.display().to_string());
        let metadata = RepositoryMetadata::new(
            display_name,
            encryption_algorithm,
            encryption_password.as_deref(),
        )?;
        write_repository_metadata(&root.join("repo.meta"), &metadata)?;

        Ok(Self { root })
    }

    /// 打开并校验已有 repository，不创建或修改磁盘结构。
    pub fn open(root: impl Into<PathBuf>) -> BackupCoreResult<Self> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(BackupError::EmptyPath("repository"));
        }
        if !root.join("repo.meta").is_file() {
            return Err(BackupError::InvalidRepository(format!(
                "missing repo.meta under {}",
                root.display()
            )));
        }
        read_repository_metadata(&root.join("repo.meta"))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 读取 `repo.meta` 中的仓库元数据。
    pub fn metadata(&self) -> BackupCoreResult<RepositoryMetadata> {
        read_repository_metadata(&self.root.join("repo.meta"))
    }

    /// 修改仓库显示名，只影响 `repo.meta`，不改磁盘目录名。
    pub fn set_display_name(&self, display_name: String) -> BackupCoreResult<RepositoryMetadata> {
        let mut metadata = self.metadata()?;
        metadata.display_name =
            normalize_repository_display_name(Some(display_name)).ok_or_else(|| {
                BackupError::InvalidRepository("repository display name must not be empty".into())
            })?;
        write_repository_metadata(&self.root.join("repo.meta"), &metadata)?;
        Ok(metadata)
    }

    pub fn verify_encryption_password(&self, password: Option<&str>) -> BackupCoreResult<()> {
        self.metadata()?.verify_encryption_password(password)
    }

    /// 修改仓库密码：只重新封装 repository master key，不重写任何 object。
    pub fn change_encryption_password(
        &self,
        old_password: &str,
        new_password: &str,
    ) -> BackupCoreResult<RepositoryMetadata> {
        let current = self.metadata()?;
        if current.encryption_algorithm == EncryptionAlgorithm::None {
            return Err(BackupError::InvalidRepository(
                "repository encryption is not configured".into(),
            ));
        }
        let updated = current.rewrap_master_key(old_password, new_password)?;
        let meta_path = self.root.join("repo.meta");
        let temp_path = self.root.join("repo.meta.tmp");
        // 先写临时文件并验证新密码可解锁，再替换 repo.meta，降低中途失败造成的损坏概率。
        write_repository_metadata(&temp_path, &updated).map_err(|error| {
            BackupError::InvalidRepository(format!(
                "failed to write temporary repo metadata: {error}"
            ))
        })?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temp_path)?;
        file.sync_all()?;
        drop(file);
        read_repository_metadata(&temp_path)?.verify_encryption_password(Some(new_password))?;
        match fs::rename(&temp_path, &meta_path) {
            Ok(()) => {}
            Err(_error) if meta_path.exists() => {
                let backup_path = self.root.join("repo.meta.bak");
                let _ = fs::remove_file(&backup_path);
                if fs::rename(&meta_path, &backup_path).is_ok() {
                    if let Err(error) = fs::rename(&temp_path, &meta_path) {
                        let _ = fs::rename(&backup_path, &meta_path);
                        return Err(BackupError::InvalidRepository(format!(
                            "failed to promote temporary repo metadata: {error}"
                        )));
                    }
                    let _ = fs::remove_file(&backup_path);
                } else {
                    fs::copy(&temp_path, &meta_path).map_err(|error| {
                        BackupError::InvalidRepository(format!(
                            "failed to replace repo metadata by copy: {error}"
                        ))
                    })?;
                    fs::remove_file(&temp_path).map_err(|error| {
                        BackupError::InvalidRepository(format!(
                            "failed to remove temporary repo metadata: {error}"
                        ))
                    })?;
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(updated)
    }

    fn snapshots_dir(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    fn snapshot_path(&self, snapshot_id: &SnapshotId) -> PathBuf {
        self.snapshots_dir()
            .join(format!("{}.snapshot", snapshot_id.as_str()))
    }

    pub fn object_store(&self) -> ObjectStore {
        ObjectStore {
            root: self.root.join("objects"),
        }
    }

    /// 创建写入口，用于添加和删除 snapshot。
    pub fn writer(&self) -> RepositoryWriter {
        RepositoryWriter {
            repository: self.clone(),
        }
    }

    /// 创建读入口，用于列出、读取和恢复 snapshot。
    pub fn reader(&self) -> RepositoryReader {
        RepositoryReader {
            repository: self.clone(),
        }
    }
}

impl RepositoryMetadata {
    /// 构造新的 repo.meta 内存模型；加密仓库会同时生成并封装 repository master key。
    fn new(
        display_name: String,
        encryption_algorithm: EncryptionAlgorithm,
        encryption_password: Option<&str>,
    ) -> BackupCoreResult<Self> {
        match encryption_algorithm {
            EncryptionAlgorithm::None => Ok(Self {
                display_name,
                encryption_algorithm,
                format_version: REPOSITORY_KEY_FORMAT_VERSION,
                kdf: "none".to_string(),
                argon2_parameters: "none".to_string(),
                salt: None,
                wrapping_algorithm: "none".to_string(),
                nonce: None,
                wrapped_master_key: None,
                key_id: None,
            }),
            EncryptionAlgorithm::Aes256Gcm => {
                let password = required_password(encryption_password)?;
                let wrapped = create_wrapped_master_key(password)?;
                Ok(Self {
                    display_name,
                    encryption_algorithm,
                    format_version: REPOSITORY_KEY_FORMAT_VERSION,
                    kdf: "argon2id".to_string(),
                    argon2_parameters: "default".to_string(),
                    salt: Some(wrapped.salt),
                    wrapping_algorithm: "aes-256-gcm".to_string(),
                    nonce: Some(wrapped.nonce),
                    wrapped_master_key: Some(wrapped.wrapped_master_key),
                    key_id: Some(wrapped.key_id),
                })
            }
        }
    }

    pub fn verify_encryption_password(&self, password: Option<&str>) -> BackupCoreResult<()> {
        self.unlock_master_key(password).map(|_| ())
    }

    /// 根据 repo.meta 和用户密码解出仓库主密钥；未加密仓库返回 `None`。
    fn unlock_master_key(
        &self,
        password: Option<&str>,
    ) -> BackupCoreResult<Option<RepositoryMasterKey>> {
        match self.encryption_algorithm {
            EncryptionAlgorithm::None => {
                self.validate_structure()?;
                Ok(None)
            }
            EncryptionAlgorithm::Aes256Gcm => {
                self.validate_structure()?;
                let password = required_password(password)?;
                let salt = self.salt.as_deref().ok_or_else(|| {
                    BackupError::InvalidRepository("encrypted repository missing salt".into())
                })?;
                let nonce = self.nonce.as_deref().ok_or_else(|| {
                    BackupError::InvalidRepository("encrypted repository missing nonce".into())
                })?;
                let wrapped_master_key = self.wrapped_master_key.as_deref().ok_or_else(|| {
                    BackupError::InvalidRepository(
                        "encrypted repository missing wrapped master key".into(),
                    )
                })?;
                let key_id = self.key_id.as_deref().ok_or_else(|| {
                    BackupError::InvalidRepository("encrypted repository missing key id".into())
                })?;
                Ok(Some(unlock_wrapped_master_key(
                    password,
                    salt,
                    nonce,
                    wrapped_master_key,
                    key_id,
                )?))
            }
        }
    }

    /// 用新密码重新封装同一个仓库主密钥。
    fn rewrap_master_key(&self, old_password: &str, new_password: &str) -> BackupCoreResult<Self> {
        let master_key = self.unlock_master_key(Some(old_password))?.ok_or_else(|| {
            BackupError::InvalidRepository("repository encryption is not configured".into())
        })?;
        let wrapped = wrap_master_key(&master_key, new_password)?;
        Ok(Self {
            display_name: self.display_name.clone(),
            encryption_algorithm: self.encryption_algorithm,
            format_version: REPOSITORY_KEY_FORMAT_VERSION,
            kdf: "argon2id".to_string(),
            argon2_parameters: "default".to_string(),
            salt: Some(wrapped.salt),
            wrapping_algorithm: "aes-256-gcm".to_string(),
            nonce: Some(wrapped.nonce),
            wrapped_master_key: Some(wrapped.wrapped_master_key),
            key_id: Some(wrapped.key_id),
        })
    }

    /// 校验 repo.meta 字段组合是否和声明的加密算法一致。
    fn validate_structure(&self) -> BackupCoreResult<()> {
        match self.encryption_algorithm {
            EncryptionAlgorithm::None => {
                if self.format_version != REPOSITORY_KEY_FORMAT_VERSION {
                    return Err(BackupError::InvalidRepository(format!(
                        "unsupported repository key format version: {}",
                        self.format_version
                    )));
                }
                if self.kdf != "none" {
                    return Err(BackupError::InvalidRepository(format!(
                        "unsupported repository kdf: {}",
                        self.kdf
                    )));
                }
                if self.argon2_parameters != "none" || self.wrapping_algorithm != "none" {
                    return Err(BackupError::InvalidRepository(
                        "unencrypted repository must not contain key wrapping metadata".into(),
                    ));
                }
                if self.salt.is_some()
                    || self.nonce.is_some()
                    || self.wrapped_master_key.is_some()
                    || self.key_id.is_some()
                {
                    return Err(BackupError::InvalidRepository(
                        "unencrypted repository must not contain encryption verifier fields".into(),
                    ));
                }
                Ok(())
            }
            EncryptionAlgorithm::Aes256Gcm => {
                if self.format_version != REPOSITORY_KEY_FORMAT_VERSION {
                    return Err(BackupError::InvalidRepository(format!(
                        "unsupported repository key format version: {}",
                        self.format_version
                    )));
                }
                if self.kdf != "argon2id" {
                    return Err(BackupError::InvalidRepository(format!(
                        "unsupported repository kdf: {}",
                        self.kdf
                    )));
                }
                if self.salt.as_ref().is_none_or(Vec::is_empty) {
                    return Err(BackupError::InvalidRepository(
                        "encrypted repository missing salt".into(),
                    ));
                }
                if self.argon2_parameters != "default" {
                    return Err(BackupError::InvalidRepository(format!(
                        "unsupported repository argon2 parameters: {}",
                        self.argon2_parameters
                    )));
                }
                if self.wrapping_algorithm != "aes-256-gcm" {
                    return Err(BackupError::InvalidRepository(format!(
                        "unsupported repository master key wrapping algorithm: {}",
                        self.wrapping_algorithm
                    )));
                }
                if self.nonce.as_ref().is_none_or(Vec::is_empty) {
                    return Err(BackupError::InvalidRepository(
                        "encrypted repository missing nonce".into(),
                    ));
                }
                if self.wrapped_master_key.as_ref().is_none_or(Vec::is_empty) {
                    return Err(BackupError::InvalidRepository(
                        "encrypted repository missing wrapped master key".into(),
                    ));
                }
                if self.key_id.as_ref().is_none_or(String::is_empty) {
                    return Err(BackupError::InvalidRepository(
                        "encrypted repository missing key id".into(),
                    ));
                }
                Ok(())
            }
        }
    }
}

fn normalize_repository_display_name(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_string();
    if value.is_empty() {
        return None;
    }
    Some(
        value
            .chars()
            .take(REPOSITORY_DISPLAY_NAME_MAX_CHARS)
            .collect(),
    )
}

fn read_repository_metadata(path: &Path) -> BackupCoreResult<RepositoryMetadata> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    if lines.next() != Some(REPOSITORY_META.trim_end()) {
        return Err(BackupError::InvalidRepository(
            "invalid repo.meta header".into(),
        ));
    }

    let mut display_name = None;
    let mut encryption_algorithm = None;
    let mut format_version = None;
    let mut kdf = None;
    let mut argon2_parameters = None;
    let mut salt = None;
    let mut wrapping_algorithm = None;
    let mut nonce = None;
    let mut wrapped_master_key = None;
    let mut key_id = None;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('\t') else {
            return Err(BackupError::InvalidRepository(format!(
                "invalid repo.meta line: {line}"
            )));
        };
        match key {
            "display_name" => display_name = Some(unescape_field(value)?),
            "encryption" => {
                encryption_algorithm = Some(EncryptionAlgorithm::from_object_value(value)?)
            }
            "key_format_version" => {
                format_version = Some(value.parse::<u16>().map_err(|_| {
                    BackupError::InvalidRepository(format!(
                        "invalid repository key format version: {value}"
                    ))
                })?);
            }
            "kdf" => kdf = Some(value.to_string()),
            "argon2_parameters" => argon2_parameters = Some(value.to_string()),
            "salt" => salt = parse_optional_hex(value, "repository salt")?,
            "wrapping_algorithm" => wrapping_algorithm = Some(value.to_string()),
            "nonce" => nonce = parse_optional_hex(value, "repository nonce")?,
            "wrapped_master_key" => {
                wrapped_master_key = parse_optional_hex(value, "repository wrapped master key")?
            }
            "key_id" => {
                key_id = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                }
            }
            _ => {}
        }
    }

    let display_name = display_name
        .and_then(|value| normalize_repository_display_name(Some(value)))
        .ok_or_else(|| BackupError::InvalidRepository("repo.meta missing display_name".into()))?;
    let encryption_algorithm = encryption_algorithm
        .ok_or_else(|| BackupError::InvalidRepository("repo.meta missing encryption".into()))?;
    let metadata = RepositoryMetadata {
        display_name,
        encryption_algorithm,
        format_version: format_version.ok_or_else(|| {
            BackupError::InvalidRepository(
                "repo.meta missing key_format_version; old repositories are not supported".into(),
            )
        })?,
        kdf: kdf.unwrap_or_else(|| "none".to_string()),
        argon2_parameters: argon2_parameters.unwrap_or_else(|| "none".to_string()),
        salt,
        wrapping_algorithm: wrapping_algorithm.unwrap_or_else(|| "none".to_string()),
        nonce,
        wrapped_master_key,
        key_id,
    };
    metadata.validate_structure()?;
    Ok(metadata)
}

fn write_repository_metadata(path: &Path, metadata: &RepositoryMetadata) -> BackupCoreResult<()> {
    let mut output = String::from(REPOSITORY_META);
    output.push_str("display_name\t");
    output.push_str(&escape_field(&metadata.display_name));
    output.push('\n');
    output.push_str("encryption\t");
    output.push_str(metadata.encryption_algorithm.as_object_value());
    output.push('\n');
    output.push_str("key_format_version\t");
    output.push_str(&metadata.format_version.to_string());
    output.push('\n');
    output.push_str("kdf\t");
    output.push_str(&metadata.kdf);
    output.push('\n');
    output.push_str("argon2_parameters\t");
    output.push_str(&metadata.argon2_parameters);
    output.push('\n');
    output.push_str("salt\t");
    output.push_str(&metadata.salt.as_ref().map(hex::encode).unwrap_or_default());
    output.push('\n');
    output.push_str("wrapping_algorithm\t");
    output.push_str(&metadata.wrapping_algorithm);
    output.push('\n');
    output.push_str("nonce\t");
    output.push_str(&metadata.nonce.as_ref().map(hex::encode).unwrap_or_default());
    output.push('\n');
    output.push_str("wrapped_master_key\t");
    output.push_str(
        &metadata
            .wrapped_master_key
            .as_ref()
            .map(hex::encode)
            .unwrap_or_default(),
    );
    output.push('\n');
    output.push_str("key_id\t");
    output.push_str(metadata.key_id.as_deref().unwrap_or_default());
    output.push('\n');
    fs::write(path, output)?;
    Ok(())
}

#[derive(Debug, Clone)]
/// repository 写操作入口，负责创建 snapshot 和删除 snapshot。
pub struct RepositoryWriter {
    repository: Repository,
}

#[derive(Debug, Clone)]
struct CreatedSnapshot {
    id: SnapshotId,
    unix_seconds: i64,
    nanoseconds: u32,
    sequence: u16,
}

impl RepositoryWriter {
    /// 备份单个源目录，使用默认备份选项。
    pub fn backup(
        &self,
        source: impl AsRef<Path>,
        filter: &BackupFilter,
    ) -> BackupCoreResult<Snapshot> {
        self.backup_many([source.as_ref().to_path_buf()], filter)
    }

    /// 备份单个源目录，并指定压缩、加密和标题等选项。
    pub fn backup_with_options(
        &self,
        source: impl AsRef<Path>,
        filter: &BackupFilter,
        options: BackupOptions,
    ) -> BackupCoreResult<Snapshot> {
        self.backup_many_with_options([source.as_ref().to_path_buf()], filter, options)
    }

    /// 备份多个源目录，使用默认备份选项。
    pub fn backup_many(
        &self,
        sources: impl IntoIterator<Item = impl Into<PathBuf>>,
        filter: &BackupFilter,
    ) -> BackupCoreResult<Snapshot> {
        self.backup_many_with_options(sources, filter, BackupOptions::default())
    }

    /// 备份多个源目录并生成一个 snapshot 文件。
    ///
    /// 该函数负责源路径规范化、筛选校验、object 写入和 snapshot 文件落盘。
    pub fn backup_many_with_options(
        &self,
        sources: impl IntoIterator<Item = impl Into<PathBuf>>,
        filter: &BackupFilter,
        options: BackupOptions,
    ) -> BackupCoreResult<Snapshot> {
        let raw_sources = sources.into_iter().map(Into::into).collect::<Vec<_>>();
        let normalized = normalize_sources(&raw_sources)?;
        filter.validate()?;
        let snapshot_title = normalize_snapshot_title(options.snapshot_title)?;

        let created = self.create_snapshot_id()?;
        let mut snapshot_file = SnapshotFile {
            snapshot_id: created.id.clone(),
            created_unix_seconds: created.unix_seconds,
            created_nanoseconds: created.nanoseconds,
            sequence: created.sequence,
            title: snapshot_title.clone(),
            sources: normalized
                .sources
                .iter()
                .enumerate()
                .map(|(index, source)| SourceInfo {
                    index,
                    absolute_path: source.clone(),
                    restore_root: default_restore_root(source),
                })
                .collect(),
            entries: Vec::new(),
        };
        let object_store = self.repository.object_store();
        let master_key = if options.encryption_algorithm == EncryptionAlgorithm::Aes256Gcm {
            let metadata = self.repository.metadata()?;
            if metadata.encryption_algorithm == EncryptionAlgorithm::None {
                return Err(BackupError::InvalidRepository(
                    "repository encryption is not configured".into(),
                ));
            }
            metadata.unlock_master_key(options.encryption_password.as_deref())?
        } else {
            None
        };
        // 跨同一次 snapshot 的所有源共享该表，用 device+inode 记录真实硬链接关系。
        let mut hard_link_targets = HashMap::new();

        for (source_index, source) in normalized.sources.iter().enumerate() {
            let provider = AutoFileSystemProvider::for_path(source);
            scan_into_snapshot_file(
                source,
                source,
                source_index,
                filter,
                &provider,
                &object_store,
                options.compression_algorithm,
                options.encryption_algorithm,
                master_key.as_ref(),
                &mut hard_link_targets,
                &mut snapshot_file,
            )?;
        }

        write_snapshot_file(&self.repository.snapshot_path(&created.id), &snapshot_file)?;

        Ok(Snapshot {
            id: created.id,
            created_unix_seconds: created.unix_seconds,
            created_nanoseconds: created.nanoseconds,
            sequence: created.sequence,
            title: snapshot_title,
            ignored_sources: normalized.ignored_sources,
        })
    }

    pub fn delete_snapshot(
        &self,
        snapshot_id: &SnapshotId,
    ) -> BackupCoreResult<SnapshotDeleteResult> {
        self.delete_snapshot_with_password(snapshot_id, None)
    }

    /// 删除 snapshot，并清理不再被任何其他 snapshot 引用的 object。
    ///
    /// 如果目标 snapshot 引用了 encrypted object，需要提供密码以确认调用方有权限删除。
    pub fn delete_snapshot_with_password(
        &self,
        snapshot_id: &SnapshotId,
        encryption_password: Option<&str>,
    ) -> BackupCoreResult<SnapshotDeleteResult> {
        let target_path = self.repository.snapshot_path(snapshot_id);
        if !target_path.is_file() {
            return Err(BackupError::SnapshotDoesNotExist(
                snapshot_id.as_str().to_string(),
            ));
        }

        let target = read_snapshot_file(&target_path)?;
        if target.has_encrypted_objects()? {
            self.repository
                .metadata()?
                .unlock_master_key(encryption_password)?;
        }
        let target_objects = snapshot_object_ids(&target)?;
        let mut remaining_objects = HashSet::new();
        // 先完整读取其他 snapshot 的引用，再删除目标 snapshot；如果其他 snapshot 损坏，
        // 此处会提前失败，避免误删仍被引用的 object。
        for entry in fs::read_dir(self.repository.snapshots_dir())? {
            let path = entry?.path();
            if path == target_path
                || path.extension().and_then(|value| value.to_str()) != Some("snapshot")
            {
                continue;
            }
            remaining_objects.extend(snapshot_object_ids(&read_snapshot_file(&path)?)?);
        }

        fs::remove_file(&target_path)?;

        let object_store = self.repository.object_store();
        let mut result = SnapshotDeleteResult {
            snapshot_id: snapshot_id.clone(),
            deleted_object_count: 0,
            reclaimed_bytes: 0,
            warnings: Vec::new(),
        };
        for object_id in target_objects.difference(&remaining_objects) {
            let path = object_store.path_for(object_id);
            let size = match fs::metadata(&path) {
                Ok(metadata) => metadata.len(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    result.warnings.push(format!(
                        "object was already missing: {}",
                        object_id.as_str()
                    ));
                    continue;
                }
                Err(error) => {
                    result.warnings.push(format!(
                        "failed to inspect object {}: {error}",
                        object_id.as_str()
                    ));
                    continue;
                }
            };
            match fs::remove_file(&path) {
                Ok(()) => {
                    result.deleted_object_count += 1;
                    result.reclaimed_bytes += size;
                }
                Err(error) => result.warnings.push(format!(
                    "failed to delete object {}: {error}",
                    object_id.as_str()
                )),
            }
        }

        Ok(result)
    }

    fn create_snapshot_id(&self) -> BackupCoreResult<CreatedSnapshot> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
            BackupError::InvalidRepository("system time is before unix epoch".into())
        })?;
        let unix_seconds = i64::try_from(now.as_secs()).map_err(|_| {
            BackupError::InvalidRepository("system time exceeds supported range".into())
        })?;

        for sequence in 0..1000_u16 {
            let id = SnapshotId(format!(
                "{}-{:09}-{sequence:03}",
                unix_seconds,
                now.subsec_nanos()
            ));
            if !self.repository.snapshot_path(&id).exists() {
                return Ok(CreatedSnapshot {
                    id,
                    unix_seconds,
                    nanoseconds: now.subsec_nanos(),
                    sequence,
                });
            }
        }

        Err(BackupError::InvalidRepository(
            "failed to allocate unique snapshot id".into(),
        ))
    }
}

fn snapshot_object_ids(snapshot: &SnapshotFile) -> BackupCoreResult<HashSet<ObjectId>> {
    let mut object_ids = HashSet::new();
    for entry in &snapshot.entries {
        if let Some(object_id) = &entry.object_id {
            object_id.encryption_algorithm()?;
            object_ids.insert(object_id.clone());
        }
    }
    Ok(object_ids)
}

#[derive(Debug)]
struct NormalizedSources {
    sources: Vec<PathBuf>,
    ignored_sources: Vec<PathBuf>,
}

fn normalize_sources(sources: &[PathBuf]) -> BackupCoreResult<NormalizedSources> {
    // 多源备份必须先去重和移除子路径，否则同一目录树可能被重复写入一个 snapshot。
    if sources.is_empty() {
        return Err(BackupError::EmptySources);
    }

    let mut normalized = Vec::new();
    for source in sources {
        validate_source_directory(source)?;
        normalized.push(fs::canonicalize(source).unwrap_or_else(|_| absolutize_path(source)));
    }

    normalized.sort_by_key(|path| comparable_path(path));

    let mut selected: Vec<PathBuf> = Vec::new();
    let mut ignored = Vec::new();
    let mut seen = HashSet::new();
    for source in normalized {
        let key = comparable_path(&source);
        if !seen.insert(key) {
            ignored.push(source);
            continue;
        }

        if selected.iter().any(|parent| source.starts_with(parent)) {
            ignored.push(source);
            continue;
        }

        selected.push(source);
    }

    Ok(NormalizedSources {
        sources: selected,
        ignored_sources: ignored,
    })
}

fn validate_source_directory(source: &Path) -> BackupCoreResult<()> {
    if source.as_os_str().is_empty() {
        return Err(BackupError::EmptyPath("source"));
    }
    if !source.exists() {
        return Err(BackupError::SourceDoesNotExist(source.to_path_buf()));
    }
    if !source.is_dir() {
        return Err(BackupError::SourceIsNotDirectory(source.to_path_buf()));
    }
    Ok(())
}

/// 将相对路径转为绝对路径，并规范化 `.` / `..`。
fn absolutize_path(path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    normalize_path_components(&path)
}

/// 只做路径组件层面的规范化，不要求目标一定存在。
fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// 生成用于排序和父子路径判断的比较字符串。
fn comparable_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}

#[derive(Debug, Clone)]
/// repository 读操作入口，负责列出、读取和恢复 snapshot。
pub struct RepositoryReader {
    repository: Repository,
}

impl RepositoryReader {
    /// 读取 snapshot 列表，按创建时间倒序返回。
    pub fn list_snapshots(&self) -> BackupCoreResult<Vec<SnapshotInfo>> {
        let snapshots_dir = self.repository.snapshots_dir();
        if !snapshots_dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut snapshots = Vec::new();
        for entry in fs::read_dir(snapshots_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("snapshot") {
                continue;
            }

            let snapshot_file = read_snapshot_file(&path)?;
            snapshots.push(SnapshotInfo::from_snapshot_file(&snapshot_file));
        }

        snapshots.sort_by(|left, right| {
            right
                .created_unix_seconds
                .cmp(&left.created_unix_seconds)
                .then_with(|| right.id.as_str().cmp(left.id.as_str()))
        });
        Ok(snapshots)
    }

    /// 读取一个完整 snapshot 文件。
    pub fn read_snapshot(&self, snapshot_id: &SnapshotId) -> BackupCoreResult<SnapshotFile> {
        let path = self.repository.snapshot_path(snapshot_id);
        if !path.is_file() {
            return Err(BackupError::SnapshotDoesNotExist(
                snapshot_id.as_str().to_string(),
            ));
        }
        read_snapshot_file(&path)
    }

    /// 使用默认恢复选项恢复 snapshot。
    pub fn restore(
        &self,
        snapshot_id: &SnapshotId,
        destination: impl AsRef<Path>,
    ) -> BackupCoreResult<()> {
        self.restore_with_options(snapshot_id, destination, RestoreOptions::default())
            .map(|_| ())
    }

    /// 按指定路径策略、冲突策略和解密密码恢复 snapshot。
    pub fn restore_with_options(
        &self,
        snapshot_id: &SnapshotId,
        destination: impl AsRef<Path>,
        options: RestoreOptions,
    ) -> BackupCoreResult<RestoreReport> {
        let destination = destination.as_ref();
        if destination.as_os_str().is_empty() {
            return Err(BackupError::EmptyPath("destination"));
        }

        let snapshot_file = self.read_snapshot(snapshot_id)?;
        let object_store = self.repository.object_store();
        let master_key = if snapshot_file.has_encrypted_objects()? {
            self.repository
                .metadata()?
                .unlock_master_key(options.decryption_password.as_deref())?
        } else {
            None
        };
        let writer = AutoFileSystemProvider::for_path(destination);
        let mut report = RestoreReport::default();
        let is_multi_source = snapshot_file.sources.len() > 1;
        let source_roots = resolve_source_roots(&snapshot_file, &options)?;
        fs::create_dir_all(destination)?;
        // 目录元数据延后恢复，避免先设置严格权限后导致子文件无法创建。
        let mut deferred_directory_metadata = Vec::new();
        // 记录已恢复的普通文件真实目标路径，供后续 hard link entry 复用。
        let mut restored_files: HashMap<(usize, PathBuf), PathBuf> = HashMap::new();

        for entry in &snapshot_file.entries {
            let Some(target) =
                restore_target_path(destination, &snapshot_file, &source_roots, entry, &options)?
            else {
                continue;
            };
            match entry.kind {
                FileKind::Directory => {
                    if options.path_strategy != RestorePathStrategy::Flatten {
                        writer.create_directory(&target)?;
                        let file_entry = entry.to_file_entry_at(&target);
                        deferred_directory_metadata.push((target, file_entry));
                    }
                }
                FileKind::File => {
                    let target = match resolve_file_conflict(target, &options) {
                        Ok(target) => target,
                        Err(BackupError::SkipFile(_)) => continue,
                        Err(error) => return Err(error),
                    };
                    if let Some(hard_link_target) = &entry.hard_link_target {
                        let target_key = (
                            hard_link_target.source_index,
                            hard_link_target.relative_path.clone(),
                        );
                        let original = restored_files.get(&target_key).ok_or_else(|| {
                            BackupError::InvalidSnapshot(format!(
                                "hard link target has not been restored: {}",
                                hard_link_target.relative_path.display()
                            ))
                        })?;
                        // hard link 必须链接到已经恢复出的目标路径，而不是重新写一份相同内容。
                        writer.create_hard_link(&target, original)?;
                    } else {
                        let object_id = entry.object_id.as_ref().ok_or_else(|| {
                            BackupError::InvalidSnapshot(format!(
                                "file entry missing object id: {}",
                                entry.relative_path.display()
                            ))
                        })?;
                        writer.write_file(
                            &target,
                            &object_store
                                .read_object_with_master_key(object_id, master_key.as_ref())?,
                        )?;
                    }
                    report.warnings.extend(writer.restore_metadata(
                        &target,
                        &entry.to_file_entry_at(&target),
                        options.strategy,
                    )?);
                    restored_files
                        .insert((entry.source_index, entry.relative_path.clone()), target);
                }
                FileKind::Symlink => {
                    if options.path_strategy == RestorePathStrategy::Flatten && is_multi_source {
                        continue;
                    }
                    let link_target = entry.link_target.as_ref().ok_or_else(|| {
                        BackupError::InvalidSnapshot(format!(
                            "symlink entry missing target: {}",
                            entry.relative_path.display()
                        ))
                    })?;
                    let target = match resolve_file_conflict(target, &options) {
                        Ok(target) => target,
                        Err(BackupError::SkipFile(_)) => continue,
                        Err(error) => return Err(error),
                    };
                    let file_entry = entry.to_file_entry_at(&target);
                    match writer.create_symlink(&target, link_target) {
                        Ok(warnings) => {
                            report.warnings.extend(warnings);
                            report.warnings.extend(writer.restore_metadata(
                                &target,
                                &file_entry,
                                options.strategy,
                            )?);
                        }
                        Err(error) => handle_node_restore_error(
                            &mut report,
                            &file_entry,
                            options.strategy,
                            format!("failed to restore symlink: {error}"),
                        )?,
                    }
                }
                FileKind::Fifo => {
                    if options.path_strategy == RestorePathStrategy::Flatten && is_multi_source {
                        continue;
                    }
                    let target = match resolve_file_conflict(target, &options) {
                        Ok(target) => target,
                        Err(BackupError::SkipFile(_)) => continue,
                        Err(error) => return Err(error),
                    };
                    let file_entry = entry.to_file_entry_at(&target);
                    match writer.create_fifo(&target, &file_entry) {
                        Ok(warnings) => {
                            report.warnings.extend(warnings);
                            report.warnings.extend(writer.restore_metadata(
                                &target,
                                &file_entry,
                                options.strategy,
                            )?);
                        }
                        Err(error) => handle_node_restore_error(
                            &mut report,
                            &file_entry,
                            options.strategy,
                            format!("failed to restore fifo: {error}"),
                        )?,
                    }
                }
                FileKind::Device => {
                    if options.path_strategy == RestorePathStrategy::Flatten && is_multi_source {
                        continue;
                    }
                    let target = match resolve_file_conflict(target, &options) {
                        Ok(target) => target,
                        Err(BackupError::SkipFile(_)) => continue,
                        Err(error) => return Err(error),
                    };
                    let file_entry = entry.to_file_entry_at(&target);
                    match writer.create_device(&target, &file_entry) {
                        Ok(warnings) => {
                            report.warnings.extend(warnings);
                            report.warnings.extend(writer.restore_metadata(
                                &target,
                                &file_entry,
                                options.strategy,
                            )?);
                        }
                        Err(error) => handle_node_restore_error(
                            &mut report,
                            &file_entry,
                            options.strategy,
                            format!("failed to restore device node: {error}"),
                        )?,
                    }
                }
                FileKind::Other => {
                    if options.path_strategy != RestorePathStrategy::Flatten || !is_multi_source {
                        report.warnings.extend(writer.handle_unsupported_entry(
                            &target,
                            &entry.to_file_entry_at(&target),
                            options.strategy,
                        )?);
                    }
                }
            }
        }

        for (target, entry) in deferred_directory_metadata.into_iter().rev() {
            report
                .warnings
                .extend(writer.restore_metadata(&target, &entry, options.strategy)?);
        }

        Ok(report)
    }
}

impl SnapshotInfo {
    fn from_snapshot_file(snapshot_file: &SnapshotFile) -> Self {
        let mut file_count = 0;
        let mut byte_count = 0;
        for entry in &snapshot_file.entries {
            if entry.kind == FileKind::File {
                file_count += 1;
                byte_count += entry.size;
            }
        }

        Self {
            id: snapshot_file.snapshot_id.clone(),
            file_count,
            byte_count,
            created_unix_seconds: Some(snapshot_file.created_unix_seconds),
            created_nanoseconds: Some(snapshot_file.created_nanoseconds),
            sequence: Some(snapshot_file.sequence),
            title: snapshot_file.title.clone(),
            has_encrypted_objects: snapshot_file.has_encrypted_objects().unwrap_or(false),
        }
    }
}

impl SnapshotEntry {
    fn to_file_entry_at(&self, restored_path: &Path) -> FileEntry {
        FileEntry {
            relative_path: restored_path.to_path_buf(),
            file_type: self.kind,
            metadata: self.metadata.clone(),
            hard_link_identity: None,
        }
    }
}

fn handle_node_restore_error(
    report: &mut RestoreReport,
    entry: &FileEntry,
    strategy: crate::filesystem::RestoreStrategy,
    message: String,
) -> BackupCoreResult<()> {
    match strategy {
        crate::filesystem::RestoreStrategy::DataOnly => Ok(()),
        crate::filesystem::RestoreStrategy::BestEffort => {
            report.warnings.push(crate::filesystem::RestoreWarning {
                relative_path: entry.relative_path.clone(),
                message,
            });
            Ok(())
        }
        crate::filesystem::RestoreStrategy::Strict => Err(BackupError::MetadataRestore {
            path: entry.relative_path.clone(),
            message,
        }),
    }
}

fn scan_into_snapshot_file(
    root: &Path,
    current: &Path,
    source_index: usize,
    filter: &BackupFilter,
    provider: &impl FileSystemProvider,
    object_store: &ObjectStore,
    compression_algorithm: CompressionAlgorithm,
    encryption_algorithm: EncryptionAlgorithm,
    master_key: Option<&RepositoryMasterKey>,
    hard_link_targets: &mut HashMap<crate::filesystem::HardLinkIdentity, HardLinkTarget>,
    snapshot_file: &mut SnapshotFile,
) -> BackupCoreResult<()> {
    // 扫描阶段同时完成筛选、特殊节点识别、object 写入和 hard link 关系记录。
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) if is_skippable_scan_io_error(&error) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if is_skippable_scan_io_error(&error) => continue,
            Err(error) => return Err(error.into()),
        };
        let path = entry.path();
        let file_entry = match provider.read_entry(root, &path) {
            Ok(file_entry) => file_entry,
            Err(error) if is_skippable_scan_error(&error) => continue,
            Err(error) => return Err(error),
        };

        if file_entry.file_type == FileKind::Directory {
            // 目录即使不匹配文件筛选也要进入 snapshot，用于恢复目录结构和继续遍历子节点。
            snapshot_file.entries.push(SnapshotEntry::from_file_entry(
                source_index,
                file_entry.clone(),
                None,
                None,
            ));
            scan_into_snapshot_file(
                root,
                &path,
                source_index,
                filter,
                provider,
                object_store,
                compression_algorithm,
                encryption_algorithm,
                master_key,
                hard_link_targets,
                snapshot_file,
            )?;
            continue;
        }

        if !filter.allows_file_entry(&path, &file_entry)? {
            continue;
        }

        if file_entry.file_type == FileKind::Symlink {
            // symlink 只保存链接目标，不读取目标文件内容，避免跟随循环或悬空链接。
            let link_target = match provider.read_link(&path) {
                Ok(link_target) => link_target,
                Err(error) if is_skippable_scan_error(&error) => {
                    snapshot_file
                        .entries
                        .push(SnapshotEntry::unsupported_from_file_entry(
                            source_index,
                            file_entry,
                        ));
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            snapshot_file
                .entries
                .push(SnapshotEntry::symlink_from_file_entry(
                    source_index,
                    file_entry,
                    link_target,
                ));
            continue;
        }

        if matches!(
            file_entry.file_type,
            FileKind::Fifo | FileKind::Device | FileKind::Other
        ) {
            // FIFO/设备节点只记录节点和元数据；管道中的运行时数据不属于可备份内容。
            snapshot_file.entries.push(SnapshotEntry::from_file_entry(
                source_index,
                file_entry,
                None,
                None,
            ));
            continue;
        }

        let bytes = match provider.read_file(&path) {
            Ok(bytes) => bytes,
            Err(error) if is_skippable_scan_error(&error) => {
                snapshot_file
                    .entries
                    .push(SnapshotEntry::unsupported_from_file_entry(
                        source_index,
                        file_entry,
                    ));
                continue;
            }
            Err(error) => return Err(error),
        };
        let stored_object = object_store.write_object_with_options(
            &bytes,
            compression_algorithm,
            encryption_algorithm,
            master_key,
        )?;

        let hard_link_target = file_entry.hard_link_identity.and_then(|identity| {
            let current = HardLinkTarget {
                source_index,
                relative_path: file_entry.relative_path.clone(),
            };
            // 第一次看到 inode 时写入真实 object；后续同 inode 文件记录为 hard link target。
            match hard_link_targets.get(&identity) {
                Some(target) => Some(target.clone()),
                None => {
                    hard_link_targets.insert(identity, current);
                    None
                }
            }
        });
        snapshot_file.push_entry(
            source_index,
            file_entry,
            Some(stored_object.object_id),
            hard_link_target,
        );
    }

    Ok(())
}

fn is_skippable_scan_io_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
        || matches!(error.raw_os_error(), Some(1 | 433 | 995))
}

fn is_skippable_scan_error(error: &BackupError) -> bool {
    matches!(error, BackupError::Io(io_error) if is_skippable_scan_io_error(io_error))
}

impl SnapshotFile {
    pub fn has_encrypted_objects(&self) -> BackupCoreResult<bool> {
        for entry in &self.entries {
            if let Some(object_id) = &entry.object_id {
                if object_id.encryption_algorithm()? == EncryptionAlgorithm::Aes256Gcm {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn push_entry(
        &mut self,
        source_index: usize,
        file_entry: FileEntry,
        object_id: Option<ObjectId>,
        hard_link_target: Option<HardLinkTarget>,
    ) {
        self.entries.push(SnapshotEntry::from_file_entry(
            source_index,
            file_entry,
            object_id,
            hard_link_target,
        ));
    }
}

impl SnapshotEntry {
    fn unsupported_from_file_entry(source_index: usize, mut file_entry: FileEntry) -> Self {
        file_entry.file_type = FileKind::Other;
        Self::from_file_entry(source_index, file_entry, None, None)
    }

    fn from_file_entry(
        source_index: usize,
        file_entry: FileEntry,
        object_id: Option<ObjectId>,
        hard_link_target: Option<HardLinkTarget>,
    ) -> Self {
        Self {
            source_index,
            relative_path: file_entry.relative_path,
            kind: file_entry.file_type,
            size: file_entry.metadata.size,
            modified_unix_seconds: file_entry.metadata.modified_unix_seconds,
            object_id,
            hard_link_target,
            link_target: None,
            metadata: file_entry.metadata,
        }
    }

    fn symlink_from_file_entry(
        source_index: usize,
        file_entry: FileEntry,
        link_target: PathBuf,
    ) -> Self {
        Self {
            source_index,
            relative_path: file_entry.relative_path,
            kind: file_entry.file_type,
            size: file_entry.metadata.size,
            modified_unix_seconds: file_entry.metadata.modified_unix_seconds,
            object_id: None,
            hard_link_target: None,
            link_target: Some(link_target),
            metadata: file_entry.metadata,
        }
    }
}

fn restore_target_path(
    destination: &Path,
    snapshot_file: &SnapshotFile,
    source_roots: &[Option<PathBuf>],
    entry: &SnapshotEntry,
    options: &RestoreOptions,
) -> BackupCoreResult<Option<PathBuf>> {
    // 先按路径策略展开目标路径，再由文件冲突策略决定是否改名、跳过或失败。
    match options.path_strategy {
        RestorePathStrategy::PreserveRelativePath => {
            if snapshot_file.sources.len() > 1 {
                let root = source_roots.get(entry.source_index).ok_or_else(|| {
                    BackupError::InvalidSnapshot(format!(
                        "missing restore root for source index: {}",
                        entry.source_index
                    ))
                })?;
                let Some(root) = root.as_ref() else {
                    return Ok(None);
                };
                Ok(Some(destination.join(root).join(&entry.relative_path)))
            } else {
                Ok(Some(destination.join(&entry.relative_path)))
            }
        }
        RestorePathStrategy::PreserveFullPath => {
            let source = snapshot_file
                .sources
                .iter()
                .find(|source| source.index == entry.source_index)
                .ok_or_else(|| {
                    BackupError::InvalidSnapshot(format!(
                        "missing source index: {}",
                        entry.source_index
                    ))
                })?;
            Ok(Some(
                destination
                    .join(safe_full_path(&source.absolute_path))
                    .join(&entry.relative_path),
            ))
        }
        RestorePathStrategy::Flatten => {
            if entry.kind == FileKind::Directory {
                return Ok(Some(destination.to_path_buf()));
            }
            let name = entry.relative_path.file_name().ok_or_else(|| {
                BackupError::InvalidSnapshot(format!(
                    "flatten entry missing file name: {}",
                    entry.relative_path.display()
                ))
            })?;
            Ok(Some(destination.join(name)))
        }
    }
}

fn resolve_source_roots(
    snapshot_file: &SnapshotFile,
    options: &RestoreOptions,
) -> BackupCoreResult<Vec<Option<PathBuf>>> {
    // PreserveRelativePath 多源恢复时使用源根名隔离；根名冲突复用 Flatten 冲突策略。
    let mut roots = vec![None; snapshot_file.sources.len()];
    if options.path_strategy != RestorePathStrategy::PreserveRelativePath
        || snapshot_file.sources.len() <= 1
    {
        return Ok(roots);
    }

    let mut used = HashSet::new();
    for source in &snapshot_file.sources {
        let mut candidate = source.restore_root.clone();
        if candidate.as_os_str().is_empty() {
            candidate = default_restore_root(&source.absolute_path);
        }

        let key = comparable_path(&candidate);
        let resolved = if used.insert(key) {
            Some(candidate)
        } else {
            match options.flatten_conflict_strategy {
                FlattenConflictStrategy::Error => {
                    return Err(BackupError::PathConflict(candidate));
                }
                FlattenConflictStrategy::Skip => None,
                FlattenConflictStrategy::Overwrite => Some(candidate),
                FlattenConflictStrategy::Rename => {
                    let renamed = renamed_source_root(&candidate, &mut used);
                    Some(renamed)
                }
            }
        };

        if source.index >= roots.len() {
            return Err(BackupError::InvalidSnapshot(format!(
                "source index out of range: {}",
                source.index
            )));
        }
        roots[source.index] = resolved;
    }

    Ok(roots)
}

fn renamed_source_root(root: &Path, used: &mut HashSet<String>) -> PathBuf {
    let text = root.to_string_lossy();
    for index in 1..10_000 {
        let candidate = PathBuf::from(format!("{text} ({index})"));
        if used.insert(comparable_path(&candidate)) {
            return candidate;
        }
    }
    PathBuf::from(format!("{text} (9999)"))
}

fn default_restore_root(source: &Path) -> PathBuf {
    if let Some(name) = source.file_name() {
        return PathBuf::from(sanitize_component(name));
    }

    for component in source.components() {
        if let Component::Prefix(prefix) = component {
            return prefix_restore_root(prefix.as_os_str());
        }
    }

    PathBuf::from("root")
}

fn prefix_restore_root(value: &std::ffi::OsStr) -> PathBuf {
    let text = value.to_string_lossy();
    if text.starts_with(r"\\") {
        let parts = text
            .trim_start_matches('\\')
            .split('\\')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.len() >= 2 {
            return PathBuf::from(sanitize_component(format!("{}_{}", parts[0], parts[1])));
        }
        return PathBuf::from("UNC");
    }
    PathBuf::from(sanitize_component(text.trim_end_matches(':')))
}

fn safe_full_path(path: &Path) -> PathBuf {
    // PreserveFullPath 不能直接写盘符、UNC 前缀或根目录，必须编码成安全路径组件。
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => push_sanitized_prefix(&mut output, prefix.as_os_str()),
            Component::RootDir => {}
            Component::Normal(value) => output.push(sanitize_component(value)),
            Component::CurDir | Component::ParentDir => {}
        }
    }
    output
}

fn push_sanitized_prefix(output: &mut PathBuf, value: &std::ffi::OsStr) {
    let text = value.to_string_lossy();
    if text.starts_with(r"\\") {
        output.push("UNC");
        for part in text.trim_start_matches('\\').split('\\') {
            if !part.is_empty() {
                output.push(sanitize_component(part));
            }
        }
    } else {
        output.push(sanitize_component(text.trim_end_matches(':')));
    }
}

fn sanitize_component(value: impl AsRef<std::ffi::OsStr>) -> OsString {
    let text = value.as_ref().to_string_lossy();
    let mut sanitized = String::new();
    for character in text.chars() {
        match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => sanitized.push('_'),
            _ => sanitized.push(character),
        }
    }
    if sanitized.is_empty() {
        OsString::from("_")
    } else {
        OsString::from(sanitized)
    }
}

fn resolve_file_conflict(path: PathBuf, options: &RestoreOptions) -> BackupCoreResult<PathBuf> {
    // 统一处理 Flatten 冲突策略；默认 Rename 会保留所有冲突文件。
    if options.path_strategy != RestorePathStrategy::Flatten || !path.exists() {
        return Ok(path);
    }

    match options.flatten_conflict_strategy {
        FlattenConflictStrategy::Error => Err(BackupError::PathConflict(path)),
        FlattenConflictStrategy::Skip => Err(BackupError::SkipFile(path)),
        FlattenConflictStrategy::Overwrite => Ok(path),
        FlattenConflictStrategy::Rename => renamed_path(path),
    }
}

fn renamed_path(path: PathBuf) -> BackupCoreResult<PathBuf> {
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let extension = path.extension().and_then(|value| value.to_str());

    for index in 1..10_000 {
        let file_name = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = parent.join(file_name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(BackupError::PathConflict(path))
}
