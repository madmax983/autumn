CREATE TABLE bookmarks (
    id BIGSERIAL PRIMARY KEY,
    url TEXT NOT NULL,
    title TEXT NOT NULL,
    tag TEXT NOT NULL DEFAULT 'general',
    alive BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_bookmarks_tag ON bookmarks (tag);
CREATE INDEX idx_bookmarks_url ON bookmarks (url);
CREATE INDEX idx_bookmarks_alive ON bookmarks (alive);
