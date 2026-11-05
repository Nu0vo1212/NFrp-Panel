//! 审计日志端点。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::audit::AuditView;
use crate::db::repo::AuditFilter;
use crate::error::ApiError;
use crate::state::AppState;

use super::{AuthUser, ListResponse, PageQuery};

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub page_size: Option<i64>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
}

fn filter_of(q: &AuditQuery) -> AuditFilter {
    AuditFilter {
        action: q.action.clone().filter(|s| !s.is_empty()),
        result: q.result.clone().filter(|s| !s.is_empty()),
        actor: q.actor.clone().filter(|s| !s.is_empty()),
        target: q.target.clone().filter(|s| !s.is_empty()),
        from: q.from.clone().filter(|s| !s.is_empty()),
        to: q.to.clone().filter(|s| !s.is_empty()),
        q: q.q.clone().filter(|s| !s.is_empty()),
    }
}

pub async fn list(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Query(q): Query<AuditQuery>,
) -> Result<Json<ListResponse<AuditView>>, ApiError> {
    let (page, page_size) = PageQuery {
        page: q.page,
        page_size: q.page_size,
    }
    .resolve();
    let (items, total) = st.audit.list(&filter_of(&q), page, page_size).await?;
    Ok(Json(ListResponse::new(items, page, page_size, total)))
}

pub async fn export(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Query(q): Query<AuditQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let csv = st.audit.export_csv(&filter_of(&q)).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "text/csv; charset=utf-8".parse().unwrap(),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        "attachment; filename=\"audit-logs.csv\"".parse().unwrap(),
    );
    // Excel 打开 UTF-8 CSV 需要 BOM
    Ok((StatusCode::OK, headers, format!("\u{feff}{csv}")))
}
