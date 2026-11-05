//! 主机密钥：指纹计算、known_hosts 查询与信任登记。
//!
//! 规则（需求第七节）：
//! - 首次连接**必须**展示指纹并要求人工确认；
//! - 指纹与已保存记录不同时拒绝连接（防止中间人），必须由管理员先删除旧记录再重新信任；
//! - 没有"默认关闭校验"的开关。

use std::sync::OnceLock;

use russh::keys::ssh_key::{HashAlg, PublicKey};
use sqlx::SqlitePool;

use crate::db::repo;
use crate::error::ApiError;
use crate::ssh::session::{Phase, SshError};

/// 进程内 known_hosts 缓存（避免每次握手都查库）。
fn cache() -> &'static tokio::sync::Mutex<std::collections::HashMap<(String, u16, String), String>> {
    static CACHE: OnceLock<
        tokio::sync::Mutex<std::collections::HashMap<(String, u16, String), String>>,
    > = OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

static GLOBAL_DB: OnceLock<SqlitePool> = OnceLock::new();

/// 由 `AppState` 初始化时注入数据库句柄（用于主机密钥校验回调）。
pub fn init(db: SqlitePool) {
    let _ = GLOBAL_DB.set(db);
}

/// `SHA256:base64` 形式的指纹（与 OpenSSH `ssh-keygen -lf` 一致）。
pub fn fingerprint_of(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// 查询已知主机；返回 `Ok(true)` 表示指纹一致，`Ok(false)` 表示未登记或不一致。
pub async fn lookup_known(host: &str, port: u16, fingerprint: &str) -> Result<bool, SshError> {
    // 先查缓存
    {
        let c = cache().lock().await;
        for ((h, p, alg), fp) in c.iter() {
            if h == host && *p == port {
                let _ = alg;
                return Ok(fp == fingerprint);
            }
        }
    }

    let Some(db) = GLOBAL_DB.get() else {
        return Err(SshError::new(
            Phase::HostKey,
            "面板尚未初始化主机密钥存储（内部错误）",
        ));
    };

    let rows = repo::list_host_keys(db)
        .await
        .map_err(|e| SshError::new(Phase::HostKey, format!("读取 known_hosts 失败：{e}")))?;

    let mut c = cache().lock().await;
    for r in &rows {
        c.insert(
            (r.host.clone(), r.port, r.algorithm.clone()),
            r.fingerprint.clone(),
        );
    }

    let matched = rows
        .iter()
        .filter(|r| r.host == host && r.port == port)
        .collect::<Vec<_>>();

    if matched.is_empty() {
        return Ok(false);
    }
    Ok(matched.iter().any(|r| r.fingerprint == fingerprint))
}

/// 信任一个主机密钥（写入数据库 + 刷新缓存）。
pub async fn trust(
    db: &SqlitePool,
    host: &str,
    port: u16,
    algorithm: &str,
    fingerprint: &str,
    public_key: &str,
) -> Result<(), ApiError> {
    repo::upsert_host_key(db, host, port, algorithm, fingerprint, public_key).await?;
    let mut c = cache().lock().await;
    c.insert(
        (host.to_string(), port, algorithm.to_string()),
        fingerprint.to_string(),
    );
    Ok(())
}

/// 删除某主机的全部记录（指纹变更后需要重新信任）。
pub async fn forget(db: &SqlitePool, host: &str, port: u16) -> Result<usize, ApiError> {
    let rows = repo::list_host_keys(db).await?;
    let mut n = 0;
    for r in rows.into_iter().filter(|r| r.host == host && r.port == port) {
        repo::delete_host_key(db, r.id).await?;
        n += 1;
    }
    let mut c = cache().lock().await;
    c.retain(|(h, p, _), _| !(h == host && *p == port));
    Ok(n)
}

/// 清空缓存（测试用）。
pub async fn clear_cache() {
    cache().lock().await.clear();
}

/// 只完成 SSH 握手、**不做认证**，取回服务端主机密钥指纹。
///
/// 用途：向导里"首次连接需确认指纹"这一步——在把密码发出去之前，
/// 先把指纹展示给用户，由用户决定是否信任。
pub async fn probe_fingerprint(
    params: &crate::ssh::SshConnectParams,
    cfg: &crate::config::RuntimeConfig,
) -> Option<(String, String)> {
    use std::sync::Arc;
    use std::time::Duration;

    struct AcceptAll(Arc<tokio::sync::Mutex<Option<(String, String)>>>);

    impl russh::client::Handler for AcceptAll {
        type Error = russh::Error;
        async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
            *self.0.lock().await = Some((key.algorithm().to_string(), fingerprint_of(key)));
            // 返回 false：握手到此结束，绝不认证、绝不执行任何命令
            Ok(false)
        }
    }

    let seen = Arc::new(tokio::sync::Mutex::new(None));
    let ssh_config = Arc::new(russh::client::Config {
        inactivity_timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    });
    let addr = format!("{}:{}", params.host, params.port);
    let timeout = Duration::from_millis(cfg.raw.ssh.connect_timeout_ms);
    let connect = russh::client::connect(ssh_config, addr, AcceptAll(seen.clone()));
    let _ = tokio::time::timeout(timeout, connect).await;
    let out = seen.lock().await.clone();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::PrivateKey;

    #[test]
    fn fingerprint_matches_openssh_format() {
        let key = PrivateKey::random(&mut russh::keys::key::safe_rng(), Default::default())
            .unwrap();
        let fp = fingerprint_of(&key.public_key().clone());
        assert!(fp.starts_with("SHA256:"), "{fp}");
        assert!(!fp.contains('='));
    }

    #[tokio::test]
    async fn lookup_and_trust_roundtrip() {
        let db = crate::db::connect_memory().await.unwrap();
        init(db.clone());
        clear_cache().await;
        assert!(!lookup_known("1.2.3.4", 22, "SHA256:aaa").await.unwrap());
        trust(&db, "1.2.3.4", 22, "ssh-ed25519", "SHA256:aaa", "AAAA")
            .await
            .unwrap();
        clear_cache().await;
        assert!(lookup_known("1.2.3.4", 22, "SHA256:aaa").await.unwrap());
        // 指纹变更：拒绝
        assert!(!lookup_known("1.2.3.4", 22, "SHA256:bbb").await.unwrap());
        // 其它端口不影响
        assert!(!lookup_known("1.2.3.4", 2222, "SHA256:aaa").await.unwrap());
        let n = forget(&db, "1.2.3.4", 22).await.unwrap();
        assert_eq!(n, 1);
        clear_cache().await;
        assert!(!lookup_known("1.2.3.4", 22, "SHA256:aaa").await.unwrap());
    }
}
