use autumn::get;

#[get("hello")]
async fn hello() -> &'static str {
    "Hello!"
}

fn main() {}
