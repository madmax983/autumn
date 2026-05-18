# Pagination

Autumn ships two complementary pagination flavours, both available out of the
box in every `#[repository]`-backed resource and in scaffold-generated index
views:

| Flavour | Query params | Best for |
|---------|-------------|----------|
| **Offset** (`PageRequest` / `Page`) | `?page=N&size=M` | Browse-style UIs, admin tables |
| **Cursor** (`CursorRequest` / `CursorPage`) | `?cursor=<token>&size=M` | Feeds, large tables, replicas |

---

## Offset pagination

### How it works

Offset pagination uses a `LIMIT` / `OFFSET` SQL pair.  The client picks a
1-based page number (`?page=2`) and a page size (`?size=25`).  The server
executes two queries — `COUNT(*)` and the page slice — and returns a `Page<T>`
response that bundles the content together with total-pages metadata.

### In a `#[repository]` trait

Every `#[repository]`-generated struct gets a `page` method automatically:

```rust
// Defined in your repository trait (generated):
async fn page(&self, req: &PageRequest) -> AutumnResult<Page<Post>>;
```

Call it from any handler:

```rust
use autumn_web::pagination::{Page, PageRequest};
use crate::repositories::post::PgPostRepository;

#[get("/posts")]
async fn index(page: PageRequest, repo: PgPostRepository) -> AutumnResult<Json<Page<Post>>> {
    Ok(Json(repo.page(&page).await?))
}
```

### In a scaffold-generated index view

`autumn generate scaffold Post title:String body:Text` emits an `index` action
that uses the `PageRequest` extractor to call `repo.page()`.  Out-of-range or
missing values are clamped silently — consistent with the framework rule that
list endpoints never return HTTP 400 for pagination parameters:

```
GET /posts          → page 1, 20 items  (DEFAULT_PAGE_SIZE)
GET /posts?page=3   → page 3, 20 items
GET /posts?size=10  → page 1, 10 items
GET /posts?size=200 → page 1, 100 items (clamped to MAX_PAGE_SIZE)
GET /posts?size=abc → page 1, 20 items  (unparseable → default)
```

A Maud `pagination_nav` helper renders Previous / Next links with `hx-get`
attributes for htmx-friendly partial updates.

### Overriding page size

`PageRequest` uses `DEFAULT_PAGE_SIZE = 20` and `MAX_PAGE_SIZE = 100`.  Both
are public constants you can reference in your own code:

```rust
use autumn_web::pagination::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, PageRequest};

let req = PageRequest::new(1, 50); // page 1, 50 items
```

Values outside the valid range are clamped silently — `PageRequest` never
returns HTTP 400 on its own.

### Response shape

```json
{
  "content": [ ... ],
  "page": 1,
  "size": 25,
  "total_elements": 137,
  "total_pages": 6,
  "has_next": true,
  "has_previous": false
}
```

---

## Cursor pagination

### When to use it

Cursor (keyset) pagination is O(1) regardless of page depth and produces
**zero duplicate or skipped rows** under concurrent inserts — making it the
correct choice for:

- Real-time feeds and notification inboxes
- Admin-safe full-table iteration (exports, data migrations)
- Apps running on multiple read replicas where `OFFSET` can diverge

### Declaring a cursor key

Add `cursor_key = field` to the `#[repository]` attribute to generate the
`cursor_page` method.  The field is used as the primary sort column (descending)
with `id` as the tie-breaker:

```rust
#[autumn_web::repository(Post, cursor_key = created_at)]
pub trait PostRepository {}
```

This generates a `cursor_page` method that orders by `(created_at DESC, id DESC)`
and uses `id` as the sole cursor payload — correct whenever `created_at` values
are monotonically correlated with `id` (the typical case for auto-increment PKs).

For **universally correct** keyset pagination (e.g. backfilled or imported rows
where timestamps and ids may diverge), also supply `cursor_key_type`:

```rust
#[autumn_web::repository(Post, cursor_key = created_at, cursor_key_type = chrono::NaiveDateTime)]
pub trait PostRepository {}
```

With `cursor_key_type` the cursor encodes both `(NaiveDateTime, i64)` and the
WHERE clause becomes the full two-part predicate:
```sql
WHERE (created_at < $after_k) OR (created_at = $after_k AND id < $after_id)
```

> **Constraint:** `cursor_key` must be declared on a **non-nullable** column.
> In SQL, comparisons involving `NULL` (`<`, `=`) evaluate to `UNKNOWN`, so
> a nullable sort key silently drops rows from all keyset pagination queries.
> Make the column `NOT NULL` or implement `cursor_page` manually.

### Calling `cursor_page`

```rust
use autumn_web::pagination::{CursorPage, CursorRequest};
use crate::repositories::post::PgPostRepository;

#[get("/feed")]
async fn feed(cursor: CursorRequest, repo: PgPostRepository) -> AutumnResult<Json<CursorPage<Post>>> {
    Ok(Json(repo.cursor_page(&cursor).await?))
}
```

The first request omits `?cursor`; subsequent requests pass the `next_cursor`
token returned by the previous response.

### Cursor token format

Cursor tokens are base64url-encoded JSON, URL-safe without percent-encoding,
and opaque to clients.  Forging a token is equivalent to seeking to an
arbitrary offset — for sort-key-only cursors (timestamps + ids) this is not a
security concern.  If your cursor encodes **sensitive data** (tenant ids, access
scopes) use the signed cursor API:

```rust
// Encoding
let token = Cursor::encode_signed(&my_value, signing_key);

// Decoding in a handler
let value = cursor_req.decode_signed::<MyValue>(signing_key);
```

See the [`pagination`](https://docs.rs/autumn-web/latest/autumn_web/pagination/index.html)
module docs for the full signing API.

### Response shape

```json
{
  "content": [ ... ],
  "size": 20,
  "next_cursor": "eyJpZCI6MTIzfQ",
  "has_next": true
}
```

`next_cursor` is `null` on the final page.

---

## Offset vs cursor: decision guide

| Question | Use offset | Use cursor |
|---------|-----------|-----------|
| Do users navigate to a specific page number? | ✓ | |
| Is the list mostly stable (rarely updated)? | ✓ | |
| Is the list a feed with concurrent inserts? | | ✓ |
| Is the table > 1 M rows? | | ✓ |
| Do you run read replicas? | | ✓ |
| Do you need infinite scroll / exports? | | ✓ |

---

## htmx wiring

Scaffold-generated pagination links carry `hx-get` and `hx-target="body"`
attributes so htmx replaces the full page body on click — no additional JS
needed.  For partial updates (replacing only the list, not the full layout),
change `hx-target` to the id of your list container and set `hx-swap="innerHTML"`.

Example fragment from a generated `index.html`:

```html
<a href="/posts?page=2&size=25"
   hx-get="/posts?page=2&size=25"
   hx-target="body">
  Next →
</a>
```

---

## Further reading

- [`autumn_web::pagination`](https://docs.rs/autumn-web/latest/autumn_web/pagination/index.html) — API reference
- [`#[repository]` macro](./macro-transparency.md) — generated method inventory
- [Generators guide](./generators.md) — `autumn generate scaffold` options
