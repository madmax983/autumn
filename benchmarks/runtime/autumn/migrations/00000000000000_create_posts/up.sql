CREATE TABLE posts (
    id         BIGSERIAL    PRIMARY KEY,
    title      TEXT         NOT NULL,
    body       TEXT         NOT NULL,
    published  BOOLEAN      NOT NULL DEFAULT FALSE,
    author     TEXT         NOT NULL,
    created_at TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE TRIGGER posts_set_updated_at
BEFORE UPDATE ON posts
FOR EACH ROW EXECUTE FUNCTION set_updated_at();
