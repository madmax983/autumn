use autumn_web::prelude::*;

#[get("/../../../etc/passwd")]
async fn passwd() -> &'static str {
    "hacked"
}

fn main() {}
