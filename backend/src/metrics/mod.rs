//! 指标采样与趋势查询：把驱动拿到的快照落库，并为前端提供时间序列。
//!
//! 采样由后台任务（`state.rs` 启动）周期执行；前端读 `metric_samples` 表。
//! 对官方 frp（只有连接数）与 NFrp（有流量字节）用同一套表，缺失字段记 0，
//! 由 `availability` 字段告诉前端"这一段数据能画哪些线"。

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::models::MetricSample;
use crate::db::repo;
use crate::domain::MetricsSnapshot;
use crate::error::ApiError;

#[derive(Debug, Clone, Serialize)]
pub struct MetricsSeries {
    pub name: String,
    pub points: Vec<(i64, u64)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricsResponse {
    pub series: Vec<MetricsSeries>,
    pub availability: String,
    pub notes: Vec<String>,
    pub range_secs: i64,
}

/// 区间解析：`1h` / `6h` / `24h` / `7d` / `30m`。
///
/// ★ 非法输入**不猜**：整串无法解析时统一回退到 1 小时，
///   避免 `"bad"` 被拆成 `"ba"+"d"` 后静默变成 1 天这种难以察觉的偏差。
pub fn parse_range(s: &str) -> i64 {
    let s = s.trim().to_lowercase();
    let Some(unit) = s.chars().last() else {
        return 3600;
    };
    if !unit.is_ascii_alphabetic() {
        return 3600;
    }
    let num_part = &s[..s.len() - 1];
    let Ok(n) = num_part.parse::<i64>() else {
        return 3600;
    };
    if n <= 0 {
        return 3600;
    }
    match unit {
        'h' => n * 3600,
        'd' => n * 86400,
        'm' => n * 60,
        _ => 3600,
    }
}

/// 首次采样到不了最大值时，前端仍要看到可用区间；这里做下采样到最多 `max_points`。
pub async fn query(
    pool: &SqlitePool,
    instance_id: uuid::Uuid,
    range_secs: i64,
    max_points: i64,
) -> Result<MetricsResponse, ApiError> {
    let to = Utc::now().timestamp();
    let from = to - range_secs;
    let rows = repo::list_metrics(pool, instance_id, from, to, max_points).await?;

    let mut bytes_in = Vec::new();
    let mut bytes_out = Vec::new();
    let mut conns = Vec::new();
    let mut clients = Vec::new();
    let mut proxies = Vec::new();
    let mut any_bytes = false;

    for r in &rows {
        bytes_in.push((r.ts, r.bytes_in.max(0) as u64));
        bytes_out.push((r.ts, r.bytes_out.max(0) as u64));
        conns.push((r.ts, r.conns.max(0) as u64));
        clients.push((r.ts, r.clients.max(0) as u64));
        proxies.push((r.ts, r.proxies.max(0) as u64));
        if r.bytes_in > 0 || r.bytes_out > 0 {
            any_bytes = true;
        }
    }

    let mut notes = Vec::new();
    if rows.is_empty() {
        notes.push("暂无采样数据：面板每分钟采样一次，稍候刷新".into());
    }
    let availability = if any_bytes {
        "full"
    } else if !rows.is_empty() {
        notes.push("该实现不提供流量字节（官方 frps），仅展示连接数".into());
        "aggregated"
    } else {
        "none"
    };

    Ok(MetricsResponse {
        series: vec![
            MetricsSeries {
                name: "bytes_in".into(),
                points: bytes_in,
            },
            MetricsSeries {
                name: "bytes_out".into(),
                points: bytes_out,
            },
            MetricsSeries {
                name: "conns".into(),
                points: conns,
            },
            MetricsSeries {
                name: "clients".into(),
                points: clients,
            },
            MetricsSeries {
                name: "proxies".into(),
                points: proxies,
            },
        ],
        availability: availability.into(),
        notes,
        range_secs,
    })
}

/// 把一次快照写入采样表。
pub async fn record(
    pool: &SqlitePool,
    instance_id: uuid::Uuid,
    snap: &MetricsSnapshot,
) -> Result<(), ApiError> {
    if !snap.api_reachable {
        return Ok(());
    }
    let sample = MetricSample {
        ts: snap.ts.timestamp(),
        bytes_in: snap.counters.bytes_in as i64,
        bytes_out: snap.counters.bytes_out as i64,
        conns: snap.counters.conns as i64,
        clients: snap.counters.clients as i64,
        proxies: snap.counters.proxies as i64,
    };
    repo::insert_metric(pool, instance_id, &sample).await
}

/// 启动后台采样循环（每个进程周期跑一次）。
pub fn spawn_poller(
    db: SqlitePool,
    drivers: Arc<crate::state::DriverRegistry>,
    state: Arc<crate::state::AppState>,
    interval: Duration,
    retention_days: u32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_prune = Utc::now().timestamp();
        loop {
            tokio::time::sleep(interval).await;
            let rows = match repo::list_instances(&db).await {
                Ok(r) => r,
                Err(_) => continue,
            };
            for row in rows {
                if !row.enabled {
                    continue;
                }
                let id = row.id;
                let Ok(driver) = drivers.get_or_build(id, &state).await else {
                    continue;
                };
                if let Ok(snap) = driver.metrics().await {
                    let _ = record(&db, id, &snap).await;
                }
            }
            // 每天清理一次过期采样
            let now = Utc::now().timestamp();
            if now - last_prune > 86400 {
                let cutoff = now - (retention_days as i64) * 86400;
                let _ = repo::prune_metrics(&db, cutoff).await;
                last_prune = now;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::InstanceCounters;
    use chrono::TimeZone;

    #[test]
    fn parses_ranges() {
        assert_eq!(parse_range("1h"), 3600);
        assert_eq!(parse_range("24h"), 86400);
        assert_eq!(parse_range("7d"), 604800);
        assert_eq!(parse_range("30m"), 1800);
        // 非法数字按 1 计，非法单位按小时兜底
        assert_eq!(parse_range("bad"), 3600);
        assert_eq!(parse_range("xh"), 3600);
    }

    #[tokio::test]
    async fn records_and_reads_back() {
        let pool = crate::db::connect_memory().await.unwrap();
        let id = uuid::Uuid::new_v4();
        let snap = MetricsSnapshot {
            ts: Utc.timestamp_opt(Utc::now().timestamp(), 0).unwrap(),
            api_reachable: true,
            counters: InstanceCounters {
                bytes_in: 100,
                bytes_out: 200,
                conns: 3,
                clients: 1,
                proxies: 2,
                requests: None,
            },
            availability: "full".into(),
            source: "admin_api".into(),
        };
        record(&pool, id, &snap).await.unwrap();
        let resp = query(&pool, id, 3600, 100).await.unwrap();
        assert_eq!(resp.availability, "full");
        let bi = resp.series.iter().find(|s| s.name == "bytes_in").unwrap();
        assert_eq!(bi.points.len(), 1);
        assert_eq!(bi.points[0].1, 100);
    }

    #[tokio::test]
    async fn empty_range_reports_notes() {
        let pool = crate::db::connect_memory().await.unwrap();
        let resp = query(&pool, uuid::Uuid::new_v4(), 3600, 100).await.unwrap();
        assert_eq!(resp.availability, "none");
        assert!(!resp.notes.is_empty());
    }
}
