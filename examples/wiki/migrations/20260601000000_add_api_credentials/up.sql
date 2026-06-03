-- Stored third-party API tokens, encrypted at rest (issue #805).
--
-- The `token` column holds a base64 AES-256-GCM envelope, never plaintext. The
-- application encrypts on write and decrypts on read transparently via the
-- `#[encrypted]` attribute on the `ApiCredential` model. Configure the key with
-- `autumn credentials edit`:
--
--   [active_record_encryption]
--   primary_key = "<64 hex chars from `openssl rand -hex 32`>"
CREATE TABLE api_credentials (
    id         BIGSERIAL   PRIMARY KEY,
    label      TEXT        NOT NULL,
    token      TEXT        NOT NULL,
    created_at TIMESTAMP   NOT NULL DEFAULT NOW()
);
