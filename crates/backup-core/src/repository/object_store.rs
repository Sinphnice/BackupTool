use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::{BackupCoreResult, BackupError};

use super::crypto::{
    decrypt_payload, encrypt_payload, parse_optional_hex, validate_encryption_key,
    RepositoryMasterKey,
};
use super::{CompressionAlgorithm, EncryptionAlgorithm, ObjectId};

// object 文件负责保存普通文件内容；压缩、加密和 CRC 都只作用于 payload。
pub struct ObjectStore {
    pub(super) root: PathBuf,
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

    pub(super) fn write_object_with_options(
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
        // object id 由原始内容和加密状态决定；压缩算法变化时覆盖物理表示，
        // 但不能让 encrypted/plain 两种状态互相影响。
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

    pub(super) fn read_object_with_master_key(
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

    pub(super) fn path_for(&self, object_id: &ObjectId) -> PathBuf {
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
    crc32: u32,
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
    // object_id 始终基于原始 bytes；下面的压缩、加密和 header 都不参与内容 hash。
    let compressed = match compression_algorithm {
        CompressionAlgorithm::None => Ok(bytes.to_vec()),
        CompressionAlgorithm::Zstd => zstd::stream::encode_all(bytes, 3).map_err(BackupError::Io),
    }?;
    let encrypted = encrypt_payload(&compressed, encryption_algorithm, master_key)?;
    let header = format!(
        "{OBJECT_HEADER_MAGIC}\ncompression\t{}\nencryption\t{}\nkey_id\t{}\nnonce\t{}\ncrc32\t{:08x}\noriginal_size\t{}\npayload_size\t{}\n\n",
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
        crc32(bytes),
        bytes.len(),
        encrypted.payload.len()
    );
    // header 保持明文，便于恢复前判断压缩/加密/校验参数；payload 才是可压缩/可加密数据段。
    let mut output = Vec::with_capacity(header.len() + encrypted.payload.len());
    output.extend_from_slice(header.as_bytes());
    output.extend_from_slice(&encrypted.payload);
    Ok(output)
}

fn decode_object(
    bytes: &[u8],
    master_key: Option<&RepositoryMasterKey>,
) -> BackupCoreResult<Vec<u8>> {
    let header = read_object_header(bytes)?;
    let payload = &bytes[header.header_len..];
    // 先校验物理 payload 长度，避免把截断或追加的数据送入解密/解压流程。
    if payload.len() != usize::try_from(header.payload_size).unwrap_or(usize::MAX) {
        return Err(BackupError::InvalidRepository(format!(
            "object payload size mismatch: expected {}, got {}",
            header.payload_size,
            payload.len()
        )));
    }

    let decrypted = decrypt_payload(
        payload,
        header.encryption_algorithm,
        header.key_id.as_deref(),
        header.nonce.as_deref(),
        master_key,
    )?;
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
    // CRC 校验针对原始文件数据，必须在解密和解压之后执行。
    let actual_crc32 = crc32(&decoded);
    if actual_crc32 != header.crc32 {
        return Err(BackupError::InvalidRepository(format!(
            "object CRC32 mismatch: expected {:08x}, got {:08x}",
            header.crc32, actual_crc32
        )));
    }
    Ok(decoded)
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

    // object header 采用 key-value 文本格式；未知字段直接拒绝，避免静默忽略格式升级。
    let mut compression_algorithm = None;
    let mut encryption_algorithm = None;
    let mut key_id = None;
    let mut nonce = None;
    let mut crc32 = None;
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
                key_id = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "nonce" => {
                nonce = parse_optional_hex(value, "nonce")?;
            }
            "crc32" => {
                crc32 = Some(u32::from_str_radix(value, 16).map_err(|_| {
                    BackupError::InvalidRepository(format!("invalid object CRC32: {value}"))
                })?);
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
        crc32: crc32.ok_or_else(|| {
            BackupError::InvalidRepository(
                "object CRC32 is missing; old object format is not supported".into(),
            )
        })?,
        original_size: original_size.ok_or_else(|| {
            BackupError::InvalidRepository("object original size is missing".into())
        })?,
        payload_size: payload_size.ok_or_else(|| {
            BackupError::InvalidRepository("object payload size is missing".into())
        })?,
        header_len,
    })
}

fn find_header_separator(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\n\n")
}

fn crc32(bytes: &[u8]) -> u32 {
    // 标准 CRC32/IEEE 多项式；只用于损坏检测，不作为安全哈希。
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
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
