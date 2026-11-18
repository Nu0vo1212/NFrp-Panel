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

/// 实时日志流（SSE）：周期性 tail 日志，只推送新增内容。
/// 增量判定用「行数 + 最后一行内容」指纹，日志轮转/清空后自动重新同步。
pub async fn stream(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<axum::response::Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>, ApiError> {
    let iid = Uuid::parse_str(&id).map_err(|_| ApiError::bad_request("实例 ID 不合法"))?;
    // 首次拉取在流建立前做，确保实例/权限有效
    let _ = st.driver(iid).await?;

    let stream = futures::stream::unfold(
        (st, iid, 0usize, String::new()),
        |(st, iid, last_count, last_tail)| async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                let page = match st.driver(iid).await {
                    Ok(d) => d.logs(LogQuery { offset: None, limit: Some(2000), q: None, level: None, tail: Some(true) }).await,
                    Err(_) => continue,
                };
                let page = match page { Ok(p) => p, Err(_) => continue };
                let lines = &page.lines;
                let tail = lines.last().map(|l| l.raw.clone()).unwrap_or_default();
                let rotated = lines.len() < last_count || (lines.len() == last_count && tail != last_tail);
                let start = if rotated { 0 } else { last_count };
                if lines.len() > start {
                    let chunk: Vec<serde_json::Value> = lines[start..]
                        .iter()
                        .map(|l| serde_json::json!({ "raw": l.raw, "level": l.level }))
                        .collect();
                    let event = axum::response::sse::Event::default()
                        .event("log")
                        .data(serde_json::json!({ "lines": chunk, "reset": rotated }).to_string());
                    return Some((Ok(event), (st, iid, lines.len(), tail)));
                }
            }
        },
    );
    Ok(axum::response::Sse::new(stream))
}
