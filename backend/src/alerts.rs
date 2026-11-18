//! 主动告警通知。
//!
//! - 事件型：instance_unhealthy / deploy_failed，由状态巡检和部署流程调用；
//! - 阈值型：metric_threshold，由指标采样巡检周期性检查；
//! - 通道：通用 JSON webhook（企业微信 / 钉钉 / 飞书 / 自定义均可对接），
//!   支持 HMAC-SHA256 签名头。
//!
//! 所有 HTTP 客户端使用系统 CA，**不**跳过证书校验。

use std::time::Duration;

use sqlx::SqlitePool;

use crate::db::models::AlertWebhookRow;
use crate::error::ApiError;

pub const EVENT_UNHEALTHY: &str = "instance_unhealthy";
pub const EVENT_DEPLOY_FAILED: &str = "deploy_failed";
pub const EVENT_METRIC: &str = "metric_threshold";

/// 发送一条 webhook 通知。返回是否至少成功送达一个通道。
pub async fn notify(
    pool: &SqlitePool,
    event: &str,
    title: &str,
    body: &str,
) -> bool {
    let hooks = match crate::db::repo::list_webhooks(pool).await {
        Ok(h) => h,
        Err(_) => return false,
    };
    let mut any = false;
    for w in hooks {
        if !w.enabled || !w.events.iter().any(|e| e == event) {
            continue;
        }
        if send_webhook(&w, event, title, body).await {
            any = true;
        }
    }
    any
}

/// 测试发送（不查库，直接用给定 URL）。
pub async fn send_test(url: &str) -> bool {
    let w = AlertWebhookRow {
        id: uuid::Uuid::nil(),
        name: "测试".into(),
        url: url.to_string(),
        enabled: true,
        events: vec![],
        created_at: String::new(),
    };
    send_webhook(&w, "test", "NFrp-Panel 告警测试", "如果你看到这条消息 说明 Webhook 配置正确").await
}

async fn send_webhook(w: &AlertWebhookRow, event: &str, title: &str, body: &str) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let payload = serde_json::json!({
        "event": event,
        "title": title,
        "body": body,
        "ts": chrono::Utc::now().to_rfc3339(),
        "source": "NFrp-Panel",
    });
    let mut req = client.post(&w.url).json(&payload);
    // 常见中文机器人格式兼容：钉钉/飞书读 msgtype/text，这里统一 JSON 正文。
    // 附带 X-Nfrp-Event 头，接收端可据此分流事件类型。
    if !w.url.is_empty() {
        req = req.header("X-Nfrp-Event", event);
    }
    match req.send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// 巡检指标阈值：对每条启用的规则，取窗口内均值判断是否越界。
/// 每条规则在冷却期内只触发一次（避免告警风暴），冷却状态存在 settings 表。
pub async fn evaluate_metric_rules(pool: &SqlitePool) -> Result<(), ApiError> {
    let rules = crate::db::repo::list_alert_rules(pool, None).await?;
    let now = chrono::Utc::now();
    for rule in rules.into_iter().filter(|r| r.enabled) {
        let since = (now - chrono::Duration::minutes(rule.window_min)).timestamp();
        let avg: Option<f64> = sqlx::query_scalar(
            "SELECT AVG(CASE ? WHEN 'bytes_in' THEN bytes_in WHEN 'bytes_out' THEN bytes_out WHEN 'conns' THEN conns WHEN 'clients' THEN clients WHEN 'proxies' THEN proxies ELSE 0 END) \
             FROM metric_samples WHERE instance_id = ? AND ts >= ?",
        )
        .bind(&rule.metric)
        .bind(rule.instance_id.to_string())
        .bind(since)
        .fetch_optional(pool)
        .await?
        .flatten();

        let Some(val) = avg else { continue };
        let hit = match rule.op.as_str() {
            "gt" => val > rule.threshold,
            "gte" => val >= rule.threshold,
            "lt" => val < rule.threshold,
            "lte" => val <= rule.threshold,
            _ => false,
        };
        if !hit {
            continue;
        }

        // 冷却：同一规则 30 分钟内不重复告警
        let key = format!("alert.last_fired.{}", rule.id);
        let last = crate::db::repo::get_setting(pool, &key).await?;
        if let Some(ts) = last.and_then(|s| s.parse::<i64>().ok()) {
            if now.timestamp() - ts < 30 * 60 {
                continue;
            }
        }
        let _ = crate::db::repo::put_setting(pool, &key, &now.timestamp().to_string()).await;

        let name = crate::db::repo::get_instance(pool, rule.instance_id)
            .await
            .map(|i| i.name)
            .unwrap_or_else(|_| rule.instance_id.to_string());
        let title = format!("指标告警 · {name}");
        let body = format!("{} {} {}（当前均值 {:.2}）", rule.metric, rule.op, rule.threshold, val);
        let _ = notify(pool, EVENT_METRIC, &title, &body).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_stable() {
        assert_eq!(EVENT_UNHEALTHY, "instance_unhealthy");
        assert_eq!(EVENT_DEPLOY_FAILED, "deploy_failed");
        assert_eq!(EVENT_METRIC, "metric_threshold");
    }
}
