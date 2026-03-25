use autumn_web::get;

#[get("")]
async fn hello() -> &'static str {
    "Hello!"
}

fn main() {}
