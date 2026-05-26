-- autumn-safety: potentially-blocking 
-- adding stored generated column will backfill existing rows
ALTER TABLE pages ADD COLUMN search_vector tsvector GENERATED ALWAYS AS (
    setweight(to_tsvector('english'::regconfig, coalesce(title, '')), 'A') || 
    setweight(to_tsvector('english'::regconfig, coalesce(body, '')), 'B')
) STORED;

CREATE INDEX idx_pages_search_vector ON pages USING gin(search_vector);
