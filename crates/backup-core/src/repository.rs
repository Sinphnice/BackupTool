use crate::filesystem::{
    AutoFileSystemProvider, FileEntry, FileSystemProvider, FileSystemWriter,
    FlattenConflictStrategy, Metadata, PlatformMetadata, RestoreOptions, RestorePathStrategy,
    RestoreReport,
};
use crate::{BackupCoreResult, BackupError, BackupFilter};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tar::{Archive, Builder};

const REPOSITORY_META: &str = "backup-tool repository v1\n";
const SNAPSHOT_HEADER: &str = "backup-tool snapshot v1";
const SNAPSHOT_TITLE_MAX_CHARS: usize = 120;
const REPOSITORY_DISPLAY_NAME_MAX_CHARS: usize = 120;
const REPOSITORY_KEY_FORMAT_VERSION: u16 = 1;
const REPOSITORY_MASTER_KEY_LEN: usize = 32;
const AES_GCM_NONCE_LEN: usize = 12;
const ARGON2_SALT_LEN: usize = 16;
const KEY_ID_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
pub struct Snapshot {
    pub id: SnapshotId,
    pub created_unix_seconds: i64,
    pub created_nanoseconds: u32,
    pub sequence: u16,
    pub title: Option<String>,
    pub ignored_sources: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
pub struct SnapshotDeleteResult {
    pub snapshot_id: SnapshotId,
    pub deleted_object_count: u64,
    pub reclaimed_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
struct RepositoryMasterKey {
    key: [u8; REPOSITORY_MASTER_KEY_LEN],
    key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub index: usize,
    pub absolute_path: PathBuf,
    pub restore_root: PathBuf,
}

pub use crate::filesystem::FileType as FileKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub source_index: usize,
    pub relative_path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    pub modified_unix_seconds: Option<i64>,
    pub object_id: Option<ObjectId>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
pub enum ArchiveAlgorithm {
    Tar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveResult {
    pub algorithm: ArchiveAlgorithm,
    pub path: PathBuf,
    pub byte_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    None,
    Zstd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionAlgorithm {
    None,
    Aes256Gcm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
pub struct Repository {
    root: PathBuf,
}

impl Repository {
    pub fn init(root: impl Into<PathBuf>) -> BackupCoreResult<Self> {
        Self::init_with_options(root, None, EncryptionAlgorithm::None, None)
    }

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
            .or_else(|| root.file_name().and_then(|value| value.to_str()).map(ToOwned::to_owned))
            .unwrap_or_else(|| root.display().to_string());
        let metadata = RepositoryMetadata::new(
            display_name,
            encryption_algorithm,
            encryption_password.as_deref(),
        )?;
        write_repository_metadata(&root.join("repo.meta"), &metadata)?;

        Ok(Self { root })
    }

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

    pub fn metadata(&self) -> BackupCoreResult<RepositoryMetadata> {
        read_repository_metadata(&self.root.join("repo.meta"))
    }

    pub fn set_display_name(&self, display_name: String) -> BackupCoreResult<RepositoryMetadata> {
        let mut metadata = self.metadata()?;
        metadata.display_name = normalize_repository_display_name(Some(display_name)).ok_or_else(|| {
            BackupError::InvalidRepository("repository display name must not be empty".into())
        })?;
        write_repository_metadata(&self.root.join("repo.meta"), &metadata)?;
        Ok(metadata)
    }

    pub fn verify_encryption_password(&self, password: Option<&str>) -> BackupCoreResult<()> {
        self.metadata()?.verify_encryption_password(password)
    }

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
        write_repository_metadata(&temp_path, &updated).map_err(|error| {
            BackupError::InvalidRepository(format!("failed to write temporary repo metadata: {error}"))
        })?;
        let file = fs::OpenOptions::new().read(true).write(true).open(&temp_path)?;
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

    pub fn writer(&self) -> RepositoryWriter {
        RepositoryWriter {
            repository: self.clone(),
        }
    }

    pub fn reader(&self) -> RepositoryReader {
        RepositoryReader {
            repository: self.clone(),
        }
    }

    pub fn export_archive(
        &self,
        output_file: impl AsRef<Path>,
        algorithm: ArchiveAlgorithm,
    ) -> BackupCoreResult<ArchiveResult> {
        match algorithm {
            ArchiveAlgorithm::Tar => self.export_tar(output_file.as_ref()),
        }
    }

    pub fn import_archive(
        archive_file: impl AsRef<Path>,
        destination: impl Into<PathBuf>,
        algorithm: ArchiveAlgorithm,
    ) -> BackupCoreResult<Self> {
        let destination = destination.into();
        match algorithm {
            ArchiveAlgorithm::Tar => Self::import_tar(archive_file.as_ref(), &destination),
        }
    }

    fn export_tar(&self, output_file: &Path) -> BackupCoreResult<ArchiveResult> {
        Repository::open(&self.root)?;
        if output_file.as_os_str().is_empty() {
            return Err(BackupError::EmptyPath("archive"));
        }
        if let Some(parent) = output_file.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let output_skip_path = canonical_output_path(output_file);
        let file = fs::File::create(output_file)?;
        let mut builder = Builder::new(file);
        append_repository_component(
            &mut builder,
            &self.root,
            Path::new("repo.meta"),
            &output_skip_path,
        )?;
        append_repository_component(
            &mut builder,
            &self.root,
            Path::new("objects"),
            &output_skip_path,
        )?;
        append_repository_component(
            &mut builder,
            &self.root,
            Path::new("snapshots"),
            &output_skip_path,
        )?;
        append_repository_component(
            &mut builder,
            &self.root,
            Path::new("indexes"),
            &output_skip_path,
        )?;
        builder.finish()?;
        drop(builder);

        Ok(ArchiveResult {
            algorithm: ArchiveAlgorithm::Tar,
            path: output_file.to_path_buf(),
            byte_count: fs::metadata(output_file)?.len(),
        })
    }

    fn import_tar(archive_file: &Path, destination: &Path) -> BackupCoreResult<Self> {
        if archive_file.as_os_str().is_empty() {
            return Err(BackupError::EmptyPath("archive"));
        }
        if destination.as_os_str().is_empty() {
            return Err(BackupError::EmptyPath("repository"));
        }
        if destination.exists() && fs::read_dir(destination)?.next().transpose()?.is_some() {
            return Err(BackupError::InvalidRepository(format!(
                "import destination exists and is not empty: {}",
                destination.display()
            )));
        }

        fs::create_dir_all(destination)?;
        let file = fs::File::open(archive_file)?;
        let mut archive = Archive::new(file);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_type = entry.header().entry_type();
            if !entry_type.is_file() && !entry_type.is_dir() {
                return Err(BackupError::InvalidArchive(format!(
                    "unsupported archive entry type: {:?}",
                    entry_type
                )));
            }
            let entry_path = entry.path()?;
            let safe_path = safe_archive_path(&entry_path)?;
            entry.unpack(destination.join(safe_path))?;
        }

        Repository::open(destination)
    }
}

impl RepositoryMetadata {
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
                let master_key = random_bytes(REPOSITORY_MASTER_KEY_LEN);
                let salt = random_bytes(ARGON2_SALT_LEN);
                let nonce = random_bytes(AES_GCM_NONCE_LEN);
                let key_id = hex::encode(random_bytes(KEY_ID_LEN));
                let cipher = Aes256Gcm::new_from_slice(&derive_encryption_key(password, &salt)?)
                    .map_err(|_| BackupError::InvalidRepository("failed to create repository cipher".into()))?;
                let wrapped_master_key = cipher
                    .encrypt(Nonce::from_slice(&nonce), master_key.as_slice())
                    .map_err(|_| BackupError::InvalidRepository("failed to wrap repository master key".into()))?;
                Ok(Self {
                    display_name,
                    encryption_algorithm,
                    format_version: REPOSITORY_KEY_FORMAT_VERSION,
                    kdf: "argon2id".to_string(),
                    argon2_parameters: "default".to_string(),
                    salt: Some(salt),
                    wrapping_algorithm: "aes-256-gcm".to_string(),
                    nonce: Some(nonce),
                    wrapped_master_key: Some(wrapped_master_key),
                    key_id: Some(key_id),
                })
            }
        }
    }

    pub fn verify_encryption_password(&self, password: Option<&str>) -> BackupCoreResult<()> {
        self.unlock_master_key(password).map(|_| ())
    }

    fn unlock_master_key(&self, password: Option<&str>) -> BackupCoreResult<Option<RepositoryMasterKey>> {
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
                    BackupError::InvalidRepository("encrypted repository missing wrapped master key".into())
                })?;
                let cipher = Aes256Gcm::new_from_slice(&derive_encryption_key(password, salt)?)
                    .map_err(|_| BackupError::InvalidRepository("failed to create repository cipher".into()))?;
                let decrypted = cipher.decrypt(Nonce::from_slice(nonce), wrapped_master_key).map_err(|_| {
                    BackupError::InvalidRepository(
                        "failed to unlock repository; password may be incorrect".into(),
                    )
                })?;
                if decrypted.len() != REPOSITORY_MASTER_KEY_LEN {
                    return Err(BackupError::InvalidRepository(
                        "invalid repository master key length".into(),
                    ));
                }
                let mut key = [0_u8; REPOSITORY_MASTER_KEY_LEN];
                key.copy_from_slice(&decrypted);
                Ok(Some(RepositoryMasterKey {
                    key,
                    key_id: self.key_id.clone().ok_or_else(|| {
                        BackupError::InvalidRepository("encrypted repository missing key id".into())
                    })?,
                }))
            }
        }
    }

    fn rewrap_master_key(&self, old_password: &str, new_password: &str) -> BackupCoreResult<Self> {
        let master_key = self.unlock_master_key(Some(old_password))?.ok_or_else(|| {
            BackupError::InvalidRepository("repository encryption is not configured".into())
        })?;
        required_password(Some(new_password))?;
        let salt = random_bytes(ARGON2_SALT_LEN);
        let nonce = random_bytes(AES_GCM_NONCE_LEN);
        let cipher = Aes256Gcm::new_from_slice(&derive_encryption_key(new_password, &salt)?)
            .map_err(|_| BackupError::InvalidRepository("failed to create repository cipher".into()))?;
        let wrapped_master_key = cipher
            .encrypt(Nonce::from_slice(&nonce), master_key.key.as_slice())
            .map_err(|_| BackupError::InvalidRepository("failed to wrap repository master key".into()))?;
        Ok(Self {
            display_name: self.display_name.clone(),
            encryption_algorithm: self.encryption_algorithm,
            format_version: REPOSITORY_KEY_FORMAT_VERSION,
            kdf: "argon2id".to_string(),
            argon2_parameters: "default".to_string(),
            salt: Some(salt),
            wrapping_algorithm: "aes-256-gcm".to_string(),
            nonce: Some(nonce),
            wrapped_master_key: Some(wrapped_master_key),
            key_id: self.key_id.clone(),
        })
    }

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
    Some(value.chars().take(REPOSITORY_DISPLAY_NAME_MAX_CHARS).collect())
}

fn read_repository_metadata(path: &Path) -> BackupCoreResult<RepositoryMetadata> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    if lines.next() != Some(REPOSITORY_META.trim_end()) {
        return Err(BackupError::InvalidRepository("invalid repo.meta header".into()));
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
            "encryption" => encryption_algorithm = Some(EncryptionAlgorithm::from_object_value(value)?),
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
            "key_id" => key_id = if value.is_empty() { None } else { Some(value.to_string()) },
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
    output.push_str(&metadata.wrapped_master_key.as_ref().map(hex::encode).unwrap_or_default());
    output.push('\n');
    output.push_str("key_id\t");
    output.push_str(metadata.key_id.as_deref().unwrap_or_default());
    output.push('\n');
    fs::write(path, output)?;
    Ok(())
}

fn canonical_output_path(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path).ok();
    }
    let parent = path.parent()?;
    let name = path.file_name()?;
    fs::canonicalize(parent)
        .ok()
        .map(|parent| parent.join(name))
}

fn append_repository_component(
    builder: &mut Builder<fs::File>,
    repository_root: &Path,
    relative_path: &Path,
    output_skip_path: &Option<PathBuf>,
) -> BackupCoreResult<()> {
    let absolute_path = repository_root.join(relative_path);
    if !absolute_path.exists() {
        return Err(BackupError::InvalidRepository(format!(
            "missing repository component: {}",
            absolute_path.display()
        )));
    }
    append_archive_path(builder, &absolute_path, relative_path, output_skip_path)
}

fn append_archive_path(
    builder: &mut Builder<fs::File>,
    absolute_path: &Path,
    relative_path: &Path,
    output_skip_path: &Option<PathBuf>,
) -> BackupCoreResult<()> {
    if should_skip_archive_output(absolute_path, output_skip_path) {
        return Ok(());
    }

    if absolute_path.is_dir() {
        builder.append_dir(relative_path, absolute_path)?;
        let mut entries = fs::read_dir(absolute_path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let child_relative = relative_path.join(entry.file_name());
            append_archive_path(builder, &entry.path(), &child_relative, output_skip_path)?;
        }
        return Ok(());
    }

    if absolute_path.is_file() {
        builder.append_path_with_name(absolute_path, relative_path)?;
    }
    Ok(())
}

fn should_skip_archive_output(path: &Path, output_skip_path: &Option<PathBuf>) -> bool {
    let Some(output_skip_path) = output_skip_path else {
        return false;
    };
    fs::canonicalize(path)
        .map(|path| path == output_skip_path.as_path())
        .unwrap_or(false)
}

fn safe_archive_path(path: &Path) -> BackupCoreResult<PathBuf> {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => output.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(BackupError::InvalidArchive(format!(
                    "unsafe archive path: {}",
                    path.display()
                )));
            }
        }
    }
    if output.as_os_str().is_empty() {
        return Err(BackupError::InvalidArchive("empty archive path".into()));
    }
    Ok(output)
}

#[derive(Debug, Clone)]
pub struct ObjectStore {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub object_id: ObjectId,
    pub compression_algorithm: CompressionAlgorithm,
    pub encryption_algorithm: EncryptionAlgorithm,
}

impl ObjectStore {
    pub fn write_object(&self, bytes: &[u8]) -> BackupCoreResult<ObjectId> {
        self.write_object_with_options(
            bytes,
            CompressionAlgorithm::None,
            EncryptionAlgorithm::None,
            None,
        )
        .map(|object| object.object_id)
    }

    pub fn write_object_with_compression(
        &self,
        bytes: &[u8],
        compression_algorithm: CompressionAlgorithm,
    ) -> BackupCoreResult<StoredObject> {
        self.write_object_with_options(
            bytes,
            compression_algorithm,
            EncryptionAlgorithm::None,
            None,
        )
    }

    fn write_object_with_options(
        &self,
        bytes: &[u8],
        compression_algorithm: CompressionAlgorithm,
        encryption_algorithm: EncryptionAlgorithm,
        master_key: Option<&RepositoryMasterKey>,
    ) -> BackupCoreResult<StoredObject> {
        validate_encryption_key(encryption_algorithm, master_key)?;
        fs::create_dir_all(&self.root)?;
        let object_id = ContentHasher::hash_bytes(bytes, encryption_algorithm);
        let path = self.path_for(&object_id);
        let should_write = if path.exists() {
            let existing = read_object_header(&fs::read(&path)?)?;
            if existing.encryption_algorithm != encryption_algorithm {
                return Err(BackupError::InvalidRepository(format!(
                    "object id encryption state does not match its header: {}",
                    object_id.as_str()
                )));
            }
            if existing.encryption_algorithm == EncryptionAlgorithm::Aes256Gcm
                && encryption_algorithm == EncryptionAlgorithm::Aes256Gcm
            {
                let decoded = self.read_object_with_master_key(&object_id, master_key)?;
                if decoded != bytes {
                    return Err(BackupError::InvalidRepository(format!(
                        "existing encrypted object content mismatch: {}",
                        object_id.as_str()
                    )));
                }
            }
            existing.compression_algorithm != compression_algorithm
        } else {
            true
        };
        if should_write {
            let mut file = fs::File::create(path)?;
            file.write_all(&encode_object(
                bytes,
                compression_algorithm,
                encryption_algorithm,
                master_key,
            )?)?;
        }
        Ok(StoredObject {
            object_id,
            compression_algorithm,
            encryption_algorithm,
        })
    }

    pub fn read_object(&self, object_id: &ObjectId) -> BackupCoreResult<Vec<u8>> {
        self.read_object_with_master_key(object_id, None)
    }

    fn read_object_with_master_key(
        &self,
        object_id: &ObjectId,
        master_key: Option<&RepositoryMasterKey>,
    ) -> BackupCoreResult<Vec<u8>> {
        let bytes = fs::read(self.path_for(object_id))?;
        let header = read_object_header(&bytes)?;
        if object_id.encryption_algorithm()? != header.encryption_algorithm {
            return Err(BackupError::InvalidRepository(format!(
                "object id encryption state does not match its header: {}",
                object_id.as_str()
            )));
        }
        decode_object(&bytes, master_key)
    }

    fn path_for(&self, object_id: &ObjectId) -> PathBuf {
        self.root.join(object_id.as_str())
    }
}

const OBJECT_HEADER_MAGIC: &str = "backup-tool object v1";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObjectHeader {
    compression_algorithm: CompressionAlgorithm,
    encryption_algorithm: EncryptionAlgorithm,
    key_id: Option<String>,
    nonce: Option<Vec<u8>>,
    original_size: u64,
    payload_size: u64,
    header_len: usize,
}

fn encode_object(
    bytes: &[u8],
    compression_algorithm: CompressionAlgorithm,
    encryption_algorithm: EncryptionAlgorithm,
    master_key: Option<&RepositoryMasterKey>,
) -> BackupCoreResult<Vec<u8>> {
    validate_encryption_key(encryption_algorithm, master_key)?;
    let compressed = match compression_algorithm {
        CompressionAlgorithm::None => Ok(bytes.to_vec()),
        CompressionAlgorithm::Zstd => zstd::stream::encode_all(bytes, 3).map_err(BackupError::Io),
    }?;
    let encrypted = encrypt_payload(&compressed, encryption_algorithm, master_key)?;
    let header = format!(
        "{OBJECT_HEADER_MAGIC}\ncompression\t{}\nencryption\t{}\nkey_id\t{}\nnonce\t{}\noriginal_size\t{}\npayload_size\t{}\n\n",
        compression_algorithm.as_object_value(),
        encryption_algorithm.as_object_value(),
        encrypted
            .key_id
            .as_ref()
            .map(String::as_str)
            .unwrap_or_default(),
        encrypted
            .nonce
            .as_ref()
            .map(hex::encode)
            .unwrap_or_default(),
        bytes.len(),
        encrypted.payload.len()
    );
    let mut output = Vec::with_capacity(header.len() + encrypted.payload.len());
    output.extend_from_slice(header.as_bytes());
    output.extend_from_slice(&encrypted.payload);
    Ok(output)
}

fn decode_object(bytes: &[u8], master_key: Option<&RepositoryMasterKey>) -> BackupCoreResult<Vec<u8>> {
    let header = read_object_header(bytes)?;
    let payload = &bytes[header.header_len..];
    if payload.len() != usize::try_from(header.payload_size).unwrap_or(usize::MAX) {
        return Err(BackupError::InvalidRepository(format!(
            "object payload size mismatch: expected {}, got {}",
            header.payload_size,
            payload.len()
        )));
    }

    let decrypted = decrypt_payload(payload, &header, master_key)?;
    let decoded = match header.compression_algorithm {
        CompressionAlgorithm::None => Ok(decrypted),
        CompressionAlgorithm::Zstd => {
            zstd::stream::decode_all(decrypted.as_slice()).map_err(BackupError::Io)
        }
    }?;
    if decoded.len() != usize::try_from(header.original_size).unwrap_or(usize::MAX) {
        return Err(BackupError::InvalidRepository(format!(
            "object original size mismatch: expected {}, got {}",
            header.original_size,
            decoded.len()
        )));
    }
    Ok(decoded)
}

struct EncryptedPayload {
    payload: Vec<u8>,
    key_id: Option<String>,
    nonce: Option<Vec<u8>>,
}

fn encrypt_payload(
    payload: &[u8],
    encryption_algorithm: EncryptionAlgorithm,
    master_key: Option<&RepositoryMasterKey>,
) -> BackupCoreResult<EncryptedPayload> {
    match encryption_algorithm {
        EncryptionAlgorithm::None => Ok(EncryptedPayload {
            payload: payload.to_vec(),
            key_id: None,
            nonce: None,
        }),
        EncryptionAlgorithm::Aes256Gcm => {
            let master_key = master_key.ok_or_else(|| {
                BackupError::InvalidRepository("repository master key is required".into())
            })?;
            let nonce = random_bytes(AES_GCM_NONCE_LEN);
            let cipher = Aes256Gcm::new_from_slice(&master_key.key)
                .map_err(|_| BackupError::InvalidRepository("invalid AES key length".into()))?;
            let encrypted = cipher
                .encrypt(Nonce::from_slice(&nonce), payload)
                .map_err(|_| BackupError::InvalidRepository("object encryption failed".into()))?;
            Ok(EncryptedPayload {
                payload: encrypted,
                key_id: Some(master_key.key_id.clone()),
                nonce: Some(nonce),
            })
        }
    }
}

fn decrypt_payload(
    payload: &[u8],
    header: &ObjectHeader,
    master_key: Option<&RepositoryMasterKey>,
) -> BackupCoreResult<Vec<u8>> {
    match header.encryption_algorithm {
        EncryptionAlgorithm::None => Ok(payload.to_vec()),
        EncryptionAlgorithm::Aes256Gcm => {
            let master_key = master_key.ok_or_else(|| {
                BackupError::InvalidRepository("encryption password must not be empty".into())
            })?;
            let key_id = header.key_id.as_deref().ok_or_else(|| {
                BackupError::InvalidRepository("encrypted object key id is missing".into())
            })?;
            if key_id != master_key.key_id {
                return Err(BackupError::InvalidRepository(format!(
                    "object key id does not match repository key id: {key_id}"
                )));
            }
            let nonce = header.nonce.as_deref().ok_or_else(|| {
                BackupError::InvalidRepository("encrypted object nonce is missing".into())
            })?;
            if nonce.len() != 12 {
                return Err(BackupError::InvalidRepository(format!(
                    "invalid AES-GCM nonce length: {}",
                    nonce.len()
                )));
            }
            let cipher = Aes256Gcm::new_from_slice(&master_key.key)
                .map_err(|_| BackupError::InvalidRepository("invalid AES key length".into()))?;
            cipher
                .decrypt(Nonce::from_slice(nonce), payload)
                .map_err(|_| {
                    BackupError::InvalidRepository(
                        "failed to decrypt object payload; password may be incorrect".into(),
                    )
                })
        }
    }
}

fn validate_encryption_key(
    encryption_algorithm: EncryptionAlgorithm,
    master_key: Option<&RepositoryMasterKey>,
) -> BackupCoreResult<()> {
    if encryption_algorithm == EncryptionAlgorithm::Aes256Gcm && master_key.is_none() {
        return Err(BackupError::InvalidRepository(
            "repository master key is required".into(),
        ));
    }
    Ok(())
}

fn validate_encryption_password(
    encryption_algorithm: EncryptionAlgorithm,
    encryption_password: Option<&str>,
) -> BackupCoreResult<()> {
    if encryption_algorithm == EncryptionAlgorithm::Aes256Gcm {
        required_password(encryption_password)?;
    }
    Ok(())
}

fn required_password(value: Option<&str>) -> BackupCoreResult<&str> {
    let password = value.unwrap_or_default();
    if password.is_empty() {
        return Err(BackupError::InvalidRepository(
            "encryption password must not be empty".into(),
        ));
    }
    Ok(password)
}

fn derive_encryption_key(password: &str, salt: &[u8]) -> BackupCoreResult<[u8; 32]> {
    let mut key = [0_u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| BackupError::InvalidRepository("failed to derive encryption key".into()))?;
    Ok(key)
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; len];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

fn read_object_header(bytes: &[u8]) -> BackupCoreResult<ObjectHeader> {
    let separator = find_header_separator(bytes).ok_or_else(|| {
        BackupError::InvalidRepository("object header terminator is missing".into())
    })?;
    let header_len = separator + 2;
    let header = std::str::from_utf8(&bytes[..separator])
        .map_err(|_| BackupError::InvalidRepository("object header is not utf-8".into()))?;
    let mut lines = header.lines();
    match lines.next() {
        Some(OBJECT_HEADER_MAGIC) => {}
        _ => {
            return Err(BackupError::InvalidRepository(
                "invalid object magic or version".into(),
            ))
        }
    }

    let mut compression_algorithm = None;
    let mut encryption_algorithm = None;
    let mut key_id = None;
    let mut nonce = None;
    let mut original_size = None;
    let mut payload_size = None;
    for line in lines {
        let mut parts = line.splitn(2, '\t');
        let key = parts.next().unwrap_or_default();
        let value = parts.next().ok_or_else(|| {
            BackupError::InvalidRepository(format!("invalid object header line: {line}"))
        })?;
        match key {
            "compression" => {
                compression_algorithm = Some(CompressionAlgorithm::from_object_value(value)?);
            }
            "encryption" => {
                encryption_algorithm = Some(EncryptionAlgorithm::from_object_value(value)?);
            }
            "key_id" => {
                key_id = if value.is_empty() { None } else { Some(value.to_string()) };
            }
            "nonce" => {
                nonce = parse_optional_hex(value, "nonce")?;
            }
            "original_size" => {
                original_size = Some(value.parse::<u64>().map_err(|_| {
                    BackupError::InvalidRepository(format!("invalid original size: {value}"))
                })?);
            }
            "payload_size" => {
                payload_size = Some(value.parse::<u64>().map_err(|_| {
                    BackupError::InvalidRepository(format!("invalid payload size: {value}"))
                })?);
            }
            _ => {
                return Err(BackupError::InvalidRepository(format!(
                    "unknown object header field: {key}"
                )))
            }
        }
    }

    Ok(ObjectHeader {
        compression_algorithm: compression_algorithm.ok_or_else(|| {
            BackupError::InvalidRepository("object compression is missing".into())
        })?,
        encryption_algorithm: encryption_algorithm.ok_or_else(|| {
            BackupError::InvalidRepository(
                "object encryption is missing; old object format is not supported".into(),
            )
        })?,
        key_id,
        nonce,
        original_size: original_size.ok_or_else(|| {
            BackupError::InvalidRepository("object original size is missing".into())
        })?,
        payload_size: payload_size.ok_or_else(|| {
            BackupError::InvalidRepository("object payload size is missing".into())
        })?,
        header_len,
    })
}

fn parse_optional_hex(value: &str, name: &str) -> BackupCoreResult<Option<Vec<u8>>> {
    if value.is_empty() {
        return Ok(None);
    }
    hex::decode(value)
        .map(Some)
        .map_err(|_| BackupError::InvalidRepository(format!("invalid object {name} hex value")))
}

fn find_header_separator(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\n\n")
}

pub struct ContentHasher;

impl ContentHasher {
    pub fn hash_bytes(bytes: &[u8], encryption_algorithm: EncryptionAlgorithm) -> ObjectId {
        let hash = Sha256::digest(bytes);
        ObjectId(format!(
            "{hash:x}-{}",
            encryption_algorithm.object_id_suffix()
        ))
    }
}

#[derive(Debug, Clone)]
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
    pub fn backup(
        &self,
        source: impl AsRef<Path>,
        filter: &BackupFilter,
    ) -> BackupCoreResult<Snapshot> {
        self.backup_many([source.as_ref().to_path_buf()], filter)
    }

    pub fn backup_with_options(
        &self,
        source: impl AsRef<Path>,
        filter: &BackupFilter,
        options: BackupOptions,
    ) -> BackupCoreResult<Snapshot> {
        self.backup_many_with_options([source.as_ref().to_path_buf()], filter, options)
    }

    pub fn backup_many(
        &self,
        sources: impl IntoIterator<Item = impl Into<PathBuf>>,
        filter: &BackupFilter,
    ) -> BackupCoreResult<Snapshot> {
        self.backup_many_with_options(sources, filter, BackupOptions::default())
    }

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

fn comparable_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}

#[derive(Debug, Clone)]
pub struct RepositoryReader {
    repository: Repository,
}

impl RepositoryReader {
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

    pub fn read_snapshot(&self, snapshot_id: &SnapshotId) -> BackupCoreResult<SnapshotFile> {
        let path = self.repository.snapshot_path(snapshot_id);
        if !path.is_file() {
            return Err(BackupError::SnapshotDoesNotExist(
                snapshot_id.as_str().to_string(),
            ));
        }
        read_snapshot_file(&path)
    }

    pub fn restore(
        &self,
        snapshot_id: &SnapshotId,
        destination: impl AsRef<Path>,
    ) -> BackupCoreResult<()> {
        self.restore_with_options(snapshot_id, destination, RestoreOptions::default())
            .map(|_| ())
    }

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
                        report.warnings.extend(writer.restore_metadata(
                            &target,
                            &entry.to_file_entry_at(&target),
                            options.strategy,
                        )?);
                    }
                }
                FileKind::File => {
                    let object_id = entry.object_id.as_ref().ok_or_else(|| {
                        BackupError::InvalidSnapshot(format!(
                            "file entry missing object id: {}",
                            entry.relative_path.display()
                        ))
                    })?;
                    let target = match resolve_file_conflict(target, &options) {
                        Ok(target) => target,
                        Err(BackupError::SkipFile(_)) => continue,
                        Err(error) => return Err(error),
                    };
                    writer.write_file(
                        &target,
                        &object_store.read_object_with_master_key(object_id, master_key.as_ref())?,
                    )?;
                    report.warnings.extend(writer.restore_metadata(
                        &target,
                        &entry.to_file_entry_at(&target),
                        options.strategy,
                    )?);
                }
                FileKind::Symlink | FileKind::Other => {
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
        }
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
    snapshot_file: &mut SnapshotFile,
) -> BackupCoreResult<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_entry = provider.read_entry(root, &path)?;

        if file_entry.file_type == FileKind::Directory {
            snapshot_file.entries.push(SnapshotEntry::from_file_entry(
                source_index,
                file_entry.clone(),
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
                snapshot_file,
            )?;
            continue;
        }

        if file_entry.file_type == FileKind::Symlink || file_entry.file_type == FileKind::Other {
            snapshot_file.entries.push(SnapshotEntry::from_file_entry(
                source_index,
                file_entry,
                None,
            ));
            continue;
        }

        let metadata = fs::metadata(&path)?;
        if file_entry.file_type != FileKind::File
            || !filter.allows(&file_entry.relative_path, &metadata)?
        {
            continue;
        }

        let bytes = provider.read_file(&path)?;
        let stored_object = object_store.write_object_with_options(
            &bytes,
            compression_algorithm,
            encryption_algorithm,
            master_key,
        )?;

        snapshot_file.push_entry(source_index, file_entry, Some(stored_object.object_id));
    }

    Ok(())
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
    ) {
        self.entries.push(SnapshotEntry::from_file_entry(
            source_index,
            file_entry,
            object_id,
        ));
    }
}

impl SnapshotEntry {
    fn from_file_entry(
        source_index: usize,
        file_entry: FileEntry,
        object_id: Option<ObjectId>,
    ) -> Self {
        Self {
            source_index,
            relative_path: file_entry.relative_path,
            kind: file_entry.file_type,
            size: file_entry.metadata.size,
            modified_unix_seconds: file_entry.metadata.modified_unix_seconds,
            object_id,
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

fn write_snapshot_file(path: &Path, snapshot_file: &SnapshotFile) -> BackupCoreResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut output = String::new();
    output.push_str(SNAPSHOT_HEADER);
    output.push('\n');
    output.push_str("snapshot\t");
    output.push_str(snapshot_file.snapshot_id.as_str());
    output.push('\n');
    output.push_str("created\t");
    output.push_str(&snapshot_file.created_unix_seconds.to_string());
    output.push('\t');
    output.push_str(&snapshot_file.created_nanoseconds.to_string());
    output.push('\t');
    output.push_str(&snapshot_file.sequence.to_string());
    output.push('\n');
    output.push_str("title\t");
    if let Some(title) = &snapshot_file.title {
        output.push_str(&escape_field(title));
    }
    output.push('\n');

    for source in &snapshot_file.sources {
        output.push_str("source\t");
        output.push_str(&source.index.to_string());
        output.push('\t');
        output.push_str(&escape_field(&source.absolute_path.to_string_lossy()));
        output.push('\t');
        output.push_str(&escape_field(&source.restore_root.to_string_lossy()));
        output.push('\n');
    }

    for entry in &snapshot_file.entries {
        output.push_str("entry\t");
        output.push_str(&entry.source_index.to_string());
        output.push('\t');
        output.push_str(entry.kind.as_snapshot_value());
        output.push('\t');
        output.push_str(&escape_field(&entry.relative_path.to_string_lossy()));
        output.push('\t');
        output.push_str(&entry.size.to_string());
        output.push('\t');
        output.push_str(
            &entry
                .modified_unix_seconds
                .map(|value| value.to_string())
                .unwrap_or_default(),
        );
        output.push('\t');
        if let Some(object_id) = &entry.object_id {
            output.push_str(object_id.as_str());
        }
        output.push('\t');
        output.push_str(
            &entry
                .metadata
                .accessed_unix_seconds
                .map(|value| value.to_string())
                .unwrap_or_default(),
        );
        output.push('\t');
        output.push_str(
            &entry
                .metadata
                .created_unix_seconds
                .map(|value| value.to_string())
                .unwrap_or_default(),
        );
        output.push('\t');
        output.push_str(if entry.metadata.readonly { "1" } else { "0" });
        output.push('\t');
        output.push_str(platform_snapshot_value(&entry.metadata.platform));
        output.push('\n');
    }

    fs::write(path, output)?;
    Ok(())
}

fn read_snapshot_file(path: &Path) -> BackupCoreResult<SnapshotFile> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    match lines.next() {
        Some(SNAPSHOT_HEADER) => {}
        _ => return Err(BackupError::InvalidSnapshot("invalid header".into())),
    }

    let snapshot_line = lines
        .next()
        .ok_or_else(|| BackupError::InvalidSnapshot("missing snapshot line".into()))?;
    let mut snapshot_parts = snapshot_line.splitn(2, '\t');
    if snapshot_parts.next() != Some("snapshot") {
        return Err(BackupError::InvalidSnapshot("invalid snapshot line".into()));
    }
    let snapshot_id = SnapshotId(
        snapshot_parts
            .next()
            .ok_or_else(|| BackupError::InvalidSnapshot("missing snapshot id".into()))?
            .to_string(),
    );

    let created_line = lines
        .next()
        .ok_or_else(|| BackupError::InvalidSnapshot("missing created line".into()))?;
    let created_parts = created_line.split('\t').collect::<Vec<_>>();
    if created_parts.len() != 4 || created_parts.first().copied() != Some("created") {
        return Err(BackupError::InvalidSnapshot("invalid created line".into()));
    }
    let created_unix_seconds = parse_i64(created_parts[1])?;
    let created_nanoseconds = parse_u32(created_parts[2])?;
    let sequence = parse_u16(created_parts[3])?;

    let title_line = lines
        .next()
        .ok_or_else(|| BackupError::InvalidSnapshot("missing title line".into()))?;
    let mut title_parts = title_line.splitn(2, '\t');
    if title_parts.next() != Some("title") {
        return Err(BackupError::InvalidSnapshot("invalid title line".into()));
    }
    let title = normalize_snapshot_title(Some(
        title_parts
            .next()
            .map(unescape_field)
            .transpose()?
            .unwrap_or_default(),
    ))?;

    let mut sources = Vec::new();
    let mut entries = Vec::new();
    for line in lines {
        let parts = line.split('\t').collect::<Vec<_>>();
        match parts.first().copied() {
            Some("source") => {
                if parts.len() != 3 && parts.len() != 4 {
                    return Err(BackupError::InvalidSnapshot(format!(
                        "invalid source line: {line}"
                    )));
                }
                let absolute_path = PathBuf::from(unescape_field(parts[2])?);
                let restore_root = if parts.len() == 4 {
                    PathBuf::from(unescape_field(parts[3])?)
                } else {
                    default_restore_root(&absolute_path)
                };
                sources.push(SourceInfo {
                    index: parts[1].parse::<usize>().map_err(|_| {
                        BackupError::InvalidSnapshot(format!("invalid source index: {}", parts[1]))
                    })?,
                    absolute_path,
                    restore_root,
                });
            }
            Some("entry") => entries.push(parse_entry_line(&parts, line)?),
            _ => {
                return Err(BackupError::InvalidSnapshot(format!(
                    "invalid snapshot line: {line}"
                )))
            }
        }
    }

    Ok(SnapshotFile {
        snapshot_id,
        created_unix_seconds,
        created_nanoseconds,
        sequence,
        title,
        sources,
        entries,
    })
}

fn parse_entry_line(parts: &[&str], line: &str) -> BackupCoreResult<SnapshotEntry> {
    match parts.len() {
        11 => parse_current_entry_line(parts, line),
        _ => Err(BackupError::InvalidSnapshot(format!(
            "invalid entry line: {line}"
        ))),
    }
}

fn parse_current_entry_line(parts: &[&str], line: &str) -> BackupCoreResult<SnapshotEntry> {
    if parts.first().copied() != Some("entry") {
        return Err(BackupError::InvalidSnapshot(format!(
            "invalid entry line: {line}"
        )));
    }
    let source_index = parts[1]
        .parse::<usize>()
        .map_err(|_| BackupError::InvalidSnapshot(format!("invalid source index: {}", parts[1])))?;
    parse_entry_fields(
        source_index,
        parts[2],
        parts[3],
        parts[4],
        parts[5],
        parts[6],
        &parts[7..],
    )
}

fn parse_entry_fields(
    source_index: usize,
    kind: &str,
    relative_path: &str,
    size: &str,
    modified: &str,
    object_id: &str,
    extra: &[&str],
) -> BackupCoreResult<SnapshotEntry> {
    let kind = FileKind::from_snapshot_value(kind)?;
    let relative_path = PathBuf::from(unescape_field(relative_path)?);
    let size = size
        .parse::<u64>()
        .map_err(|_| BackupError::InvalidSnapshot(format!("invalid size: {size}")))?;
    let modified_unix_seconds = parse_optional_i64(modified)?;
    let object_id = if object_id.is_empty() {
        None
    } else {
        Some(ObjectId(object_id.to_string()))
    };
    let accessed_unix_seconds = parse_optional_i64(extra.first().copied().unwrap_or(""))?;
    let created_unix_seconds = parse_optional_i64(extra.get(1).copied().unwrap_or(""))?;
    let readonly = parse_readonly(extra.get(2).copied().unwrap_or(""))?;
    let platform = parse_platform_metadata(extra.get(3).copied().unwrap_or(""))?;

    Ok(SnapshotEntry {
        source_index,
        relative_path,
        kind,
        size,
        modified_unix_seconds,
        object_id,
        metadata: Metadata {
            size,
            modified_unix_seconds,
            accessed_unix_seconds,
            created_unix_seconds,
            readonly,
            platform,
        },
    })
}

fn escape_field(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn parse_optional_i64(value: &str) -> BackupCoreResult<Option<i64>> {
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse::<i64>()
            .map(Some)
            .map_err(|_| BackupError::InvalidSnapshot(format!("invalid integer: {value}")))
    }
}

fn parse_i64(value: &str) -> BackupCoreResult<i64> {
    value
        .parse::<i64>()
        .map_err(|_| BackupError::InvalidSnapshot(format!("invalid integer: {value}")))
}

fn parse_u32(value: &str) -> BackupCoreResult<u32> {
    value
        .parse::<u32>()
        .map_err(|_| BackupError::InvalidSnapshot(format!("invalid integer: {value}")))
}

fn parse_u16(value: &str) -> BackupCoreResult<u16> {
    value
        .parse::<u16>()
        .map_err(|_| BackupError::InvalidSnapshot(format!("invalid integer: {value}")))
}

fn parse_readonly(value: &str) -> BackupCoreResult<bool> {
    match value {
        "" | "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(BackupError::InvalidSnapshot(format!(
            "invalid readonly value: {value}"
        ))),
    }
}

fn platform_snapshot_value(platform: &PlatformMetadata) -> &'static str {
    match platform {
        PlatformMetadata::Basic => "basic",
        PlatformMetadata::Windows(_) => "windows",
        PlatformMetadata::Posix(_) => "posix",
    }
}

fn parse_platform_metadata(value: &str) -> BackupCoreResult<PlatformMetadata> {
    match value {
        "" | "basic" => Ok(PlatformMetadata::Basic),
        "windows" => Ok(PlatformMetadata::Windows(
            crate::filesystem::WindowsMetadata {
                file_attributes: None,
                is_symlink: false,
                is_reparse_point: false,
            },
        )),
        "posix" => Ok(PlatformMetadata::Posix(crate::filesystem::PosixMetadata {
            mode: None,
            uid: None,
            gid: None,
            is_symlink: false,
            is_fifo: false,
            is_device: false,
        })),
        _ => Err(BackupError::InvalidSnapshot(format!(
            "invalid platform metadata: {value}"
        ))),
    }
}

fn unescape_field(value: &str) -> BackupCoreResult<String> {
    let mut unescaped = String::new();
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            unescaped.push(character);
            continue;
        }

        match chars.next() {
            Some('\\') => unescaped.push('\\'),
            Some('t') => unescaped.push('\t'),
            Some('n') => unescaped.push('\n'),
            Some('r') => unescaped.push('\r'),
            Some(other) => {
                return Err(BackupError::InvalidSnapshot(format!(
                    "invalid escape sequence: \\{other}"
                )))
            }
            None => {
                return Err(BackupError::InvalidSnapshot(
                    "unterminated escape sequence".into(),
                ))
            }
        }
    }
    Ok(unescaped)
}

fn normalize_snapshot_title(value: Option<String>) -> BackupCoreResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let title = value.trim().to_string();
    if title.is_empty() {
        return Ok(None);
    }
    if title.chars().count() > SNAPSHOT_TITLE_MAX_CHARS {
        return Err(BackupError::InvalidSnapshot(format!(
            "snapshot title must be at most {SNAPSHOT_TITLE_MAX_CHARS} characters"
        )));
    }
    Ok(Some(title))
}
