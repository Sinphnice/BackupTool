use crate::filesystem::{
    BasicFileSystemProvider, FileEntry, FileSystemProvider, FileSystemWriter, Metadata,
    PlatformMetadata, RestoreOptions, RestoreReport,
};
use crate::{BackupCoreResult, BackupError, BackupFilter};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const REPOSITORY_META: &str = "backup-tool repository v1\n";
const MANIFEST_HEADER: &str = "backup-tool manifest v1";

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
}

impl From<String> for ObjectId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub id: SnapshotId,
}

pub use crate::filesystem::FileType as FileKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub relative_path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    pub modified_unix_seconds: Option<i64>,
    pub object_id: Option<ObjectId>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub snapshot_id: SnapshotId,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone)]
pub struct Repository {
    root: PathBuf,
}

impl Repository {
    pub fn init(root: impl Into<PathBuf>) -> BackupCoreResult<Self> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(BackupError::EmptyPath("repository"));
        }

        fs::create_dir_all(root.join("snapshots"))?;
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("indexes"))?;
        fs::write(root.join("repo.meta"), REPOSITORY_META)?;

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
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn snapshots_dir(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    fn manifest_path(&self, snapshot_id: &SnapshotId) -> PathBuf {
        self.snapshots_dir()
            .join(format!("{}.manifest", snapshot_id.as_str()))
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
}

#[derive(Debug, Clone)]
pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    pub fn write_object(&self, bytes: &[u8]) -> BackupCoreResult<ObjectId> {
        fs::create_dir_all(&self.root)?;
        let object_id = ContentHasher::hash_bytes(bytes);
        let path = self.path_for(&object_id);
        if !path.exists() {
            let mut file = fs::File::create(path)?;
            file.write_all(bytes)?;
        }
        Ok(object_id)
    }

    pub fn read_object(&self, object_id: &ObjectId) -> BackupCoreResult<Vec<u8>> {
        Ok(fs::read(self.path_for(object_id))?)
    }

    fn path_for(&self, object_id: &ObjectId) -> PathBuf {
        self.root.join(object_id.as_str())
    }
}

pub struct ContentHasher;

impl ContentHasher {
    pub fn hash_bytes(bytes: &[u8]) -> ObjectId {
        // FNV-1a is enough for this first repository format: stable, tiny, and deterministic.
        // Later milestones can replace this with a cryptographic content hash if needed.
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        ObjectId(format!("{hash:016x}-{}", bytes.len()))
    }
}

#[derive(Debug, Clone)]
pub struct RepositoryWriter {
    repository: Repository,
}

impl RepositoryWriter {
    pub fn backup(
        &self,
        source: impl AsRef<Path>,
        filter: &BackupFilter,
    ) -> BackupCoreResult<Snapshot> {
        let source = source.as_ref();
        validate_source_directory(source)?;

        let snapshot_id = self.create_snapshot_id()?;
        let mut manifest = Manifest {
            snapshot_id: snapshot_id.clone(),
            entries: Vec::new(),
        };
        let provider = BasicFileSystemProvider;
        let object_store = self.repository.object_store();
        scan_into_manifest(
            source,
            source,
            filter,
            &provider,
            &object_store,
            &mut manifest,
        )?;
        write_manifest(&self.repository.manifest_path(&snapshot_id), &manifest)?;

        Ok(Snapshot { id: snapshot_id })
    }

    fn create_snapshot_id(&self) -> BackupCoreResult<SnapshotId> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
            BackupError::InvalidRepository("system time is before unix epoch".into())
        })?;

        for sequence in 0..1000_u16 {
            let id = SnapshotId(format!(
                "snapshot-{}-{:09}-{sequence:03}",
                now.as_secs(),
                now.subsec_nanos()
            ));
            if !self.repository.manifest_path(&id).exists() {
                return Ok(id);
            }
        }

        Err(BackupError::InvalidRepository(
            "failed to allocate unique snapshot id".into(),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct RepositoryReader {
    repository: Repository,
}

impl RepositoryReader {
    pub fn read_manifest(&self, snapshot_id: &SnapshotId) -> BackupCoreResult<Manifest> {
        let path = self.repository.manifest_path(snapshot_id);
        if !path.is_file() {
            return Err(BackupError::SnapshotDoesNotExist(
                snapshot_id.as_str().to_string(),
            ));
        }
        read_manifest(&path)
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

        let manifest = self.read_manifest(snapshot_id)?;
        let object_store = self.repository.object_store();
        let writer = BasicFileSystemProvider;
        let mut report = RestoreReport::default();
        fs::create_dir_all(destination)?;

        for entry in manifest.entries {
            let target = destination.join(&entry.relative_path);
            match entry.kind {
                FileKind::Directory => {
                    writer.create_directory(&target)?;
                    report.warnings.extend(writer.restore_metadata(
                        &target,
                        &entry.to_file_entry(),
                        options.strategy,
                    )?);
                }
                FileKind::File => {
                    let object_id = entry.object_id.as_ref().ok_or_else(|| {
                        BackupError::InvalidManifest(format!(
                            "file entry missing object id: {}",
                            entry.relative_path.display()
                        ))
                    })?;
                    writer.write_file(&target, &object_store.read_object(&object_id)?)?;
                    report.warnings.extend(writer.restore_metadata(
                        &target,
                        &entry.to_file_entry(),
                        options.strategy,
                    )?);
                }
                FileKind::Symlink | FileKind::Other => {
                    report.warnings.extend(writer.handle_unsupported_entry(
                        &target,
                        &entry.to_file_entry(),
                        options.strategy,
                    )?);
                }
            }
        }

        Ok(report)
    }
}

impl ManifestEntry {
    fn to_file_entry(&self) -> FileEntry {
        FileEntry {
            relative_path: self.relative_path.clone(),
            file_type: self.kind,
            metadata: self.metadata.clone(),
        }
    }
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

fn scan_into_manifest(
    root: &Path,
    current: &Path,
    filter: &BackupFilter,
    provider: &impl FileSystemProvider,
    object_store: &ObjectStore,
    manifest: &mut Manifest,
) -> BackupCoreResult<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_entry = provider.read_entry(root, &path)?;

        if file_entry.file_type == FileKind::Directory {
            manifest
                .entries
                .push(ManifestEntry::from_file_entry(file_entry.clone(), None));
            scan_into_manifest(root, &path, filter, provider, object_store, manifest)?;
            continue;
        }

        if file_entry.file_type == FileKind::Symlink || file_entry.file_type == FileKind::Other {
            manifest
                .entries
                .push(ManifestEntry::from_file_entry(file_entry, None));
            continue;
        }

        let metadata = fs::metadata(&path)?;
        if file_entry.file_type != FileKind::File
            || !filter.allows(&file_entry.relative_path, &metadata)?
        {
            continue;
        }

        let bytes = provider.read_file(&path)?;
        let object_id = object_store.write_object(&bytes)?;

        manifest.push_entry(file_entry, Some(object_id));
    }

    Ok(())
}

impl Manifest {
    fn push_entry(&mut self, file_entry: FileEntry, object_id: Option<ObjectId>) {
        self.entries
            .push(ManifestEntry::from_file_entry(file_entry, object_id));
    }
}

impl ManifestEntry {
    fn from_file_entry(file_entry: FileEntry, object_id: Option<ObjectId>) -> Self {
        Self {
            relative_path: file_entry.relative_path,
            kind: file_entry.file_type,
            size: file_entry.metadata.size,
            modified_unix_seconds: file_entry.metadata.modified_unix_seconds,
            object_id,
            metadata: file_entry.metadata,
        }
    }
}

fn write_manifest(path: &Path, manifest: &Manifest) -> BackupCoreResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut output = String::new();
    output.push_str(MANIFEST_HEADER);
    output.push('\n');
    output.push_str("snapshot\t");
    output.push_str(manifest.snapshot_id.as_str());
    output.push('\n');

    for entry in &manifest.entries {
        output.push_str("entry\t");
        output.push_str(entry.kind.as_manifest_value());
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
        output.push_str(platform_manifest_value(&entry.metadata.platform));
        output.push('\n');
    }

    fs::write(path, output)?;
    Ok(())
}

fn read_manifest(path: &Path) -> BackupCoreResult<Manifest> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    match lines.next() {
        Some(MANIFEST_HEADER) => {}
        _ => return Err(BackupError::InvalidManifest("invalid header".into())),
    }

    let snapshot_line = lines
        .next()
        .ok_or_else(|| BackupError::InvalidManifest("missing snapshot line".into()))?;
    let mut snapshot_parts = snapshot_line.splitn(2, '\t');
    if snapshot_parts.next() != Some("snapshot") {
        return Err(BackupError::InvalidManifest("invalid snapshot line".into()));
    }
    let snapshot_id = SnapshotId(
        snapshot_parts
            .next()
            .ok_or_else(|| BackupError::InvalidManifest("missing snapshot id".into()))?
            .to_string(),
    );

    let mut entries = Vec::new();
    for line in lines {
        let parts = line.split('\t').collect::<Vec<_>>();
        if parts.len() != 6 && parts.len() != 10 || parts[0] != "entry" {
            return Err(BackupError::InvalidManifest(format!(
                "invalid entry line: {line}"
            )));
        }

        let kind = FileKind::from_manifest_value(parts[1])?;
        let relative_path = PathBuf::from(unescape_field(parts[2])?);
        let size = parts[3]
            .parse::<u64>()
            .map_err(|_| BackupError::InvalidManifest(format!("invalid size: {}", parts[3])))?;
        let modified_unix_seconds = if parts[4].is_empty() {
            None
        } else {
            Some(parts[4].parse::<i64>().map_err(|_| {
                BackupError::InvalidManifest(format!("invalid modified time: {}", parts[4]))
            })?)
        };
        let object_id = if parts[5].is_empty() {
            None
        } else {
            Some(ObjectId(parts[5].to_string()))
        };
        let accessed_unix_seconds = parse_optional_i64(parts.get(6).copied().unwrap_or(""))?;
        let created_unix_seconds = parse_optional_i64(parts.get(7).copied().unwrap_or(""))?;
        let readonly = parse_readonly(parts.get(8).copied().unwrap_or(""))?;
        let platform = parse_platform_metadata(parts.get(9).copied().unwrap_or(""))?;

        entries.push(ManifestEntry {
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
        });
    }

    Ok(Manifest {
        snapshot_id,
        entries,
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
            .map_err(|_| BackupError::InvalidManifest(format!("invalid integer: {value}")))
    }
}

fn parse_readonly(value: &str) -> BackupCoreResult<bool> {
    match value {
        "" | "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(BackupError::InvalidManifest(format!(
            "invalid readonly value: {value}"
        ))),
    }
}

fn platform_manifest_value(platform: &PlatformMetadata) -> &'static str {
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
        _ => Err(BackupError::InvalidManifest(format!(
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
                return Err(BackupError::InvalidManifest(format!(
                    "invalid escape sequence: \\{other}"
                )))
            }
            None => {
                return Err(BackupError::InvalidManifest(
                    "unterminated escape sequence".into(),
                ))
            }
        }
    }
    Ok(unescaped)
}
