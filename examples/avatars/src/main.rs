//! Profile-picture upload example demonstrating Autumn's pluggable
//! [`BlobStore`](autumn_web::storage::BlobStore) abstraction.
//!
//! Run:
//!
//! ```sh
//! cd examples/avatars
//! cargo run
//! # then POST a multipart form to http://localhost:3000/avatar
//! ```
//!
//! The integration test in `tests/avatars.rs` exercises the full
//! upload-then-render flow end-to-end against the local backend.

use autumn_web::extract::{Multipart, State};
use autumn_web::prelude::*;
use autumn_web::storage::BlobStoreState;

const AVATAR_KEY: &str = "avatars/me.png";

#[get("/")]
async fn index(State(state): State<AppState>) -> Markup {
    let avatar_url = if let Some(blobs) = state.extension::<BlobStoreState>() {
        let store = blobs.store().clone();
        store
            .presigned_url(AVATAR_KEY, std::time::Duration::from_secs(60 * 5))
            .await
            .ok()
    } else {
        None
    };
    html! {
        h1 { "Avatars" }
        @if let Some(url) = avatar_url {
            img src=(url) alt="avatar";
        }
        form action="/avatar" method="post" enctype="multipart/form-data" {
            input type="file" name="avatar" accept="image/png,image/jpeg";
            button type="submit" { "Upload" }
        }
    }
}

#[post("/avatar")]
async fn upload(State(state): State<AppState>, mut form: Multipart) -> AutumnResult<Markup> {
    let blobs = state
        .extension::<BlobStoreState>()
        .ok_or_else(|| AutumnError::internal_server_error_msg("storage not configured"))?;
    while let Some(field) = form.next_field().await? {
        if field.name() == Some("avatar") {
            let store = blobs.store().clone();
            let blob = field.save_to_blob_store(&*store, AVATAR_KEY).await?;
            return Ok(html! {
                p { "Uploaded " (blob.byte_size) " bytes (" (blob.content_type) ")." }
                p { a href="/" { "Back" } }
            });
        }
    }
    Err(AutumnError::bad_request_msg("missing 'avatar' field"))
}

#[autumn_web::main]
async fn main() {
    autumn_web::app().routes(routes![index, upload]).run().await;
}
