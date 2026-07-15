use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use rand::RngCore;

use crate::{BackupCoreResult, BackupError};

use super::EncryptionAlgorithm;

const REPOSITORY_MASTER_KEY_LEN: usize = 32;
const AES_GCM_NONCE_LEN: usize = 12;
const ARGON2_SALT_LEN: usize = 16;
const KEY_ID_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepositoryMasterKey {
    // 真正用于 object payload 加密的仓库主密钥；用户密码只用于封装/解封装它。
    pub(super) key: [u8; REPOSITORY_MASTER_KEY_LEN],
    pub(super) key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WrappedRepositoryMasterKey {
    pub(super) salt: Vec<u8>,
    pub(super) nonce: Vec<u8>,
    pub(super) wrapped_master_key: Vec<u8>,
    pub(super) key_id: String,
}

pub(super) struct EncryptedPayload {
    pub(super) payload: Vec<u8>,
    pub(super) key_id: Option<String>,
    pub(super) nonce: Option<Vec<u8>>,
}

pub(super) fn create_wrapped_master_key(
    password: &str,
) -> BackupCoreResult<WrappedRepositoryMasterKey> {
    required_password(Some(password))?;
    // 新建加密仓库时生成随机主密钥，避免用户密码变更时必须重写所有 object。
    let master_key = RepositoryMasterKey {
        key: random_key(),
        key_id: hex::encode(random_bytes(KEY_ID_LEN)),
    };
    wrap_master_key(&master_key, password)
}

pub(super) fn unlock_wrapped_master_key(
    password: &str,
    salt: &[u8],
    nonce: &[u8],
    wrapped_master_key: &[u8],
    key_id: &str,
) -> BackupCoreResult<RepositoryMasterKey> {
    required_password(Some(password))?;
    // repo.meta 中只保存被密码派生密钥封装后的主密钥；解封装失败通常表示密码错误。
    let cipher = Aes256Gcm::new_from_slice(&derive_encryption_key(password, salt)?)
        .map_err(|_| BackupError::InvalidRepository("failed to create repository cipher".into()))?;
    let decrypted = cipher
        .decrypt(Nonce::from_slice(nonce), wrapped_master_key)
        .map_err(|_| {
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
    Ok(RepositoryMasterKey {
        key,
        key_id: key_id.to_string(),
    })
}

pub(super) fn wrap_master_key(
    master_key: &RepositoryMasterKey,
    password: &str,
) -> BackupCoreResult<WrappedRepositoryMasterKey> {
    required_password(Some(password))?;
    // 每次重新封装都使用新的 salt/nonce；主密钥和 key_id 保持不变。
    let salt = random_bytes(ARGON2_SALT_LEN);
    let nonce = random_bytes(AES_GCM_NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(&derive_encryption_key(password, &salt)?)
        .map_err(|_| BackupError::InvalidRepository("failed to create repository cipher".into()))?;
    let wrapped_master_key = cipher
        .encrypt(Nonce::from_slice(&nonce), master_key.key.as_slice())
        .map_err(|_| {
            BackupError::InvalidRepository("failed to wrap repository master key".into())
        })?;
    Ok(WrappedRepositoryMasterKey {
        salt,
        nonce,
        wrapped_master_key,
        key_id: master_key.key_id.clone(),
    })
}

pub(super) fn encrypt_payload(
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
            // nonce 属于单个 object payload，不能跨加密操作复用。
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

pub(super) fn decrypt_payload(
    payload: &[u8],
    encryption_algorithm: EncryptionAlgorithm,
    key_id: Option<&str>,
    nonce: Option<&[u8]>,
    master_key: Option<&RepositoryMasterKey>,
) -> BackupCoreResult<Vec<u8>> {
    match encryption_algorithm {
        EncryptionAlgorithm::None => Ok(payload.to_vec()),
        EncryptionAlgorithm::Aes256Gcm => {
            let master_key = master_key.ok_or_else(|| {
                BackupError::InvalidRepository("encryption password must not be empty".into())
            })?;
            let key_id = key_id.ok_or_else(|| {
                BackupError::InvalidRepository("encrypted object key id is missing".into())
            })?;
            // key_id 用于确认 object 仍属于当前仓库主密钥，避免误用其他仓库密码。
            if key_id != master_key.key_id {
                return Err(BackupError::InvalidRepository(format!(
                    "object key id does not match repository key id: {key_id}"
                )));
            }
            let nonce = nonce.ok_or_else(|| {
                BackupError::InvalidRepository("encrypted object nonce is missing".into())
            })?;
            if nonce.len() != AES_GCM_NONCE_LEN {
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

pub(super) fn validate_encryption_key(
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

pub(super) fn validate_encryption_password(
    encryption_algorithm: EncryptionAlgorithm,
    encryption_password: Option<&str>,
) -> BackupCoreResult<()> {
    if encryption_algorithm == EncryptionAlgorithm::Aes256Gcm {
        required_password(encryption_password)?;
    }
    Ok(())
}

pub(super) fn required_password(value: Option<&str>) -> BackupCoreResult<&str> {
    let password = value.unwrap_or_default();
    if password.is_empty() {
        return Err(BackupError::InvalidRepository(
            "encryption password must not be empty".into(),
        ));
    }
    Ok(password)
}

pub(super) fn parse_optional_hex(value: &str, name: &str) -> BackupCoreResult<Option<Vec<u8>>> {
    if value.is_empty() {
        return Ok(None);
    }
    hex::decode(value)
        .map(Some)
        .map_err(|_| BackupError::InvalidRepository(format!("invalid object {name} hex value")))
}

fn derive_encryption_key(password: &str, salt: &[u8]) -> BackupCoreResult<[u8; 32]> {
    let mut key = [0_u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| BackupError::InvalidRepository("failed to derive encryption key".into()))?;
    Ok(key)
}

fn random_key() -> [u8; REPOSITORY_MASTER_KEY_LEN] {
    let mut key = [0_u8; REPOSITORY_MASTER_KEY_LEN];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; len];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}
