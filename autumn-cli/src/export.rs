use std::fs::File;
use std::io::Write;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Value, json};

pub fn run(url: &str, output: &str) {
    if let Err(e) = run_inner(url, output) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

pub fn run_inner(url: &str, output: &str) -> Result<(), String> {
    let base_url = url.trim_end_matches('/');
    println!("Exporting diagnostics from {base_url}");

    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

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
                return Err(format!("Failed to write to file '{output}': {e}"));
            }
            println!("Successfully exported diagnostics to {output}");
            Ok(())
        }
        Err(e) => Err(format!("Failed to create file '{output}': {e}")),
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

    #[test]
    fn test_run_inner_success() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");

        thread::spawn(move || {
            // Need to handle 4 requests: health, metrics, tasks, loggers
            for _ in 0..4 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut reader = BufReader::new(&mut stream);
                    let mut req_line = String::new();
                    if reader.read_line(&mut req_line).is_err() || req_line.is_empty() {
                        continue;
                    }

                    loop {
                        let mut header_line = String::new();
                        if reader.read_line(&mut header_line).is_err()
                            || header_line == "\r\n"
                            || header_line.trim().is_empty()
                        {
                            break;
                        }
                    }

                    let body = r#"{"status": "ok"}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });

        let output_file = tempfile::NamedTempFile::new().unwrap();
        let output_path = output_file.path().to_str().unwrap();

        let result = run_inner(&url, output_path);
        assert!(result.is_ok());

        let contents = std::fs::read_to_string(output_path).unwrap();
        let json: Value = serde_json::from_str(&contents).unwrap();

        assert_eq!(json["url"], url);
        assert_eq!(json["health"]["status"], "ok");
        assert_eq!(json["metrics"]["status"], "ok");
        assert_eq!(json["tasks"]["status"], "ok");
        assert_eq!(json["loggers"]["status"], "ok");
    }

    #[test]
    fn test_fetch_endpoint_invalid_json() {
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

                let body = r#"{"status": "ok", "#; // Invalid JSON
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
        assert!(
            val["error"]
                .as_str()
                .unwrap()
                .contains("Failed to parse JSON")
        );
    }

    #[test]
    fn test_run_inner_file_creation_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");

        thread::spawn(move || {
            // Need to handle 4 requests: health, metrics, tasks, loggers
            for _ in 0..4 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut reader = BufReader::new(&mut stream);
                    let mut req_line = String::new();
                    if reader.read_line(&mut req_line).is_err() || req_line.is_empty() {
                        continue;
                    }

                    loop {
                        let mut header_line = String::new();
                        if reader.read_line(&mut header_line).is_err()
                            || header_line == "\r\n"
                            || header_line.trim().is_empty()
                        {
                            break;
                        }
                    }

                    let body = r#"{"status": "ok"}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });

        // Use an invalid path that cannot be created
        let result = run_inner(&url, "/invalid/path/that/does/not/exist/diag.json");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to create file"));
    }
}
