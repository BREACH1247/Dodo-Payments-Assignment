use super::super::{AppState, Result};
use crate::ApiError;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

pub(crate) async fn webhook_worker(s: AppState) {
    loop {
        if let Err(error) = process_one_webhook(&s).await {
            tracing::error!(%error, "webhook worker error");
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn process_one_webhook(s: &AppState) -> Result<()> {
    let claimed = sqlx::query("WITH picked AS (SELECT id FROM webhook_deliveries WHERE status='pending' AND next_run_at<=now() AND (lease_until IS NULL OR lease_until<now()) ORDER BY next_run_at LIMIT 1 FOR UPDATE SKIP LOCKED) UPDATE webhook_deliveries d SET lease_until=now()+interval '15 seconds',attempt_count=d.attempt_count+1 FROM picked WHERE d.id=picked.id RETURNING d.id,d.event_id,d.endpoint_id,d.attempt_count")
        .fetch_optional(&s.db).await.map_err(ApiError::internal)?;
    let Some(row) = claimed else {
        return Ok(());
    };
    let delivery_id: Uuid = row.get("id");
    let event_id: i64 = row.get("event_id");
    let endpoint_id: Uuid = row.get("endpoint_id");
    let count: i32 = row.get("attempt_count");
    let detail = sqlx::query("SELECT e.payload,w.url,w.signing_secret FROM events e JOIN webhook_endpoints w ON w.id=$2 WHERE e.id=$1")
        .bind(event_id).bind(endpoint_id).fetch_one(&s.db).await.map_err(ApiError::internal)?;
    let payload: Value = detail.get("payload");
    let url: String = detail.get("url");
    let secret: Vec<u8> = detail.get("signing_secret");
    let body = serde_json::to_string(&payload).map_err(ApiError::internal)?;
    let timestamp = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let message = format!("{}.{}.{}", timestamp, event_id, body);
    let mut mac = Hmac::<Sha256>::new_from_slice(&secret).map_err(ApiError::internal)?;
    mac.update(message.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());
    let response = s
        .http
        .post(&url)
        .header("content-type", "application/json")
        .header("X-Dodo-Event-Id", event_id.to_string())
        .header("X-Dodo-Timestamp", timestamp)
        .header("X-Dodo-Signature", format!("v1={signature}"))
        .body(body)
        .send()
        .await;
    let http_status = response.as_ref().ok().map(|r| r.status().as_u16() as i32);
    if response.as_ref().is_ok_and(|r| r.status().is_success()) {
        sqlx::query("UPDATE webhook_deliveries SET status='delivered',lease_until=NULL,last_http_status=$2,delivered_at=now() WHERE id=$1")
            .bind(delivery_id).bind(http_status).execute(&s.db).await.map_err(ApiError::internal)?;
    } else {
        let error = response
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| format!("HTTP {}", http_status.unwrap_or(0)));
        if count >= 6 {
            sqlx::query("UPDATE webhook_deliveries SET status='exhausted',lease_until=NULL,last_http_status=$2,last_error=$3 WHERE id=$1")
                .bind(delivery_id).bind(http_status).bind(error).execute(&s.db).await.map_err(ApiError::internal)?;
        } else {
            let delays = [5_i64, 30, 120, 600, 3600];
            let delay = delays[(count - 1) as usize];
            sqlx::query("UPDATE webhook_deliveries SET lease_until=NULL,next_run_at=now()+($2 * interval '1 second'),last_http_status=$3,last_error=$4 WHERE id=$1")
                .bind(delivery_id).bind(delay).bind(http_status).bind(error).execute(&s.db).await.map_err(ApiError::internal)?;
        }
    }
    Ok(())
}
