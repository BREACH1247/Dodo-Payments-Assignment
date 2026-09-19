# Dodo Payments Invoice and Payment Service

A small Rust API, mock payment processor, and PostgreSQL database. Invoice amounts are integer USD cents. Payments are asynchronous: `POST /pay` returns an attempt ID, and the payment worker confirms the eventual result.

## Run

Start Docker Desktop, then from this directory:

```sh
docker compose up
```

The API is at `http://localhost:3000`, the mock PSP is at `http://localhost:3001`, and PostgreSQL is at `localhost:5432`. The API runs migrations and creates a demo business and key automatically. The local demo key is:

```text
dodo_test_demo_key_for_local_only_0123456789abcdef
```

This key and the Compose database password are intentionally local demo credentials. Do not deploy them. `GET /health` on both HTTP services returns `200` when ready.

## Code layout

`src/bin/api.rs` is the small executable entry point. `src/api/mod.rs` builds the router, database pool, HTTP client, and background workers. The HTTP handlers live in `src/api/customers.rs`, `invoices.rs`, `payments.rs`, `webhooks.rs`, and `api_keys.rs`; authentication and request/response types live in `auth.rs` and `models.rs`. Payment claiming, PSP lookup, and finalization live in `src/api/workers/payment.rs`. Webhook signing, sending, and retries live in `src/api/workers/webhook.rs`. The separate mock processor is `src/bin/mock_psp.rs`.

## Quick demo

The following examples use a POSIX shell and `jq` to pass returned IDs into later requests. The API itself does not require `jq`.

```sh
export KEY=dodo_test_demo_key_for_local_only_0123456789abcdef
export BASE=http://localhost:3000

# 1. Create a customer.
CUSTOMER_ID=$(curl -sS -X POST "$BASE/customers" \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"name":"Ada Example","email":"ada@example.test"}' | jq -r .id)

# 2. Register the local webhook sink before creating an invoice.
curl -sS -X POST "$BASE/webhook-endpoints" \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"url":"http://mock-psp:3001/demo/webhook-sink"}'

# 3. Create an invoice. 2 x 1250 cents = 2500 cents.
INVOICE_ID=$(curl -sS -X POST "$BASE/invoices" \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d "{\"customer_id\":\"$CUSTOMER_ID\",\"due_date\":\"2026-12-31\",\"items\":[{\"description\":\"Example service\",\"quantity\":2,\"unit_amount_cents\":1250}]}" | jq -r .id)

# 4. Attempt a successful payment; then inspect its eventual result.
ATTEMPT_ID=$(curl -sS -X POST "$BASE/invoices/$INVOICE_ID/pay" \
  -H "Authorization: Bearer $KEY" -H 'Idempotency-Key: demo-success-1' \
  -H 'Content-Type: application/json' -d '{"mock_card_token":"tok_success"}' | jq -r .attempt_id)
curl -sS "$BASE/payment-attempts/$ATTEMPT_ID" -H "Authorization: Bearer $KEY"

# 5. Create a second invoice and pay it with tok_card_declined.
SECOND_INVOICE_ID=$(curl -sS -X POST "$BASE/invoices" \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d "{\"customer_id\":\"$CUSTOMER_ID\",\"due_date\":\"2026-12-31\",\"items\":[{\"description\":\"Decline demo\",\"quantity\":1,\"unit_amount_cents\":1250}]}" | jq -r .id)
curl -sS -X POST "$BASE/invoices/$SECOND_INVOICE_ID/pay" \
  -H "Authorization: Bearer $KEY" -H 'Idempotency-Key: demo-decline-1' \
  -H 'Content-Type: application/json' -d '{"mock_card_token":"tok_card_declined"}'
```

The attempt status is initially `pending`; poll its URL until `succeeded` or `failed`. With `tok_timeout`, the mock PSP **stores a successful charge before** delaying its first `POST /charges` response for 30 seconds. The worker's two-second HTTP deadline expires, then `GET /charges/{attempt_id}` can find that charge. Therefore the attempt may succeed after only a few seconds; it is not expected to remain pending for 30 seconds. If both calls leave the outcome unknown, the attempt stays pending and the worker retries. `tok_network_error` returns a PSP 500 while leaving a definitive failed result available for lookup. View received demo webhooks with `docker compose logs -f mock-psp`.

## Postman video demo

Import [postman/Dodo-Payments.postman_collection.json](postman/Dodo-Payments.postman_collection.json) and [postman/Dodo-Payments.local.postman_environment.json](postman/Dodo-Payments.local.postman_environment.json), then select the **Dodo Payments — Local** environment (Postman may otherwise leave `{{baseUrl}}` and `{{apiKey}}` unresolved). Run **0. Shared setup** once, then run the **Success**, **Decline**, and **Timeout** folders in order. Run **4. Reconcile events** to inspect the durable event stream. Requests that poll payment attempts may initially show `pending`; resend until `succeeded` or `failed`. The PSP-inspection requests need the attempt ID captured by the preceding pay request. Keep `docker compose logs -f mock-psp` visible beside Postman to show webhook receipt. The mock PSP is also the local demo webhook receiver; it logs delivered events but does not itself decide invoice state from those webhook calls.

## Reset local demo data

The current Compose file has one project-owned PostgreSQL volume. From this directory, the following removes **all** local Dodo Payments records (including customers, invoices, attempts, PSP charges, events, and webhook endpoints), then recreates the schema and demo API key:

```sh
docker compose down -v
docker compose up -d --no-build db api mock-psp
```

This is irreversible without a separate backup. It does not clear Postman environment variables: rerun **0. Shared setup** and the subsequent folders in order so IDs and idempotency keys refer to the new database.

## Verification

Run the integration tests against running Compose services:

```sh
docker compose --profile test run --build --rm tests
```

The seven integration tests exercise concurrent payment reservation, cross-invoice idempotency-key reuse, idempotent replay, malformed input, timeout reconciliation, network failure, crash-gap recovery, webhook delivery, and integer total computation. The concurrency and idempotency tests assert the mock PSP received exactly one charge request. The service always uses the same attempt UUID for PSP retries. See [DESIGN.md](DESIGN.md) for the state diagram, failure analysis, webhook policy, and deliberate cuts; [ER.md](ER.md) for the field-level Mermaid data model; and [API.md](API.md) for endpoint shapes.

## Demo Video

1. [Part 1](https://www.loom.com/share/920ea107074d4c4abe96edeffabbaf02)
2. [Part 2](https://www.loom.com/share/6cc6cf8c546c48f0983bacca72d1614b)
3. [Part 3](https://www.loom.com/share/a7b121d44ac64bf0944cf4e62ff969fd)
