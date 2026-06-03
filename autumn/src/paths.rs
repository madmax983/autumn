//! Typed path helpers and the [`PathExt`] fluent query-string builder.
//!
//! Route macros emit a `__autumn_path_{name}(…) -> String` companion alongside
//! every handler. This module provides the [`PathExt`] extension trait that
//! lets callers append query parameters to those strings with a single
//! chained expression:
//!
//! ```ignore
//! let url = paths::list_posts().with_query("page", 2).with_query("size", 10);
//! // → "/posts?page=2&size=10"
//! ```

/// Fluent query-string builder for path strings produced by typed path helpers.
///
/// Automatically imported via [`autumn_web::prelude`].
pub trait PathExt {
    /// Append a percent-encoded `key=value` query parameter.
    ///
    /// The first call adds `?key=value`; subsequent calls add `&key=value`.
    /// Both key and value are percent-encoded (RFC 3986 §2.1).
    ///
    /// # Examples
    ///
    /// ```
    /// use autumn_web::paths::PathExt;
    ///
    /// let url = "/posts".to_string().with_query("page", 2).with_query("q", "hello world");
    /// assert_eq!(url, "/posts?page=2&q=hello%20world");
    /// ```
    #[must_use]
    fn with_query(self, key: impl std::fmt::Display, value: impl std::fmt::Display) -> String;
}

impl PathExt for String {
    fn with_query(self, key: impl std::fmt::Display, value: impl std::fmt::Display) -> String {
        let encoded_key = percent_encode(&key.to_string());
        let encoded_value = percent_encode(&value.to_string());
        let sep = if self.contains('?') { '&' } else { '?' };
        format!("{self}{sep}{encoded_key}={encoded_value}")
    }
}

/// Percent-encode one dynamic route path segment.
///
/// Route macro helpers use this before interpolating path parameters so
/// Display values like `a/b` remain a single segment (`a%2Fb`).
#[doc(hidden)]
#[must_use]
pub fn encode_path_segment(value: impl std::fmt::Display) -> String {
    percent_encode(&value.to_string())
}

/// Percent-encode a query component per RFC 3986.
///
/// Unreserved characters (ALPHA / DIGIT / `-` / `_` / `.` / `~`) are left
/// unchanged; everything else is `%XX`-encoded.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b => {
                out.push('%');
                let hi = b >> 4;
                let lo = b & 0xF;
                out.push(
                    char::from_digit(u32::from(hi), 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit(u32::from(lo), 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_query_first_param() {
        assert_eq!("/posts".to_string().with_query("page", 1), "/posts?page=1");
    }

    #[test]
    fn with_query_second_param() {
        let url = "/posts"
            .to_string()
            .with_query("page", 1)
            .with_query("size", 20);
        assert_eq!(url, "/posts?page=1&size=20");
    }

    #[test]
    fn with_query_encodes_space() {
        assert_eq!(
            "/search".to_string().with_query("q", "hello world"),
            "/search?q=hello%20world"
        );
    }

    #[test]
    fn with_query_encodes_equals_and_ampersand() {
        assert_eq!(
            "/x".to_string().with_query("filter", "a=b&c"),
            "/x?filter=a%3Db%26c"
        );
    }

    #[test]
    fn with_query_leaves_unreserved_chars_alone() {
        assert_eq!(
            "/x".to_string()
                .with_query("tag", "hello-world_foo.bar~baz"),
            "/x?tag=hello-world_foo.bar~baz"
        );
    }
}

/// Normalizes a request path by resolving `..` and `.` segments, mimicking routing behavior
/// to prevent path traversal bypasses in middleware prefix matching.
pub(crate) fn normalize_path_for_routing(path: &str) -> String {
    // Fast path: if the path has no traversal segments, it's already normalized
    if !path.contains('.') {
        return path.to_owned();
    }

    let mut segments = Vec::new();
    let is_absolute = path.starts_with('/');

    for segment in path.split('/') {
        if segment == ".." {
            segments.pop();
        } else if segment == "." || segment.is_empty() {
            // Do nothing
        } else {
            segments.push(segment);
        }
    }

    let mut result = String::with_capacity(path.len());
    if is_absolute {
        result.push('/');
    }
    result.push_str(&segments.join("/"));

    if path.ends_with('/') && result.len() > 1 {
        result.push('/');
    }

    if result.is_empty() {
        result.push('/');
    }

    result
}
