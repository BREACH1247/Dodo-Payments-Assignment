use super::{
    auth::AuthBusiness,
    models::{Customer, NewCustomer, PageQuery},
    page, AppState, Result,
};
use crate::{ApiError, ApiJson, ApiPath, ApiQuery};
use axum::{extract::State, http::StatusCode, Json};
use uuid::Uuid;

pub(crate) async fn create_customer(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiJson(input): ApiJson<NewCustomer>,
) -> Result<(StatusCode, Json<Customer>)> {
    let name = input.name.trim();
    let email = input.email.trim();
    if name.is_empty() || name.len() > 200 || !email.contains('@') || email.len() > 320 {
        return Err(ApiError::bad("Valid name and email are required"));
    }
    let customer = sqlx::query_as::<_, Customer>("INSERT INTO customers(id,business_id,name,email) VALUES($1,$2,$3,$4) RETURNING id,name,email,created_at")
        .bind(Uuid::new_v4()).bind(b).bind(name).bind(email).fetch_one(&s.db).await.map_err(ApiError::internal)?;
    Ok((StatusCode::CREATED, Json(customer)))
}

pub(crate) async fn get_customer(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Customer>> {
    let customer = sqlx::query_as::<_, Customer>(
        "SELECT id,name,email,created_at FROM customers WHERE id=$1 AND business_id=$2",
    )
    .bind(id)
    .bind(b)
    .fetch_optional(&s.db)
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(ApiError::not_found)?;
    Ok(Json(customer))
}

pub(crate) async fn list_customers(
    State(s): State<AppState>,
    AuthBusiness(b): AuthBusiness,
    ApiQuery(q): ApiQuery<PageQuery>,
) -> Result<Json<Vec<Customer>>> {
    let (limit, offset) = page(q.limit, q.offset)?;
    let rows = sqlx::query_as::<_, Customer>("SELECT id,name,email,created_at FROM customers WHERE business_id=$1 ORDER BY created_at,id LIMIT $2 OFFSET $3")
        .bind(b).bind(limit).bind(offset).fetch_all(&s.db).await.map_err(ApiError::internal)?;
    Ok(Json(rows))
}
