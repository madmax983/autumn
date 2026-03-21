use autumn::{get, routes};

#[get("/")]
async fn index() -> &'static str {
    "Welcome to Autumn!"
}

#[get("/hello")]
async fn hello() -> &'static str {
    "Hello, Autumn!"
}

#[get("/hello/{name}")]
async fn hello_name(name: autumn::extract::Path<String>) -> String {
    format!("Hello, {}!", *name)
}

#[autumn::main]
async fn main() {
    autumn::app()
        .routes(routes![index, hello, hello_name])
        .run()
        .await;
}
