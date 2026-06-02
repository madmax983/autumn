use axum::http::HeaderMap;

fn main() {
    let mut headers = HeaderMap::new();
    headers.append("x-forwarded-host", "evil.com".parse().unwrap());
    headers.append("x-forwarded-host", "legit.com".parse().unwrap());

    let expected_host = headers.get("x-forwarded-host").unwrap().to_str().unwrap();
    println!("First header is: {}", expected_host);
}
