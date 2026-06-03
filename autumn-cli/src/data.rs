//! `autumn data export` and `autumn data import` — CSV data utilities.
//!
//! Both commands delegate to the running application's admin HTTP layer.
//! The admin plugin must be mounted at `/admin` (or the URL base you
//! specify) and the model must have CSV export/import enabled.

use std::fmt::Write as FmtWrite;
use std::fs;
use std::time::Duration;

use reqwest::blocking::{Client, multipart};

fn make_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .no_proxy()
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))
}

/// `autumn data export <model> [options]`
///
/// Calls `GET {url}/admin/{model}/export.csv` and writes the response body to
/// `out` (or `{model}.csv` when omitted).
pub fn run_export(model: &str, base_url: &str, out: Option<&str>, filter: Option<&str>) {
    if let Err(e) = run_export_inner(model, base_url, out, filter) {
        eprintln!("autumn data export: {e}");
        std::process::exit(1);
    }
}

fn run_export_inner(
    model: &str,
    base_url: &str,
    out: Option<&str>,
    filter: Option<&str>,
) -> Result<(), String> {
    let client = make_client()?;
    let base = base_url.trim_end_matches('/');
    let mut url = format!("{base}/admin/{model}/export.csv");

    if let Some(q) = filter {
        let encoded = percent_encode(q);
        url.push_str(&format!("?q={encoded}"));
    }

    println!("Exporting {model} from {url}");

    let response = client
        .get(&url)
        .send()
        .map_err(|e| format!("Request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Server returned HTTP {} for {url}",
            response.status()
        ));
    }

    let bytes = response
        .bytes()
        .map_err(|e| format!("Failed to read response body: {e}"))?;

    let output_path = out
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{model}.csv"));

    fs::write(&output_path, &bytes)
        .map_err(|e| format!("Failed to write '{output_path}': {e}"))?;

    let row_count = bytes.iter().filter(|&&b| b == b'\n').count().saturating_sub(1);
    println!("Exported {row_count} rows to {output_path}");
    Ok(())
}

/// `autumn data import <model> --in <file> [options]`
///
/// Uploads `input` as a multipart CSV to `POST {url}/admin/{model}/import`.
/// Prints the resulting `ImportReport` summary to stdout.
pub fn run_import(
    model: &str,
    base_url: &str,
    input: &str,
    dry_run: bool,
    _upsert_by: Option<&str>,
) {
    if let Err(e) = run_import_inner(model, base_url, input, dry_run) {
        eprintln!("autumn data import: {e}");
        std::process::exit(1);
    }
}

fn run_import_inner(
    model: &str,
    base_url: &str,
    input: &str,
    dry_run: bool,
) -> Result<(), String> {
    let client = make_client()?;
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/admin/{model}/import");

    let csv_bytes =
        fs::read(input).map_err(|e| format!("Failed to read '{input}': {e}"))?;

    let mode_value = if dry_run { "dry_run" } else { "insert" };
    let label = if dry_run { "Dry run" } else { "Import" };

    println!("{label}ing {model} from {input} → {url}");

    let form = multipart::Form::new()
        .part(
            "file",
            multipart::Part::bytes(csv_bytes)
                .file_name(
                    std::path::Path::new(input)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("import.csv")
                        .to_owned(),
                )
                .mime_str("text/csv")
                .map_err(|e| format!("MIME type error: {e}"))?,
        )
        .text("mode", mode_value.to_owned());

    let response = client
        .post(&url)
        .multipart(form)
        .send()
        .map_err(|e| format!("Request failed: {e}"))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|e| format!("Failed to read response: {e}"))?;

    if !status.is_success() {
        return Err(format!("Server returned HTTP {status}:\n{body}"));
    }

    // The admin plugin returns HTML. Parse out the key numbers from the
    // import result page if we can; otherwise just print the status.
    print_import_summary(&body, dry_run);
    Ok(())
}

/// Extract inserted/updated/skipped/error counts from the HTML result page
/// and print a concise summary.  Falls back gracefully if the page structure
/// changes.
fn print_import_summary(html: &str, dry_run: bool) {
    // Simple heuristic: look for digit-only content in the summary grid cells.
    let counts: Vec<u64> = html
        .split("font-size: 1.5rem")
        .skip(1)
        .take(4)
        .filter_map(|chunk| {
            chunk
                .find('>')
                .and_then(|start| chunk[start + 1..].find('<').map(|end| (start + 1, start + 1 + end)))
                .and_then(|(s, e)| chunk[s..e].trim().parse::<u64>().ok())
        })
        .collect();

    if counts.len() == 4 {
        let prefix = if dry_run { "(dry run) " } else { "" };
        println!("{prefix}inserted={} updated={} skipped={} errors={}", counts[0], counts[1], counts[2], counts[3]);
        if counts[3] > 0 {
            eprintln!("Import completed with {} errors. Check the admin UI for details.", counts[3]);
            std::process::exit(1);
        } else {
            println!("Import completed successfully.");
        }
    } else {
        println!("Import request accepted. Check the admin UI for details.");
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            b => {
                let _ = write!(out, "%{b:02X}"); // FmtWrite for String
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_encodes_special_chars() {
        assert_eq!(percent_encode("hello world"), "hello+world");
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(percent_encode("safe-chars_123"), "safe-chars_123");
    }

    #[test]
    fn run_export_prints_error_and_exits_on_bad_url() {
        // We cannot connect to localhost:9 (IANA discard port)
        // so the function should fail gracefully. We test that the
        // inner function returns Err rather than testing that it exits.
        let result = run_export_inner("posts", "http://127.0.0.1:9", None, None);
        assert!(result.is_err(), "should fail with unreachable URL");
    }

    #[test]
    fn run_import_prints_error_on_missing_file() {
        let result = run_import_inner(
            "posts",
            "http://127.0.0.1:3000",
            "/tmp/nonexistent_autumn_csv_import_test.csv",
            false,
        );
        assert!(result.is_err(), "should fail when file doesn't exist");
        assert!(
            result.unwrap_err().contains("Failed to read"),
            "error should mention file read failure"
        );
    }

    #[test]
    fn print_import_summary_handles_incomplete_html_gracefully() {
        // Should not panic on HTML that doesn't have the expected grid structure
        print_import_summary("<html>Something went wrong</html>", false);
    }
}
