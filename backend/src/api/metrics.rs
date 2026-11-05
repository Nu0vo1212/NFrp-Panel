//! 监控指标端点：实时快照 + 历史趋势。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::MetricsSnapshot;
use crate::error::ApiError;
use crate::metrics::MetricsResponse;
use crate::state::AppState;

use super::AuthUser;

#[derive(Debug, Deserialize)]
pub struct RangeQuery {
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub max_points: Option<i64>,
}

fn parse_id(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::bad_request("实例 ID 不合法"))
}

pub async fn history(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<MetricsResponse>, ApiError> {
    let iid = parse_id(&id)?;
    let range = crate::metrics::parse_range(q.range.as_deref().unwrap_or("1h"));
    let resp = crate::metrics::query(&st.db, iid, range, q.max_points.unwrap_or(720)).await?;
    Ok(Json(resp))
}

pub async fn live(
    State(st): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<MetricsSnapshot>, ApiError> {
    let driver = st.driver(parse_id(&id)?).await?;
    Ok(Json(driver.metrics().await?))
}
