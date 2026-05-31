CREATE TABLE oauth_identities (
    id         BIGSERIAL PRIMARY KEY,
    user_id    BIGINT    NOT NULL,
    provider   TEXT      NOT NULL,
    subject    TEXT      NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    UNIQUE (provider, subject)
);

CREATE INDEX idx_oauth_identities_user ON oauth_identities (user_id);
