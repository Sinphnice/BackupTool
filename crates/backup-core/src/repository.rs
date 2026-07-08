use crate::filesystem::{
    AutoFileSystemProvider, FileEntry, FileSystemProvider, FileSystemWriter,
    FlattenConflictStrategy, Metadata, PlatformMetadata, RestoreOptions, RestorePathStrategy,
    RestoreReport,
};
use crate::{BackupCoreResult, BackupError, BackupFilter};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
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
    pub ignored_sources: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub id: SnapshotId,
    pub file_count: u64,
    pub byte_count: u64,
    pub created_unix_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub index: usize,
    pub absolute_path: PathBuf,
    pub restore_root: PathBuf,
}

pub use crate::filesystem::FileType as FileKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub source_index: usize,
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
    pub sources: Vec<SourceInfo>,
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
        self.backup_many([source.as_ref().to_path_buf()], filter)
    }

    pub fn backup_many(
        &self,
        sources: impl IntoIterator<Item = impl Into<PathBuf>>,
        filter: &BackupFilter,
    ) -> BackupCoreResult<Snapshot> {
        let raw_sources = sources.into_iter().map(Into::into).collect::<Vec<_>>();
        let normalized = normalize_sources(&raw_sources)?;

        let snapshot_id = self.create_snapshot_id()?;
        let mut manifest = Manifest {
            snapshot_id: snapshot_id.clone(),
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

        for (source_index, source) in normalized.sources.iter().enumerate() {
            let provider = AutoFileSystemProvider::for_path(source);
            scan_into_manifest(
                source,
                source,
                source_index,
                filter,
                &provider,
                &object_store,
                &mut manifest,
            )?;
        }

        write_manifest(&self.repository.manifest_path(&snapshot_id), &manifest)?;

        Ok(Snapshot {
            id: snapshot_id,
            ignored_sources: normalized.ignored_sources,
        })
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
            if path.extension().and_then(|value| value.to_str()) != Some("manifest") {
                continue;
            }

            let manifest = read_manifest(&path)?;
            snapshots.push(SnapshotInfo::from_manifest(&manifest));
        }

        snapshots.sort_by(|left, right| {
            right
                .created_unix_seconds
                .cmp(&left.created_unix_seconds)
                .then_with(|| right.id.as_str().cmp(left.id.as_str()))
        });
        Ok(snapshots)
    }

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
        let writer = AutoFileSystemProvider::for_path(destination);
        let mut report = RestoreReport::default();
        let is_multi_source = manifest.sources.len() > 1;
        let source_roots = resolve_source_roots(&manifest, &options)?;
        fs::create_dir_all(destination)?;

        for entry in &manifest.entries {
            let Some(target) =
                restore_target_path(destination, &manifest, &source_roots, entry, &options)?
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
                        BackupError::InvalidManifest(format!(
                            "file entry missing object id: {}",
                            entry.relative_path.display()
                        ))
                    })?;
                    let target = match resolve_file_conflict(target, &options) {
                        Ok(target) => target,
                        Err(BackupError::SkipFile(_)) => continue,
                        Err(error) => return Err(error),
                    };
                    writer.write_file(&target, &object_store.read_object(object_id)?)?;
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
    fn from_manifest(manifest: &Manifest) -> Self {
        let mut file_count = 0;
        let mut byte_count = 0;
        for entry in &manifest.entries {
            if entry.kind == FileKind::File {
                file_count += 1;
                byte_count += entry.size;
            }
        }

        Self {
            id: manifest.snapshot_id.clone(),
            file_count,
            byte_count,
            created_unix_seconds: parse_snapshot_created_unix_seconds(&manifest.snapshot_id),
        }
    }
}

impl ManifestEntry {
    fn to_file_entry_at(&self, restored_path: &Path) -> FileEntry {
        FileEntry {
            relative_path: restored_path.to_path_buf(),
            file_type: self.kind,
            metadata: self.metadata.clone(),
        }
    }
}

fn scan_into_manifest(
    root: &Path,
    current: &Path,
    source_index: usize,
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
            manifest.entries.push(ManifestEntry::from_file_entry(
                source_index,
                file_entry.clone(),
                None,
            ));
            scan_into_manifest(
                root,
                &path,
                source_index,
                filter,
                provider,
                object_store,
                manifest,
            )?;
            continue;
        }

        if file_entry.file_type == FileKind::Symlink || file_entry.file_type == FileKind::Other {
            manifest.entries.push(ManifestEntry::from_file_entry(
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
        let object_id = object_store.write_object(&bytes)?;

        manifest.push_entry(source_index, file_entry, Some(object_id));
    }

    Ok(())
}

impl Manifest {
    fn push_entry(
        &mut self,
        source_index: usize,
        file_entry: FileEntry,
        object_id: Option<ObjectId>,
    ) {
        self.entries.push(ManifestEntry::from_file_entry(
            source_index,
            file_entry,
            object_id,
        ));
    }
}

impl ManifestEntry {
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
    manifest: &Manifest,
    source_roots: &[Option<PathBuf>],
    entry: &ManifestEntry,
    options: &RestoreOptions,
) -> BackupCoreResult<Option<PathBuf>> {
    match options.path_strategy {
        RestorePathStrategy::PreserveRelativePath => {
            if manifest.sources.len() > 1 {
                let root = source_roots.get(entry.source_index).ok_or_else(|| {
                    BackupError::InvalidManifest(format!(
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
            let source = manifest
                .sources
                .iter()
                .find(|source| source.index == entry.source_index)
                .ok_or_else(|| {
                    BackupError::InvalidManifest(format!(
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
                BackupError::InvalidManifest(format!(
                    "flatten entry missing file name: {}",
                    entry.relative_path.display()
                ))
            })?;
            Ok(Some(destination.join(name)))
        }
    }
}

fn resolve_source_roots(
    manifest: &Manifest,
    options: &RestoreOptions,
) -> BackupCoreResult<Vec<Option<PathBuf>>> {
    let mut roots = vec![None; manifest.sources.len()];
    if options.path_strategy != RestorePathStrategy::PreserveRelativePath
        || manifest.sources.len() <= 1
    {
        return Ok(roots);
    }

    let mut used = HashSet::new();
    for source in &manifest.sources {
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
            return Err(BackupError::InvalidManifest(format!(
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

    for source in &manifest.sources {
        output.push_str("source\t");
        output.push_str(&source.index.to_string());
        output.push('\t');
        output.push_str(&escape_field(&source.absolute_path.to_string_lossy()));
        output.push('\t');
        output.push_str(&escape_field(&source.restore_root.to_string_lossy()));
        output.push('\n');
    }

    for entry in &manifest.entries {
        output.push_str("entry\t");
        output.push_str(&entry.source_index.to_string());
        output.push('\t');
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

    let mut sources = Vec::new();
    let mut entries = Vec::new();
    for line in lines {
        let parts = line.split('\t').collect::<Vec<_>>();
        match parts.first().copied() {
            Some("source") => {
                if parts.len() != 3 && parts.len() != 4 {
                    return Err(BackupError::InvalidManifest(format!(
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
                        BackupError::InvalidManifest(format!("invalid source index: {}", parts[1]))
                    })?,
                    absolute_path,
                    restore_root,
                });
            }
            Some("entry") => entries.push(parse_entry_line(&parts, line)?),
            _ => {
                return Err(BackupError::InvalidManifest(format!(
                    "invalid manifest line: {line}"
                )))
            }
        }
    }

    if sources.is_empty() {
        sources.push(SourceInfo {
            index: 0,
            absolute_path: PathBuf::from("source-0"),
            restore_root: PathBuf::from("source-0"),
        });
    }

    Ok(Manifest {
        snapshot_id,
        sources,
        entries,
    })
}

fn parse_entry_line(parts: &[&str], line: &str) -> BackupCoreResult<ManifestEntry> {
    match parts.len() {
        6 | 10 => parse_legacy_entry_line(parts, line),
        11 => parse_current_entry_line(parts, line),
        _ => Err(BackupError::InvalidManifest(format!(
            "invalid entry line: {line}"
        ))),
    }
}

fn parse_legacy_entry_line(parts: &[&str], line: &str) -> BackupCoreResult<ManifestEntry> {
    if parts.first().copied() != Some("entry") {
        return Err(BackupError::InvalidManifest(format!(
            "invalid entry line: {line}"
        )));
    }
    parse_entry_fields(
        0,
        parts[1],
        parts[2],
        parts[3],
        parts[4],
        parts[5],
        &parts[6..],
    )
}

fn parse_current_entry_line(parts: &[&str], line: &str) -> BackupCoreResult<ManifestEntry> {
    if parts.first().copied() != Some("entry") {
        return Err(BackupError::InvalidManifest(format!(
            "invalid entry line: {line}"
        )));
    }
    let source_index = parts[1]
        .parse::<usize>()
        .map_err(|_| BackupError::InvalidManifest(format!("invalid source index: {}", parts[1])))?;
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
) -> BackupCoreResult<ManifestEntry> {
    let kind = FileKind::from_manifest_value(kind)?;
    let relative_path = PathBuf::from(unescape_field(relative_path)?);
    let size = size
        .parse::<u64>()
        .map_err(|_| BackupError::InvalidManifest(format!("invalid size: {size}")))?;
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

    Ok(ManifestEntry {
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

fn parse_snapshot_created_unix_seconds(snapshot_id: &SnapshotId) -> Option<i64> {
    snapshot_id
        .as_str()
        .strip_prefix("snapshot-")?
        .split('-')
        .next()?
        .parse::<i64>()
        .ok()
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
