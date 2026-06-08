CREATE TABLE api_tokens (
    id         BIGSERIAL    PRIMARY KEY,
    token      TEXT         NOT NULL UNIQUE,
    principal  TEXT         NOT NULL,
    created_at TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);
