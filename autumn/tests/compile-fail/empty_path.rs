use autumn::get;

#[get("")]
async fn hello() -> &'static str {
    "Hello!"
}

fn main() {}
