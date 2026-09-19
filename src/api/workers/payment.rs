use super::super::{webhooks::insert_event, AppState, Result};
use crate::ApiError;
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

#[derive(Deserialize)]
struct PspResult {
    status: String,
    psp_ref: Option<Uuid>,
    code: Option<String>,
}

pub(crate) async fn payment_worker(s: AppState) {
    loop {
        if let Err(error) = process_one_payment(&s).await {
            tracing::error!(%error, "payment worker error");
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn process_one_payment(s: &AppState) -> Result<()> {
    let claimed = sqlx::query("WITH picked AS (SELECT id FROM payment_attempts WHERE status='pending' AND next_run_at<=now() AND (lease_until IS NULL OR lease_until<now()) ORDER BY next_run_at LIMIT 1 FOR UPDATE SKIP LOCKED) UPDATE payment_attempts p SET lease_until=now()+interval '45 seconds', attempt_count=p.attempt_count+1 FROM picked WHERE p.id=picked.id RETURNING p.id,p.invoice_id,p.business_id,p.mock_token,p.attempt_count")
        .fetch_optional(&s.db).await.map_err(ApiError::internal)?;
    let Some(row) = claimed else {
        return Ok(());
    };
    let attempt_id: Uuid = row.get("id");
    let invoice_id: Uuid = row.get("invoice_id");
    let business_id: Uuid = row.get("business_id");
    let token: String = row.get("mock_token");
    let attempt_count: i32 = row.get("attempt_count");
    let result = psp_result(s, attempt_id, invoice_id, &token).await;
    if let Some(result) = result {
        if result.status == "succeeded" || result.status == "failed" {
            finalize_payment(s, business_id, invoice_id, attempt_id, result).await?;
            return Ok(());
        }
    }
    let delay = 2_i64.pow((attempt_count as u32).min(6)).min(60);
    sqlx::query("UPDATE payment_attempts SET lease_until=NULL,next_run_at=now()+($2 * interval '1 second'),updated_at=now() WHERE id=$1 AND status='pending'")
        .bind(attempt_id).bind(delay).execute(&s.db).await.map_err(ApiError::internal)?;
    Ok(())
}

async fn psp_lookup(s: &AppState, id: Uuid) -> Option<PspResult> {
    let response = s
        .http
        .get(format!("{}/charges/{}", s.psp_url, id))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<PspResult>().await.ok()
}

async fn psp_result(
    s: &AppState,
    attempt_id: Uuid,
    invoice_id: Uuid,
    token: &str,
) -> Option<PspResult> {
    if let Some(result) = psp_lookup(s, attempt_id).await {
        return Some(result);
    }
    let amount: i64 = sqlx::query_scalar("SELECT total_cents FROM invoices WHERE id=$1")
        .bind(invoice_id)
        .fetch_one(&s.db)
        .await
        .ok()?;
    let response = s
        .http
        .post(format!("{}/charges", s.psp_url))
        .json(&json!({"attempt_id":attempt_id,"token":token,"amount_cents":amount}))
        .send()
        .await;
    if let Ok(response) = response {
        if response.status().is_success() {
            if let Ok(result) = response.json::<PspResult>().await {
                return Some(result);
            }
        }
    }
    psp_lookup(s, attempt_id).await
}

async fn finalize_payment(
    s: &AppState,
    business_id: Uuid,
    invoice_id: Uuid,
    attempt_id: Uuid,
    outcome: PspResult,
) -> Result<()> {
    let mut tx = s.db.begin().await.map_err(ApiError::internal)?;
    let invoice = sqlx::query(
        "SELECT state,total_cents FROM invoices WHERE id=$1 AND business_id=$2 FOR UPDATE",
    )
    .bind(invoice_id)
    .bind(business_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::internal)?;
    let attempt = sqlx::query("SELECT status FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::internal)?;
    if attempt.get::<String, _>("status") != "pending" {
        return Ok(());
    }
    if invoice.get::<String, _>("state") != "open" {
        return Err(ApiError::internal("Pending payment on a non-open invoice"));
    }
    if outcome.status == "succeeded" {
        let psp_ref = outcome
            .psp_ref
            .ok_or_else(|| ApiError::internal("PSP success omitted reference"))?;
        sqlx::query("UPDATE payment_attempts SET status='succeeded',psp_ref=$2,lease_until=NULL,updated_at=now() WHERE id=$1")
            .bind(attempt_id).bind(psp_ref).execute(&mut *tx).await.map_err(ApiError::internal)?;
        sqlx::query("UPDATE invoices SET state='paid',updated_at=now() WHERE id=$1")
            .bind(invoice_id)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::internal)?;
        insert_event(&mut tx,business_id,invoice_id,Some(attempt_id),"invoice.paid",json!({"invoice_id":invoice_id,"payment_attempt_id":attempt_id,"state":"paid","total_cents":invoice.get::<i64,_>("total_cents")})).await?;
    } else {
        let code = outcome
            .code
            .unwrap_or_else(|| "processor_failed".to_owned());
        sqlx::query("UPDATE payment_attempts SET status='failed',failure_code=$2,lease_until=NULL,updated_at=now() WHERE id=$1")
            .bind(attempt_id).bind(&code).execute(&mut *tx).await.map_err(ApiError::internal)?;
        insert_event(&mut tx,business_id,invoice_id,Some(attempt_id),"invoice.payment_failed",json!({"invoice_id":invoice_id,"payment_attempt_id":attempt_id,"state":"open","failure_code":code})).await?;
    }
    tx.commit().await.map_err(ApiError::internal)?;
    Ok(())
}
