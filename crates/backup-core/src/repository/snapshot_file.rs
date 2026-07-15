use std::fs;
use std::path::{Path, PathBuf};

use crate::filesystem::{Metadata, PlatformMetadata};
use crate::{BackupCoreResult, BackupError};

use super::{
    default_restore_root, FileKind, HardLinkTarget, ObjectId, SnapshotEntry, SnapshotFile,
    SnapshotId, SourceInfo, SNAPSHOT_HEADER, SNAPSHOT_TITLE_MAX_CHARS,
};

// snapshot 文件是仓库的索引入口；这里集中处理文本格式、字段转义和兼容性校验。
pub(super) fn write_snapshot_file(
    path: &Path,
    snapshot_file: &SnapshotFile,
) -> BackupCoreResult<()> {
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
        output.push_str(&platform_snapshot_value(&entry.metadata.platform));
        output.push('\t');
        // link target 和 hard link target 放在尾部，避免调整前面固定字段的含义。
        if let Some(link_target) = &entry.link_target {
            output.push_str(&escape_field(&link_target.to_string_lossy()));
        }
        output.push('\t');
        if let Some(hard_link_target) = &entry.hard_link_target {
            output.push_str(&hard_link_target.source_index.to_string());
            output.push(':');
            output.push_str(&escape_field(
                &hard_link_target.relative_path.to_string_lossy(),
            ));
        }
        output.push('\n');
    }

    fs::write(path, output)?;
    Ok(())
}

pub(super) fn read_snapshot_file(path: &Path) -> BackupCoreResult<SnapshotFile> {
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
    // 当前格式最多 13 列；保留较短列数解析，让格式演进时错误更可控。
    match parts.len() {
        11 | 12 | 13 => parse_current_entry_line(parts, line),
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
    let link_target = extra
        .get(4)
        .copied()
        .filter(|value| !value.is_empty())
        .map(unescape_field)
        .transpose()?
        .map(PathBuf::from);
    let hard_link_target = extra
        .get(5)
        .copied()
        .filter(|value| !value.is_empty())
        .map(parse_hard_link_target)
        .transpose()?;

    Ok(SnapshotEntry {
        source_index,
        relative_path,
        kind,
        size,
        modified_unix_seconds,
        object_id,
        hard_link_target,
        link_target,
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

fn parse_hard_link_target(value: &str) -> BackupCoreResult<HardLinkTarget> {
    let (source_index, relative_path) = value.split_once(':').ok_or_else(|| {
        BackupError::InvalidSnapshot(format!("invalid hard link target: {value}"))
    })?;
    Ok(HardLinkTarget {
        source_index: source_index.parse::<usize>().map_err(|_| {
            BackupError::InvalidSnapshot(format!("invalid hard link source index: {source_index}"))
        })?,
        relative_path: PathBuf::from(unescape_field(relative_path)?),
    })
}

pub(super) fn escape_field(value: &str) -> String {
    // snapshot 是制表符分隔文本，路径和标题中的控制字符必须显式转义。
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

fn platform_snapshot_value(platform: &PlatformMetadata) -> String {
    match platform {
        PlatformMetadata::Basic => "basic".to_string(),
        PlatformMetadata::Windows(metadata) => format!(
            "windows,{},{},{}",
            metadata
                .file_attributes
                .map(|value| value.to_string())
                .unwrap_or_default(),
            bool_snapshot_value(metadata.is_symlink),
            bool_snapshot_value(metadata.is_reparse_point)
        ),
        PlatformMetadata::Posix(metadata) => format!(
            "posix,{},{},{},{},{},{},{},{}",
            metadata
                .mode
                .map(|value| format!("{value:x}"))
                .unwrap_or_default(),
            metadata
                .uid
                .map(|value| value.to_string())
                .unwrap_or_default(),
            metadata
                .gid
                .map(|value| value.to_string())
                .unwrap_or_default(),
            bool_snapshot_value(metadata.is_symlink),
            bool_snapshot_value(metadata.is_fifo),
            bool_snapshot_value(metadata.is_device),
            metadata
                .device_major
                .map(|value| value.to_string())
                .unwrap_or_default(),
            metadata
                .device_minor
                .map(|value| value.to_string())
                .unwrap_or_default()
        ),
    }
}

fn parse_platform_metadata(value: &str) -> BackupCoreResult<PlatformMetadata> {
    if value.is_empty() || value == "basic" {
        return Ok(PlatformMetadata::Basic);
    }
    let parts = value.split(',').collect::<Vec<_>>();
    match parts.first().copied() {
        Some("windows") => Ok(PlatformMetadata::Windows(
            crate::filesystem::WindowsMetadata {
                file_attributes: parse_optional_u32(parts.get(1).copied().unwrap_or(""))?,
                is_symlink: parse_optional_bool(parts.get(2).copied().unwrap_or(""))?,
                is_reparse_point: parse_optional_bool(parts.get(3).copied().unwrap_or(""))?,
            },
        )),
        Some("posix") => Ok(PlatformMetadata::Posix(crate::filesystem::PosixMetadata {
            mode: parse_optional_hex_u32(parts.get(1).copied().unwrap_or(""))?,
            uid: parse_optional_u32(parts.get(2).copied().unwrap_or(""))?,
            gid: parse_optional_u32(parts.get(3).copied().unwrap_or(""))?,
            is_symlink: parse_optional_bool(parts.get(4).copied().unwrap_or(""))?,
            is_fifo: parse_optional_bool(parts.get(5).copied().unwrap_or(""))?,
            is_device: parse_optional_bool(parts.get(6).copied().unwrap_or(""))?,
            device_major: parse_optional_u64(parts.get(7).copied().unwrap_or(""))?,
            device_minor: parse_optional_u64(parts.get(8).copied().unwrap_or(""))?,
            filesystem_device: None,
            inode: None,
        })),
        _ => Err(BackupError::InvalidSnapshot(format!(
            "invalid platform metadata: {value}"
        ))),
    }
}

fn bool_snapshot_value(value: bool) -> &'static str {
    if value {
        "1"
    } else {
        "0"
    }
}

fn parse_optional_bool(value: &str) -> BackupCoreResult<bool> {
    match value {
        "" | "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(BackupError::InvalidSnapshot(format!(
            "invalid boolean value: {value}"
        ))),
    }
}

fn parse_optional_u32(value: &str) -> BackupCoreResult<Option<u32>> {
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse::<u32>()
            .map(Some)
            .map_err(|_| BackupError::InvalidSnapshot(format!("invalid integer: {value}")))
    }
}

fn parse_optional_u64(value: &str) -> BackupCoreResult<Option<u64>> {
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| BackupError::InvalidSnapshot(format!("invalid integer: {value}")))
    }
}

fn parse_optional_hex_u32(value: &str) -> BackupCoreResult<Option<u32>> {
    if value.is_empty() {
        Ok(None)
    } else {
        u32::from_str_radix(value, 16)
            .map(Some)
            .map_err(|_| BackupError::InvalidSnapshot(format!("invalid hex integer: {value}")))
    }
}

pub(super) fn unescape_field(value: &str) -> BackupCoreResult<String> {
    // 只接受本格式定义的转义序列，避免拼写错误被静默读成其他内容。
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

pub(super) fn normalize_snapshot_title(value: Option<String>) -> BackupCoreResult<Option<String>> {
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
