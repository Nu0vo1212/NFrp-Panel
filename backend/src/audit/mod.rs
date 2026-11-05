//! 审计日志：写入前统一脱敏，读取时支持过滤与分页。

use serde::Serialize;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::db::models::{AuditResult, AuditRow};
use crate::db::repo;
use crate::error::ApiError;

/// 常用动作常量（前端筛选与文档共用）。
pub mod action {
    pub const AUTH_LOGIN: &str = "auth.login";
    pub const AUTH_LOGIN_FAILED: &str = "auth.login_failed";
    pub const AUTH_LOGOUT: &str = "auth.logout";
    pub const INSTANCE_CREATE: &str = "instance.create";
    pub const INSTANCE_UPDATE: &str = "instance.update";
    pub const INSTANCE_DELETE: &str = "instance.delete";
    pub const INSTANCE_START: &str = "instance.start";
    pub const INSTANCE_STOP: &str = "instance.stop";
    pub const INSTANCE_RESTART: &str = "instance.restart";
    pub const INSTANCE_RELOAD: &str = "instance.reload";
    pub const CONFIG_WRITE: &str = "config.write";
    pub const CONFIG_RESTORE: &str = "config.restore";
    pub const PROXY_CREATE: &str = "proxy.create";
    pub const PROXY_UPDATE: &str = "proxy.update";
    pub const PROXY_DELETE: &str = "proxy.delete";
    pub const CREDENTIAL_CREATE: &str = "credential.create";
    pub const CREDENTIAL_DELETE: &str = "credential.delete";
    pub const HOSTKEY_TRUST: &str = "ssh.host_key_trust";
    pub const HOSTKEY_DELETE: &str = "ssh.host_key_delete";
    pub const SSH_TEST: &str = "ssh.test";
    pub const DEPLOY_RUN: &str = "deploy.run";
    pub const DEPLOY_ROLLBACK: &str = "deploy.rollback";
    pub const SETTINGS_UPDATE: &str = "settings.update";
}

/// 审计写入器。
#[derive(Clone)]
pub struct AuditWriter {
    pool: SqlitePool,
    enabled: bool,
}

#[derive(Debug, Clone)]
pub struct Actor {
    pub username: String,
    pub ip: String,
}

impl Actor {
    pub fn anonymous(ip: impl Into<String>) -> Self {
        Self {
            username: "anonymous".into(),
            ip: ip.into(),
        }
    }
}

impl AuditWriter {
    pub fn new(pool: SqlitePool, enabled: bool) -> Self {
        Self { pool, enabled }
    }

    /// 记录一条审计。**失败不应影响主流程**，因此返回 `()` 并只记日志。
    pub async fn record(
        &self,
        actor: &Actor,
        action: &str,
        target: &str,
        result: AuditResult,
        duration_ms: i64,
        detail: serde_json::Value,
    ) {
        if !self.enabled {
            return;
        }
        let safe_detail = redact_json(&detail);
        if let Err(e) = repo::insert_audit(
            &self.pool,
            repo::AuditInsert {
                actor: &actor.username,
                actor_ip: &actor.ip,
                action,
                target,
                result,
                duration_ms,
                detail: safe_detail,
            },
        )
        .await
        {
            tracing::warn!(error = %e, action, "写审计日志失败");
        }
    }

    pub async fn success(&self, actor: &Actor, action: &str, target: &str, detail: serde_json::Value) {
        self.record(actor, action, target, AuditResult::Success, 0, detail)
            .await;
    }

    pub async fn failure(
        &self,
        actor: &Actor,
        action: &str,
        target: &str,
        err: &str,
        duration_ms: i64,
    ) {
        self.record(
            actor,
            action,
            target,
            AuditResult::Failure,
            duration_ms,
            serde_json::json!({ "error": err }),
        )
        .await;
    }

    pub async fn denied(&self, actor: &Actor, action: &str, target: &str, reason: &str) {
        self.record(
            actor,
            action,
            target,
            AuditResult::Denied,
            0,
            serde_json::json!({ "reason": reason }),
        )
        .await;
    }

    pub async fn list(
        &self,
        filter: &repo::AuditFilter,
        page: i64,
        page_size: i64,
    ) -> Result<(Vec<AuditView>, i64), ApiError> {
        let (rows, total) = repo::list_audit(&self.pool, filter, page, page_size).await?;
        Ok((rows.into_iter().map(AuditView::from_row).collect(), total))
    }

    pub async fn export_csv(&self, filter: &repo::AuditFilter) -> Result<String, ApiError> {
        let (rows, _) = repo::list_audit(&self.pool, filter, 1, 5000).await?;
        let mut out = String::from("时间,操作者,来源IP,动作,目标,结果,耗时ms,详情\n");
        for r in rows {
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                r.ts,
                csv_escape(&r.actor),
                csv_escape(&r.actor_ip),
                csv_escape(&r.action),
                csv_escape(&r.target),
                r.result.as_str(),
                r.duration_ms,
                csv_escape(&r.detail.to_string())
            ));
        }
        Ok(out)
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditView {
    pub id: String,
    pub ts: String,
    pub actor: String,
    pub actor_ip: String,
    pub action: String,
    pub target: String,
    pub result: String,
    pub duration_ms: i64,
    pub detail: serde_json::Value,
}

impl AuditView {
    fn from_row(r: AuditRow) -> Self {
        Self {
            id: r.id.to_string(),
            ts: r.ts,
            actor: r.actor,
            actor_ip: r.actor_ip,
            action: r.action,
            target: r.target,
            result: r.result.as_str().to_string(),
            duration_ms: r.duration_ms,
            detail: r.detail,
        }
    }
}

/// 递归脱敏 JSON：敏感键值替换为掩码，字符串内容走 `redact_line`。
pub fn redact_json(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                if crate::crypto::is_sensitive_key(k) {
                    out.insert(k.clone(), Value::String("••••••••".into()));
                } else {
                    out.insert(k.clone(), redact_json(val));
                }
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(redact_json).collect()),
        Value::String(s) => Value::String(crate::crypto::redact_line(s)),
        other => other.clone(),
    }
}

/// 测试辅助：把 UUID 转字符串。
pub fn id_str(id: Uuid) -> String {
    id.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_nested_sensitive_keys() {
        let v = json!({
            "name": "web",
            "token": "abc123",
            "nested": { "password": "p", "note": "ok" },
            "list": [{ "secret_key": "k" }]
        });
        let r = redact_json(&v);
        assert_eq!(r["token"], "••••••••");
        assert_eq!(r["nested"]["password"], "••••••••");
        assert_eq!(r["nested"]["note"], "ok");
        assert_eq!(r["list"][0]["secret_key"], "••••••••");
        assert!(!r.to_string().contains("abc123"));
    }

    #[test]
    fn string_values_are_line_redacted() {
        let v = json!({ "cmd": "frps -c x.toml --token=s3cr3t" });
        let r = redact_json(&v);
        assert!(!r["cmd"].as_str().unwrap().contains("s3cr3t"));
    }

    #[tokio::test]
    async fn writes_and_lists_audit() {
        let pool = crate::db::connect_memory().await.unwrap();
        let w = AuditWriter::new(pool.clone(), true);
        let actor = Actor {
            username: "admin".into(),
            ip: "127.0.0.1".into(),
        };
        w.success(&actor, action::INSTANCE_START, "web", json!({"token": "x"}))
            .await;
        w.failure(&actor, action::INSTANCE_STOP, "web", "boom", 12)
            .await;
        w.denied(&actor, action::CONFIG_WRITE, "web", "viewer").await;

        let (items, total) = w.list(&repo::AuditFilter::default(), 1, 10).await.unwrap();
        assert_eq!(total, 3);
        assert_eq!(items.len(), 3);
        // 敏感字段已脱敏
        assert!(items.iter().all(|i| !i.detail.to_string().contains("\"x\"")));
    }

    #[tokio::test]
    async fn filters_by_action_and_result() {
        let pool = crate::db::connect_memory().await.unwrap();
        let w = AuditWriter::new(pool, true);
        let a = Actor::anonymous("1.1.1.1");
        w.success(&a, action::INSTANCE_START, "x", json!({})).await;
        w.failure(&a, action::INSTANCE_STOP, "x", "e", 1).await;

        let f = repo::AuditFilter {
            action: Some("instance.start".into()),
            ..Default::default()
        };
        let (items, total) = w.list(&f, 1, 10).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].action, "instance.start");

        let f2 = repo::AuditFilter {
            result: Some("failure".into()),
            ..Default::default()
        };
        let (_, total2) = w.list(&f2, 1, 10).await.unwrap();
        assert_eq!(total2, 1);
    }

    #[tokio::test]
    async fn csv_export_escapes_commas() {
        let pool = crate::db::connect_memory().await.unwrap();
        let w = AuditWriter::new(pool, true);
        let a = Actor::anonymous("1.1.1.1");
        w.success(&a, "x", "a,b", json!({})).await;
        let csv = w.export_csv(&repo::AuditFilter::default()).await.unwrap();
        assert!(csv.contains("\"a,b\""));
    }
}
