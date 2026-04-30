use autumn_web::get;
use autumn_web::extract::Path;

// Handler was renamed from `get_post` to `show_post` —
// the old `__autumn_path_get_post` no longer exists.
#[get("/posts/{id}")]
async fn show_post(_id: Path<i64>) -> &'static str {
    "post"
}

fn main() {
    // Stale reference to old helper name; must not compile.
    let _url = __autumn_path_get_post(42i64);
}
