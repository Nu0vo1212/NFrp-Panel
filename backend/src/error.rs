//! 统一错误类型与 HTTP 错误信封。
//!
//! 所有 API 错误都以 `{"error":{"code","message","details"}}` 返回；
//! `code` 是**接口契约**的一部分，前端据此决定按钮灰化 / 重试 / 引导。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::json;

/// 机器可读错误码。
pub mod code {
    pub const BAD_REQUEST: &str = "bad_request";
    pub const UNAUTHORIZED: &str = "unauthorized";
    pub const FORBIDDEN: &str = "forbidden";
    pub const NOT_FOUND: &str = "not_found";
    pub const CONFLICT: &str = "conflict";
    pub const DRIVER_UNSUPPORTED: &str = "driver_unsupported";
    pub const UPSTREAM_FAILED: &str = "upstream_failed";
    pub const VALIDATION_FAILED: &str = "validation_failed";
    pub const INTERNAL: &str = "internal";
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    Unauthorized(String),

    #[error("{0}")]
    Forbidden(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    /// 当前驱动不具备该能力（前端据此灰化入口并展示原因）。
    #[error("{0}")]
    DriverUnsupported(String),

    /// 下游失败：frp Admin API、SSH、部署等。
    #[error("{0}")]
    Upstream(String),

    #[error("{0}")]
    Validation(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::DriverUnsupported(msg.into())
    }
    pub fn upstream(msg: impl Into<String>) -> Self {
        Self::Upstream(msg.into())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::DriverUnsupported(_) => StatusCode::CONFLICT,
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => code::BAD_REQUEST,
            Self::Unauthorized(_) => code::UNAUTHORIZED,
            Self::Forbidden(_) => code::FORBIDDEN,
            Self::NotFound(_) => code::NOT_FOUND,
            Self::Conflict(_) => code::CONFLICT,
            Self::DriverUnsupported(_) => code::DRIVER_UNSUPPORTED,
            Self::Upstream(_) => code::UPSTREAM_FAILED,
            Self::Validation(_) => code::VALIDATION_FAILED,
            Self::Internal(_) => code::INTERNAL,
        }
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // 内部错误不把 anyhow 链直接吐给前端（可能含路径/凭据片段），只留一行摘要。
        let message = match &self {
            Self::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                "服务端内部错误，详见面板日志".to_string()
            }
            other => other.to_string(),
        };
        let status = self.status();
        let body = ErrorEnvelope {
            error: ErrorBody {
                code: self.code(),
                message,
                details: None,
            },
        };
        (status, Json(body)).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => ApiError::NotFound("记录不存在".into()),
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                ApiError::Conflict("记录已存在（唯一键冲突）".into())
            }
            other => ApiError::Internal(anyhow::Error::new(other)),
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError::Internal(anyhow::Error::new(e))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::Internal(anyhow::Error::new(e))
    }
}

/// 便捷构造：带 details 的错误。
pub fn with_details(err: ApiError, details: serde_json::Value) -> (ApiError, serde_json::Value) {
    (err, details)
}

/// 一个成功但需要附带提示的响应体。
#[derive(Debug, Serialize)]
pub struct OkMessage {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl OkMessage {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            details: None,
        }
    }
}

pub fn json_error_body(code: &str, message: &str) -> serde_json::Value {
    json!({ "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_and_status_mapping() {
        assert_eq!(ApiError::bad_request("x").status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            ApiError::unsupported("x").code(),
            code::DRIVER_UNSUPPORTED
        );
        assert_eq!(ApiError::upstream("x").status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn internal_error_display_keeps_detail_for_logs() {
        // Display 保留细节供日志排查；脱敏发生在 `IntoResponse`（HTTP 响应体）。
        let err = ApiError::Internal(anyhow::anyhow!("secret path C:/keys/id_rsa"));
        assert!(err.to_string().contains("id_rsa"));
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.code(), code::INTERNAL);
    }

    #[test]
    fn unsupported_maps_to_conflict() {
        let err = ApiError::unsupported("驱动不支持");
        assert_eq!(err.status(), StatusCode::CONFLICT);
        assert_eq!(err.code(), code::DRIVER_UNSUPPORTED);
    }
}
