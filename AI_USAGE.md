# AI Usage

Codex was used to inspect the assignment, draft the architecture and ER model, generate the initial Rust service, migration, documentation, and integration tests, and help diagnose compiler or test failures. Its outputs must be checked against the assignment and the running system; the final submission should describe the checks that actually passed.

## Decisions I made

The following decisions were made after reviewing AI-generated alternatives and
the assignment requirements. The final choices and their rationale are mine.

### 1. Asynchronous payment processing

AI helped me compare a synchronous payment endpoint with an asynchronous
worker-based flow. I chose to create a pending payment attempt, return
`202 Accepted`, and process the PSP call in the background.

This handles `tok_timeout` without keeping the API request open for 30 seconds
and gives the service a durable attempt that can be recovered after a crash.
The tradeoff is that clients must poll `GET /payment-attempts/{id}` instead of
receiving the final payment result immediately.

### 2. PSP idempotency and lookup by attempt UUID

AI helped identify that an application-level idempotency key is not enough to
prevent a duplicate external charge. I chose to use the payment attempt UUID
as the mock PSP idempotency key and added `GET /charges/{attempt_id}` for
reconciliation.

If the PSP records a charge and the API crashes before saving the result, the
worker can look up the same attempt instead of sending an unrelated second
charge. The tradeoff is that the mock PSP has a larger contract than the
minimum token-response behavior in the assignment, but it makes the crash-gap
behavior testable and explicit.

### 3. UUIDs instead of distributed time-sortable IDs

This was an independent design decision. I considered Sonyflake or another
time-sortable ID generator for better index locality and smaller identifiers,
but chose UUIDs because this service has one PostgreSQL database and does not
need independently generated IDs across regions or services.

I documented time-sortable IDs as a future scalability option rather than
adding clock, worker-ID, sequence, and rollback-handling complexity to the
assignment.

## AI correction

An earlier AI state diagram treated `payment_pending` as if it were an invoice
state and showed lease expiry transitioning the invoice back to `open`. I
corrected this: the invoice remains `open` while a separate payment attempt is
`pending`, and lease expiry only makes the work eligible for another worker
attempt. It does not determine the payment outcome.

I verified this against the invoice state constraint, the payment-attempt
schema, the payment worker, and the integration tests for timeout, network
failure, and crash-gap recovery.

## Verification performed

I ran `docker compose --profile test run --build --rm tests` against the
Compose services. All seven integration tests passed, including concurrency,
idempotency, PSP timeout/network failure, crash-gap reconciliation, and webhook
delivery. The mock processor remains a local simulation; passing these tests
does not establish production PSP compatibility or replace the candidate's
own video walkthrough.
