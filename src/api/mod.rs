pub(crate) mod api_keys;
pub(crate) mod auth;
pub(crate) mod customers;
pub(crate) mod invoices;
pub(crate) mod models;
pub(crate) mod payments;
pub(crate) mod webhooks;
pub(crate) mod workers;

use crate::{init_tracing, migrate_and_seed, ApiError};
use axum::{
    http::StatusCode,
    routing::{delete, get, post},
    Router,
};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::{env, time::Duration};

use self::{
    api_keys::{create_api_key, revoke_api_key},
    customers::{create_customer, get_customer, list_customers},
    invoices::{create_invoice, get_invoice, list_invoices, void_invoice},
    payments::{get_attempt, pay_invoice},
    webhooks::{create_endpoint, list_events},
    workers::{payment::payment_worker, webhook::webhook_worker},
};

pub(crate) type Result<T> = std::result::Result<T, ApiError>;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) db: PgPool,
    pub(crate) http: reqwest::Client,
    pub(crate) psp_url: String,
}

pub async fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let database_url = env::var("DATABASE_URL")?;
    let demo_key = env::var("DEMO_API_KEY")?;
    let psp_url = env::var("PSP_URL")?;
    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;
    migrate_and_seed(&db, &demo_key).await?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let state = AppState { db, http, psp_url };
    tokio::spawn(payment_worker(state.clone()));
    tokio::spawn(webhook_worker(state.clone()));
    let app = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/customers", post(create_customer).get(list_customers))
        .route("/customers/{id}", get(get_customer))
        .route("/invoices", post(create_invoice).get(list_invoices))
        .route("/invoices/{id}", get(get_invoice))
        .route("/invoices/{id}/void", post(void_invoice))
        .route("/invoices/{id}/pay", post(pay_invoice))
        .route("/payment-attempts/{id}", get(get_attempt))
        .route("/api-keys", post(create_api_key))
        .route("/api-keys/{id}", delete(revoke_api_key))
        .route("/webhook-endpoints", post(create_endpoint))
        .route("/events", get(list_events))
        .fallback(|| async { ApiError::not_found() })
        .method_not_allowed_fallback(|| async {
            ApiError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "Method not allowed",
            )
        })
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!("API listening on port 3000");
    axum::serve(listener, app).await?;
    Ok(())
}

pub(crate) fn page(limit: Option<i64>, offset: Option<i64>) -> Result<(i64, i64)> {
    let limit = limit.unwrap_or(50);
    let offset = offset.unwrap_or(0);
    if !(1..=100).contains(&limit) || offset < 0 {
        return Err(ApiError::bad("limit must be 1..100 and offset nonnegative"));
    }
    Ok((limit, offset))
}
