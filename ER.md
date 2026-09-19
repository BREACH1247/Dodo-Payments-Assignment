# Entity–relationship diagram

This diagram reflects `migrations/202609190001_initial.sql` and `migrations/202609190002_track_psp_charge_requests.sql`. Money is stored only in integer USD cents. The invoice item amount is `unit_amount_cents`; its extended amount is `quantity × unit_amount_cents`, computed by the API. `invoices.total_cents` is the persisted sum of those extended amounts, and `psp_charges.amount_cents` is the amount sent to the mock processor. These are deliberately distinct fields.

```mermaid
erDiagram
    businesses {
        uuid id PK "Stable tenant identifier"
        text name "Business display name"
        timestamptz created_at "Tenant creation time UTC"
    }
    api_keys {
        uuid id PK "Key record identifier"
        uuid business_id FK "Tenant that owns the key"
        text prefix UK "Public lookup hint unique globally"
        bytea key_hash UK "SHA256 of full bearer key"
        timestamptz created_at "Key issue time UTC"
        timestamptz revoked_at "Revocation time or null if active"
    }
    customers {
        uuid id PK "Customer identifier"
        uuid business_id FK "Tenant that owns the customer"
        text name "Nonblank customer name"
        text email "Nonblank customer email"
        timestamptz created_at "Customer creation time UTC"
    }
    invoices {
        uuid id PK "Invoice identifier"
        uuid business_id FK "Tenant that owns the invoice"
        uuid customer_id FK "Customer in the same tenant"
        text state "open paid or void"
        bigint total_cents "Server-computed invoice total USD cents"
        date due_date "Invoice due calendar date"
        timestamptz created_at "Invoice creation time UTC"
        timestamptz updated_at "Last invoice state change UTC"
    }
    invoice_items {
        uuid id PK "Line item identifier"
        uuid invoice_id FK "Parent invoice identifier"
        text description "Nonblank line item description"
        bigint quantity "Positive unit count"
        bigint unit_amount_cents "Positive price per unit USD cents"
    }
    payment_attempts {
        uuid id PK "Attempt identifier and PSP idempotency key"
        uuid business_id FK "Tenant that owns the attempt"
        uuid invoice_id FK "Invoice in the same tenant"
        text idempotency_key "Caller supplied key unique per tenant"
        bytea request_hash "SHA256 of canonical pay operation"
        text mock_token "Synthetic card token for mock PSP"
        text status "pending succeeded or failed"
        uuid psp_ref UK "Processor reference on success or null"
        text failure_code "Processor failure reason or null"
        smallint response_status "Original HTTP status for replay"
        text response_body "Original JSON body for replay"
        integer attempt_count "Worker claim count including retries"
        timestamptz next_run_at "Earliest next worker claim time UTC"
        timestamptz lease_until "Current worker lease expiry or null"
        timestamptz created_at "Attempt reservation time UTC"
        timestamptz updated_at "Last attempt update time UTC"
    }
    psp_charges {
        uuid attempt_id PK "Stable external idempotency identifier"
        bytea token_hash "SHA256 fingerprint of amount and token"
        bigint amount_cents "Amount submitted to PSP USD cents"
        text status "succeeded or failed"
        uuid psp_ref "Processor charge reference or null"
        text failure_code "Decline code or null"
        integer request_count "Number of POST charge requests for this attempt"
        timestamptz created_at "Processor charge creation time UTC"
    }
    webhook_endpoints {
        uuid id PK "Webhook destination identifier"
        uuid business_id FK "Tenant that owns the destination"
        text url "Destination HTTPS URL or local demo HTTP URL"
        bytea signing_secret "Random HMAC secret shown once"
        boolean active "Whether new events enqueue deliveries"
        timestamptz created_at "Endpoint registration time UTC"
    }
    events {
        bigint id PK "Monotonic event cursor"
        uuid business_id FK "Tenant that owns the event"
        uuid invoice_id FK "Invoice in the same tenant"
        uuid payment_attempt_id FK "Related attempt or null for creation"
        text event_type "Created paid failed or voided event"
        jsonb payload "Immutable webhook event body"
        timestamptz created_at "Event commit time UTC"
    }
    webhook_deliveries {
        uuid id PK "Delivery work item identifier"
        uuid business_id FK "Tenant that owns the delivery"
        bigint event_id FK "Event in the same tenant"
        uuid endpoint_id FK "Destination in the same tenant"
        text status "pending delivered or exhausted"
        integer attempt_count "Number of HTTP sends attempted"
        timestamptz next_run_at "Earliest next send time UTC"
        timestamptz lease_until "Current worker lease expiry or null"
        integer last_http_status "Most recent HTTP code or null"
        text last_error "Most recent delivery error or null"
        timestamptz delivered_at "First successful send time or null"
    }

    businesses ||--o{ api_keys : owns
    businesses ||--o{ customers : owns
    businesses ||--o{ invoices : owns
    customers ||--o{ invoices : billed_for
    invoices ||--|{ invoice_items : contains
    businesses ||--o{ payment_attempts : owns
    invoices ||--o{ payment_attempts : paid_by
    payment_attempts o|..o| psp_charges : "logical ID no database FK"
    businesses ||--o{ webhook_endpoints : owns
    businesses ||--o{ events : owns
    invoices ||--o{ events : emits
    payment_attempts o|--o{ events : may_emit
    events ||--o{ webhook_deliveries : dispatched_as
    webhook_endpoints ||--o{ webhook_deliveries : receives
```

Additional constraints: `(business_id, customer_id)` and `(business_id, invoice_id)` composite foreign keys enforce tenant consistency; `(business_id, idempotency_key)` is unique; each invoice has at most one pending attempt through a partial unique index; `(event_id, endpoint_id)` is unique. The mock PSP's `psp_charges` table has no database foreign key to `payment_attempts` because the API must treat the PSP as an external system and reconcile through HTTP.
