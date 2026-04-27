# File Storage in Autumn

Autumn ships a pluggable file-storage abstraction so apps that accept
user-uploaded files (avatars, attachments, generated reports) don't
have to pick an SDK, design a key scheme, or hand-roll URL signing
every time.

This is the layer that turns the
[`Multipart`](../../autumn/src/extract.rs) extractor's "stream-to-disk"
primitive into something that survives container restarts and works
across replicas.

## When you need it

Reach for `BlobStore` if you answer "yes" to any of:

- Does my app accept user-uploaded files?
- Will I run more than one web replica?
- Do I redeploy on a schedule, where local disk is wiped between
  containers?

If "no" to all three, the existing `MultipartField::save_to(path)`
primitive is fine.

## Quick start

Enable the `storage` cargo feature on `autumn-web`:

```toml
[dependencies]
autumn-web = { version = "0.4", features = ["storage", "multipart"] }
```

The framework gives you a working `Local` backend in `dev` out of the
box — bytes land under `target/blobs/` and signed URLs are served
from `/_blobs/...`. No further config required.

```rust,ignore
use autumn_web::extract::{Multipart, State};
use autumn_web::prelude::*;
use autumn_web::storage::BlobStoreState;

#[post("/avatar")]
async fn upload(
    State(state): State<AppState>,
    mut form: Multipart,
) -> AutumnResult<String> {
    let blobs = state
        .extension::<BlobStoreState>()
        .ok_or_else(|| AutumnError::internal_server_error_msg("storage not configured"))?;
    let store = blobs.store().clone();
    while let Some(field) = form.next_field().await? {
        if field.name() == Some("avatar") {
            let blob = field
                .save_to_blob_store(&*store, "avatars/me.png")
                .await?;
            return Ok(blob.key);
        }
    }
    Err(AutumnError::bad_request_msg("missing 'avatar' field"))
}
```

The full working version, including a Maud-rendered upload form and
an `<img src="..." />` that round-trips through a presigned URL,
lives in [`examples/avatars`](../../examples/avatars).

## Configuration

```toml
[storage]
backend = "local"            # "local" | "s3" | "disabled"
default_provider = "default"
allow_local_in_production = false

[storage.local]
root = "target/blobs"
mount_path = "/_blobs"
default_url_expiry_secs = 900
# signing_key = "..."        # optional; falls back to AUTUMN_STORAGE__LOCAL__SIGNING_KEY

[storage.s3]
bucket = "my-app-uploads"
region = "us-east-1"
endpoint = "https://s3.amazonaws.com"   # optional; required for R2/MinIO/Spaces/Wasabi
access_key_id_env = "AWS_ACCESS_KEY_ID"
secret_access_key_env = "AWS_SECRET_ACCESS_KEY"
force_path_style = false
```

Every field is overridable via `AUTUMN_STORAGE__*` env vars (see the
in-source [config docs](../../autumn/src/config.rs) for the canonical
list).

## Profile-aware defaults

| Profile | `[storage].backend` | Notes |
|---------|---------------------|-------|
| `dev`   | `disabled`          | Opt in by setting `backend = "local"` |
| `prod`  | `disabled`          | `backend = "local"` fails fast unless `storage.allow_local_in_production = true` |

The fail-fast in `prod` is intentional: a single-replica `Local`
deployment is fine, but it has to be explicitly acknowledged. Apps
that scale beyond one replica should select `s3`.

## The `Blob` column story

Apps store `Blob` columns; the `BlobStore` owns the bytes; the
database owns lifecycle.

```rust,ignore
use autumn_web::model;
use autumn_web::storage::Blob;

#[model]
pub struct User {
    pub id: i64,
    pub name: String,
    pub avatar: Option<Blob>,
}
```

`Blob` is `Serialize + Deserialize` and (when the `db` feature is on)
implements `AsExpression` / `FromSqlRow` for Postgres `JSONB`, so the
default `#[model]` derives Just Work.

Adding a blob column to an existing table is one column add:

```sql
ALTER TABLE users ADD COLUMN avatar JSONB NULL;
```

The framework intentionally does not provide `add_blob_column!`-style
DDL macros today: a single-column JSONB add is straightforward and
your existing migration tooling already knows how to do it.

## Presigned-URL semantics

| Backend | URL shape | Signing |
|---------|-----------|---------|
| `Local` | `/{mount_path}/{key}?exp=…&sig=…` | HMAC-SHA256 over `{key}:{exp}`, verified by the mounted serving route |
| `S3`    | Real S3 presigned URL | AWS SigV4 (or your provider's equivalent) |

Both expire. Both are tamper-resistant. Both are safe to embed in
templates and emails.

For the local backend, set `[storage.local].signing_key` (or the
`AUTUMN_STORAGE__LOCAL__SIGNING_KEY` env var) so URLs survive a
process restart and replicas agree on signatures. Without it the
framework generates a random key per process — fine for `dev`, never
for `prod`.

## Multi-replica safety

The local backend writes to a single host's disk. That's broken across
replicas:

- replica A serves the upload, the bytes land on A's disk
- replica B serves the next request, can't see A's bytes

The framework doesn't try to paper over this. It surfaces the
constraint:

1. `prod` + `local` without `allow_local_in_production` fails fast at
   startup.
2. `prod` + `local` + acknowledgement logs a warning explaining the
   replicas can't see each other's bytes.

Multi-replica production should choose `backend = "s3"`.

## Production checklist

Before flipping a real app to `backend = "s3"`:

- [ ] Bucket exists and is private (no public-read policy unless you
      really mean it).
- [ ] Bucket policy permits `PutObject`, `GetObject`, `DeleteObject`,
      and (for `head`) `HeadObject` from the credentials your app
      uses.
- [ ] CORS is configured if you'll ever generate browser-served
      presigned URLs across origins.
- [ ] Lifecycle rules are in place to expire orphaned blobs (the
      framework's first slice deliberately does not garbage-collect
      for you — `delete` the row and `delete` the blob in a
      transaction-bracketed pattern).
- [ ] Credentials come from your secrets manager via
      `access_key_id_env` / `secret_access_key_env`, not committed
      `autumn.toml`.
- [ ] You're using a region that's geographically near your app
      tier (latency on every `put` / `get`).

## What's out of scope (for now)

- **Image processing / resizing.** Track separately. `image` and
  `imageproc` have their own dependency surfaces.
- **Direct-to-S3 browser uploads (presigned PUT).** Useful eventually;
  the first slice keeps bytes flowing through the autumn process so
  the multipart MIME / size-cap policies still apply.
- **Native non-S3 backends (GCS, Azure Blob, B2 native).** Anyone
  whose object store speaks S3 is covered by `storage-s3`. Native
  backends are a future feature-flagged extension.
- **Antivirus / content moderation.** Compose a Tower middleware on
  top of `BlobStore` for this.
- **Orphan-blob garbage collection.** Document: lifecycle is the
  application's job (delete the row, then delete the blob). A
  `harvest`-backed sweeper can come later.
- **Migration tooling for moving data between backends.** Not
  framework's job today.

## Status: S3 backend

Acceptance for issue #494 ships the trait surface, the Local backend
with HMAC-signed URLs, the multipart integration, and the
configuration story. The `storage-s3` cargo feature builds, the
[`S3BlobStore`](../../autumn/src/storage/s3.rs) shell exists, but the
on-the-wire SDK calls are not yet wired up — operations return
`BlobStoreError::Unsupported` so applications fail loudly rather than
silently dropping bytes.

Hooking up an SDK (engineering's call between `aws-sdk-s3` and
`rust-s3`) is a follow-up. The trait surface is designed so the swap
is local to `autumn/src/storage/s3.rs`; nothing downstream of the
trait needs to change.
