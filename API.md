# HTTP API

All public endpoints except `/health` require `Authorization: Bearer <api-key>`. JSON requests require `Content-Type: application/json`. IDs are UUIDs unless identified as event IDs. List endpoints accept `limit=1..100` (default 50) and `offset>=0` (default 0), except events use `after_id` instead of offset. Cross-business resource lookups return `404`.

Errors use this shape:

```json
{"error":{"code":"payment_in_progress","message":"Invoice has a pending payment","request_id":"7eeaa0ce-6a48-4db3-80bb-743b24d132d3"}}
```

`401` means the key is missing or inactive; `404` means unavailable; `409` means an invalid state or conflicting idempotency key; `422` means invalid input; `500` means an unexpected server failure. The `request_id` is generated for each error response.

## Customers

- `POST /customers` accepts `{"name":"Ada Example","email":"ada@example.test"}`. Returns `201` with `id`, `name`, `email`, and `created_at`.
- `GET /customers/{id}` returns that shape.
- `GET /customers?limit=50&offset=0` returns an array of that shape.

## Invoices

- `POST /invoices` accepts `{"customer_id":"<uuid>","due_date":"2026-12-31","items":[{"description":"Service","quantity":2,"unit_amount_cents":1250}]}`. Returns `201` with `id`, `customer_id`, `state:"open"`, server-computed `total_cents:2500`, `due_date`, timestamps, and items with their IDs. A client supplied total is rejected because the request schema has no total field. Items must be nonempty, and integer overflow is rejected.
- `GET /invoices/{id}` returns the invoice and its items.
- `GET /invoices?state=open&limit=50&offset=0` returns invoice summaries. `state` may be `open`, `paid`, or `void`.
- `POST /invoices/{id}/void` accepts no body. Returns the void invoice. It returns `409` if the invoice is already terminal or has a pending payment.

## Payments

- `POST /invoices/{id}/pay` requires `Idempotency-Key: <1..255 character key>` and `{"mock_card_token":"tok_success"}`. The other supported tokens are `tok_card_declined`, `tok_insufficient_funds`, `tok_timeout`, and `tok_network_error`. Returns `202` with `{"attempt_id":"<uuid>","status":"pending"}`. Repeating the same key and body returns the original response. Reusing the key with a different invoice or token returns `409 idempotency_key_reused`. A new key on a paid invoice returns `409 invoice_already_paid`; another key during a pending attempt returns `409 payment_in_progress`.
- `GET /payment-attempts/{id}` returns `id`, `invoice_id`, `status` (`pending`, `succeeded`, or `failed`), `psp_ref` if successful, `failure_code` if failed, and timestamps. Poll this endpoint for the final result. `tok_timeout` delays the PSP's first POST response by 30 seconds **after** recording success; the worker's immediate charge lookup can resolve the attempt much sooner. If the outcome remains unknown, the attempt stays pending and another payment on that invoice is blocked.

## Events and webhooks

- `POST /webhook-endpoints` accepts `{"url":"https://example.test/webhooks"}`. Returns `201` with `id`, `url`, and a Base64url `signing_secret` shown once. HTTP is also allowed for the local mock sink.
- `GET /events?after_id=0&limit=50` returns business events ordered by increasing numeric ID. Each record contains `id`, `event_type`, `payload`, and `created_at`. Use the last processed ID to reconcile missed webhooks.

Webhook POSTs contain the immutable event JSON and `X-Dodo-Event-Id`, `X-Dodo-Timestamp`, and `X-Dodo-Signature: v1=<hex HMAC-SHA256>`. The signed message is the UTF-8 bytes of `timestamp + "." + event_id + "." + raw JSON body`, with the decoded endpoint secret as the HMAC key. Check the timestamp is within five minutes, compare signatures in constant time, and deduplicate event IDs. Delivery may be repeated. The local mock PSP's `/demo/webhook-sink` is only a demo receiver; PSP charge creation happens through `/charges`, not through webhooks.

## API keys

- `POST /api-keys` creates a new key for the authenticated business. Returns `201` with `id`, `prefix`, and full `api_key`; the full key is never returned again.
- `DELETE /api-keys/{id}` revokes a key owned by the authenticated business and returns `204`. Requests using it then return `401`.

## Mock PSP contract

The API calls `POST /charges` with `{"attempt_id":"<uuid>","token":"tok_success","amount_cents":2500}` and reads `GET /charges/{attempt_id}` for reconciliation. The PSP returns `{"status":"succeeded","psp_ref":"<uuid>"}` or `{"status":"failed","code":"card_declined"}`. A repeated attempt ID with a different token or amount returns `409`.
