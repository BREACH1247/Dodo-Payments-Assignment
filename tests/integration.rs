use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, Row};
use std::{env, time::Duration};
use tokio::time::{sleep, Instant};
use uuid::Uuid;

fn api() -> String {
    env::var("API_BASE").unwrap_or_else(|_| "http://localhost:3000".into())
}
fn psp() -> String {
    env::var("PSP_BASE").unwrap_or_else(|_| "http://localhost:3001".into())
}
fn key() -> String {
    env::var("DEMO_API_KEY")
        .unwrap_or_else(|_| "dodo_test_demo_key_for_local_only_0123456789abcdef".into())
}
fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

async fn setup_invoice(http: &Client) -> (Uuid, Uuid) {
    let customer: Value = http
        .post(format!("{}/customers", api()))
        .bearer_auth(key())
        .json(&json!({"name":"Integration Test","email":"test@example.test"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let customer_id = Uuid::parse_str(customer["id"].as_str().unwrap()).unwrap();
    let invoice: Value = http.post(format!("{}/invoices", api())).bearer_auth(key())
        .json(&json!({"customer_id":customer_id,"due_date":"2026-12-31","items":[{"description":"Two units","quantity":2,"unit_amount_cents":1250}]}))
        .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(invoice["total_cents"], 2500);
    (
        customer_id,
        Uuid::parse_str(invoice["id"].as_str().unwrap()).unwrap(),
    )
}

async fn pay(http: &Client, invoice: Uuid, token: &str, idempotency: &str) -> reqwest::Response {
    http.post(format!("{}/invoices/{invoice}/pay", api()))
        .bearer_auth(key())
        .header("Idempotency-Key", idempotency)
        .json(&json!({"mock_card_token":token}))
        .send()
        .await
        .unwrap()
}

async fn wait_attempt(http: &Client, id: Uuid) -> Value {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let value: Value = http
            .get(format!("{}/payment-attempts/{id}", api()))
            .bearer_auth(key())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        if value["status"] != "pending" {
            return value;
        }
        assert!(Instant::now() < deadline, "attempt stayed pending: {value}");
        sleep(Duration::from_millis(250)).await;
    }
}

async fn psp_request_count(attempt_id: Uuid) -> i32 {
    let db = PgPoolOptions::new()
        .connect(&env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    sqlx::query_scalar("SELECT request_count FROM psp_charges WHERE attempt_id=$1")
        .bind(attempt_id)
        .fetch_one(&db)
        .await
        .unwrap()
}

#[tokio::test]
async fn concurrent_requests_reserve_one_charge() {
    let http = client();
    let (_, invoice) = setup_invoice(&http).await;
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let http = http.clone();
        let idem = Uuid::new_v4().to_string();
        tasks.push(tokio::spawn(async move {
            let r = pay(&http, invoice, "tok_success", &idem).await;
            (r.status(), r.json::<Value>().await.unwrap())
        }));
    }
    let mut accepted = Vec::new();
    for task in tasks {
        let (status, body) = task.await.unwrap();
        if status == StatusCode::ACCEPTED {
            accepted.push(body);
        } else {
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
        }
    }
    assert_eq!(accepted.len(), 1);
    let attempt_id = Uuid::parse_str(accepted[0]["attempt_id"].as_str().unwrap()).unwrap();
    assert_eq!(wait_attempt(&http, attempt_id).await["status"], "succeeded");
    let invoice_body: Value = http
        .get(format!("{}/invoices/{invoice}", api()))
        .bearer_auth(key())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(invoice_body["state"], "paid");
    let charge: Value = http
        .get(format!("{}/charges/{attempt_id}", psp()))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(charge["status"], "succeeded");
    assert_eq!(
        psp_request_count(attempt_id).await,
        1,
        "exactly one PSP charge request is allowed"
    );
}

#[tokio::test]
async fn idempotency_replays_original_response_and_rejects_changed_body() {
    let http = client();
    let (_, invoice) = setup_invoice(&http).await;
    let idem = Uuid::new_v4().to_string();
    let first = pay(&http, invoice, "tok_success", &idem).await;
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first_body: Value = first.json().await.unwrap();
    let attempt_id = Uuid::parse_str(first_body["attempt_id"].as_str().unwrap()).unwrap();
    assert_eq!(wait_attempt(&http, attempt_id).await["status"], "succeeded");
    let second = pay(&http, invoice, "tok_success", &idem).await;
    assert_eq!(second.status(), StatusCode::ACCEPTED);
    assert_eq!(second.json::<Value>().await.unwrap(), first_body);
    assert_eq!(
        psp_request_count(attempt_id).await,
        1,
        "an idempotent replay must not call the PSP again"
    );
    let changed = pay(&http, invoice, "tok_card_declined", &idem).await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    let body: Value = changed.json().await.unwrap();
    assert_eq!(body["error"]["code"], "idempotency_key_reused");
}

#[tokio::test]
async fn timeout_reconciles_and_network_error_leaves_invoice_open() {
    let http = client();
    let (_, timeout_invoice) = setup_invoice(&http).await;
    let started = Instant::now();
    let response = pay(
        &http,
        timeout_invoice,
        "tok_timeout",
        &Uuid::new_v4().to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(started.elapsed() < Duration::from_secs(2));
    let body: Value = response.json().await.unwrap();
    let attempt_id = Uuid::parse_str(body["attempt_id"].as_str().unwrap()).unwrap();
    assert_eq!(wait_attempt(&http, attempt_id).await["status"], "succeeded");

    let (_, failed_invoice) = setup_invoice(&http).await;
    let response = pay(
        &http,
        failed_invoice,
        "tok_network_error",
        &Uuid::new_v4().to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: Value = response.json().await.unwrap();
    let failed_id = Uuid::parse_str(body["attempt_id"].as_str().unwrap()).unwrap();
    let attempt = wait_attempt(&http, failed_id).await;
    assert_eq!(attempt["status"], "failed");
    assert_eq!(attempt["failure_code"], "network_error");
    let invoice_body: Value = http
        .get(format!("{}/invoices/{failed_invoice}", api()))
        .bearer_auth(key())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(invoice_body["state"], "open");
    let retry = pay(
        &http,
        failed_invoice,
        "tok_success",
        &Uuid::new_v4().to_string(),
    )
    .await;
    assert_eq!(retry.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn committed_psp_charge_is_reconciled_after_missing_api_commit() {
    let http = client();
    let (_, invoice) = setup_invoice(&http).await;
    let attempt_id = Uuid::new_v4();
    let business_id = Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
    let db = PgPoolOptions::new()
        .connect(&env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    sqlx::query("INSERT INTO payment_attempts(id,business_id,invoice_id,idempotency_key,request_hash,mock_token,response_status,response_body,next_run_at) VALUES($1,$2,$3,$4,$5,'tok_success',202,$6,now()+interval '10 seconds')")
        .bind(attempt_id).bind(business_id).bind(invoice).bind(Uuid::new_v4().to_string())
        .bind(vec![0u8;32]).bind(json!({"attempt_id":attempt_id,"status":"pending"}).to_string())
        .execute(&db).await.unwrap();
    let psp_result: Value = http
        .post(format!("{}/charges", psp()))
        .json(&json!({"attempt_id":attempt_id,"token":"tok_success","amount_cents":2500}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(psp_result["status"], "succeeded");
    sqlx::query("UPDATE payment_attempts SET next_run_at=now() WHERE id=$1")
        .bind(attempt_id)
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(wait_attempt(&http, attempt_id).await["status"], "succeeded");
    let charge_count: i64 =
        sqlx::query("SELECT count(*) AS count FROM psp_charges WHERE attempt_id=$1")
            .bind(attempt_id)
            .fetch_one(&db)
            .await
            .unwrap()
            .get("count");
    assert_eq!(charge_count, 1);
}

#[tokio::test]
async fn webhook_delivery_is_durable_and_sent() {
    let http = client();
    let db = PgPoolOptions::new()
        .connect(&env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    let business_id = Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
    // The Compose test environment persists between invocations. Retire old demo
    // destinations and their queued work so retries from prior runs cannot starve
    // the fresh delivery this test is asserting.
    sqlx::query("UPDATE webhook_endpoints SET active=false WHERE business_id=$1")
        .bind(business_id)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("UPDATE webhook_deliveries SET status='exhausted',lease_until=NULL WHERE business_id=$1 AND status='pending'")
        .bind(business_id)
        .execute(&db)
        .await
        .unwrap();
    let endpoint: Value = http
        .post(format!("{}/webhook-endpoints", api()))
        .bearer_auth(key())
        .json(&json!({"url":format!("{}/demo/webhook-sink",psp())}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let endpoint_id = Uuid::parse_str(endpoint["id"].as_str().unwrap()).unwrap();
    let (_, invoice) = setup_invoice(&http).await;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let row = sqlx::query("SELECT d.status FROM webhook_deliveries d JOIN events e ON e.id=d.event_id WHERE d.endpoint_id=$1 AND e.invoice_id=$2 AND e.event_type='invoice.created'")
            .bind(endpoint_id).bind(invoice).fetch_optional(&db).await.unwrap();
        if let Some(row) = row {
            if row.get::<String, _>("status") == "delivered" {
                break;
            }
        }
        assert!(Instant::now() < deadline, "webhook not delivered");
        sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn malformed_requests_use_json_error_envelope() {
    let http = client();
    let (_, invoice) = setup_invoice(&http).await;
    let response = http
        .post(format!("{}/invoices", api()))
        .bearer_auth(key())
        .json(&json!({"customer_id": Uuid::new_v4(), "items": [], "total_cents": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_json");

    let response = http
        .get(format!("{}/invoices/not-a-uuid", api()))
        .bearer_auth(key())
        .send()
        .await
        .unwrap();
    assert!(response.status().is_client_error());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_path");

    let response = http
        .get(format!("{}/invoices/{invoice}/unknown", api()))
        .bearer_auth(key())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn concurrent_same_key_on_different_invoices_conflicts() {
    let http = client();
    let (_, first_invoice) = setup_invoice(&http).await;
    let (_, second_invoice) = setup_invoice(&http).await;
    let idem = Uuid::new_v4().to_string();
    let (first, second) = tokio::join!(
        pay(&http, first_invoice, "tok_success", &idem),
        pay(&http, second_invoice, "tok_success", &idem)
    );
    let first_status = first.status();
    let second_status = second.status();
    assert!(
        (first_status == StatusCode::ACCEPTED && second_status == StatusCode::CONFLICT)
            || (first_status == StatusCode::CONFLICT && second_status == StatusCode::ACCEPTED),
        "statuses: {first_status}, {second_status}"
    );
    let conflict = if first_status == StatusCode::CONFLICT {
        first
    } else {
        second
    };
    let body: Value = conflict.json().await.unwrap();
    assert_eq!(body["error"]["code"], "idempotency_key_reused");
}
