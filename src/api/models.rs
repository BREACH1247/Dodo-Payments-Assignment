use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Customer {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) email: String,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewCustomer {
    pub(crate) name: String,
    pub(crate) email: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Invoice {
    pub(crate) id: Uuid,
    pub(crate) customer_id: Uuid,
    pub(crate) state: String,
    pub(crate) total_cents: i64,
    pub(crate) due_date: NaiveDate,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct InvoiceItem {
    pub(crate) id: Uuid,
    pub(crate) description: String,
    pub(crate) quantity: i64,
    pub(crate) unit_amount_cents: i64,
}

#[derive(Serialize)]
pub(crate) struct InvoiceDetail {
    #[serde(flatten)]
    pub(crate) invoice: Invoice,
    pub(crate) items: Vec<InvoiceItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewInvoiceItem {
    pub(crate) description: String,
    pub(crate) quantity: i64,
    pub(crate) unit_amount_cents: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewInvoice {
    pub(crate) customer_id: Uuid,
    pub(crate) due_date: NaiveDate,
    pub(crate) items: Vec<NewInvoiceItem>,
}

#[derive(Deserialize)]
pub(crate) struct PageQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) offset: Option<i64>,
    pub(crate) state: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct EventQuery {
    pub(crate) after_id: Option<i64>,
    pub(crate) limit: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Event {
    pub(crate) id: i64,
    pub(crate) event_type: String,
    pub(crate) payload: Value,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewEndpoint {
    pub(crate) url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PayRequest {
    pub(crate) mock_card_token: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Attempt {
    pub(crate) id: Uuid,
    pub(crate) invoice_id: Uuid,
    pub(crate) status: String,
    pub(crate) psp_ref: Option<Uuid>,
    pub(crate) failure_code: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}
