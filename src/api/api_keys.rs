use super::{auth::AuthBusiness, AppState, Result};
use crate::{random_bytes, sha256, ApiError, ApiPath};
use axum::{extract::State, http::StatusCode, Json};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use uuid::Uuid;

pub(crate) async fn create_api_key(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
) -> Result<(StatusCode, Json<Value>)> {
    let prefix = hex::encode(random_bytes::<4>());
    let secret = URL_SAFE_NO_PAD.encode(random_bytes::<32>());
    let key = format!("dodo_live_{prefix}_{secret}");
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO api_keys(id,business_id,prefix,key_hash) VALUES($1,$2,$3,$4)")
        .bind(id)
        .bind(b)
        .bind(&prefix)
        .bind(sha256(key.as_bytes()))
        .execute(&s.db)
        .await
        .map_err(ApiError::internal)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":id,"prefix":prefix,"api_key":key})),
    ))
}

pub(crate) async fn revoke_api_key(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode> {
    let result = sqlx::query("UPDATE api_keys SET revoked_at=now() WHERE id=$1 AND business_id=$2 AND revoked_at IS NULL")
        .bind(id).bind(b).execute(&s.db).await.map_err(ApiError::internal)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}
