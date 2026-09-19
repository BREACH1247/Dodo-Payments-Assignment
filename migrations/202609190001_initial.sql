CREATE TABLE businesses (
    id uuid PRIMARY KEY,
    name text NOT NULL CHECK (length(trim(name)) > 0),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE api_keys (
    id uuid PRIMARY KEY,
    business_id uuid NOT NULL REFERENCES businesses(id),
    prefix text NOT NULL UNIQUE,
    key_hash bytea NOT NULL UNIQUE CHECK (octet_length(key_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);

CREATE TABLE customers (
    id uuid PRIMARY KEY,
    business_id uuid NOT NULL REFERENCES businesses(id),
    name text NOT NULL CHECK (length(trim(name)) > 0),
    email text NOT NULL CHECK (length(trim(email)) > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (business_id, id)
);
CREATE INDEX customers_list_idx ON customers (business_id, created_at, id);

CREATE TABLE invoices (
    id uuid PRIMARY KEY,
    business_id uuid NOT NULL REFERENCES businesses(id),
    customer_id uuid NOT NULL,
    state text NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'paid', 'void')),
    total_cents bigint NOT NULL CHECK (total_cents > 0),
    due_date date NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (business_id, id),
    FOREIGN KEY (business_id, customer_id) REFERENCES customers(business_id, id)
);
CREATE INDEX invoices_list_idx ON invoices (business_id, state, created_at, id);

CREATE TABLE invoice_items (
    id uuid PRIMARY KEY,
    invoice_id uuid NOT NULL REFERENCES invoices(id),
    description text NOT NULL CHECK (length(trim(description)) > 0),
    quantity bigint NOT NULL CHECK (quantity > 0),
    unit_amount_cents bigint NOT NULL CHECK (unit_amount_cents > 0)
);
CREATE INDEX invoice_items_invoice_idx ON invoice_items (invoice_id);

CREATE TABLE payment_attempts (
    id uuid PRIMARY KEY,
    business_id uuid NOT NULL REFERENCES businesses(id),
    invoice_id uuid NOT NULL,
    idempotency_key text NOT NULL CHECK (length(idempotency_key) BETWEEN 1 AND 255),
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    mock_token text NOT NULL,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'succeeded', 'failed')),
    psp_ref uuid UNIQUE,
    failure_code text,
    response_status smallint NOT NULL,
    response_body text NOT NULL,
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_run_at timestamptz NOT NULL DEFAULT now(),
    lease_until timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (business_id, id),
    UNIQUE (business_id, idempotency_key),
    FOREIGN KEY (business_id, invoice_id) REFERENCES invoices(business_id, id)
);
CREATE UNIQUE INDEX one_pending_attempt_per_invoice ON payment_attempts (invoice_id) WHERE status = 'pending';
CREATE INDEX payment_attempts_due_idx ON payment_attempts (status, next_run_at);

-- The mock PSP owns this table. The API never queries it directly.
CREATE TABLE psp_charges (
    attempt_id uuid PRIMARY KEY,
    token_hash bytea NOT NULL CHECK (octet_length(token_hash) = 32),
    amount_cents bigint NOT NULL CHECK (amount_cents > 0),
    status text NOT NULL CHECK (status IN ('succeeded', 'failed')),
    psp_ref uuid,
    failure_code text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE webhook_endpoints (
    id uuid PRIMARY KEY,
    business_id uuid NOT NULL REFERENCES businesses(id),
    url text NOT NULL,
    signing_secret bytea NOT NULL,
    active boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (business_id, id)
);

CREATE TABLE events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    business_id uuid NOT NULL REFERENCES businesses(id),
    invoice_id uuid NOT NULL,
    payment_attempt_id uuid,
    event_type text NOT NULL CHECK (event_type IN ('invoice.created', 'invoice.paid', 'invoice.payment_failed', 'invoice.voided')),
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (business_id, id),
    FOREIGN KEY (business_id, invoice_id) REFERENCES invoices(business_id, id),
    FOREIGN KEY (business_id, payment_attempt_id) REFERENCES payment_attempts(business_id, id)
);
CREATE INDEX events_business_cursor_idx ON events (business_id, id);

CREATE TABLE webhook_deliveries (
    id uuid PRIMARY KEY,
    business_id uuid NOT NULL REFERENCES businesses(id),
    event_id bigint NOT NULL,
    endpoint_id uuid NOT NULL,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'delivered', 'exhausted')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_run_at timestamptz NOT NULL DEFAULT now(),
    lease_until timestamptz,
    last_http_status integer,
    last_error text,
    delivered_at timestamptz,
    UNIQUE (event_id, endpoint_id),
    FOREIGN KEY (business_id, event_id) REFERENCES events(business_id, id),
    FOREIGN KEY (business_id, endpoint_id) REFERENCES webhook_endpoints(business_id, id)
);
CREATE INDEX webhook_deliveries_due_idx ON webhook_deliveries (status, next_run_at);
