use autumn_web::cached;

#[cached]
async fn no_args() -> String {
    "hello".to_string()
}

#[cached(ttl = "5m")]
async fn with_ttl(id: i64) -> String {
    format!("user-{id}")
}

#[cached(ttl = "1h", max = 100)]
async fn with_ttl_and_max(key: String) -> Vec<i32> {
    vec![1, 2, 3]
}

#[cached(max = 50)]
async fn max_only(a: i32, b: i32) -> i32 {
    a + b
}

#[cached]
fn sync_cached(x: i32) -> i32 {
    x * 2
}

fn main() {}
