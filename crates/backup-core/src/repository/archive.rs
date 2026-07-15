use std::fs;
use std::path::{Component, Path, PathBuf};

use tar::{Archive, Builder};

use crate::{BackupCoreResult, BackupError};

use super::{ArchiveAlgorithm, ArchiveResult, Repository};

// repository 归档只处理完整仓库目录；导入时拒绝绝对路径、.. 和非常规 entry。
impl Repository {
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

        // 允许用户把 tar 输出到 repository 内部；导出时跳过该输出文件，避免递归打包自身。
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

        // 导入目标必须是空目录，避免 repository 结构和已有普通文件混合。
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
            // tar 内路径必须是相对路径；拒绝绝对路径、盘符、.. 等路径穿越形式。
            let safe_path = safe_archive_path(&entry_path)?;
            entry.unpack(destination.join(safe_path))?;
        }

        Repository::open(destination)
    }
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
        // 固定排序让导出的 tar 更稳定，便于调试和比较。
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
