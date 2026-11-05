//! 日志查看、搜索与下载。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::{LogPage, LogQuery};
use crate::error::ApiError;
use crate::state::AppState;

use super::AuthUser;

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub tail: Option<bool>,
}

pub async fn list(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<LogsQuery>,
) -> Result<Json<LogPage>, ApiError> {
    let iid =
        Uuid::parse_str(&id).map_err(|_| ApiError::bad_request("实例 ID 不合法"))?;
    let driver = st.driver(iid).await?;
    let page = driver
        .logs(LogQuery {
            offset: q.offset,
            limit: q.limit,
            q: q.q,
            level: q.level,
            tail: q.tail,
        })
        .await?;
    Ok(Json(page))
}

/// 下载日志（当前实现：把最近 N 行打包成 text/plain 附件）。
pub async fn download(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<ListQuerySize>,
) -> Result<impl IntoResponse, ApiError> {
    let iid =
        Uuid::parse_str(&id).map_err(|_| ApiError::bad_request("实例 ID 不合法"))?;
    let driver = st.driver(iid).await?;
    let page = driver
        .logs(LogQuery {
            offset: None,
            limit: Some(q.limit.unwrap_or(2000).clamp(1, 5000)),
            q: None,
            level: None,
            tail: Some(true),
        })
        .await?;

    let body = page
        .lines
        .iter()
        .map(|l| l.raw.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "text/plain; charset=utf-8".parse().unwrap(),
    );
    let filename = format!("{}-logs.txt", id);
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{filename}\"").parse().unwrap(),
    );
    Ok((StatusCode::OK, headers, body))
}

#[derive(Debug, Deserialize)]
pub struct ListQuerySize {
    #[serde(default)]
    pub limit: Option<usize>,
}
