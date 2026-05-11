use http::{HeaderMap, HeaderValue};

fn main() {
    let mut headers = HeaderMap::new();
    headers.append("X-Forwarded-For", HeaderValue::from_static("attacker_ip"));
    headers.append("X-Forwarded-For", HeaderValue::from_static("real_ip"));

    // `.get()` only returns the FIRST header value!
    let first = headers.get("x-forwarded-for").unwrap().to_str().unwrap();
    println!("get() returns: {}", first);
}
