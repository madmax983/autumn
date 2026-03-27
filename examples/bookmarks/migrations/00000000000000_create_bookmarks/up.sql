CREATE TABLE bookmarks (
    id SERIAL PRIMARY KEY,
    url TEXT NOT NULL,
    title TEXT NOT NULL,
    tag TEXT NOT NULL DEFAULT 'general',
    alive BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_bookmarks_tag ON bookmarks (tag);
CREATE INDEX idx_bookmarks_alive ON bookmarks (alive);
