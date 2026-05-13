-- Deterministic seed data for the benchmark suite.
--
-- Run this after migrations on each framework's database so every
-- implementation starts from an identical data set.
--
-- 1 000 posts (deterministic titles and bodies) + 1 API token used by
-- the load-test authenticated route.

TRUNCATE TABLE posts RESTART IDENTITY CASCADE;

INSERT INTO posts (title, body, published, author)
SELECT
    'Post number ' || n,
    'This is the body of post number ' || n || '. It contains enough text to be realistic. ' ||
    repeat('Lorem ipsum dolor sit amet. ', 3),
    (n % 3 != 0),   -- every third post is a draft
    CASE n % 5
        WHEN 0 THEN 'alice'
        WHEN 1 THEN 'bob'
        WHEN 2 THEN 'carol'
        WHEN 3 THEN 'dave'
        ELSE         'eve'
    END
FROM generate_series(1, 1000) AS g(n);

-- Single well-known API token for the authenticated-route load test.
TRUNCATE TABLE api_tokens RESTART IDENTITY CASCADE;
INSERT INTO api_tokens (token, principal)
VALUES ('benchmark-token-abc123', 'benchmark-user');
