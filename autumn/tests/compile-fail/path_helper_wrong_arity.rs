use autumn_web::get;
use autumn_web::extract::Path;

#[get("/posts/{id}")]
async fn show_post(_id: Path<i64>) -> &'static str {
    "post"
}

fn main() {
    // Calling with two arguments when only one is expected.
    let _url = __autumn_path_show_post(42i64, 99i64);
}
