use super::{AppState, Result};
use crate::{sha256, ApiError};
use axum::{extract::FromRequestParts, http::request::Parts};
use sqlx::Row;
use uuid::Uuid;

pub(crate) struct AuthBusiness(pub(crate) Uuid);

impl FromRequestParts<AppState> for AuthBusiness {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let key = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(ApiError::unauthorized)?;
        let digest = sha256(key.as_bytes());
        let row = sqlx::query(
            "SELECT business_id FROM api_keys WHERE key_hash=$1 AND revoked_at IS NULL",
        )
        .bind(digest)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::internal)?;
        row.map(|r| Self(r.get("business_id")))
            .ok_or_else(ApiError::unauthorized)
    }
}
