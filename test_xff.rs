use http::{HeaderMap, HeaderValue};

fn main() {
    let mut headers = HeaderMap::new();
    headers.append("X-Forwarded-For", HeaderValue::from_static("attacker_ip"));
    headers.append("X-Forwarded-For", HeaderValue::from_static("real_ip"));

    let first = headers.get("X-Forwarded-For").unwrap().to_str().unwrap();
    println!("get(): {}", first);

    let all: Vec<_> = headers.get_all("X-Forwarded-For").iter().map(|v| v.to_str().unwrap()).collect();
    println!("get_all(): {:?}", all);
}
