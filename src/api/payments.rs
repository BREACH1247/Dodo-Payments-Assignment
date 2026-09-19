use super::{
    auth::AuthBusiness,
    models::{Attempt, PayRequest},
    AppState, Result,
};
use crate::{sha256, ApiError, ApiJson, ApiPath};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

fn request_hash(invoice_id: Uuid, token: &str) -> Vec<u8> {
    sha256(
        &serde_json::to_vec(&json!({"invoice_id": invoice_id, "mock_card_token": token}))
            .expect("JSON serialization"),
    )
}

fn replay(row: sqlx::postgres::PgRow, expected_hash: &[u8]) -> Result<(StatusCode, Json<Value>)> {
    let stored_hash: Vec<u8> = row.get("request_hash");
    if stored_hash != expected_hash {
        return Err(ApiError::conflict(
            "idempotency_key_reused",
            "Idempotency key was used with a different request",
        ));
    }
    let status: i16 = row.get("response_status");
    let body: String = row.get("response_body");
    let value = serde_json::from_str(&body).map_err(ApiError::internal)?;
    Ok((
        StatusCode::from_u16(status as u16).map_err(ApiError::internal)?,
        Json(value),
    ))
}

pub(crate) async fn pay_invoice(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiPath(id): ApiPath<Uuid>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<PayRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::bad("Idempotency-Key header is required"))?;
    if key.is_empty() || key.len() > 255 {
        return Err(ApiError::bad("Idempotency-Key must be 1..255 characters"));
    }
    if ![
        "tok_success",
        "tok_insufficient_funds",
        "tok_card_declined",
        "tok_timeout",
        "tok_network_error",
    ]
    .contains(&input.mock_card_token.as_str())
    {
        return Err(ApiError::bad("Unknown mock card token"));
    }
    let digest = request_hash(id, &input.mock_card_token);
    let existing = sqlx::query("SELECT request_hash,response_status,response_body FROM payment_attempts WHERE business_id=$1 AND idempotency_key=$2")
        .bind(b).bind(key).fetch_optional(&s.db).await.map_err(ApiError::internal)?;
    if let Some(row) = existing {
        return replay(row, &digest);
    }

    let mut tx = s.db.begin().await.map_err(ApiError::internal)?;
    let invoice =
        sqlx::query("SELECT state FROM invoices WHERE id=$1 AND business_id=$2 FOR UPDATE")
            .bind(id)
            .bind(b)
            .fetch_optional(&mut *tx)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::not_found)?;
    let existing = sqlx::query("SELECT request_hash,response_status,response_body FROM payment_attempts WHERE business_id=$1 AND idempotency_key=$2")
        .bind(b).bind(key).fetch_optional(&mut *tx).await.map_err(ApiError::internal)?;
    if let Some(row) = existing {
        return replay(row, &digest);
    }
    let state: String = invoice.get("state");
    if state == "paid" {
        return Err(ApiError::conflict(
            "invoice_already_paid",
            "Paid invoices cannot be charged again",
        ));
    }
    if state != "open" {
        return Err(ApiError::conflict(
            "invoice_voided",
            "Void invoices cannot be charged",
        ));
    }
    let pending =
        sqlx::query("SELECT 1 FROM payment_attempts WHERE invoice_id=$1 AND status='pending'")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(ApiError::internal)?
            .is_some();
    if pending {
        return Err(ApiError::conflict(
            "payment_in_progress",
            "Invoice has a pending payment",
        ));
    }
    let attempt_id = Uuid::new_v4();
    let body = json!({"attempt_id":attempt_id,"status":"pending"});
    let serialized = serde_json::to_string(&body).map_err(ApiError::internal)?;
    let insert = sqlx::query("INSERT INTO payment_attempts(id,business_id,invoice_id,idempotency_key,request_hash,mock_token,response_status,response_body) VALUES($1,$2,$3,$4,$5,$6,202,$7)")
        .bind(attempt_id).bind(b).bind(id).bind(key).bind(&digest).bind(&input.mock_card_token).bind(serialized)
        .execute(&mut *tx).await;
    if let Err(error) = insert {
        let unique_violation = error
            .as_database_error()
            .is_some_and(|e| e.code().as_deref() == Some("23505"));
        tx.rollback().await.map_err(ApiError::internal)?;
        if unique_violation {
            let existing = sqlx::query("SELECT request_hash,response_status,response_body FROM payment_attempts WHERE business_id=$1 AND idempotency_key=$2")
                .bind(b).bind(key).fetch_optional(&s.db).await.map_err(ApiError::internal)?;
            if let Some(row) = existing {
                return replay(row, &digest);
            }
            return Err(ApiError::conflict(
                "payment_in_progress",
                "Invoice has a pending payment",
            ));
        }
        return Err(ApiError::internal(error));
    }
    tx.commit().await.map_err(ApiError::internal)?;
    Ok((StatusCode::ACCEPTED, Json(body)))
}

pub(crate) async fn get_attempt(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Attempt>> {
    let row = sqlx::query_as::<_, Attempt>("SELECT id,invoice_id,status,psp_ref,failure_code,created_at,updated_at FROM payment_attempts WHERE id=$1 AND business_id=$2")
        .bind(id).bind(b).fetch_optional(&s.db).await.map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
    Ok(Json(row))
}
