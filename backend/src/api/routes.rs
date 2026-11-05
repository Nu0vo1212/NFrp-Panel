//! 路由装配：把所有端点挂到 `/api` 下，并配置中间件。

use std::sync::Arc;

use axum::routing::{delete, get, patch, post};
use axum::{middleware, Router};
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

use super::{audit, auth, config, credentials, deploy, instances, logs, metrics, proxies, settings, ssh};

pub fn build(state: Arc<AppState>) -> Router {
    let api = api_router(state.clone());

    // 静态资源：存在则挂载前端构建产物，否则只提供 API。
    //
    // ★ 挂载方式：`fallback_service(ServeDir)` + 自定义 404 → index.html。
    //   不能只把 `/assets` 单独 nest 再把 index 当 fallback —— 那样 `/assets/*`
    //   会先被 fallback 捕获（返回 index.html），浏览器拿到 HTML 当 JS 解析失败。
    let mut router = Router::new().nest("/api", api).with_state(state.clone());

    let static_dir = state.cfg.raw.server.static_dir.clone();
    if !static_dir.is_empty() {
        let dir = state.cfg.resolve_base(&static_dir);
        if dir.exists() {
            let index = dir.join("index.html");
            let serve_dir = tower_http::services::ServeDir::new(&dir)
                .append_index_html_on_directories(false)
                // 未命中的路径统一回 SPA 入口（前端路由自己处理）
                .fallback(tower_http::services::ServeFile::new(&index));
            router = router.fallback_service(serve_dir);
        } else {
            tracing::warn!(dir = %dir.display(), "静态目录不存在，跳过前端资源挂载");
        }
    }

    router
        .layer(RequestBodyLimitLayer::new(132 * 1024 * 1024)) // 允许上传 128MB 二进制
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(reject_unauthenticated))
}

/// 粗粒度防护：对于 `/api` 下除白名单外、完全没有 Authorization 头的请求，
/// 直接 401，避免每个 handler 都重复一次"没登录"的判断。
async fn reject_unauthenticated(
    req: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    const PUBLIC: &[&str] = &[
        "/api/health",
        "/api/auth/login",
        "/api/auth/refresh",
    ];
    let path = req.uri().path();
    let is_api = path.starts_with("/api");
    let has_auth = req.headers().contains_key(axum::http::header::AUTHORIZATION);
    if is_api && !has_auth && !PUBLIC.contains(&path) {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(crate::error::json_error_body(
                crate::error::code::UNAUTHORIZED,
                "未登录：请先通过 POST /api/auth/login 获取 token",
            )),
        )
            .into_response();
    }
    next.run(req).await
}

use axum::response::IntoResponse;

fn api_router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        // ---- 健康检查与认证 ----
        .route("/health", get(auth::health))
        .route("/auth/login", post(auth::login))
        .route("/auth/refresh", post(auth::refresh))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/password", post(auth::change_password))
        // ---- 实例 ----
        .route("/instances", get(instances::list).post(instances::create))
        .route(
            "/instances/{id}",
            get(instances::get)
                .patch(instances::update)
                .delete(instances::delete),
        )
        .route("/instances/{id}/start", post(instances::start))
        .route("/instances/{id}/stop", post(instances::stop))
        .route("/instances/{id}/restart", post(instances::restart))
        .route("/instances/{id}/status", get(instances::status))
        .route("/instances/{id}/health", get(instances::health_check))
        .route("/instances/{id}/reload", post(instances::reload))
        .route("/instances/{id}/capabilities", get(instances::capabilities))
        .route(
            "/instances/{id}/capabilities/diagnose",
            get(instances::diagnose_capabilities),
        )
        // ---- 配置 ----
        .route(
            "/instances/{id}/config",
            get(config::get_config).put(config::put_config),
        )
        .route("/instances/{id}/config/validate", post(config::validate))
        .route("/instances/{id}/config/backups", get(config::list_backups))
        .route("/instances/{id}/config/restore", post(config::restore))
        .route("/config/template", post(config::template))
        // ---- 代理 ----
        .route(
            "/instances/{id}/proxies",
            get(proxies::list).post(proxies::create),
        )
        .route(
            "/instances/{id}/proxies/{name}",
            patch(proxies::update).delete(proxies::delete),
        )
        .route(
            "/instances/{id}/proxies/preflight",
            post(proxies::preflight),
        )
        .route("/instances/{id}/visitors", get(proxies::list_visitors))
        // ---- 日志 ----
        .route("/instances/{id}/logs", get(logs::list))
        .route("/instances/{id}/logs/download", get(logs::download))
        // ---- 监控 ----
        .route("/instances/{id}/metrics", get(metrics::history))
        .route("/instances/{id}/metrics/live", get(metrics::live))
        // ---- 部署 ----
        .route(
            "/instances/{id}/deploy/preflight",
            post(deploy::preflight_run),
        )
        .route("/instances/{id}/deploy", post(deploy::deploy))
        .route("/instances/{id}/deploy/status", get(deploy::deploy_status))
        .route(
            "/instances/{id}/deploy/rollback",
            post(deploy::rollback),
        )
        .route(
            "/instances/{id}/deploy/history",
            get(deploy::deploy_history),
        )
        .route("/deploy/tasks/{task_id}/stream", get(deploy::task_stream))
        .route("/deploy/binary/fetch", post(deploy::fetch_binary))
        // ---- SSH ----
        .route("/ssh/test-connection", post(ssh::test_connection))
        .route("/ssh/probe", post(ssh::probe_environment))
        .route("/ssh/host-keys", get(ssh::list_host_keys))
        .route("/ssh/host-keys/trust", post(ssh::trust_host_key))
        .route("/ssh/host-keys/{id}", delete(ssh::delete_host_key))
        // ---- 凭据 ----
        .route(
            "/credentials",
            get(credentials::list).post(credentials::create),
        )
        .route("/credentials/{id}", delete(credentials::delete))
        // ---- 审计 ----
        .route("/audit-logs", get(audit::list))
        .route("/audit-logs/export", get(audit::export))
        // ---- 设置 ----
        .route("/settings", get(settings::get_settings).patch(settings::update_settings))
        .route("/settings/catalog", get(settings::catalog))
        .with_state(state)
}
