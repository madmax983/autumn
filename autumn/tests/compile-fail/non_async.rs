use autumn_web::get;

#[get("/hello")]
fn hello() -> &'static str {
    "Hello!"
}

fn main() {}
