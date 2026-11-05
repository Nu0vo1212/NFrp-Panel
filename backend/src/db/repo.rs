//! 仓储层：每个聚合一组函数，SQL 只出现在这里。

use std::str::FromStr;

use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::db::models::*;
use crate::domain::*;
use crate::error::ApiError;

fn uuid_of(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::Internal(anyhow::anyhow!("非法 UUID: {s}")))
}

// ---------------------------------------------------------------------------
// users
// ---------------------------------------------------------------------------

pub async fn count_users(pool: &SqlitePool) -> Result<i64, ApiError> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

pub async fn insert_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
    role: Role,
) -> Result<UserRow, ApiError> {
    let id = Uuid::new_v4();
    let now = crate::db::now_str();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, created_at) VALUES (?,?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(username)
    .bind(password_hash)
    .bind(role.as_str())
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(UserRow {
        id,
        username: username.to_string(),
        password_hash: password_hash.to_string(),
        role,
        created_at: now,
        last_login_at: None,
    })
}

fn user_from_row(r: &sqlx::sqlite::SqliteRow) -> UserRow {
    UserRow {
        id: Uuid::parse_str(r.get::<String, _>("id").as_str()).unwrap_or_default(),
        username: r.get("username"),
        password_hash: r.get("password_hash"),
        role: Role::parse(&r.get::<String, _>("role")),
        created_at: r.get("created_at"),
        last_login_at: r.get("last_login_at"),
    }
}

pub async fn find_user(pool: &SqlitePool, username: &str) -> Result<Option<UserRow>, ApiError> {
    let row = sqlx::query("SELECT * FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(user_from_row))
}

pub async fn find_user_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<UserRow>, ApiError> {
    let row = sqlx::query("SELECT * FROM users WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(user_from_row))
}

pub async fn list_users(pool: &SqlitePool) -> Result<Vec<UserRow>, ApiError> {
    let rows = sqlx::query("SELECT * FROM users ORDER BY created_at")
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(user_from_row).collect())
}

pub async fn touch_login(pool: &SqlitePool, id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
        .bind(crate::db::now_str())
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_password(pool: &SqlitePool, id: Uuid, hash: &str) -> Result<(), ApiError> {
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// credentials
// ---------------------------------------------------------------------------

pub async fn insert_credential(
    pool: &SqlitePool,
    name: &str,
    kind: CredentialKind,
    username: Option<&str>,
    secret_enc: &str,
    passphrase_enc: Option<&str>,
    algorithm_hint: Option<&str>,
) -> Result<CredentialRow, ApiError> {
    insert_credential_with_id(
        pool,
        Uuid::new_v4(),
        name,
        kind,
        username,
        secret_enc,
        passphrase_enc,
        algorithm_hint,
    )
    .await
}

/// 指定 ID 插入。
///
/// ★ 存在这个版本是因为**加密的 AAD 绑定了记录 ID**：
///   必须先确定 ID 才能生成密文，否则密文无法与记录对应。
#[allow(clippy::too_many_arguments)]
pub async fn insert_credential_with_id(
    pool: &SqlitePool,
    id: Uuid,
    name: &str,
    kind: CredentialKind,
    username: Option<&str>,
    secret_enc: &str,
    passphrase_enc: Option<&str>,
    algorithm_hint: Option<&str>,
) -> Result<CredentialRow, ApiError> {
    let now = crate::db::now_str();
    sqlx::query(
        "INSERT INTO credentials (id, name, kind, username, secret_enc, passphrase_enc, algorithm_hint, created_at)
         VALUES (?,?,?,?,?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(kind.as_str())
    .bind(username)
    .bind(secret_enc)
    .bind(passphrase_enc)
    .bind(algorithm_hint)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            ApiError::Conflict(format!("凭据名称 [{name}] 已存在"))
        }
        other => ApiError::from(other),
    })?;
    Ok(CredentialRow {
        id,
        name: name.to_string(),
        kind,
        username: username.map(str::to_string),
        secret_enc: secret_enc.to_string(),
        passphrase_enc: passphrase_enc.map(str::to_string),
        algorithm_hint: algorithm_hint.map(str::to_string),
        created_at: now,
        last_used_at: None,
    })
}

fn credential_from_row(r: &sqlx::sqlite::SqliteRow) -> CredentialRow {
    CredentialRow {
        id: uuid_of(&r.get::<String, _>("id")).unwrap_or_default(),
        name: r.get("name"),
        kind: CredentialKind::parse(&r.get::<String, _>("kind")).unwrap_or(CredentialKind::Token),
        username: r.get("username"),
        secret_enc: r.get("secret_enc"),
        passphrase_enc: r.get("passphrase_enc"),
        algorithm_hint: r.get("algorithm_hint"),
        created_at: r.get("created_at"),
        last_used_at: r.get("last_used_at"),
    }
}

pub async fn list_credentials(pool: &SqlitePool) -> Result<Vec<CredentialRow>, ApiError> {
    let rows = sqlx::query("SELECT * FROM credentials ORDER BY created_at DESC")
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(credential_from_row).collect())
}

pub async fn get_credential(pool: &SqlitePool, id: Uuid) -> Result<CredentialRow, ApiError> {
    let row = sqlx::query("SELECT * FROM credentials WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    row.as_ref()
        .map(credential_from_row)
        .ok_or_else(|| ApiError::not_found("凭据不存在"))
}

pub async fn delete_credential(pool: &SqlitePool, id: Uuid) -> Result<(), ApiError> {
    let res = sqlx::query("DELETE FROM credentials WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::not_found("凭据不存在"));
    }
    Ok(())
}

pub async fn touch_credential(pool: &SqlitePool, id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE credentials SET last_used_at = ? WHERE id = ?")
        .bind(crate::db::now_str())
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// instances
// ---------------------------------------------------------------------------

fn instance_from_row(r: &sqlx::sqlite::SqliteRow) -> Result<InstanceRow, ApiError> {
    let tags: Vec<String> = serde_json::from_str(&r.get::<String, _>("tags")).unwrap_or_default();
    let connection: Connection = serde_json::from_str(&r.get::<String, _>("connection"))
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("connection 解析失败：{e}")))?;
    let paths: InstancePaths = serde_json::from_str(&r.get::<String, _>("paths"))
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("paths 解析失败：{e}")))?;
    let caps: Option<Capabilities> = r
        .get::<Option<String>, _>("capabilities")
        .and_then(|s| serde_json::from_str(&s).ok());
    Ok(InstanceRow {
        id: uuid_of(&r.get::<String, _>("id"))?,
        name: r.get("name"),
        role: InstanceRole::parse(&r.get::<String, _>("role")).unwrap_or(InstanceRole::Server),
        driver: DriverKind::parse(&r.get::<String, _>("driver")).unwrap_or(DriverKind::Binary),
        flavor: BinaryFlavor::parse(&r.get::<String, _>("flavor")).unwrap_or(BinaryFlavor::Nfrp),
        enabled: r.get::<i64, _>("enabled") != 0,
        tags,
        connection,
        paths,
        service_mode: ServiceMode::parse(&r.get::<String, _>("service_mode"))
            .unwrap_or(ServiceMode::Process),
        service_unit: r.get("service_unit"),
        admin_api_base_url: r.get("admin_api_base_url"),
        admin_api_user: r.get("admin_api_user"),
        admin_api_password_enc: r.get("admin_api_password_enc"),
        admin_api_prometheus_url: r.get("admin_api_prometheus_url"),
        admin_api_enabled: r.get::<i64, _>("admin_api_enabled") != 0,
        binary_version: r.get("binary_version"),
        deployment_status: DeploymentStatus::parse(&r.get::<String, _>("deployment_status"))
            .unwrap_or(DeploymentStatus::NotDeployed),
        capabilities: caps,
        last_deployed_at: r.get("last_deployed_at"),
        last_error: r.get("last_error"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    })
}

pub async fn list_instances(pool: &SqlitePool) -> Result<Vec<InstanceRow>, ApiError> {
    let rows = sqlx::query("SELECT * FROM instances ORDER BY created_at")
        .fetch_all(pool)
        .await?;
    rows.iter().map(instance_from_row).collect()
}

pub async fn get_instance(pool: &SqlitePool, id: Uuid) -> Result<InstanceRow, ApiError> {
    let row = sqlx::query("SELECT * FROM instances WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    match row {
        Some(r) => instance_from_row(&r),
        None => Err(ApiError::not_found("实例不存在")),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_instance(
    pool: &SqlitePool,
    name: &str,
    role: InstanceRole,
    driver: DriverKind,
    flavor: BinaryFlavor,
    tags: &[String],
    connection: &Connection,
    paths: &InstancePaths,
    service_mode: ServiceMode,
    service_unit: Option<&str>,
    admin_base: &str,
    admin_user: &str,
    admin_pw_enc: Option<&str>,
    prometheus_url: Option<&str>,
) -> Result<Uuid, ApiError> {
    let id = Uuid::new_v4();
    let now = crate::db::now_str();
    sqlx::query(
        "INSERT INTO instances
         (id,name,role,driver,flavor,enabled,tags,connection,paths,service_mode,service_unit,
          admin_api_base_url,admin_api_user,admin_api_password_enc,admin_api_prometheus_url,
          admin_api_enabled,deployment_status,created_at,updated_at)
         VALUES (?,?,?,?,?,1,?,?,?,?,?,?,?,?,?,1,'not_deployed',?,?)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(role.as_str())
    .bind(driver.as_str())
    .bind(flavor.as_str())
    .bind(serde_json::to_string(tags)?)
    .bind(serde_json::to_string(connection)?)
    .bind(serde_json::to_string(paths)?)
    .bind(service_mode.as_str())
    .bind(service_unit)
    .bind(admin_base)
    .bind(admin_user)
    .bind(admin_pw_enc)
    .bind(prometheus_url)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            ApiError::Conflict(format!("实例名称 [{name}] 已存在"))
        }
        other => ApiError::from(other),
    })?;
    Ok(id)
}

/// 实例更新：只更新传入的字段（None 表示不改）。
#[derive(Default)]
pub struct InstanceUpdate {
    pub name: Option<String>,
    pub role: Option<InstanceRole>,
    pub driver: Option<DriverKind>,
    pub flavor: Option<BinaryFlavor>,
    pub enabled: Option<bool>,
    pub tags: Option<Vec<String>>,
    pub connection: Option<Connection>,
    pub paths: Option<InstancePaths>,
    pub service_mode: Option<ServiceMode>,
    pub service_unit: Option<Option<String>>,
    pub admin_api_base_url: Option<String>,
    pub admin_api_user: Option<String>,
    /// `Some(None)` 表示清空密码
    pub admin_api_password_enc: Option<Option<String>>,
    pub admin_api_prometheus_url: Option<Option<String>>,
    pub admin_api_enabled: Option<bool>,
    pub binary_version: Option<Option<String>>,
    pub deployment_status: Option<DeploymentStatus>,
    pub capabilities: Option<Option<Capabilities>>,
    pub last_deployed_at: Option<Option<String>>,
    pub last_error: Option<Option<String>>,
}

pub async fn update_instance(
    pool: &SqlitePool,
    id: Uuid,
    up: InstanceUpdate,
) -> Result<(), ApiError> {
    // 读-改-写整行，避免拼出巨长的动态 SQL。
    let mut row = get_instance(pool, id).await?;

    if let Some(v) = up.name {
        row.name = v;
    }
    if let Some(v) = up.role {
        row.role = v;
    }
    if let Some(v) = up.driver {
        row.driver = v;
    }
    if let Some(v) = up.flavor {
        row.flavor = v;
    }
    if let Some(v) = up.enabled {
        row.enabled = v;
    }
    if let Some(v) = up.tags {
        row.tags = v;
    }
    if let Some(v) = up.connection {
        row.connection = v;
    }
    if let Some(v) = up.paths {
        row.paths = v;
    }
    if let Some(v) = up.service_mode {
        row.service_mode = v;
    }
    if let Some(v) = up.service_unit {
        row.service_unit = v;
    }
    if let Some(v) = up.admin_api_base_url {
        row.admin_api_base_url = v;
    }
    if let Some(v) = up.admin_api_user {
        row.admin_api_user = v;
    }
    if let Some(v) = up.admin_api_password_enc {
        row.admin_api_password_enc = v;
    }
    if let Some(v) = up.admin_api_prometheus_url {
        row.admin_api_prometheus_url = v;
    }
    if let Some(v) = up.admin_api_enabled {
        row.admin_api_enabled = v;
    }
    if let Some(v) = up.binary_version {
        row.binary_version = v;
    }
    if let Some(v) = up.deployment_status {
        row.deployment_status = v;
    }
    if let Some(v) = up.capabilities {
        row.capabilities = v;
    }
    if let Some(v) = up.last_deployed_at {
        row.last_deployed_at = v;
    }
    if let Some(v) = up.last_error {
        row.last_error = v;
    }

    let now = crate::db::now_str();
    sqlx::query(
        "UPDATE instances SET name=?, role=?, driver=?, flavor=?, enabled=?, tags=?, connection=?,
           paths=?, service_mode=?, service_unit=?, admin_api_base_url=?, admin_api_user=?,
           admin_api_password_enc=?, admin_api_prometheus_url=?, admin_api_enabled=?,
           binary_version=?, deployment_status=?, capabilities=?, last_deployed_at=?,
           last_error=?, updated_at=?
         WHERE id=?",
    )
    .bind(&row.name)
    .bind(row.role.as_str())
    .bind(row.driver.as_str())
    .bind(row.flavor.as_str())
    .bind(row.enabled as i64)
    .bind(serde_json::to_string(&row.tags)?)
    .bind(serde_json::to_string(&row.connection)?)
    .bind(serde_json::to_string(&row.paths)?)
    .bind(row.service_mode.as_str())
    .bind(&row.service_unit)
    .bind(&row.admin_api_base_url)
    .bind(&row.admin_api_user)
    .bind(&row.admin_api_password_enc)
    .bind(&row.admin_api_prometheus_url)
    .bind(row.admin_api_enabled as i64)
    .bind(&row.binary_version)
    .bind(row.deployment_status.as_str())
    .bind(
        row.capabilities
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
    )
    .bind(&row.last_deployed_at)
    .bind(&row.last_error)
    .bind(&now)
    .bind(id.to_string())
    .execute(pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            ApiError::Conflict(format!("实例名称 [{}] 已存在", row.name))
        }
        other => ApiError::from(other),
    })?;
    Ok(())
}

pub async fn delete_instance(pool: &SqlitePool, id: Uuid) -> Result<(), ApiError> {
    let res = sqlx::query("DELETE FROM instances WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::not_found("实例不存在"));
    }
    let _ = sqlx::query("DELETE FROM metric_samples WHERE instance_id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await;
    Ok(())
}

// ---------------------------------------------------------------------------
// host keys
// ---------------------------------------------------------------------------

pub async fn list_host_keys(pool: &SqlitePool) -> Result<Vec<HostKeyRow>, ApiError> {
    let rows = sqlx::query("SELECT * FROM host_keys ORDER BY trusted_at DESC")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| HostKeyRow {
            id: uuid_of(&r.get::<String, _>("id")).unwrap_or_default(),
            host: r.get("host"),
            port: r.get::<i64, _>("port") as u16,
            algorithm: r.get("algorithm"),
            fingerprint: r.get("fingerprint"),
            public_key: r.get("public_key"),
            trusted_at: r.get("trusted_at"),
        })
        .collect())
}

pub async fn find_host_key(
    pool: &SqlitePool,
    host: &str,
    port: u16,
    algorithm: &str,
) -> Result<Option<HostKeyRow>, ApiError> {
    let row = sqlx::query("SELECT * FROM host_keys WHERE host=? AND port=? AND algorithm=?")
        .bind(host)
        .bind(port as i64)
        .bind(algorithm)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| HostKeyRow {
        id: uuid_of(&r.get::<String, _>("id")).unwrap_or_default(),
        host: r.get("host"),
        port: r.get::<i64, _>("port") as u16,
        algorithm: r.get("algorithm"),
        fingerprint: r.get("fingerprint"),
        public_key: r.get("public_key"),
        trusted_at: r.get("trusted_at"),
    }))
}

pub async fn upsert_host_key(
    pool: &SqlitePool,
    host: &str,
    port: u16,
    algorithm: &str,
    fingerprint: &str,
    public_key: &str,
) -> Result<HostKeyRow, ApiError> {
    let now = crate::db::now_str();
    sqlx::query(
        "INSERT INTO host_keys (id,host,port,algorithm,fingerprint,public_key,trusted_at)
         VALUES (?,?,?,?,?,?,?)
         ON CONFLICT(host,port,algorithm) DO UPDATE SET
           fingerprint=excluded.fingerprint, public_key=excluded.public_key, trusted_at=excluded.trusted_at",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(host)
    .bind(port as i64)
    .bind(algorithm)
    .bind(fingerprint)
    .bind(public_key)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(HostKeyRow {
        id: Uuid::new_v4(),
        host: host.to_string(),
        port,
        algorithm: algorithm.to_string(),
        fingerprint: fingerprint.to_string(),
        public_key: public_key.to_string(),
        trusted_at: now,
    })
}

pub async fn delete_host_key(pool: &SqlitePool, id: Uuid) -> Result<(), ApiError> {
    let res = sqlx::query("DELETE FROM host_keys WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::not_found("主机密钥不存在"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// audit
// ---------------------------------------------------------------------------

pub struct AuditInsert<'a> {
    pub actor: &'a str,
    pub actor_ip: &'a str,
    pub action: &'a str,
    pub target: &'a str,
    pub result: AuditResult,
    pub duration_ms: i64,
    pub detail: serde_json::Value,
}

pub async fn insert_audit(pool: &SqlitePool, a: AuditInsert<'_>) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO audit_logs (id,ts,actor,actor_ip,action,target,result,duration_ms,detail)
         VALUES (?,?,?,?,?,?,?,?,?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(crate::db::now_str())
    .bind(a.actor)
    .bind(a.actor_ip)
    .bind(a.action)
    .bind(a.target)
    .bind(a.result.as_str())
    .bind(a.duration_ms)
    .bind(serde_json::to_string(&a.detail)?)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Default, Clone)]
pub struct AuditFilter {
    pub action: Option<String>,
    pub result: Option<String>,
    pub actor: Option<String>,
    pub target: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub q: Option<String>,
}

pub async fn list_audit(
    pool: &SqlitePool,
    f: &AuditFilter,
    page: i64,
    page_size: i64,
) -> Result<(Vec<AuditRow>, i64), ApiError> {
    let mut where_sql = String::from(" WHERE 1=1");
    if let Some(v) = &f.action {
        where_sql.push_str(&format!(" AND action LIKE '%{}%'", v.replace('\'', "")));
    }
    if let Some(v) = &f.result {
        where_sql.push_str(&format!(" AND result = '{}'", v.replace('\'', "")));
    }
    if let Some(v) = &f.actor {
        where_sql.push_str(&format!(" AND actor LIKE '%{}%'", v.replace('\'', "")));
    }
    if let Some(v) = &f.target {
        where_sql.push_str(&format!(" AND target LIKE '%{}%'", v.replace('\'', "")));
    }
    if let Some(v) = &f.from {
        where_sql.push_str(&format!(" AND ts >= '{}'", v.replace('\'', "")));
    }
    if let Some(v) = &f.to {
        where_sql.push_str(&format!(" AND ts <= '{}'", v.replace('\'', "")));
    }
    if let Some(v) = &f.q {
        where_sql.push_str(&format!(
            " AND (detail LIKE '%{}%' OR action LIKE '%{}%' OR target LIKE '%{}%')",
            v.replace('\'', ""),
            v.replace('\'', ""),
            v.replace('\'', "")
        ));
    }

    let total: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM audit_logs{where_sql}"))
        .fetch_one(pool)
        .await?;

    let sql = format!(
        "SELECT * FROM audit_logs{where_sql} ORDER BY ts DESC LIMIT ? OFFSET ?"
    );
    let rows = sqlx::query(&sql)
        .bind(page_size)
        .bind((page - 1).max(0) * page_size)
        .fetch_all(pool)
        .await?;

    let items = rows
        .iter()
        .map(|r| AuditRow {
            id: uuid_of(&r.get::<String, _>("id")).unwrap_or_default(),
            ts: r.get("ts"),
            actor: r.get("actor"),
            actor_ip: r.get("actor_ip"),
            action: r.get("action"),
            target: r.get("target"),
            result: AuditResult::parse(&r.get::<String, _>("result")),
            duration_ms: r.get("duration_ms"),
            detail: serde_json::from_str(&r.get::<String, _>("detail"))
                .unwrap_or(serde_json::Value::Null),
        })
        .collect();
    Ok((items, total))
}

// ---------------------------------------------------------------------------
// metrics
// ---------------------------------------------------------------------------

pub async fn insert_metric(
    pool: &SqlitePool,
    instance_id: Uuid,
    s: &MetricSample,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO metric_samples (instance_id,ts,bytes_in,bytes_out,conns,clients,proxies)
         VALUES (?,?,?,?,?,?,?)
         ON CONFLICT(instance_id,ts) DO UPDATE SET
           bytes_in=excluded.bytes_in, bytes_out=excluded.bytes_out, conns=excluded.conns,
           clients=excluded.clients, proxies=excluded.proxies",
    )
    .bind(instance_id.to_string())
    .bind(s.ts)
    .bind(s.bytes_in)
    .bind(s.bytes_out)
    .bind(s.conns)
    .bind(s.clients)
    .bind(s.proxies)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_metrics(
    pool: &SqlitePool,
    instance_id: Uuid,
    from_ts: i64,
    to_ts: i64,
    max_points: i64,
) -> Result<Vec<MetricSample>, ApiError> {
    let rows = sqlx::query(
        "SELECT ts, bytes_in, bytes_out, conns, clients, proxies FROM metric_samples
         WHERE instance_id=? AND ts BETWEEN ? AND ? ORDER BY ts ASC LIMIT ?",
    )
    .bind(instance_id.to_string())
    .bind(from_ts)
    .bind(to_ts)
    .bind(max_points)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| MetricSample {
            ts: r.get("ts"),
            bytes_in: r.get("bytes_in"),
            bytes_out: r.get("bytes_out"),
            conns: r.get("conns"),
            clients: r.get("clients"),
            proxies: r.get("proxies"),
        })
        .collect())
}

pub async fn prune_metrics(pool: &SqlitePool, older_than: i64) -> Result<u64, ApiError> {
    let res = sqlx::query("DELETE FROM metric_samples WHERE ts < ?")
        .bind(older_than)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

// ---------------------------------------------------------------------------
// config backups
// ---------------------------------------------------------------------------

pub async fn insert_backup(
    pool: &SqlitePool,
    instance_id: Uuid,
    path: &str,
    sha: &str,
    size: i64,
    label: &str,
) -> Result<ConfigBackupRow, ApiError> {
    let id = Uuid::new_v4();
    let now = crate::db::now_str();
    sqlx::query(
        "INSERT INTO config_backups (id,instance_id,path,content_sha,size,created_at,label)
         VALUES (?,?,?,?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(instance_id.to_string())
    .bind(path)
    .bind(sha)
    .bind(size)
    .bind(&now)
    .bind(label)
    .execute(pool)
    .await?;
    Ok(ConfigBackupRow {
        id,
        instance_id,
        path: path.to_string(),
        content_sha: sha.to_string(),
        size,
        created_at: now,
        label: label.to_string(),
    })
}

pub async fn list_backups(
    pool: &SqlitePool,
    instance_id: Uuid,
    limit: i64,
) -> Result<Vec<ConfigBackupRow>, ApiError> {
    let rows = sqlx::query(
        "SELECT * FROM config_backups WHERE instance_id=? ORDER BY created_at DESC LIMIT ?",
    )
    .bind(instance_id.to_string())
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| ConfigBackupRow {
            id: uuid_of(&r.get::<String, _>("id")).unwrap_or_default(),
            instance_id,
            path: r.get("path"),
            content_sha: r.get("content_sha"),
            size: r.get("size"),
            created_at: r.get("created_at"),
            label: r.get("label"),
        })
        .collect())
}

pub async fn get_backup(
    pool: &SqlitePool,
    instance_id: Uuid,
    id: Uuid,
) -> Result<ConfigBackupRow, ApiError> {
    let row = sqlx::query("SELECT * FROM config_backups WHERE id=? AND instance_id=?")
        .bind(id.to_string())
        .bind(instance_id.to_string())
        .fetch_optional(pool)
        .await?;
    match row {
        Some(r) => Ok(ConfigBackupRow {
            id,
            instance_id,
            path: r.get("path"),
            content_sha: r.get("content_sha"),
            size: r.get("size"),
            created_at: r.get("created_at"),
            label: r.get("label"),
        }),
        None => Err(ApiError::not_found("备份不存在")),
    }
}

// ---------------------------------------------------------------------------
// deploy tasks
// ---------------------------------------------------------------------------

pub async fn insert_deploy_task(
    pool: &SqlitePool,
    instance_id: Uuid,
    kind: &str,
    rollback_of: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deploy_tasks (id,instance_id,kind,state,steps,logs,rollback_of,started_at)
         VALUES (?,?,?,'pending','[]','[]',?,?)",
    )
    .bind(id.to_string())
    .bind(instance_id.to_string())
    .bind(kind)
    .bind(rollback_of.map(|u| u.to_string()))
    .bind(crate::db::now_str())
    .execute(pool)
    .await?;
    Ok(id)
}

fn deploy_from_row(r: &sqlx::sqlite::SqliteRow) -> DeployTaskRow {
    DeployTaskRow {
        id: uuid_of(&r.get::<String, _>("id")).unwrap_or_default(),
        instance_id: uuid_of(&r.get::<String, _>("instance_id")).unwrap_or_default(),
        kind: r.get("kind"),
        state: r.get("state"),
        steps: serde_json::from_str(&r.get::<String, _>("steps"))
            .unwrap_or(serde_json::Value::Array(vec![])),
        logs: serde_json::from_str(&r.get::<String, _>("logs"))
            .unwrap_or(serde_json::Value::Array(vec![])),
        error: r.get("error"),
        rollback_of: r
            .get::<Option<String>, _>("rollback_of")
            .and_then(|s| Uuid::from_str(&s).ok()),
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
    }
}

pub async fn update_deploy_task(
    pool: &SqlitePool,
    id: Uuid,
    state: &str,
    steps: &serde_json::Value,
    logs: &serde_json::Value,
    error: Option<&str>,
    finished: bool,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE deploy_tasks SET state=?, steps=?, logs=?, error=?, finished_at=? WHERE id=?",
    )
    .bind(state)
    .bind(serde_json::to_string(steps)?)
    .bind(serde_json::to_string(logs)?)
    .bind(error)
    .bind(if finished {
        Some(crate::db::now_str())
    } else {
        None
    })
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_deploy_task(pool: &SqlitePool, id: Uuid) -> Result<DeployTaskRow, ApiError> {
    let row = sqlx::query("SELECT * FROM deploy_tasks WHERE id=?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    row.as_ref()
        .map(deploy_from_row)
        .ok_or_else(|| ApiError::not_found("部署任务不存在"))
}

pub async fn latest_deploy_task(
    pool: &SqlitePool,
    instance_id: Uuid,
) -> Result<Option<DeployTaskRow>, ApiError> {
    let row = sqlx::query(
        "SELECT * FROM deploy_tasks WHERE instance_id=? ORDER BY started_at DESC LIMIT 1",
    )
    .bind(instance_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(deploy_from_row))
}

pub async fn list_deploy_tasks(
    pool: &SqlitePool,
    instance_id: Uuid,
    limit: i64,
) -> Result<Vec<DeployTaskRow>, ApiError> {
    let rows = sqlx::query(
        "SELECT * FROM deploy_tasks WHERE instance_id=? ORDER BY started_at DESC LIMIT ?",
    )
    .bind(instance_id.to_string())
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(deploy_from_row).collect())
}

// ---------------------------------------------------------------------------
// settings (key-value)
// ---------------------------------------------------------------------------

pub async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>, ApiError> {
    let v: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key=?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(v)
}

pub async fn put_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO settings (key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}
