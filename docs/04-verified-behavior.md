# 实测行为记录（证据链）

> 本文记录架构决策所依赖的**真实观测**，便于复核与回归。
> 观测时间：2026-11-03 · 环境：Windows 11 / rustc 1.98.1 / node 24.19.0

## 0. 被测对象

| 对象 | 版本 | 来源 |
|---|---|---|
| NFrp（`frps` / `frpc`） | `nfrp-server 0.7.4`，frp 兼容版本 `0.71.0` | `E:\项目\RustFrp\dist\release\nfrp-windows-amd64.zip`（由 `dist\windows\frpc.exe` 构建产物同源） |
| 官方 frp | `0.71.1` | `https://github.com/fatedier/frp/releases/tag/v0.71.1`（`frp_0.71.1_windows_amd64.zip`） |
| NFrp 源码 | workspace `0.7.4` | `E:\项目\nfrp` |

版本探测方式（两套二进制都支持）：

```text
frps --version   →  nfrp-server 0.7.4      （NFrp 自身版本）
frps -v          →  0.71.0                 （frp 兼容版本，官方 frps 也是这个格式）
```

---

## 1. NFrp 服务端面板 API

启动命令：`frps.exe -c frps.toml`，配置含 `dashboard_port = 17500` + `webServer.port = 17500`（两者同时写）。

| 端点 | 鉴权 | 状态 | 说明 |
|---|---|---|---|
| `GET /api/status` | 无 | 200 | 顶层 `{status, clients, visitors, ports, limits, p2p_enabled}` |
| `GET /api/v2/version` | 无 | 200 | `{api:"v2", data:{api, server:"0.7.4", wire_protocols:["v1","v2"], frp_wire_version:"0.71.0"}}` |
| `GET /api/v2/status` | 无 | 200 | `{api, data:{metrics, counts, limits, features, audit}}` |
| `GET /api/v2/clients?all=1` | 无 | 200 | 分页信封 `{api, data:[...], pagination}` |
| `GET /api/v2/proxies?all=1` | 无 | 200 | 同上 |
| `GET /api/v2/ports` | 无 | 200 | `{api, data:[{port, backends:[...]}]}` |
| `GET /api/healthz` | 无 | 200 | 纯文本 `ok` |
| `GET /metrics` | 无 | 200 | 需 `enablePrometheus = true` |
| `POST /api/proxies/add` | Basic | 200 | 服务端向客户端推代理（`{run_id, proxy}`） |
| `POST /api/proxies/remove` | Basic | — | 移除 |
| `POST /api/clients/kick` | Basic | — | 踢出客户端 |

> **注意**：NFrp 的 `/api/*` 在实测中**未要求 Basic Auth**（`/api/status` 无头也返回 200），
> 鉴权只加在写接口（`POST`）上。面板一律携带凭据，不做假设。

关键响应样本（服务端，1 个客户端在线）：

```json
{"clients":[{"backlog":0,"client_id":"client-34592","idle_work_conns":1,"managed":true,
 "proxies":["web"],"run_id":"991087b7b4a4a526832a1febbd5fbe8d","user":""}],
 "limits":{"max_clients":0,"max_conns_per_client":0,"max_total_conns":0},
 "p2p_enabled":false,"ports":[17080],
 "status":{"bytes_down":0,"bytes_up":0,"clients_active":1,"clients_total":1,
  "conns_active":0,"conns_total":0,"proxies_active":1,"proxies_total":1,"uptime_secs":21,...},
 "visitors":[]}
```

`POST /api/proxies/add` 成功返回：

```json
{"ok":true,"message":"代理 [dyn1] 已生效：127.0.0.1:17081"}
```

端口被占时（v2 路径）：

```json
{"api":"v2","error":{"code":"admin_failed","message":"服务端注册代理失败（已回滚）：监听 127.0.0.1:17081 失败"}}
```

⇒ 面板预检必须先做端口占用检测，否则 UI 上会出现"看起来成功、实际失败"。

---

## 2. NFrp 客户端面板 API

`[webServer] port = 17400` + Basic Auth（`cuser`）。

| 端点 | 鉴权 | 结果 |
|---|---|---|
| `GET /api/status` | Basic | 200 `{data:{client_id, connected, run_id, wire_protocol, proxies:[...], visitor_count, store, features}}` |
| `GET /api/proxies` | Basic | 200 `{data:[{name,type,local_addr,remote_port,custom_domains,healthy,dynamic}]}` |
| `GET /api/healthz` | 无 | 200 `ok` |
| `POST /api/proxies/add` | Basic + **`X-Nfrp-Client: 1`** | 缺头 → **403** `{"error":{"code":"forbidden","message":"写操作必须带 x-nfrp-client 头…"}}`；带头 → 200 `{"data":{"message":"代理 [dyn2] 已生效：127.0.0.1:17082"}}` |
| `POST /api/proxies/remove` | 同上 | 200 `{"data":{"message":"代理 [dyn2] 已移除"}}`；移除不存在的 → **400** `{"error":{"code":"remove_failed","message":"本地没有代理 [dyn1]"}}` |

**重要语义差异**：通过**服务端**推送的代理（`dyn1`）**不在客户端本地表**里，
客户端 `POST /api/proxies/remove` 会失败（"本地没有代理"）。⇒ 删除必须按来源选择端点：
- 客户端本地动态代理 → 客户端 Admin API；
- 服务端推送的代理 → 服务端 Admin API（`run_id` + 名称）。

---

## 3. NFrp 热重载

- 服务端 `server/src/reload.rs::watch`：**每秒轮询配置文件 mtime**，变化后重新解析并应用
  可热改字段；解析失败忽略本次变更（保持旧配置）；安全闸门不通过则拒绝。
- 实测：改写 `log_level` 后 4 秒内 `out.log` 出现热重载相关日志，进程未重启。
- **没有 HTTP reload 端点**；NFrp 未处理 SIGHUP。
- 优雅停止：`taskkill /PID <pid>`（Windows 的 WM_CLOSE → Rust 退出）→ **103 ms 内退出**。

⇒ 面板的 `reload()` = 写文件 + 等生效 + 校验；不是"调一个 API 就完事"。

---

## 4. 官方 frp 0.71.1 服务端

配置：`bindPort = 17100`、`webServer.port = 17600`、`enablePrometheus = true`。

| 端点 | 状态 | 备注 |
|---|---|---|
| `GET /api/status` | **404** | 官方 frps 没有这个端点 |
| `GET /api/v2/version` | **404** | |
| `GET /api/v2/status` | **404** | |
| `GET /api/v2/clients?all=1` | 200 | `{code,msg,data:{total,page,pageSize,items:[{key,user,clientID,runID,version,wireProtocol,hostname,clientIP,firstConnectedAt,lastConnectedAt,online}]}}` |
| `GET /api/v2/proxies?all=1` | 200 | `items:[{name,user,clientID,spec:{type,tcp:{transport,loadBalancer,remotePort}},status:{phase,todayTrafficIn,todayTrafficOut,curConns,lastStartAt,lastCloseAt}}]` |
| `GET /api/v2/visitors` | 200（空） | |
| `GET /api/healthz` | **404** | |
| `GET /metrics` | 200 | `frp_server_client_counts`、`frp_server_proxy_counts{type}`、`frp_server_proxy_counts_detailed{name,type}` |

⇒ 官方服务端的**代理指标只有连接数级别**，**没有流量字节**；流量需从 `metrics` 抓取（grafana 风格）
或从 frps 日志推算。面板的 `MetricDetail` 能力位必须体现这一点。

---

## 5. 官方 frp 0.71.1 客户端

| 端点 | 状态 | 说明 |
|---|---|---|
| `GET /api/status` | 200 | 按类型分组：`{"tcp":[{"name","type","status":"running","err","local_addr","plugin","remote_addr"}]}` |
| `GET /api/config` | 200 | 返回**配置文件 TOML 原文** |
| `PUT /api/config` | 200（空数组） | **实测无效**：扁平体与 `{name,type,params}` 体都返回 200，但 `/api/status` 无新增代理 |
| `PUT /api/config/{name}` | **404** | 本版本不存在 |
| `GET /api/reload` | 连接被关闭 | 非 GET 端点 |
| `GET /api/proxies` | **502** | 官方无此端点（frpc 内部转发失败） |

⇒ **官方 frpc 的代理动态管理不可依赖 Admin API**，必须以「配置文件改写 + reload/重启」实现。

### 5.1 配置格式严格性（实测会直接退出）

把 `webServer.*` 写在 `[[proxies]]` **之后**（即落在最后一个 proxy 表内）时：

```text
decode proxy at index 0: unmarshal ProxyConfig error: json: unknown field "webServer"
```

⇒ 官方 frpc 对未知字段**零容忍**。面板下发配置必须：
1. 使用目标二进制认识的字段名（官方 camelCase / NFrp 两者兼容）；
2. 生成时把全局段放在**所有 `[[proxies]]` 之前**；
3. 用目标二进制的 `verify`/`--check` 做最后一道校验。

### 5.2 热重载 / 信号

- 官方 frpc 支持 `--enableReload`（`frpc reload -c ...`），未开启时只能重启。
- 实测 `taskkill /PID`（Windows 优雅终止）→ frpc **直接退出**（非 reload）。

⇒ 面板在 Windows 上对官方 frpc 的「保存配置」采用**重启生效**，并在 UI 明示。

---

## 6. 配置字段差异（源码 + 实测）

来源：`nfrp-common/src/config.rs`（`ServerConfig` / `ClientConfig` / `ProxyConfig` / `WebServerConfig`）
与官方 frp 0.71 的 `camelCase` 键。

| 语义 | NFrp 原生 | 官方 frp | 面板生成策略 |
|---|---|---|---|
| 服务端监听 | `bind_addr` / `bind_port` | `bindAddr` / `bindPort` | 按目标 flavor 输出 |
| 面板端口 | `dashboard_port`（备用 `webServer.port`） | `webServer.port` | 统一输出 `webServer.*`（两边都认） |
| 面板凭据 | `dashboard_user/pwd`（优先）或 `webServer.user/password` | `webServer.user/password` | 统一 `webServer.*` |
| token | 顶层 `token` 或 `auth.token` | `auth.token` | 统一 `auth.token` |
| 客户端本地地址 | `local_addr = "ip:port"`（也认两段式） | `localIP` + `localPort` | 官方 → 两段式；NFrp → 合并式 |
| 代理类型键 | `type` | `type` | 同 |
| 传输 | `transport_protocol`、`tcp_mux`、`tls_enable` | `transport.protocol`、`transport.tcpMux`、`transport.tls.enable` | 按 target 输出 |
| Prometheus | `enablePrometheus` = `enable_prometheus` | `enablePrometheus` | 统一官方键 |
| UDP 报文上限 | `udpPacketSize` 默认 **65507** | 默认 **1500** | 显式写出并提示差异 |
| 协议扩展 | `[nlink]`、`[vnet]`、`[virtualNet]` | 不支持（未知字段直接退出） | 目标为官方时**移除并告警** |
| includes | 支持（仅合并 proxies/visitors） | 支持 | 保留 |

`nfrp-common` 提供的现成能力（面板直接复用，不重复造）：
- `config::{ServerConfig, ClientConfig}::load()` —— 同时吃 snake_case / camelCase / 两段式；
- `effective_dashboard_port()` / `effective_dashboard_creds()` —— 统一 `dashboard_*` 与 `webServer.*`；
- `effective_auth()` —— `auth.token` 与顶层 `token` 的唯一真源；
- `ClientConfig::merge_includes()` —— includes 合并与重名检测；
- `validate_remote_proxy()` —— 远程下发代理的合法性约束。

---

## 7. 对实现的直接影响（检查表）

- [x] `reload` 语义按 flavor 分流，禁止假设存在 reload 端点
- [x] Admin API 适配层区分 `nfrp` / `official` 两套结构，`ApiKind` 存库
- [x] 写操作自动附加 `X-Nfrp-Client: 1`（仅 NFrp 客户端）
- [x] 代理删除按"来源"选择端点（客户端本地 / 服务端推送）
- [x] 官方 frpc 的代理增删走配置文件 + 重启
- [x] 配置生成按目标 flavor 输出字段，官方目标剔除 NFrp 专有段
- [x] 配置写入前必做端口占用与 `verify` 校验
- [x] 指标能力位区分 `Full`（NFrp 字节）/ `Aggregated`（官方连接数）/ `None`
- [x] Windows 停止进程使用优雅终止并等待，再强制
