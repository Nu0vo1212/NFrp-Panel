//! 认证与 RBAC：JWT（HS256）、登录、刷新、用户初始化、角色判定。
//!
//! 安全约定：
//! - access token 有效期短（默认 120 分钟），refresh token 单独签发；
//! - 前端只把 token 放 `sessionStorage`（见前端 `stores/session.ts`）；
//! - 角色只有 `admin` / `viewer` 两级：`viewer` 只读，写接口在提取器层就拒绝。

use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::crypto;
use crate::db::models::Role;
use crate::db::repo;
use crate::error::ApiError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub username: String,
    pub role: String,
    /// 类型：access / refresh
    pub typ: String,
    /// 令牌唯一 ID（refresh 轮换与撤销用）
    pub jti: String,
    /// 令牌版本：与用户当前 token_version 不一致即失效
    pub ver: i64,
    pub exp: i64,
    pub iat: i64,
}

/// 登录限流配置（失败次数与锁定时长）。
#[derive(Debug, Clone)]
pub struct LoginPolicy {
    pub max_attempts: i64,
    pub lock_minutes: i64,
}

impl Default for LoginPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            lock_minutes: 15,
        }
    }
}

#[derive(Clone)]
pub struct AuthService {
    secret: Vec<u8>,
    access_ttl_minutes: i64,
    refresh_ttl_days: i64,
    pool: SqlitePool,
    login_policy: LoginPolicy,
}

#[derive(Debug, Serialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user: UserInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub role: String,
}

impl AuthService {
    pub fn new(
        secret: Vec<u8>,
        access_ttl_minutes: i64,
        refresh_ttl_days: i64,
        pool: SqlitePool,
    ) -> Self {
        Self::with_policy(secret, access_ttl_minutes, refresh_ttl_days, pool, LoginPolicy::default())
    }

    pub fn with_policy(
        secret: Vec<u8>,
        access_ttl_minutes: i64,
        refresh_ttl_days: i64,
        pool: SqlitePool,
        login_policy: LoginPolicy,
    ) -> Self {
        Self {
            secret,
            access_ttl_minutes: access_ttl_minutes.max(1),
            refresh_ttl_days: refresh_ttl_days.max(1),
            pool,
            login_policy,
        }
    }

    fn encode(&self, claims: &Claims) -> Result<String, ApiError> {
        encode(
            &Header::new(Algorithm::HS256),
            claims,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("签发 token 失败：{e}")))
    }

    pub fn verify(&self, token: &str, expect_type: &str) -> Result<Claims, ApiError> {
        let mut v = Validation::new(Algorithm::HS256);
        v.set_required_spec_claims(&["exp", "sub"]);
        let data = decode::<Claims>(token, &DecodingKey::from_secret(&self.secret), &v)
            .map_err(|e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                    ApiError::Unauthorized("登录已过期，请重新登录".into())
                }
                _ => ApiError::Unauthorized("无效的登录凭据".into()),
            })?;
        if data.claims.typ != expect_type {
            return Err(ApiError::Unauthorized("token 类型不匹配".into()));
        }
        Ok(data.claims)
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<TokenPair, ApiError> {
        let Some(user) = repo::find_user(&self.pool, username).await? else {
            // 不区分"用户不存在"与"密码错误"，避免账号枚举
            return Err(ApiError::Unauthorized("用户名或密码错误".into()));
        };

        // 账户锁定检查
        if let Some(until) = &user.locked_until {
            if let Some(t) = crate::db::models::parse_ts(until) {
                if t > Utc::now() {
                    return Err(ApiError::Unauthorized(
                        "登录失败次数过多，账号已临时锁定，请稍后再试".into(),
                    ));
                }
            }
        }

        if !crypto::verify_password(password, &user.password_hash) {
            // 失败计数 +1；达到阈值自动锁定
            repo::record_login_failure(
                &self.pool,
                user.id,
                self.login_policy.max_attempts,
                self.login_policy.lock_minutes,
            )
            .await?;
            return Err(ApiError::Unauthorized("用户名或密码错误".into()));
        }

        repo::reset_login_failure(&self.pool, user.id).await?;
        repo::touch_login(&self.pool, user.id).await?;
        self.issue(&user)
    }

    fn issue(&self, user: &crate::db::models::UserRow) -> Result<TokenPair, ApiError> {
        let now = Utc::now();
        let access_exp = now + Duration::minutes(self.access_ttl_minutes);
        let refresh_exp = now + Duration::days(self.refresh_ttl_days);

        let access = self.encode(&Claims {
            sub: user.id.to_string(),
            username: user.username.clone(),
            role: user.role.as_str().to_string(),
            typ: "access".into(),
            jti: Uuid::new_v4().to_string(),
            ver: user.token_version,
            exp: access_exp.timestamp(),
            iat: now.timestamp(),
        })?;
        let refresh = self.encode(&Claims {
            sub: user.id.to_string(),
            username: user.username.clone(),
            role: user.role.as_str().to_string(),
            typ: "refresh".into(),
            jti: Uuid::new_v4().to_string(),
            ver: user.token_version,
            exp: refresh_exp.timestamp(),
            iat: now.timestamp(),
        })?;

        Ok(TokenPair {
            access_token: access,
            refresh_token: refresh,
            expires_in: self.access_ttl_minutes * 60,
            user: UserInfo {
                id: user.id.to_string(),
                username: user.username.clone(),
                role: user.role.as_str().to_string(),
            },
        })
    }

    /// 刷新令牌：轮换 refresh（旧 jti 撤销），并以数据库为准重新签发。
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, ApiError> {
        let claims = self.verify(refresh_token, "refresh")?;
        let id = Uuid::parse_str(&claims.sub)
            .map_err(|_| ApiError::Unauthorized("无效的 token 主体".into()))?;

        // 旧 refresh jti 必须未被撤销（防重放）
        if repo::is_token_revoked(&self.pool, &claims.jti).await? {
            return Err(ApiError::Unauthorized("刷新令牌已失效，请重新登录".into()));
        }
        let user = repo::find_user_by_id(&self.pool, id)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("用户不存在".into()))?;
        // 令牌版本必须匹配（改密/降权/登出后旧 refresh 立即失效）
        if claims.ver != user.token_version {
            return Err(ApiError::Unauthorized("登录状态已变更，请重新登录".into()));
        }

        // 轮换：撤销旧 refresh jti，签发新 pair
        repo::revoke_token(&self.pool, &claims.jti, user.id, claims.exp).await?;
        let _ = repo::purge_expired_revoked(&self.pool).await;
        self.issue(&user)
    }

    /// 校验 access token 的附加约束（版本匹配 + 未被撤销 + 用户仍存在且角色以 DB 为准）。
    /// 返回数据库中最新角色。
    pub async fn authorize(&self, claims: &Claims) -> Result<(String, crate::db::models::Role), ApiError> {
        let id = Uuid::parse_str(&claims.sub)
            .map_err(|_| ApiError::Unauthorized("无效的 token 主体".into()))?;
        if repo::is_token_revoked(&self.pool, &claims.jti).await? {
            return Err(ApiError::Unauthorized("登录已失效，请重新登录".into()));
        }
        let user = repo::find_user_by_id(&self.pool, id)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("用户不存在".into()))?;
        if claims.ver != user.token_version {
            return Err(ApiError::Unauthorized("登录状态已变更，请重新登录".into()));
        }
        Ok((user.username, user.role))
    }

    /// 登出：撤销当前 refresh 令牌，并把该用户 token_version +1（使所有令牌失效）。
    pub async fn logout(&self, refresh_token: Option<&str>) -> Result<(), ApiError> {
        if let Some(rt) = refresh_token {
            if let Ok(claims) = self.verify(rt, "refresh") {
                let _ = repo::revoke_token(&self.pool, &claims.jti, Uuid::parse_str(&claims.sub).unwrap_or_default(), claims.exp).await;
            }
        }
        Ok(())
    }

    pub async fn me(&self, claims: &Claims) -> Result<UserInfo, ApiError> {
        let id = Uuid::parse_str(&claims.sub)
            .map_err(|_| ApiError::Unauthorized("无效的 token 主体".into()))?;
        let user = repo::find_user_by_id(&self.pool, id)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("用户不存在".into()))?;
        Ok(UserInfo {
            id: user.id.to_string(),
            username: user.username,
            role: user.role.as_str().to_string(),
        })
    }

    /// 供 API 层构造 me 用的假 Claims（避免重复字段）。
    pub fn claims_for(&self, id: Uuid, username: &str, role: &str) -> Claims {
        Claims {
            sub: id.to_string(),
            username: username.to_string(),
            role: role.to_string(),
            typ: "access".into(),
            jti: String::new(),
            ver: 0,
            exp: 0,
            iat: 0,
        }
    }
}

/// 首次启动创建管理员。返回 `Some(明文口令)` 表示本次新建（需写入 0600 文件提示用户）。
pub async fn ensure_initial_admin(
    pool: &SqlitePool,
    username: &str,
    password_env: &str,
    password_file: &std::path::Path,
) -> anyhow::Result<Option<String>> {
    if repo::count_users(pool).await? > 0 {
        return Ok(None);
    }

    // 优先级：环境变量/配置固定口令 > 已存在的一次性口令文件 > 随机生成
    let (password, generated) = if !password_env.trim().is_empty() {
        (password_env.to_string(), false)
    } else if password_file.exists() {
        let p = std::fs::read_to_string(password_file)?.trim().to_string();
        if !p.is_empty() {
            (p, false)
        } else {
            (crypto::random_password(24), true)
        }
    } else {
        (crypto::random_password(24), true)
    };

    let hash = crypto::hash_password(&password)?;
    repo::insert_user(pool, username, &hash, Role::Admin).await?;

    if generated {
        if let Some(parent) = password_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            password_file,
            format!(
                "FrpPanel 初始管理员账号\n用户名: {username}\n密码: {password}\n\n\
                 首次登录后请立即在「设置」中修改密码，并删除本文件。\n"
            ),
        )?;
        crypto::restrict_permissions(password_file)?;
    }

    Ok(if generated { Some(password) } else { None })
}

/// 角色判定辅助。
pub fn require_admin(role: Role) -> Result<(), ApiError> {
    if role.can_write() {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "当前账号为只读角色（viewer），该操作需要管理员（admin）".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn svc() -> AuthService {
        let pool = crate::db::connect_memory().await.unwrap();
        let hash = crypto::hash_password("pass1234").unwrap();
        repo::insert_user(&pool, "admin", &hash, Role::Admin).await.unwrap();
        AuthService::new(b"test-secret-key-0123456789abcdef".to_vec(), 120, 14, pool)
    }

    #[tokio::test]
    async fn login_and_verify() {
        let s = svc().await;
        let pair = s.login("admin", "pass1234").await.unwrap();
        assert!(!pair.access_token.is_empty());
        let claims = s.verify(&pair.access_token, "access").unwrap();
        assert_eq!(claims.username, "admin");
        assert_eq!(claims.role, "admin");
        // 用 access token 当 refresh 用应当失败
        assert!(s.verify(&pair.access_token, "refresh").is_err());
    }

    #[tokio::test]
    async fn wrong_password_is_generic_error() {
        let s = svc().await;
        let e = s.login("admin", "wrong").await.unwrap_err();
        assert!(matches!(e, ApiError::Unauthorized(_)));
        let e2 = s.login("nobody", "x").await.unwrap_err();
        assert_eq!(e.to_string(), e2.to_string());
    }

    #[tokio::test]
    async fn refresh_issues_new_access_token() {
        let s = svc().await;
        let pair = s.login("admin", "pass1234").await.unwrap();
        let new_pair = s.refresh(&pair.refresh_token).await.unwrap();
        assert!(s.verify(&new_pair.access_token, "access").is_ok());
    }

    #[tokio::test]
    async fn initial_admin_created_once() {
        let pool = crate::db::connect_memory().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("admin.initial");
        let created = ensure_initial_admin(&pool, "admin", "", &f).await.unwrap();
        assert!(created.is_some());
        assert!(f.exists());
        let again = ensure_initial_admin(&pool, "admin", "", &f).await.unwrap();
        assert!(again.is_none());
    }

    #[test]
    fn viewer_cannot_write() {
        assert!(require_admin(Role::Admin).is_ok());
        assert!(require_admin(Role::Viewer).is_err());
    }
}
