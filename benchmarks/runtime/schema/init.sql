-- Canonical benchmark schema shared across all framework implementations.
--
-- Each framework app must create a table that is logically equivalent to
-- this definition. The exact DDL may vary per framework (e.g. Django uses
-- integer PKs by default; Rails uses BIGINT), but the column set and
-- semantics must match.

CREATE TABLE IF NOT EXISTS posts (
    id          BIGSERIAL    PRIMARY KEY,
    title       TEXT         NOT NULL,
    body        TEXT         NOT NULL,
    published   BOOLEAN      NOT NULL DEFAULT FALSE,
    author      TEXT         NOT NULL,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- Trigger to keep updated_at in sync on UPDATE.
CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS posts_set_updated_at ON posts;
CREATE TRIGGER posts_set_updated_at
BEFORE UPDATE ON posts
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- API token table for the authenticated/protected-route scenario.
-- Each framework uses its own auth mechanism; this table is provided for
-- frameworks that want a simple shared token store.
CREATE TABLE IF NOT EXISTS api_tokens (
    id          BIGSERIAL    PRIMARY KEY,
    token       TEXT         NOT NULL UNIQUE,
    principal   TEXT         NOT NULL,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);
