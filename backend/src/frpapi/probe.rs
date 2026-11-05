//! 能力探测：判定 Admin API 属于哪个实现，并导出能力位。
//!
//! 探测原则（与 `docs/04-verified-behavior.md` 对应）：
//! 1. **不臆造**：每个能力位要么有端点证据，要么来自驱动语义（是否接管进程）；
//! 2. **不因单点失败而整体失败**：拿不到某项就降级并写 `notes`；
//! 3. **结果可解释**：`notes` 里写清楚"依据什么判断"，前端原样展示。

use chrono::Utc;
use serde_json::Value;

use crate::domain::{
    ApiKind, BinaryFlavor, Capabilities, CapabilityNote, InstanceRole, MetricDetail,
    PrometheusSupport,
};
use crate::frpapi::{nfrp, official, FrpAdminClient};

/// 探测到的原始证据（便于 UI 展示与排障）。
#[derive(Debug, Clone, Default)]
pub struct ProbeEvidence {
    pub endpoints: Vec<(String, u16, String)>, // path, status, 摘要
    pub version: Option<String>,
}

pub struct ProbeInput<'a> {
    pub client: Option<&'a FrpAdminClient>,
    pub role: InstanceRole,
    pub flavor: BinaryFlavor,
    /// 驱动是否接管进程（Binary/SSH = true，AdminApi = false）
    pub managed_locally: bool,
    /// 是否远程（SSH）
    pub remote: bool,
    /// 本机/远程二进制 `-v` 拿到的版本（可选）
    pub binary_version: Option<String>,
    /// 是否拥有配置文件（决定 can_edit_config）
    pub owns_config: bool,
}

/// 执行探测，返回能力位。
pub async fn probe(input: ProbeInput<'_>) -> Capabilities {
    let mut caps = Capabilities::unknown(input.flavor);
    caps.flavor_version = input.binary_version.clone();
    caps.managed_locally = input.managed_locally;
    caps.remote = input.remote;
    caps.can_start_stop = input.managed_locally;
    caps.can_edit_config = input.owns_config;
    caps.probed_at = Some(Utc::now());

    let Some(client) = input.client else {
        caps.notes.push(CapabilityNote {
            level: "info".into(),
            text: "未配置 Admin API：仅能操作进程与配置文件，无法查询运行态".into(),
        });
        caps.can_reload = input.owns_config;
        return caps;
    };

    let mut evidence = ProbeEvidence::default();

    // 1) 逐个试探端点（并发，任何失败都不影响其它）
    let paths: &[&str] = &[
        "/api/v2/version",
        "/api/v2/status",
        "/api/status",
        "/api/healthz",
        "/api/proxies",
        "/api/config",
        "/metrics",
    ];
    // ★ 判定用的原文保留（只截断，不脱敏）：
    //   `redact_line` 会抹掉含 token/secret 等关键词的值，而实现识别恰恰需要看结构键名
    //   （如 NFrp 客户端的 `run_id`、`wire_protocol`）。展示用的摘要另行脱敏。
    let mut raw_bodies: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for p in paths {
        match client.get(p).await {
            Ok(r) => {
                let summary = summarize(&r.body);
                evidence
                    .endpoints
                    .push((p.to_string(), r.status.as_u16(), summary));
                raw_bodies.insert(*p, r.body);
            }
            Err(e) => {
                evidence
                    .endpoints
                    .push((p.to_string(), 0, format!("不可达：{e}")));
            }
        }
    }

    let get = |path: &str| -> Option<(u16, String)> {
        evidence
            .endpoints
            .iter()
            .find(|(p, _, _)| p == path)
            .map(|(_, s, _)| (*s, raw_bodies.get(path).cloned().unwrap_or_default()))
    };

    let v2_version = get("/api/v2/version").filter(|(s, _)| *s == 200);
    let v2_status = get("/api/v2/status").filter(|(s, _)| *s == 200);
    let v1_status = get("/api/status").filter(|(s, _)| *s == 200);
    let healthz = get("/api/healthz").filter(|(s, _)| *s == 200);
    let metrics = get("/metrics").filter(|(s, _)| *s == 200);

    let any_reachable = evidence.endpoints.iter().any(|(_, s, _)| *s != 0);

    // 2) 判定实现（用引用匹配，后续还要读这些小 Option）
    //
    // 实测差异（见 `docs/04-verified-behavior.md`）：
    // * NFrp 服务端：`/api/status` 顶层有 `clients[]`；有 `/api/v2/version`、`/api/healthz`
    // * NFrp 客户端：`/api/status` 是 `{"data":{connected, proxies, run_id, ...}}`；
    //   有 `/api/healthz`；**没有** `/api/v2/version`
    // * 官方 frpc：  `/api/status` 是按类型分组 `{"tcp":[...]}`；**没有** `/api/healthz`
    // * 官方 frps：  只有 `/api/v2/*`，返回 `{code,msg,data:{items}}` 信封
    //
    // ★ 之前的判定漏了 NFrp 客户端（既没有 clients[]、也没有 v2/version），
    //   会被误判成 official。这里补上 `{"data":{...run_id...}}` 的结构嗅探，
    //   并把 `/api/healthz` 作为 NFrp 的强特征（官方 frp 完全没有这个端点）。
    let v1_body = v1_status
        .as_ref()
        .map(|(_, b)| b.clone())
        .unwrap_or_default();
    let looks_nfrp_client = v1_body.contains("\"run_id\"")
        || v1_body.contains("\"wire_protocol\"")
        || v1_body.contains("\"visitor_count\"");
    let looks_official_client = v1_body.contains("\"local_addr\"")
        || v1_body.contains("\"remote_addr\"")
        || ["\"tcp\":", "\"udp\":", "\"http\":", "\"https\":", "\"stcp\":"]
            .iter()
            .any(|k| v1_body.contains(k));

    let api_kind = match (&v2_version, &v2_status, &v1_status, &healthz) {
        _ if v2_version.is_some() => {
            caps.has_v2_api = true;
            caps.has_healthz = healthz.is_some();
            if let Some(v) = nfrp::version(client).await {
                caps.flavor_version = Some(v);
            }
            caps.notes.push(CapabilityNote {
                level: "info".into(),
                text: "通过 /api/v2/version 判定为 NFrp 面板 API".into(),
            });
            ApiKind::Nfrp
        }
        _ if v1_status.is_some() && looks_nfrp_client => {
            caps.has_healthz = healthz.is_some();
            caps.notes.push(CapabilityNote {
                level: "info".into(),
                text: "通过 /api/status 的 run_id/wire_protocol 结构判定为 NFrp 客户端".into(),
            });
            ApiKind::Nfrp
        }
        _ if v1_status.is_some() && v1_body.contains("\"clients\"") => {
            caps.has_v2_api = v2_status.is_some();
            caps.has_healthz = healthz.is_some();
            caps.notes.push(CapabilityNote {
                level: "info".into(),
                text: "通过 /api/status 顶层 clients[] 判定为 NFrp 服务端".into(),
            });
            ApiKind::Nfrp
        }
        _ if v1_status.is_some() && (looks_official_client || healthz.is_none()) => {
            caps.notes.push(CapabilityNote {
                level: "info".into(),
                text: "通过 /api/status 的类型分组结构判定为官方 frpc".into(),
            });
            ApiKind::Official
        }
        _ if any_reachable => {
            // 官方 frps：/api/v2/clients 有响应但没有 v1 status
            caps.notes.push(CapabilityNote {
                level: "info".into(),
                text: "只有 /api/v2/* 可用（无 /api/status、无 /api/healthz），判定为官方 frps".into(),
            });
            ApiKind::Official
        }
        _ => {
            caps.notes.push(CapabilityNote {
                level: "warning".into(),
                text: "Admin API 全部端点不可达：请确认实例已启动、webServer/dashboard 端口与凭据正确".into(),
            });
            ApiKind::None
        }
    };
    caps.api_kind = api_kind;

    // 3) 按实现 + 角色导出能力位
    match (api_kind, input.role) {
        (ApiKind::Nfrp, _) => {
            caps.can_manage_visitors = caps.has_v2_api;
            caps.can_dynamic_proxy = true; // 服务端推 / 客户端 add-remove 都可用
            caps.auth_guard_header = matches!(input.role, InstanceRole::Client);
            // ★ 实测：NFrp 客户端的 /api/status 只有 proxies/connected，没有流量字节；
            //   只有服务端（/api/status 的 status 段 或 /api/v2/status 的 metrics）有完整计数。
            caps.metrics_detail = match input.role {
                InstanceRole::Server => MetricDetail::Full,
                InstanceRole::Client => MetricDetail::Aggregated,
            };
            caps.can_reload = input.owns_config;
            let mut note = String::from(
                "NFrp 支持经由面板 API 动态增删代理，删除时按来源选择服务端或客户端端点",
            );
            if input.role == InstanceRole::Client {
                note.push_str("；客户端不暴露流量字节，监控按代理数与连接态展示");
            }
            caps.notes.push(CapabilityNote {
                level: "info".into(),
                text: note,
            });
        }
        (ApiKind::Official, InstanceRole::Server) => {
            caps.can_dynamic_proxy = false;
            caps.can_manage_visitors = true;
            caps.metrics_detail = MetricDetail::Aggregated;
            caps.can_reload = false;
            caps.notes.push(CapabilityNote {
                level: "warning".into(),
                text: "官方 frps 不提供代理动态增删接口，面板将改写配置文件后重启生效".into(),
            });
            caps.notes.push(CapabilityNote {
                level: "info".into(),
                text: "官方 frps 不暴露流量字节，仅能展示连接数与在线态".into(),
            });
            if caps.flavor_version.is_none() {
                if let Ok(st) = official::server_status(client).await {
                    caps.flavor_version = st.binary_version;
                }
            }
        }
        (ApiKind::Official, InstanceRole::Client) => {
            caps.can_dynamic_proxy = false;
            caps.can_manage_visitors = false;
            caps.metrics_detail = MetricDetail::Aggregated;
            caps.can_reload = input.owns_config;
            caps.notes.push(CapabilityNote {
                level: "warning".into(),
                text: "官方 frpc 0.71.x 的 PUT /api/config 返回 200 但不生效（实测），代理增删走配置文件改写 + 重启".into(),
            });
            if get("/api/config").map(|(s, _)| s == 200).unwrap_or(false) {
                caps.notes.push(CapabilityNote {
                    level: "info".into(),
                    text: "支持通过 /api/config 读取运行中的配置原文（只读）".into(),
                });
            }
        }
        (ApiKind::Unknown | ApiKind::None, _) => {
            caps.metrics_detail = MetricDetail::None;
            caps.can_dynamic_proxy = false;
            caps.can_reload = input.owns_config;
        }
    }

    // 4) Prometheus
    caps.prometheus = match metrics {
        Some(_) => PrometheusSupport::Enabled,
        None => {
            if matches!(api_kind, ApiKind::None) {
                PrometheusSupport::Unknown
            } else {
                PrometheusSupport::Off
            }
        }
    };
    if caps.prometheus == PrometheusSupport::Off && input.role == InstanceRole::Server {
        caps.notes.push(CapabilityNote {
            level: "info".into(),
            text: "未开启 Prometheus（配置 enablePrometheus = true 后可抓取 /metrics）".into(),
        });
    }

    caps
}

fn summarize(body: &str) -> String {
    let t = body.trim();
    let head: String = t.chars().take(160).collect();
    // 去掉可能的敏感片段
    crate::crypto::redact_line(&head)
}

/// 诊断用：直接列出探测看到的每个端点结果（给 `GET .../capabilities/diagnose` 用）。
pub async fn diagnose_endpoints(client: &FrpAdminClient) -> Vec<(String, u16, String)> {
    let paths: &[&str] = &[
        "/api/v2/version",
        "/api/v2/status",
        "/api/status",
        "/api/healthz",
        "/api/v2/clients?all=1",
        "/api/proxies",
        "/api/config",
        "/metrics",
    ];
    let mut out = Vec::new();
    for p in paths {
        match client.get(p).await {
            Ok(r) => out.push((p.to_string(), r.status.as_u16(), summarize(&r.body))),
            Err(e) => out.push((p.to_string(), 0, format!("不可达：{e}"))),
        }
    }
    out
}

/// 用于测试与调试：把证据渲染成可读文本。
pub fn evidence_to_notes(evidence: &ProbeEvidence) -> Vec<CapabilityNote> {
    evidence
        .endpoints
        .iter()
        .map(|(p, s, b)| CapabilityNote {
            level: if *s == 200 { "info" } else { "warning" }.into(),
            text: format!("{p} → HTTP {s}{}", if b.is_empty() { String::new() } else { format!("（{b}）") }),
        })
        .collect()
}

/// 官方服务端端口表：辅助"预检端口占用"。
pub async fn occupied_ports(client: &FrpAdminClient, api_kind: ApiKind) -> Vec<u16> {
    match api_kind {
        ApiKind::Nfrp => {
            let Ok(r) = client.get("/api/v2/ports").await else {
                return Vec::new();
            };
            let Ok(v) = r.json() else { return Vec::new() };
            v.get("data")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.get("port").and_then(Value::as_u64))
                        .map(|p| p as u16)
                        .collect()
                })
                .unwrap_or_default()
        }
        ApiKind::Official => official::server_ports(client)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(p, _)| p)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_redacts_secrets() {
        let s = summarize(r#"{"token":"abc123","ok":true}"#);
        assert!(!s.contains("abc123"));
    }

    #[test]
    fn unknown_caps_claim_nothing() {
        let caps = Capabilities::unknown(BinaryFlavor::Nfrp);
        assert!(!caps.can_start_stop);
        assert!(!caps.can_dynamic_proxy);
        assert_eq!(caps.metrics_detail, MetricDetail::None);
    }
}
