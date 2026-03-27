CREATE TABLE posts (
    id SERIAL PRIMARY KEY,
    title TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE,
    body TEXT NOT NULL,
    published BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);

-- Index for fast slug lookups
CREATE INDEX idx_posts_slug ON posts (slug);

-- Index for listing published posts by date
CREATE INDEX idx_posts_published ON posts (published, created_at DESC);
