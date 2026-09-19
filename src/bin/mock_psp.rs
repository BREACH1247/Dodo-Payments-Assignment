use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use dodo_assignment::{init_tracing, sha256, ApiError};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::{env, time::Duration};
use uuid::Uuid;

#[derive(Clone)]
struct PspState {
    db: PgPool,
}

#[derive(Deserialize)]
struct ChargeRequest {
    attempt_id: Uuid,
    token: String,
    amount_cents: i64,
}

#[derive(Serialize)]
struct ChargeResponse {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    psp_ref: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let db = PgPoolOptions::new()
        .max_connections(5)
        .connect(&env::var("DATABASE_URL")?)
        .await?;
    let app = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/charges", post(create_charge))
        .route("/charges/{id}", get(get_charge))
        .route("/demo/webhook-sink", post(webhook_sink))
        .with_state(PspState { db });
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await?;
    tracing::info!("mock PSP listening on port 3001");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn create_charge(
    State(s): State<PspState>,
    Json(input): Json<ChargeRequest>,
) -> Result<(StatusCode, Json<ChargeResponse>), ApiError> {
    if input.amount_cents <= 0 {
        return Err(ApiError::bad("amount_cents must be positive"));
    }
    let (status, code) = match input.token.as_str() {
        "tok_success" | "tok_timeout" => ("succeeded", None),
        "tok_insufficient_funds" => ("failed", Some("insufficient_funds")),
        "tok_card_declined" => ("failed", Some("card_declined")),
        "tok_network_error" => ("failed", Some("network_error")),
        _ => return Err(ApiError::bad("Unknown mock token")),
    };
    let fingerprint = sha256(format!("{}:{}", input.amount_cents, input.token).as_bytes());
    let psp_ref = if status == "succeeded" {
        Some(Uuid::new_v4())
    } else {
        None
    };
    let inserted = sqlx::query("INSERT INTO psp_charges(attempt_id,token_hash,amount_cents,status,psp_ref,failure_code) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT (attempt_id) DO UPDATE SET request_count=psp_charges.request_count+1 RETURNING (xmax = 0) AS inserted")
        .bind(input.attempt_id).bind(&fingerprint).bind(input.amount_cents).bind(status).bind(psp_ref).bind(code)
        .fetch_one(&s.db).await.map_err(ApiError::internal)?.get("inserted");
    let row = sqlx::query("SELECT token_hash,amount_cents,status,psp_ref,failure_code FROM psp_charges WHERE attempt_id=$1")
        .bind(input.attempt_id).fetch_one(&s.db).await.map_err(ApiError::internal)?;
    if row.get::<Vec<u8>, _>("token_hash") != fingerprint
        || row.get::<i64, _>("amount_cents") != input.amount_cents
    {
        return Err(ApiError::conflict(
            "psp_idempotency_mismatch",
            "Attempt ID was used with a different charge",
        ));
    }
    let result = ChargeResponse {
        status: row.get("status"),
        psp_ref: row.get("psp_ref"),
        code: row.get("failure_code"),
    };
    if inserted {
        if input.token == "tok_timeout" {
            tokio::time::sleep(Duration::from_secs(30)).await;
        } else {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if input.token == "tok_network_error" {
            return Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "mock_network_error",
                "Simulated PSP network failure",
            ));
        }
    }
    Ok((StatusCode::OK, Json(result)))
}

async fn get_charge(
    State(s): State<PspState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ChargeResponse>, ApiError> {
    let row =
        sqlx::query("SELECT status,psp_ref,failure_code FROM psp_charges WHERE attempt_id=$1")
            .bind(id)
            .fetch_optional(&s.db)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::not_found)?;
    Ok(Json(ChargeResponse {
        status: row.get("status"),
        psp_ref: row.get("psp_ref"),
        code: row.get("failure_code"),
    }))
}

async fn webhook_sink(headers: HeaderMap, body: String) -> StatusCode {
    tracing::info!(event_id=?headers.get("X-Dodo-Event-Id"), signature=?headers.get("X-Dodo-Signature"), payload=%body, "demo webhook received");
    StatusCode::NO_CONTENT
}
