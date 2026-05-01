//! Profile-picture (avatar) upload routes.
//!
//! Demonstrates the autumn-web `[storage]` feature end-to-end: a
//! `Blob` column on a `#[model]`, a multipart upload that streams
//! straight into the configured `BlobStore`, and a presigned URL
//! rendered in the user profile page. With `storage.backend = "local"`
//! (the dev default) bytes land under `target/blobs/`; with `s3`
//! they land in your bucket. The route is identical either way.

use autumn_web::extract::{Multipart, State};
use autumn_web::prelude::*;
use autumn_web::storage::{Blob, BlobStoreState};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::models::User;
use crate::schema::users;

use super::layout::{hx_redirect_to, layout};

/// Cap avatar uploads at 2MiB regardless of the framework's configured
/// `security.upload.max_file_size_bytes`. Routes can tighten the cap
/// further than the global, never loosen it.
const AVATAR_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Server-side MIME allowlist for avatar uploads. The HTML `accept`
/// attribute is a UI hint only — clients can declare any
/// content_type. The local backend's serving route now returns the
/// persisted content_type, so an unfiltered upload would let an
/// attacker stash e.g. an HTML file under the app's origin and have
/// it served as `text/html`. Enforce the allowlist here, not in the
/// framework's global `security.upload.allowed_mime_types`, since
/// other routes may legitimately accept other types.
const AVATAR_ALLOWED_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp"];

#[get("/settings/avatar")]
#[secured]
pub async fn avatar_form(session: Session, csrf: CsrfToken, mut db: Db) -> AutumnResult<Markup> {
    let username = session
        .get("username")
        .await
        .ok_or_else(|| AutumnError::unauthorized_msg("not logged in"))?;
    let user: User = users::table
        .filter(users::username.eq(&username))
        .select(User::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("user record missing"))?;

    Ok(layout(
        "Profile picture",
        Some(&username),
        Some(csrf.token()),
        html! {
            div class="max-w-md mx-auto bg-white rounded-lg shadow p-6" {
                h1 class="text-2xl font-bold mb-4" { "Profile picture" }
                @if let Some(blob) = &user.avatar {
                    p class="text-sm text-gray-500 mb-3" {
                        "Current: " (blob.byte_size) " bytes"
                        @if let Some(etag) = &blob.etag {
                            " · etag " span class="font-mono" { (&etag[..etag.len().min(8)]) }
                        }
                    }
                }
                form action="/settings/avatar" method="post" enctype="multipart/form-data"
                     class="space-y-3" {
                    input type="hidden" name="_csrf" value=(csrf.token());
                    input type="file" name="avatar" accept="image/png,image/jpeg,image/webp"
                          required class="block w-full text-sm";
                    button type="submit"
                           class="bg-orange-500 text-white py-2 px-4 rounded font-medium \
                                  hover:bg-orange-600" {
                        "Upload"
                    }
                }
                p class="text-xs text-gray-400 mt-4" {
                    "Max " (AVATAR_MAX_BYTES / 1024) " KiB. The bytes flow through the configured \
                     BlobStore (local in dev, S3 in prod)."
                }
            }
        },
    ))
}

#[post("/settings/avatar")]
#[secured]
pub async fn upload_avatar(
    State(state): State<AppState>,
    session: Session,
    mut db: Db,
    mut form: Multipart,
) -> AutumnResult<autumn_web::reexports::axum::response::Response> {
    let username = session
        .get("username")
        .await
        .ok_or_else(|| AutumnError::unauthorized_msg("not logged in"))?;
    let blobs = state
        .extension::<BlobStoreState>()
        .ok_or_else(|| AutumnError::internal_server_error_msg("storage not configured"))?;

    let user: User = users::table
        .filter(users::username.eq(&username))
        .select(User::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("user record missing"))?;

    // Stable per-user key. Re-uploading replaces the bytes atomically
    // through the BlobStore's temp-file + rename path.
    let key = format!("avatars/{}.bin", user.id);

    let mut new_blob: Option<Blob> = None;
    while let Some(field) = form.next_field().await? {
        if field.name() == Some("avatar") {
            // Enforce the MIME allowlist server-side. The HTML
            // `accept` attribute is hint-only; without this check a
            // crafted client could upload non-image content
            // (e.g. an HTML file) which would later be served from
            // the app origin with attacker-controlled
            // `Content-Type` — a stored-XSS vector now that the
            // local serving route honors the persisted MIME.
            let content_type = field.content_type().unwrap_or("");
            if !AVATAR_ALLOWED_MIME_TYPES.contains(&content_type) {
                return Err(AutumnError::unprocessable_msg(format!(
                    "unsupported avatar content type: {content_type:?} \
                     (allowed: {AVATAR_ALLOWED_MIME_TYPES:?})"
                )));
            }

            let store = blobs.store().clone();
            // Tighten the framework's global upload cap to a route-
            // local 2 MiB. The form text and the actual write cap
            // stay in sync this way; without `.with_max_bytes`,
            // `save_to_blob_store` would only enforce
            // `security.upload.max_file_size_bytes`, which can be
            // higher than the route policy.
            let blob = field
                .with_max_bytes(AVATAR_MAX_BYTES)
                .save_to_blob_store(&*store, &key)
                .await?;
            new_blob = Some(blob);
            break;
        }
    }
    let blob = new_blob.ok_or_else(|| AutumnError::bad_request_msg("missing avatar field"))?;

    diesel::update(users::table.filter(users::id.eq(user.id)))
        .set(users::avatar.eq(serde_json::to_value(&blob).expect("serializable")))
        .execute(&mut *db)
        .await
        .map_err(|err| AutumnError::internal_server_error_msg(err.to_string()))?;

    Ok(hx_redirect_to(&super::auth::__autumn_path_profile(
        &username,
    )))
}

autumn_web::paths![avatar_form, upload_avatar];
