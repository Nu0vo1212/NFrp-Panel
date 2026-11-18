-- 安全加固增量迁移（v0.2.0）
--  * token_version：令牌版本号，改密/降权/删除/登出时 +1 使旧令牌立即失效
--  * failed_attempts / locked_until：登录失败计数与锁定截止（防暴力破解）
--  * revoked_tokens：已被主动撤销的 refresh 令牌 jti（保留到自然过期）
--  * alert_webhooks：告警 webhook 配置（实例异常 / 部署失败主动通知）
--  * metric_alert_rules：指标阈值告警规则

ALTER TABLE users ADD COLUMN token_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN failed_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN locked_until TEXT;

CREATE TABLE IF NOT EXISTS revoked_tokens (
    jti         TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    expires_at  INTEGER NOT NULL,   -- Unix 秒，到期后可清理
    revoked_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_revoked_exp ON revoked_tokens (expires_at);

CREATE TABLE IF NOT EXISTS alert_webhooks (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    url         TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    events      TEXT NOT NULL DEFAULT '[]',   -- JSON 数组：instance_unhealthy / deploy_failed / metric_threshold
    secret      TEXT NOT NULL DEFAULT '',     -- HMAC 签名密钥（明文不入库场景留空）
    created_at  TEXT NOT NULL,
    last_sent_at TEXT,
    last_error  TEXT
);

CREATE TABLE IF NOT EXISTS metric_alert_rules (
    id          TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    metric      TEXT NOT NULL,      -- bytes_in / bytes_out / conns / clients / proxies
    op          TEXT NOT NULL,      -- gt / gte / lt / lte
    threshold   REAL NOT NULL,
    window_min  INTEGER NOT NULL DEFAULT 5,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_alert_rule_instance ON metric_alert_rules (instance_id);
