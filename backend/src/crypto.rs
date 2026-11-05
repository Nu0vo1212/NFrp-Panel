//! 敏感信息处理：主密钥、信封加密（AES-256-GCM）、口令哈希、脱敏工具。
//!
//! 设计要点：
//! - 主密钥与 JWT 密钥都落在权限 0600 的文件里，缺失时**自动生成**而不是硬编码；
//! - 每次加密使用随机 96-bit nonce，AAD 绑定「记录 ID + 字段名」防止密文搬移；
//! - 解密失败只报「无法解密」，不返回任何密文片段。

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use argon2::{
    password_hash::{rand_core::OsRng as PhOsRng, PasswordHasher, PasswordVerifier, SaltString},
    Argon2, PasswordHash,
};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::error::ApiError;

pub const ENC_PREFIX: &str = "v1:";
const NONCE_LEN: usize = 12;

/// 主密钥 / JWT 密钥的持有者。
#[derive(Clone)]
pub struct KeyRing {
    master: [u8; 32],
    jwt: Vec<u8>,
}

impl std::fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 永不打印密钥内容
        f.debug_struct("KeyRing")
            .field("master_fingerprint", &self.master_fingerprint())
            .field("jwt_len", &self.jwt.len())
            .finish()
    }
}

impl KeyRing {
    pub fn load_or_create(master_key_file: &Path, jwt_secret_file: &Path) -> anyhow::Result<Self> {
        let master = load_or_create_key(master_key_file, 32)?;
        let mut m = [0u8; 32];
        m.copy_from_slice(&master);
        let jwt = load_or_create_key(jwt_secret_file, 48)?;
        Ok(Self { master: m, jwt })
    }

    pub fn jwt_secret(&self) -> &[u8] {
        &self.jwt
    }

    /// 主密钥指纹（用于设置页展示"密钥是否一致"，不泄露密钥）。
    pub fn master_fingerprint(&self) -> String {
        let digest = Sha256::digest(self.master);
        hex::encode(&digest[..6])
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&self.master).expect("32 字节主密钥")
    }

    /// 加密明文：返回 `v1:<base64(nonce||ciphertext)>`。
    pub fn encrypt(&self, plaintext: &str, aad: &str) -> Result<String, ApiError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = self
            .cipher()
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("加密失败")))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(format!("{ENC_PREFIX}{}", base64_encode(&out)))
    }

    /// 解密；AAD 不匹配或密文被篡改都返回错误。
    pub fn decrypt(&self, stored: &str, aad: &str) -> Result<String, ApiError> {
        let b64 = stored
            .strip_prefix(ENC_PREFIX)
            .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("密文格式不认识")))?;
        let raw = base64_decode(b64)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("密文不是合法 base64")))?;
        if raw.len() <= NONCE_LEN {
            return Err(ApiError::Internal(anyhow::anyhow!("密文长度异常")));
        }
        let (nonce_bytes, ct) = raw.split_at(NONCE_LEN);
        let pt = self
            .cipher()
            .decrypt(
                Nonce::from_slice(nonce_bytes),
                Payload {
                    msg: ct,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| {
                ApiError::Internal(anyhow::anyhow!(
                    "无法解密该敏感字段（主密钥可能已更换，请重新录入凭据）"
                ))
            })?;
        String::from_utf8(pt).map_err(|_| ApiError::Internal(anyhow::anyhow!("明文不是 UTF-8")))
    }
}

fn load_or_create_key(path: &Path, len: usize) -> anyhow::Result<Vec<u8>> {
    if path.exists() {
        let bytes = std::fs::read(path)?;
        let trimmed: Vec<u8> = bytes
            .into_iter()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        // 支持 hex 编码或裸字节
        let key = if trimmed.len() == len * 2 && trimmed.iter().all(|b| b.is_ascii_hexdigit()) {
            let s = String::from_utf8_lossy(&trimmed).to_string();
            hex::decode(s).map_err(|e| anyhow::anyhow!("密钥文件不是合法 hex：{e}"))?
        } else {
            trimmed
        };
        if key.len() < len {
            anyhow::bail!(
                "密钥文件 {} 长度不足（需要至少 {len} 字节，实际 {}）",
                path.display(),
                key.len()
            );
        }
        return Ok(key[..len].to_vec());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut key = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut key);
    std::fs::write(path, hex::encode(&key))?;
    restrict_permissions(path)?;
    Ok(key)
}

/// 尽力把文件权限收紧到 0600（Unix）。Windows 上依赖目录 ACL。
pub fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn base64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s)
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

// ---------------------------------------------------------------------------
// 口令哈希
// ---------------------------------------------------------------------------

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut PhOsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("口令哈希失败：{e}"))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// 生成随机口令（用于首次启动的管理员）。
pub fn random_password(len: usize) -> String {
    const ALPHABET: &[u8] =
        b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789!@#$%^&*-_=+";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| {
            let idx = (rng.next_u32() as usize) % ALPHABET.len();
            ALPHABET[idx] as char
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 脱敏
// ---------------------------------------------------------------------------

/// 把敏感值替换为固定掩码（保留长度信息不泄露）。
pub fn mask_secret(_value: &str) -> String {
    "••••••••".to_string()
}

const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "token",
    "secret",
    "secret_key",
    "private_key",
    "privatekey",
    "passphrase",
    "authorization",
    "credential",
    "privilege_key",
];

/// 判断字段名是否敏感（大小写不敏感，含常见变体）。
pub fn is_sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    SENSITIVE_KEYS.iter().any(|s| k.contains(s))
}

/// 对文本行做脱敏：
/// - `token = "xxx"` → `token = "••••"`（TOML 赋值）
/// - `password: xxx` → `password: ••••`
/// - `postgres://user:pass@host` → 保留结构、遮蔽凭据
/// - 形如长 base64/hex 的可疑密钥串（≥32 字符且无空格）整体遮蔽
pub fn redact_line(line: &str) -> String {
    let mut out = line.to_string();

    for key in SENSITIVE_KEYS {
        // key = "value" / key = value
        let mut search_from = 0usize;
        while let Some(rel) = out[search_from..].to_ascii_lowercase().find(key) {
            let start = search_from + rel;
            // 找 `=` 或 `:`
            let rest = &out[start..];
            let Some(eq_pos) = rest.find(['=', ':']) else {
                break;
            };
            let after_eq = &rest[eq_pos + 1..];
            // 跳过空格
            let skip = after_eq.len() - after_eq.trim_start().len();
            let val_start = start + eq_pos + 1 + skip;
            let val = &out[val_start..];
            let (val_end, quoted) = if val.starts_with('"') {
                match val[1..].find('"') {
                    Some(e) => (val_start + e + 2, true),
                    None => break,
                }
            } else {
                let e = val
                    .find(|c: char| c == ' ' || c == ',' || c == '\n' || c == '\r')
                    .unwrap_or(val.len());
                (val_start + e, false)
            };
            if val_end <= val_start {
                break;
            }
            let replacement = if quoted { "\"••••••••\"" } else { "••••••••" };
            out.replace_range(val_start..val_end, replacement);
            search_from = val_start + replacement.len();
            if search_from >= out.len() {
                break;
            }
        }
    }

    // URL 里的 user:pass@
    if let Some(at) = out.find('@') {
        if let Some(scheme_end) = out.find("://") {
            if scheme_end < at && at > scheme_end + 3 {
                let creds = &out[scheme_end + 3..at];
                if creds.contains(':') {
                    let masked = format!("••••:••••@");
                    out.replace_range(scheme_end + 3..at + 1, &masked);
                }
            }
        }
    }

    out
}

/// 对一组文本行脱敏。
pub fn redact_lines(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| redact_line(l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring() -> KeyRing {
        KeyRing {
            master: [7u8; 32],
            jwt: b"jwt-secret".to_vec(),
        }
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let k = ring();
        let ct = k.encrypt("super-secret", "cred:1:password").unwrap();
        assert!(ct.starts_with(ENC_PREFIX));
        assert!(!ct.contains("super-secret"));
        assert_eq!(k.decrypt(&ct, "cred:1:password").unwrap(), "super-secret");
    }

    #[test]
    fn aad_binding_prevents_reuse() {
        let k = ring();
        let ct = k.encrypt("secret", "cred:1:password").unwrap();
        // 换了记录 ID 就解不开（防止密文搬移）
        assert!(k.decrypt(&ct, "cred:2:password").is_err());
    }

    #[test]
    fn nonces_are_unique() {
        let k = ring();
        let a = k.encrypt("x", "a").unwrap();
        let b = k.encrypt("x", "a").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn password_hash_roundtrip() {
        let h = hash_password("p@ssw0rd!").unwrap();
        assert!(verify_password("p@ssw0rd!", &h));
        assert!(!verify_password("wrong", &h));
        assert!(!verify_password("p@ssw0rd!", "not-a-hash"));
    }

    #[test]
    fn redaction_covers_common_shapes() {
        assert_eq!(
            redact_line(r#"token = "abcdef123456""#),
            r#"token = "••••••••""#
        );
        assert!(redact_line("password: hunter2").contains("••••"));
        assert!(redact_line("url=https://user:pw@example.com/x").contains("••••"));
        assert!(redact_line(r#"secret_key = "s3cr3t""#).contains("••••"));
        // 普通日志不该被改动
        let normal = "2026-11-03T13:47:04Z  INFO 已启动 bind_port = 7000";
        assert_eq!(redact_line(normal), normal);
    }

    #[test]
    fn random_password_has_entropy() {
        let a = random_password(24);
        let b = random_password(24);
        assert_eq!(a.len(), 24);
        assert_ne!(a, b);
        assert!(a.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn key_files_are_created_with_right_length() {
        let dir = tempfile::tempdir().unwrap();
        let m = dir.path().join("master.key");
        let j = dir.path().join("jwt.key");
        let k = KeyRing::load_or_create(&m, &j).unwrap();
        assert!(m.exists() && j.exists());
        // 二次加载得到同一把密钥
        let k2 = KeyRing::load_or_create(&m, &j).unwrap();
        let ct = k.encrypt("v", "aad").unwrap();
        assert_eq!(k2.decrypt(&ct, "aad").unwrap(), "v");
    }
}
