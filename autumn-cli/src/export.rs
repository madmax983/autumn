use std::fs::File;
use std::io::Write;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Value, json};

pub fn run(url: &str, output: &str) {
    let base_url = url.trim_end_matches('/');
    println!("Exporting diagnostics from {base_url}");

    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_else(|e| {
            eprintln!("Failed to create HTTP client: {e}");
            std::process::exit(1);
        });

    let health = fetch_endpoint(&client, base_url, "/actuator/health");
    let metrics = fetch_endpoint(&client, base_url, "/actuator/metrics");
    let tasks = fetch_endpoint(&client, base_url, "/actuator/tasks");
    let loggers = fetch_endpoint(&client, base_url, "/actuator/loggers");

    // Use std::time for a simple timestamp as chrono is not available
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let snapshot = json!({
        "timestamp": timestamp,
        "url": base_url,
        "health": health,
        "metrics": metrics,
        "tasks": tasks,
        "loggers": loggers,
    });

    match File::create(output) {
        Ok(mut file) => {
            let json_str = serde_json::to_string_pretty(&snapshot).unwrap();
            if let Err(e) = file.write_all(json_str.as_bytes()) {
                eprintln!("Failed to write to file '{output}': {e}");
                std::process::exit(1);
            }
            println!("Successfully exported diagnostics to {output}");
        }
        Err(e) => {
            eprintln!("Failed to create file '{output}': {e}");
            std::process::exit(1);
        }
    }
}

fn fetch_endpoint(client: &Client, base_url: &str, path: &str) -> Value {
    let full_url = format!("{base_url}{path}");
    match client.get(&full_url).send() {
        Ok(response) => {
            if response.status().is_success() || response.status().as_u16() == 503 {
                response
                    .json::<Value>()
                    .unwrap_or_else(|e| json!({ "error": format!("Failed to parse JSON: {}", e) }))
            } else {
                json!({ "error": format!("HTTP {}", response.status()) })
            }
        }
        Err(e) => {
            json!({ "error": format!("Request failed: {}", e) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn test_fetch_endpoint_success() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(&mut stream);
                let mut req_line = String::new();
                reader.read_line(&mut req_line).unwrap();

                loop {
                    let mut header_line = String::new();
                    reader.read_line(&mut header_line).unwrap();
                    if header_line == "\r\n" {
                        break;
                    }
                }

                let body = r#"{"status": "ok"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let client = Client::new();
        let val = fetch_endpoint(&client, &url, "/test");
        assert_eq!(val["status"], "ok");
    }

    #[test]
    fn test_fetch_endpoint_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // Close so connection fails

        let client = Client::builder()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let val = fetch_endpoint(&client, &format!("http://127.0.0.1:{port}"), "/test");
        assert!(val.get("error").is_some());
    }

    #[test]
    fn test_fetch_endpoint_404() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(&mut stream);
                let mut req_line = String::new();
                reader.read_line(&mut req_line).unwrap();

                loop {
                    let mut header_line = String::new();
                    reader.read_line(&mut header_line).unwrap();
                    if header_line == "\r\n" {
                        break;
                    }
                }

                let response =
                    "HTTP/1.1 404 NOT FOUND\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let client = Client::new();
        let val = fetch_endpoint(&client, &url, "/test");
        assert!(val["error"].as_str().unwrap().contains("HTTP 404"));
    }
}
