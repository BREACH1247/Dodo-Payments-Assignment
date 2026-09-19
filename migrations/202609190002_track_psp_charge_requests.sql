-- Test-visible instrumentation: each POST /charges increments this counter.
-- It lets integration tests distinguish a PSP request from a deduplicated API response.
ALTER TABLE psp_charges
    ADD COLUMN request_count integer NOT NULL DEFAULT 1 CHECK (request_count >= 1);
