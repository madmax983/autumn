use autumn::get;

#[get("/hello")]
fn hello() -> &'static str {
    "Hello!"
}

fn main() {}
