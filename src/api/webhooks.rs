use super::{
    auth::AuthBusiness,
    models::{Event, EventQuery, NewEndpoint},
    AppState, Result,
};
use crate::{random_bytes, ApiError, ApiJson, ApiQuery};
use axum::{extract::State, http::StatusCode, Json};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub(crate) async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    business_id: Uuid,
    invoice_id: Uuid,
    attempt_id: Option<Uuid>,
    event_type: &str,
    body: Value,
) -> Result<()> {
    let row = sqlx::query("INSERT INTO events(business_id,invoice_id,payment_attempt_id,event_type,payload) VALUES($1,$2,$3,$4,$5) RETURNING id,created_at")
        .bind(business_id).bind(invoice_id).bind(attempt_id).bind(event_type).bind(&body)
        .fetch_one(&mut **tx).await.map_err(ApiError::internal)?;
    let event_id: i64 = row.get("id");
    let created_at: DateTime<Utc> = row.get("created_at");
    let payload = json!({"id":event_id,"type":event_type,"created_at":created_at,"data":body});
    sqlx::query("UPDATE events SET payload=$1 WHERE id=$2")
        .bind(payload)
        .bind(event_id)
        .execute(&mut **tx)
        .await
        .map_err(ApiError::internal)?;
    let endpoints =
        sqlx::query("SELECT id FROM webhook_endpoints WHERE business_id=$1 AND active=true")
            .bind(business_id)
            .fetch_all(&mut **tx)
            .await
            .map_err(ApiError::internal)?;
    for endpoint in endpoints {
        let endpoint_id: Uuid = endpoint.get("id");
        sqlx::query("INSERT INTO webhook_deliveries(id,business_id,event_id,endpoint_id) VALUES($1,$2,$3,$4)")
            .bind(Uuid::new_v4()).bind(business_id).bind(event_id).bind(endpoint_id)
            .execute(&mut **tx).await.map_err(ApiError::internal)?;
    }
    Ok(())
}

pub(crate) async fn list_events(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiQuery(q): ApiQuery<EventQuery>,
) -> Result<Json<Vec<Event>>> {
    let limit = q.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) || q.after_id.unwrap_or(0) < 0 {
        return Err(ApiError::bad("Invalid event cursor or limit"));
    }
    let rows = sqlx::query_as::<_, Event>("SELECT id,event_type,payload,created_at FROM events WHERE business_id=$1 AND id>$2 ORDER BY id LIMIT $3")
        .bind(b).bind(q.after_id.unwrap_or(0)).bind(limit).fetch_all(&s.db).await.map_err(ApiError::internal)?;
    Ok(Json(rows))
}

pub(crate) async fn create_endpoint(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiJson(input): ApiJson<NewEndpoint>,
) -> Result<(StatusCode, Json<Value>)> {
    let url = url::Url::parse(&input.url).map_err(|_| ApiError::bad("Invalid webhook URL"))?;
    if !["http", "https"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ApiError::bad(
            "Webhook URL must be HTTP or HTTPS without credentials or fragment",
        ));
    }
    let id = Uuid::new_v4();
    let secret = random_bytes::<32>();
    sqlx::query(
        "INSERT INTO webhook_endpoints(id,business_id,url,signing_secret) VALUES($1,$2,$3,$4)",
    )
    .bind(id)
    .bind(b)
    .bind(url.as_str())
    .bind(secret.to_vec())
    .execute(&s.db)
    .await
    .map_err(ApiError::internal)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":id,"url":url.as_str(),"signing_secret":URL_SAFE_NO_PAD.encode(secret)})),
    ))
}
