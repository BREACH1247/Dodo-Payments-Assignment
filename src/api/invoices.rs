use super::{
    auth::AuthBusiness,
    models::{Invoice, InvoiceDetail, InvoiceItem, NewInvoice, PageQuery},
    page,
    webhooks::insert_event,
    AppState, Result,
};
use crate::{ApiError, ApiJson, ApiPath, ApiQuery};
use axum::{extract::State, http::StatusCode, Json};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn create_invoice(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiJson(input): ApiJson<NewInvoice>,
) -> Result<(StatusCode, Json<InvoiceDetail>)> {
    if input.items.is_empty() || input.items.len() > 100 {
        return Err(ApiError::bad("Invoice requires 1..100 items"));
    }
    let mut total = 0i64;
    for item in &input.items {
        if item.description.trim().is_empty()
            || item.description.len() > 500
            || item.quantity <= 0
            || item.unit_amount_cents <= 0
        {
            return Err(ApiError::bad(
                "Each item needs a description, positive quantity and positive unit amount",
            ));
        }
        total = total
            .checked_add(
                item.quantity
                    .checked_mul(item.unit_amount_cents)
                    .ok_or_else(|| ApiError::bad("Amount overflow"))?,
            )
            .ok_or_else(|| ApiError::bad("Amount overflow"))?;
    }
    let mut tx = s.db.begin().await.map_err(ApiError::internal)?;
    let exists = sqlx::query("SELECT 1 FROM customers WHERE business_id=$1 AND id=$2")
        .bind(b)
        .bind(input.customer_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::internal)?
        .is_some();
    if !exists {
        return Err(ApiError::not_found());
    }
    let invoice = sqlx::query_as::<_, Invoice>("INSERT INTO invoices(id,business_id,customer_id,total_cents,due_date) VALUES($1,$2,$3,$4,$5) RETURNING id,customer_id,state,total_cents,due_date,created_at,updated_at")
        .bind(Uuid::new_v4()).bind(b).bind(input.customer_id).bind(total).bind(input.due_date)
        .fetch_one(&mut *tx).await.map_err(ApiError::internal)?;
    let mut items = Vec::with_capacity(input.items.len());
    for item in input.items {
        let row = sqlx::query_as::<_, InvoiceItem>("INSERT INTO invoice_items(id,invoice_id,description,quantity,unit_amount_cents) VALUES($1,$2,$3,$4,$5) RETURNING id,description,quantity,unit_amount_cents")
            .bind(Uuid::new_v4()).bind(invoice.id).bind(item.description.trim()).bind(item.quantity).bind(item.unit_amount_cents)
            .fetch_one(&mut *tx).await.map_err(ApiError::internal)?;
        items.push(row);
    }
    insert_event(
        &mut tx,
        b,
        invoice.id,
        None,
        "invoice.created",
        json!({"invoice_id":invoice.id,"state":"open","total_cents":total}),
    )
    .await?;
    tx.commit().await.map_err(ApiError::internal)?;
    Ok((StatusCode::CREATED, Json(InvoiceDetail { invoice, items })))
}

async fn invoice_detail(s: &AppState, b: Uuid, id: Uuid) -> Result<InvoiceDetail> {
    let invoice = sqlx::query_as::<_, Invoice>("SELECT id,customer_id,state,total_cents,due_date,created_at,updated_at FROM invoices WHERE id=$1 AND business_id=$2")
        .bind(id).bind(b).fetch_optional(&s.db).await.map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
    let items = sqlx::query_as::<_, InvoiceItem>("SELECT id,description,quantity,unit_amount_cents FROM invoice_items WHERE invoice_id=$1 ORDER BY id")
        .bind(id).fetch_all(&s.db).await.map_err(ApiError::internal)?;
    Ok(InvoiceDetail { invoice, items })
}

pub(crate) async fn get_invoice(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<InvoiceDetail>> {
    Ok(Json(invoice_detail(&s, b, id).await?))
}

pub(crate) async fn list_invoices(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiQuery(q): ApiQuery<PageQuery>,
) -> Result<Json<Vec<Invoice>>> {
    let (limit, offset) = page(q.limit, q.offset)?;
    if let Some(ref state) = q.state {
        if !["open", "paid", "void"].contains(&state.as_str()) {
            return Err(ApiError::bad("Invalid invoice state filter"));
        }
    }
    let rows = sqlx::query_as::<_, Invoice>("SELECT id,customer_id,state,total_cents,due_date,created_at,updated_at FROM invoices WHERE business_id=$1 AND ($2::text IS NULL OR state=$2) ORDER BY created_at,id LIMIT $3 OFFSET $4")
        .bind(b).bind(q.state).bind(limit).bind(offset).fetch_all(&s.db).await.map_err(ApiError::internal)?;
    Ok(Json(rows))
}

pub(crate) async fn void_invoice(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<InvoiceDetail>> {
    let mut tx = s.db.begin().await.map_err(ApiError::internal)?;
    let row = sqlx::query("SELECT state FROM invoices WHERE id=$1 AND business_id=$2 FOR UPDATE")
        .bind(id)
        .bind(b)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    if row.get::<String, _>("state") != "open" {
        return Err(ApiError::conflict(
            "invoice_not_payable",
            "Only open invoices can be voided",
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
    sqlx::query("UPDATE invoices SET state='void',updated_at=now() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::internal)?;
    insert_event(
        &mut tx,
        b,
        id,
        None,
        "invoice.voided",
        json!({"invoice_id":id,"state":"void"}),
    )
    .await?;
    tx.commit().await.map_err(ApiError::internal)?;
    Ok(Json(invoice_detail(&s, b, id).await?))
}
