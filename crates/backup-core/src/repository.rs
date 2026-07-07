use crate::{BackupCoreResult, BackupError, BackupFilter};
use std::fs;
use std::io::{Read, Write};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Directory,
    File,
}

impl FileKind {
    fn as_manifest_value(self) -> &'static str {
        match self {
            Self::Directory => "dir",
            Self::File => "file",
        }
    }

    fn from_manifest_value(value: &str) -> BackupCoreResult<Self> {
        match value {
            "dir" => Ok(Self::Directory),
            "file" => Ok(Self::File),
            _ => Err(BackupError::InvalidManifest(format!(
                "unknown file kind: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub relative_path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    pub modified_unix_seconds: Option<i64>,
    pub object_id: Option<ObjectId>,
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
        let object_store = self.repository.object_store();

        scan_into_manifest(source, source, filter, &object_store, &mut manifest)?;
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
        let destination = destination.as_ref();
        if destination.as_os_str().is_empty() {
            return Err(BackupError::EmptyPath("destination"));
        }

        let manifest = self.read_manifest(snapshot_id)?;
        let object_store = self.repository.object_store();
        fs::create_dir_all(destination)?;

        for entry in manifest.entries {
            let target = destination.join(&entry.relative_path);
            match entry.kind {
                FileKind::Directory => fs::create_dir_all(target)?,
                FileKind::File => {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let object_id = entry.object_id.ok_or_else(|| {
                        BackupError::InvalidManifest(format!(
                            "file entry missing object id: {}",
                            entry.relative_path.display()
                        ))
                    })?;
                    fs::write(target, object_store.read_object(&object_id)?)?;
                }
            }
        }

        Ok(())
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
    object_store: &ObjectStore,
    manifest: &mut Manifest,
) -> BackupCoreResult<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| BackupError::SourceDoesNotExist(root.to_path_buf()))?
            .to_path_buf();

        if metadata.is_dir() {
            manifest.entries.push(ManifestEntry {
                relative_path: relative.clone(),
                kind: FileKind::Directory,
                size: 0,
                modified_unix_seconds: modified_unix_seconds(&metadata),
                object_id: None,
            });
            scan_into_manifest(root, &path, filter, object_store, manifest)?;
            continue;
        }

        if !metadata.is_file() || !filter.allows(&relative, &metadata)? {
            continue;
        }

        let mut bytes = Vec::new();
        fs::File::open(&path)?.read_to_end(&mut bytes)?;
        let object_id = object_store.write_object(&bytes)?;

        manifest.entries.push(ManifestEntry {
            relative_path: relative,
            kind: FileKind::File,
            size: metadata.len(),
            modified_unix_seconds: modified_unix_seconds(&metadata),
            object_id: Some(object_id),
        });
    }

    Ok(())
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
        if parts.len() != 6 || parts[0] != "entry" {
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

        entries.push(ManifestEntry {
            relative_path,
            kind,
            size,
            modified_unix_seconds,
            object_id,
        });
    }

    Ok(Manifest {
        snapshot_id,
        entries,
    })
}

fn modified_unix_seconds(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
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
