//! NFrp（Rust-Frp）面板 API 适配。
//!
//! 端点与响应结构依据 `E:\项目\nfrp` 源码与本机实测：
//! - 服务端：`/api/status`（v1）、`/api/v2/status`、`/api/v2/clients`、`/api/v2/proxies`、`/api/v2/ports`
//! - 客户端：`/api/status`、`/api/proxies`、`/api/proxies/add|remove`（写操作需 `X-Nfrp-Client: 1`）
//! - 写操作：`POST /api/proxies/add|remove`、`POST /api/clients/kick`（v1 与 v2 同形）

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::domain::{
    InstanceCounters, InstanceStatus, LiveState, ProxyInfo, ProxySpec, StatusDetail,
};
use crate::error::ApiError;
use crate::frpapi::FrpAdminClient;

fn as_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn as_str(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 服务端统一状态。
///
/// 优先 `/api/v2/status`（含 counts/features/audit），失败则退回 `/api/status`。
pub async fn server_status(c: &FrpAdminClient) -> Result<InstanceStatus, ApiError> {
    // ★ 两个端点并行：端口未监听时顺序探测 = 2 倍连接超时，列表/详情会被明显拖慢
    let (v2r, v1r) = tokio::join!(c.get("/api/v2/status"), c.get("/api/status"));
    let v2 = v2r.ok().filter(|r| r.is_success());
    let v1 = v1r.ok().filter(|r| r.is_success());

    if v2.is_none() && v1.is_none() {
        return Ok(InstanceStatus {
            state: LiveState::Unknown,
            healthy: false,
            pid: None,
            uptime_secs: None,
            binary_version: None,
            api_reachable: false,
            last_error: Some("Admin API 不可达（实例可能未启动或面板端口未配置）".into()),
            detail: StatusDetail {
                source: "admin_api".into(),
                notes: vec!["已尝试 /api/v2/status 与 /api/status".into()],
            },
            counters: None,
        });
    }

    let mut notes = Vec::new();
    let mut counters = InstanceCounters {
        bytes_in: 0,
        bytes_out: 0,
        conns: 0,
        clients: 0,
        proxies: 0,
        requests: None,
    };
    let mut uptime = None;

    if let Some(r) = &v2 {
        if let Ok(v) = r.json() {
            let data = v.get("data").cloned().unwrap_or(Value::Null);
            let m = data.get("metrics").cloned().unwrap_or(Value::Null);
            counters.bytes_in = as_u64(&m, "bytes_up");
            counters.bytes_out = as_u64(&m, "bytes_down");
            counters.conns = as_u64(&m, "conns_active");
            counters.requests = Some(as_u64(&m, "http_requests"));
            uptime = Some(as_u64(&m, "uptime_secs"));
            let counts = data.get("counts").cloned().unwrap_or(Value::Null);
            counters.clients = as_u64(&counts, "clients_online");
            counters.proxies = as_u64(&counts, "proxies");
            if data.get("features").is_some() {
                notes.push("检测到 NFrp API v2（含 features/counts）".into());
            }
        }
    }

    if let Some(r) = &v1 {
        if let Ok(v) = r.json() {
            let st = v.get("status").cloned().unwrap_or(Value::Null);
            if counters.bytes_in == 0 {
                counters.bytes_in = as_u64(&st, "bytes_up");
                counters.bytes_out = as_u64(&st, "bytes_down");
            }
            if counters.conns == 0 {
                counters.conns = as_u64(&st, "conns_active");
            }
            if counters.clients == 0 {
                counters.clients = as_u64(&st, "clients_active");
            }
            if counters.proxies == 0 {
                counters.proxies = as_u64(&st, "proxies_active");
            }
            if uptime.is_none() {
                uptime = Some(as_u64(&st, "uptime_secs"));
            }
        }
    }

    Ok(InstanceStatus {
        state: LiveState::Running,
        healthy: true,
        pid: None,
        uptime_secs: uptime,
        binary_version: None,
        api_reachable: true,
        last_error: None,
        detail: StatusDetail {
            source: "admin_api".into(),
            notes,
        },
        counters: Some(counters),
    })
}

/// NFrp 客户端状态：`{"data":{"connected":true,"proxies":[...],"run_id":"..."}}`
pub async fn client_status(c: &FrpAdminClient) -> Result<InstanceStatus, ApiError> {
    let resp = c.get("/api/status").await?;
    if !resp.is_success() {
        return Err(ApiError::upstream(format!(
            "NFrp 客户端状态接口返回 HTTP {}",
            resp.status.as_u16()
        )));
    }
    let v = resp.json()?;
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let connected = data
        .get("connected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let proxies = data
        .get("proxies")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0) as u64;

    let mut notes = Vec::new();
    if let Some(w) = data.get("wire_protocol").and_then(Value::as_str) {
        if !w.is_empty() {
            notes.push(format!("线协议 {w}"));
        }
    }
    if let Some(st) = data.get("store") {
        if st.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
            notes.push("已启用本地 Store（动态代理会持久化）".into());
        }
    }

    Ok(InstanceStatus {
        state: if connected {
            LiveState::Running
        } else {
            LiveState::Error
        },
        healthy: connected,
        pid: None,
        uptime_secs: None,
        binary_version: None,
        api_reachable: true,
        last_error: if connected {
            None
        } else {
            Some("客户端进程在运行，但尚未连上服务端".into())
        },
        detail: StatusDetail {
            source: "admin_api".into(),
            notes,
        },
        counters: Some(InstanceCounters {
            bytes_in: 0,
            bytes_out: 0,
            conns: 0,
            clients: 0,
            proxies,
            requests: None,
        }),
    })
}

/// 服务端代理列表：按客户端聚合 `/api/v2/clients`（每客户端带 `proxies` 名称）
/// 并结合 `/api/v2/proxies` 的条数校验；v2 不可用时退回 `/api/status`。
pub async fn server_proxies(c: &FrpAdminClient) -> Result<Vec<ProxyInfo>, ApiError> {
    // v2：先拿 clients（含 run_id 与代理名）
    let clients_resp = c.get("/api/v2/clients?all=1&page_size=500").await;
    let mut out: Vec<ProxyInfo> = Vec::new();

    if let Ok(r) = &clients_resp {
        if r.is_success() {
            if let Ok(v) = r.json() {
                if let Some(items) = v.get("data").and_then(Value::as_array) {
                    for cl in items {
                        let run_id = as_str(cl, "run_id");
                        let client_id = as_str(cl, "client_id");
                        let user = as_str(cl, "user");
                        if let Some(names) = cl.get("proxies").and_then(Value::as_array) {
                            for n in names {
                                let Some(full) = n.as_str() else { continue };
                                let display = strip_user_prefix(full, &user);
                                let mut p = ProxyInfo::minimal(&display, "unknown");
                                p.status = "running".into();
                                p.source = "dynamic".into();
                                p.managed_by = "server_push".into();
                                p.client_id = Some(client_id.clone());
                                p.run_id = Some(run_id.clone());
                                out.push(p);
                            }
                        }
                    }
                }
            }
        }
    }

    // 如果 clients 拿不到，退回 /api/status（结构同样含 clients[].proxies）
    if out.is_empty() {
        if let Ok(r) = c.get("/api/status").await {
            if let Ok(v) = r.json() {
                if let Some(items) = v.get("clients").and_then(Value::as_array) {
                    for cl in items {
                        let run_id = as_str(cl, "run_id");
                        let client_id = as_str(cl, "client_id");
                        let user = as_str(cl, "user");
                        if let Some(names) = cl.get("proxies").and_then(Value::as_array) {
                            for n in names {
                                let Some(full) = n.as_str() else { continue };
                                let mut p =
                                    ProxyInfo::minimal(&strip_user_prefix(full, &user), "unknown");
                                p.status = "running".into();
                                p.source = "dynamic".into();
                                p.managed_by = "server_push".into();
                                p.client_id = Some(client_id.clone());
                                p.run_id = Some(run_id.clone());
                                out.push(p);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(out)
}

/// NFrp 服务端注册表里的代理名带 `user.` 前缀（见 `util::strip_user_prefix`）。
pub fn strip_user_prefix(full: &str, user: &str) -> String {
    if user.is_empty() {
        return full.to_string();
    }
    let prefix = format!("{user}.");
    full.strip_prefix(&prefix).unwrap_or(full).to_string()
}

/// 客户端代理列表：`{"data":[{name,type,local_addr,remote_port,custom_domains,healthy,dynamic}]}`
pub async fn client_proxies(c: &FrpAdminClient) -> Result<Vec<ProxyInfo>, ApiError> {
    let resp = c.get("/api/proxies").await?;
    if !resp.is_success() {
        return Err(ApiError::upstream(format!(
            "NFrp 客户端代理接口返回 HTTP {}",
            resp.status.as_u16()
        )));
    }
    let v = resp.json()?;
    let arr = v
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(arr
        .iter()
        .map(|p| {
            let mut info = ProxyInfo::minimal(&as_str(p, "name"), &as_str(p, "type"));
            info.local_addr = as_str(p, "local_addr");
            info.remote_port = p
                .get("remote_port")
                .and_then(Value::as_u64)
                .map(|x| x as u16);
            info.custom_domains = p
                .get("custom_domains")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let healthy = p.get("healthy").and_then(Value::as_bool).unwrap_or(true);
            info.status = if healthy { "running" } else { "start_error" }.into();
            let dynamic = p.get("dynamic").and_then(Value::as_bool).unwrap_or(false);
            info.source = if dynamic { "dynamic" } else { "config" }.into();
            info.managed_by = if dynamic { "panel" } else { "config_file" }.into();
            info
        })
        .collect())
}

/// 由面板向**服务端**推送一条动态代理（NFrp 专有）。
pub async fn add_proxy_via_server(
    c: &FrpAdminClient,
    run_id: &str,
    spec: &ProxySpec,
) -> Result<String, ApiError> {
    let body = serde_json::json!({
        "run_id": run_id,
        "proxy": proxy_spec_to_json(spec),
    });
    // v2 的错误信封更清晰，优先；失败再退 v1（老版本可能没有 v2）
    let resp = c.post_json("/api/v2/proxies/add", &body).await?;
    if resp.is_success() {
        return Ok(resp
            .json()
            .ok()
            .and_then(|v| {
                v.get("data")
                    .and_then(|d| d.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "代理已下发".into()));
    }
    // 解析信封错误
    let v2_err = resp
        .json()
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });

    let resp1 = c.post_json("/api/proxies/add", &body).await?;
    if resp1.is_success() {
        return Ok(resp1
            .json()
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "代理已下发".into()));
    }
    let v1_err = resp1
        .json()
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string));
    Err(ApiError::upstream(format!(
        "服务端拒绝了代理 [{}]：{}",
        spec.name,
        v2_err
            .or(v1_err)
            .unwrap_or_else(|| format!("HTTP {}", resp1.status.as_u16()))
    )))
}

/// 由面板向**服务端**移除动态代理。
pub async fn remove_proxy_via_server(
    c: &FrpAdminClient,
    run_id: &str,
    name: &str,
) -> Result<String, ApiError> {
    let body = serde_json::json!({ "run_id": run_id, "name": name });
    let resp = c.post_json("/api/v2/proxies/remove", &body).await?;
    if resp.is_success() {
        return Ok("代理已移除".into());
    }
    let resp1 = c.post_json("/api/proxies/remove", &body).await?;
    if resp1.is_success() {
        return Ok("代理已移除".into());
    }
    let msg = resp1
        .json()
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| format!("HTTP {}", resp1.status.as_u16()));
    Err(ApiError::upstream(format!("服务端拒绝移除 [{name}]：{msg}")))
}

/// 由面板向**客户端**动态增删（需 `X-Nfrp-Client`，客户端角色时自动加）。
pub async fn add_proxy_via_client(
    c: &FrpAdminClient,
    spec: &ProxySpec,
) -> Result<String, ApiError> {
    let resp = c
        .post_json("/api/proxies/add", &proxy_spec_to_json(spec))
        .await?;
    if resp.is_success() {
        return Ok(resp
            .json()
            .ok()
            .and_then(|v| {
                v.get("data")
                    .and_then(|d| d.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "代理已生效".into()));
    }
    let msg = resp
        .json()
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("HTTP {}", resp.status.as_u16()));
    Err(ApiError::upstream(format!(
        "NFrp 客户端拒绝新增代理 [{}]：{msg}",
        spec.name
    )))
}

pub async fn remove_proxy_via_client(c: &FrpAdminClient, name: &str) -> Result<String, ApiError> {
    let body = serde_json::json!({ "name": name });
    let resp = c.post_json("/api/proxies/remove", &body).await?;
    if resp.is_success() {
        return Ok("代理已移除".into());
    }
    let msg = resp
        .json()
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("HTTP {}", resp.status.as_u16()));
    Err(ApiError::upstream(format!(
        "NFrp 客户端拒绝移除 [{name}]：{msg}"
    )))
}

/// ProxySpec → NFrp 的 ProxyConfig（JSON，snake_case）。
pub fn proxy_spec_to_json(spec: &ProxySpec) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("name".into(), Value::String(spec.name.clone()));
    o.insert("type".into(), Value::String(spec.proxy_type.clone()));
    let addr = spec.effective_local_addr();
    if !addr.is_empty() {
        o.insert("local_addr".into(), Value::String(addr));
    }
    if let Some(p) = spec.remote_port {
        o.insert("remote_port".into(), Value::from(p));
    }
    if !spec.custom_domains.is_empty() {
        o.insert("custom_domains".into(), serde_json::json!(spec.custom_domains));
    }
    if !spec.subdomain.is_empty() {
        o.insert("subdomain".into(), Value::String(spec.subdomain.clone()));
    }
    if let Some(k) = &spec.secret_key {
        o.insert("secret_key".into(), Value::String(k.clone()));
    }
    if !spec.allow_users.is_empty() {
        o.insert("allow_users".into(), serde_json::json!(spec.allow_users));
    }
    if spec.use_encryption {
        o.insert("use_encryption".into(), Value::Bool(true));
    }
    if spec.use_compression {
        o.insert("use_compression".into(), Value::Bool(true));
    }
    if let Some(g) = &spec.group {
        o.insert("group".into(), Value::String(g.clone()));
    }
    // 面板下发的代理不允许带插件（NFrp 0.5.5+ 会拒绝）；此处不写 plugin 字段。
    Value::Object(o)
}

/// 版本：`/api/v2/version` → `data.server`
pub async fn version(c: &FrpAdminClient) -> Option<String> {
    let r = c.get("/api/v2/version").await.ok()?;
    if !r.is_success() {
        return None;
    }
    let v = r.json().ok()?;
    v.get("data")
        .and_then(|d| d.get("server"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 供指标采样：`/api/status` 的 `status` 段（官方口径的 19 个计数器）。
pub async fn raw_counters(c: &FrpAdminClient) -> Option<InstanceCounters> {
    let r = c.get("/api/status").await.ok()?;
    if !r.is_success() {
        return None;
    }
    let v = r.json().ok()?;
    let st = v.get("status")?;
    Some(InstanceCounters {
        bytes_in: as_u64(st, "bytes_up"),
        bytes_out: as_u64(st, "bytes_down"),
        conns: as_u64(st, "conns_active"),
        clients: as_u64(st, "clients_active"),
        proxies: as_u64(st, "proxies_active"),
        requests: Some(as_u64(st, "http_requests")),
    })
}

#[allow(dead_code)]
fn ts_from_unix(secs: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(secs, 0).single()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_user_prefix() {
        assert_eq!(strip_user_prefix("alice.web", "alice"), "web");
        assert_eq!(strip_user_prefix("web", "alice"), "web");
        assert_eq!(strip_user_prefix("alice.web", ""), "alice.web");
    }

    #[test]
    fn proxy_spec_json_uses_nfrp_field_names() {
        let spec = ProxySpec {
            name: "web".into(),
            proxy_type: "tcp".into(),
            local_addr: "127.0.0.1:8080".into(),
            remote_port: Some(6000),
            ..Default::default()
        };
        let v = proxy_spec_to_json(&spec);
        assert_eq!(v["local_addr"], "127.0.0.1:8080");
        assert_eq!(v["remote_port"], 6000);
        // 面板下发不允许带插件
        assert!(v.get("plugin").is_none());
    }

    #[test]
    fn proxy_spec_json_merges_split_addr() {
        let spec = ProxySpec {
            name: "w".into(),
            proxy_type: "tcp".into(),
            local_ip: Some("10.1.1.1".into()),
            local_port: Some(9000),
            remote_port: Some(6000),
            ..Default::default()
        };
        assert_eq!(proxy_spec_to_json(&spec)["local_addr"], "10.1.1.1:9000");
    }
}
