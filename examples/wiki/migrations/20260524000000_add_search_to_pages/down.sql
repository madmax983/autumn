DROP INDEX IF EXISTS idx_pages_search_vector;
ALTER TABLE pages DROP COLUMN IF EXISTS search_vector;
