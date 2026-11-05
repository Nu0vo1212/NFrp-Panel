//! 官方 frp（fatedier/frp）面板 API 适配。
//!
//! 依据实测（0.71.1）：
//! - 服务端：`/api/v2/clients`、`/api/v2/proxies`、`/api/v2/visitors`、`/api/v2/ports`，
//!   响应为 `{code,msg,data:{total,page,pageSize,items}}`；**没有** `/api/status`、`/api/v2/status`、`/api/healthz`。
//! - 客户端：`/api/status` 按类型分组 `{"tcp":[{name,type,status,err,local_addr,plugin,remote_addr}]}`，
//!   `GET /api/config` 返回配置原文；**动态增删代理在 0.71.1 上不可用**（PUT 返回 200 但无效）。
//!
//! ⇒ 官方实现的"代理增删"由上层走「配置文件改写 + 重启」，这里只提供读与能力说明。

use chrono::{TimeZone, Utc};
use serde_json::Value;

use crate::domain::{
    InstanceCounters, InstanceStatus, LiveState, ProxyInfo, StatusDetail,
};
use crate::error::ApiError;
use crate::frpapi::FrpAdminClient;

fn items_of(v: &Value) -> Vec<Value> {
    v.get("data")
        .and_then(|d| d.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn u(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

/// 服务端状态：聚合 `/api/v2/clients` 与 `/api/v2/proxies` 的计数。
pub async fn server_status(c: &FrpAdminClient) -> Result<InstanceStatus, ApiError> {
    // ★ 并行拉取：Admin API 不可达时避免串行等两次连接超时
    let (clients, proxies) = tokio::join!(
        c.get("/api/v2/clients?all=1&page_size=500"),
        c.get("/api/v2/proxies?all=1&page_size=500")
    );

    let (Ok(cl), Ok(pr)) = (&clients, &proxies) else {
        return Ok(InstanceStatus {
            state: LiveState::Unknown,
            healthy: false,
            pid: None,
            uptime_secs: None,
            binary_version: None,
            api_reachable: false,
            last_error: Some(
                "官方 frps 的 Admin API 不可达（确认 webServer.port 已配置且实例在运行）".into(),
            ),
            detail: StatusDetail {
                source: "admin_api".into(),
                notes: vec![
                    "官方 frps 没有 /api/status 与 /api/healthz，只有 /api/v2/*".into(),
                ],
            },
            counters: None,
        });
    };

    if !cl.is_success() || !pr.is_success() {
        return Err(ApiError::upstream(format!(
            "官方 frps Admin API 鉴权失败或版本不支持（clients: HTTP {}，proxies: HTTP {}）",
            cl.status.as_u16(),
            pr.status.as_u16()
        )));
    }

    let cl_json = cl.json()?;
    let pr_json = pr.json()?;
    let client_items = items_of(&cl_json);
    let proxy_items = items_of(&pr_json);

    let online = client_items
        .iter()
        .filter(|c| c.get("online").and_then(Value::as_bool).unwrap_or(false))
        .count() as u64;

    let mut total_conns = 0u64;
    for p in &proxy_items {
        if let Some(st) = p.get("status") {
            total_conns += u(st, "curConns");
        }
    }

    let binary_version = client_items
        .iter()
        .filter_map(|c| c.get("version").and_then(Value::as_str))
        .find(|v| !v.is_empty())
        .map(str::to_string);

    Ok(InstanceStatus {
        state: LiveState::Running,
        healthy: true,
        pid: None,
        uptime_secs: None,
        binary_version,
        api_reachable: true,
        last_error: None,
        detail: StatusDetail {
            source: "admin_api".into(),
            notes: vec![
                "官方 frps 不暴露流量字节，面板按连接数聚合".into(),
                "代理动态增删不在官方 API 中，面板改写配置后重启生效".into(),
            ],
        },
        counters: Some(InstanceCounters {
            bytes_in: 0,
            bytes_out: 0,
            conns: total_conns,
            clients: online,
            proxies: proxy_items.len() as u64,
            requests: None,
        }),
    })
}

/// 服务端代理列表：解析 `spec.{type,<type>}` 与 `status`。
pub async fn server_proxies(c: &FrpAdminClient) -> Result<Vec<ProxyInfo>, ApiError> {
    let resp = c.get("/api/v2/proxies?all=1&page_size=500").await?;
    if !resp.is_success() {
        return Err(ApiError::upstream(format!(
            "官方 frps 代理接口返回 HTTP {}（请检查 webServer.user/password）",
            resp.status.as_u16()
        )));
    }
    let v = resp.json()?;
    // 官方按需返回 spec：有些版本需要 &spec=1
    let items = items_of(&v);

    Ok(items
        .iter()
        .map(|p| {
            let name = s(p, "name");
            let client_id = {
                let v = s(p, "clientID");
                if v.is_empty() {
                    s(p, "clientId")
                } else {
                    v
                }
            };
            let spec = p.get("spec").cloned().unwrap_or(Value::Null);
            let ptype = {
                let t = s(&spec, "type");
                if t.is_empty() {
                    "unknown".to_string()
                } else {
                    t
                }
            };
            let mut info = ProxyInfo::minimal(&name, &ptype);

            // 类型专有段：spec.tcp / spec.http ...
            if let Some(inner) = spec.get(&ptype) {
                let (addr, port) = official_proxy_addr(inner);
                info.local_addr = addr;
                info.remote_port = port.or_else(|| {
                    inner
                        .get("remotePort")
                        .and_then(Value::as_u64)
                        .map(|x| x as u16)
                });
                if let Some(domains) = inner.get("customDomains").and_then(Value::as_array) {
                    info.custom_domains = domains
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect();
                }
                if let Some(sd) = inner.get("subdomain").and_then(Value::as_str) {
                    info.subdomain = sd.to_string();
                }
            }
            if info.local_addr.is_empty() {
                info.local_addr = s(&spec, "localAddr");
            }

            let status = p.get("status").cloned().unwrap_or(Value::Null);
            let phase = s(&status, "phase");
            info.status = match phase.as_str() {
                "online" | "running" => "running".into(),
                "offline" => "offline".into(),
                "waiting" => "waiting".into(),
                "start_error" | "error" => "start_error".into(),
                "" => "unknown".into(),
                other => other.to_string(),
            };
            if let Some(err) = status.get("err").and_then(Value::as_str) {
                if !err.is_empty() {
                    info.error = Some(err.to_string());
                }
            }
            info.cur_conns = u(&status, "curConns");
            info.traffic_in = Some(u(&status, "todayTrafficIn"));
            info.traffic_out = Some(u(&status, "todayTrafficOut"));
            info.last_start_at = status
                .get("lastStartAt")
                .and_then(Value::as_i64)
                .filter(|x| *x > 0)
                .and_then(|x| Utc.timestamp_opt(x, 0).single());
            info.last_active_at = status
                .get("lastCloseAt")
                .and_then(Value::as_i64)
                .filter(|x| *x > 0)
                .and_then(|x| Utc.timestamp_opt(x, 0).single());
            info.source = "config".into();
            info.managed_by = "config_file".into();
            if !client_id.is_empty() {
                info.client_id = Some(client_id.clone());
                info.run_id = Some(client_id);
            }
            info
        })
        .collect())
}

/// 官方 spec 里的本地地址字段随类型不同：
/// tcp/udp 用 `localIP`+`localPort`，http 等用 `localAddr` / `customDomains`。
fn official_proxy_addr(inner: &Value) -> (String, Option<u16>) {
    let ip = inner.get("localIP").and_then(Value::as_str).unwrap_or("");
    let port = inner
        .get("localPort")
        .and_then(Value::as_u64)
        .map(|x| x as u16);
    if !ip.is_empty() {
        let addr = format!("{ip}:{}", port.unwrap_or(0));
        return (addr, port);
    }
    if let Some(a) = inner.get("localAddr").and_then(Value::as_str) {
        if !a.is_empty() {
            return (a.to_string(), port);
        }
    }
    let qs = inner
        .get("plugin")
        .and_then(|p| p.get("pluginName"))
        .and_then(Value::as_str)
        .unwrap_or("");
    (qs.to_string(), port)
}

/// 官方 frpc 状态：`{"tcp":[{"name":..,"status":"running",..}]}`
pub async fn client_status(c: &FrpAdminClient) -> Result<InstanceStatus, ApiError> {
    let resp = c.get("/api/status").await?;
    if !resp.is_success() {
        return Err(ApiError::upstream(format!(
            "官方 frpc 状态接口返回 HTTP {}（确认 frpc 的 webServer.port/user/password）",
            resp.status.as_u16()
        )));
    }
    let v = resp.json()?;

    let mut total = 0u64;
    let mut running = 0u64;
    if let Some(obj) = v.as_object() {
        for (_k, arr) in obj {
            if let Some(list) = arr.as_array() {
                for p in list {
                    total += 1;
                    if p.get("status").and_then(Value::as_str) == Some("running") {
                        running += 1;
                    }
                }
            }
        }
    }

    Ok(InstanceStatus {
        state: LiveState::Running,
        healthy: true,
        pid: None,
        uptime_secs: None,
        binary_version: None,
        api_reachable: true,
        last_error: None,
        detail: StatusDetail {
            source: "admin_api".into(),
            notes: vec![
                "官方 frpc 的 /api/status 按代理类型分组返回".into(),
                if total == running {
                    format!("{total} 个代理全部运行中")
                } else {
                    format!("{running}/{total} 个代理运行中")
                },
            ],
        },
        counters: Some(InstanceCounters {
            bytes_in: 0,
            bytes_out: 0,
            conns: 0,
            clients: 0,
            proxies: total,
            requests: None,
        }),
    })
}

/// 官方 frpc 代理列表。
pub async fn client_proxies(c: &FrpAdminClient) -> Result<Vec<ProxyInfo>, ApiError> {
    let resp = c.get("/api/status").await?;
    if !resp.is_success() {
        return Err(ApiError::upstream(format!(
            "官方 frpc 状态接口返回 HTTP {}",
            resp.status.as_u16()
        )));
    }
    let v = resp.json()?;
    let mut out = Vec::new();
    if let Some(obj) = v.as_object() {
        for (ptype, arr) in obj {
            let Some(list) = arr.as_array() else { continue };
            for p in list {
                let mut info = ProxyInfo::minimal(&s(p, "name"), ptype);
                info.status = match s(p, "status").as_str() {
                    "running" => "running".into(),
                    "wait_start" | "waiting" => "waiting".into(),
                    "start_error" => "start_error".into(),
                    "closed" => "offline".into(),
                    other => other.to_string(),
                };
                info.local_addr = s(p, "local_addr");
                info.error = {
                    let e = s(p, "err");
                    if e.is_empty() {
                        None
                    } else {
                        Some(e)
                    }
                };
                info.remote_port = s(p, "remote_addr")
                    .rsplit_once(':')
                    .and_then(|(_, port)| port.parse().ok());
                info.source = "config".into();
                info.managed_by = "config_file".into();
                out.push(info);
            }
        }
    }
    Ok(out)
}

/// 读官方 frpc 的配置原文（`GET /api/config`）。
pub async fn client_config_raw(c: &FrpAdminClient) -> Option<String> {
    let r = c.get("/api/config").await.ok()?;
    if r.is_success() {
        Some(r.body)
    } else {
        None
    }
}

/// 官方服务端的端口占用表 `/api/v2/ports`（若该版本支持）。
pub async fn server_ports(c: &FrpAdminClient) -> Option<Vec<(u16, usize)>> {
    let r = c.get("/api/v2/ports").await.ok()?;
    if !r.is_success() {
        return None;
    }
    let v = r.json().ok()?;
    let items = v
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .or_else(|| items_of(&v).into())
        .unwrap_or_default();
    Some(
        items
            .iter()
            .filter_map(|p| {
                let port = p.get("port")?.as_u64()? as u16;
                let n = p
                    .get("backends")
                    .and_then(Value::as_array)
                    .map(|a| a.len())
                    .unwrap_or(0);
                Some((port, n))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_official_server_proxy_item() {
        // 实测样本（0.71.1）
        let v = json!({
          "code":200,"msg":"success","data":{"total":1,"page":1,"pageSize":50,"items":[{
            "name":"web","user":"","clientID":"ba05ae0e25ac53d2",
            "spec":{"type":"tcp","tcp":{"transport":{"useEncryption":false},
              "loadBalancer":{"group":""},"remotePort":17180}},
            "status":{"phase":"online","todayTrafficIn":123,"todayTrafficOut":456,
              "curConns":2,"lastStartAt":1793713790}
          }]}
        });
        let items = items_of(&v);
        assert_eq!(items.len(), 1);
        let p = &items[0];
        let spec = p.get("spec").unwrap();
        assert_eq!(spec["type"], "tcp");
        let (addr, port) = official_proxy_addr(&spec["tcp"]);
        assert_eq!(port, None);
        assert_eq!(addr, "");
        assert_eq!(u(&p["status"], "curConns"), 2);
    }

    #[test]
    fn parses_official_client_status_grouping() {
        let v = json!({
          "tcp":[{"name":"web","type":"tcp","status":"running","err":"",
                  "local_addr":"127.0.0.1:8080","plugin":"","remote_addr":"127.0.0.1:17180"}]
        });
        let arr = v.get("tcp").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(s(&arr[0], "status"), "running");
    }
}
