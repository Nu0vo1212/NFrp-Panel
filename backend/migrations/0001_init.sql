-- FrpPanel 初始 schema
-- 说明：
--  * 所有敏感字段只存密文（v1:<base64>）或引用，绝不明文落库；
--  * 时间统一 RFC3339 UTC 文本；
--  * capabilities / paths 等结构用 JSON 文本列，方便扩展。

CREATE TABLE IF NOT EXISTS users (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('admin', 'viewer')),
    created_at    TEXT NOT NULL,
    last_login_at TEXT
);

CREATE TABLE IF NOT EXISTS credentials (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL UNIQUE,
    kind           TEXT NOT NULL CHECK (kind IN ('ssh_password','ssh_private_key','admin_password','token')),
    username       TEXT,
    -- 明文加密后存储：AAD = credential:<id>:secret
    secret_enc     TEXT NOT NULL,
    -- ssh_private_key 的 passphrase（可选）
    passphrase_enc TEXT,
    algorithm_hint TEXT,
    created_at     TEXT NOT NULL,
    last_used_at   TEXT
);

CREATE TABLE IF NOT EXISTS instances (
    id                 TEXT PRIMARY KEY,
    name               TEXT NOT NULL UNIQUE,
    role               TEXT NOT NULL CHECK (role IN ('server','client')),
    driver             TEXT NOT NULL,
    flavor             TEXT NOT NULL,
    enabled            INTEGER NOT NULL DEFAULT 1,
    tags               TEXT NOT NULL DEFAULT '[]',
    connection         TEXT NOT NULL,          -- JSON: Connection
    paths              TEXT NOT NULL,          -- JSON: InstancePaths
    service_mode       TEXT NOT NULL DEFAULT 'process',
    service_unit       TEXT,
    -- Admin API：密码单独加密存这里，JSON 列里不出现明文
    admin_api_base_url TEXT NOT NULL DEFAULT '',
    admin_api_user     TEXT NOT NULL DEFAULT '',
    admin_api_password_enc TEXT,
    admin_api_prometheus_url TEXT,
    admin_api_enabled  INTEGER NOT NULL DEFAULT 1,
    binary_version     TEXT,
    deployment_status  TEXT NOT NULL DEFAULT 'not_deployed',
    capabilities       TEXT,                   -- JSON: Capabilities（探测结果缓存）
    last_deployed_at   TEXT,
    last_error         TEXT,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS host_keys (
    id          TEXT PRIMARY KEY,
    host        TEXT NOT NULL,
    port        INTEGER NOT NULL,
    algorithm   TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    public_key  TEXT NOT NULL,
    trusted_at  TEXT NOT NULL,
    UNIQUE (host, port, algorithm)
);

CREATE TABLE IF NOT EXISTS audit_logs (
    id          TEXT PRIMARY KEY,
    ts          TEXT NOT NULL,
    actor       TEXT NOT NULL,
    actor_ip    TEXT NOT NULL DEFAULT '',
    action      TEXT NOT NULL,
    target      TEXT NOT NULL DEFAULT '',
    result      TEXT NOT NULL CHECK (result IN ('success','failure','denied')),
    duration_ms INTEGER NOT NULL DEFAULT 0,
    detail      TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_logs (ts DESC);
CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_logs (action);

CREATE TABLE IF NOT EXISTS metric_samples (
    instance_id TEXT NOT NULL,
    ts          INTEGER NOT NULL,    -- Unix 秒
    bytes_in    INTEGER NOT NULL DEFAULT 0,
    bytes_out   INTEGER NOT NULL DEFAULT 0,
    conns       INTEGER NOT NULL DEFAULT 0,
    clients     INTEGER NOT NULL DEFAULT 0,
    proxies     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (instance_id, ts)
);

CREATE INDEX IF NOT EXISTS idx_metric_ts ON metric_samples (instance_id, ts DESC);

CREATE TABLE IF NOT EXISTS config_backups (
    id          TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    path        TEXT NOT NULL,
    content_sha TEXT NOT NULL,
    size        INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    label       TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_backup_instance ON config_backups (instance_id, created_at DESC);

CREATE TABLE IF NOT EXISTS deploy_tasks (
    id          TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    kind        TEXT NOT NULL,
    state       TEXT NOT NULL,
    steps       TEXT NOT NULL DEFAULT '[]',
    logs        TEXT NOT NULL DEFAULT '[]',
    error       TEXT,
    rollback_of TEXT,
    started_at  TEXT NOT NULL,
    finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_deploy_instance ON deploy_tasks (instance_id, started_at DESC);

-- 面板运行期设置（键值对，JSON 值）
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
