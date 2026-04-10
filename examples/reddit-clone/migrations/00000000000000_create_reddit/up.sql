-- Users
CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    karma BIGINT NOT NULL DEFAULT 0,
    role TEXT NOT NULL DEFAULT 'user',
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_users_username ON users (username);

-- Subreddits
CREATE TABLE subreddits (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    slug TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    creator_id BIGINT NOT NULL REFERENCES users(id),
    subscriber_count BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_subreddits_slug ON subreddits (slug);
CREATE INDEX idx_subreddits_creator_id ON subreddits (creator_id);

-- Posts
CREATE TABLE posts (
    id BIGSERIAL PRIMARY KEY,
    title TEXT NOT NULL,
    slug TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    url TEXT,
    author_id BIGINT NOT NULL REFERENCES users(id),
    subreddit_id BIGINT NOT NULL REFERENCES subreddits(id),
    score BIGINT NOT NULL DEFAULT 0,
    hot_rank DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    comment_count BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_posts_slug ON posts (slug);
CREATE INDEX idx_posts_subreddit_id ON posts (subreddit_id);
CREATE INDEX idx_posts_author_id ON posts (author_id);
CREATE INDEX idx_posts_hot_rank ON posts (hot_rank DESC);
CREATE INDEX idx_posts_created_at ON posts (created_at DESC);

-- Comments
CREATE TABLE comments (
    id BIGSERIAL PRIMARY KEY,
    body TEXT NOT NULL,
    author_id BIGINT NOT NULL REFERENCES users(id),
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    parent_id BIGINT REFERENCES comments(id) ON DELETE CASCADE,
    score BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_comments_post_id ON comments (post_id);
CREATE INDEX idx_comments_author_id ON comments (author_id);
CREATE INDEX idx_comments_parent_id ON comments (parent_id);

-- Votes
CREATE TABLE votes (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    post_id BIGINT REFERENCES posts(id) ON DELETE CASCADE,
    comment_id BIGINT REFERENCES comments(id) ON DELETE CASCADE,
    value SMALLINT NOT NULL CHECK (value IN (-1, 1)),
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    CONSTRAINT votes_target_check CHECK (
        (post_id IS NOT NULL AND comment_id IS NULL) OR
        (post_id IS NULL AND comment_id IS NOT NULL)
    ),
    CONSTRAINT votes_unique_post UNIQUE (user_id, post_id),
    CONSTRAINT votes_unique_comment UNIQUE (user_id, comment_id)
);

CREATE INDEX idx_votes_user_id ON votes (user_id);
CREATE INDEX idx_votes_post_id ON votes (post_id);
CREATE INDEX idx_votes_comment_id ON votes (comment_id);

-- Durable live-feed events for cross-process WebSocket fan-out
CREATE TABLE live_feed_events (
    id BIGSERIAL PRIMARY KEY,
    subreddit_slug TEXT NOT NULL,
    event JSONB NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_live_feed_events_created_at ON live_feed_events (created_at);
