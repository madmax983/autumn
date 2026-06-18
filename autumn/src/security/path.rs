//! Path normalization helper for security middleware.

/// Normalizes a URL path by resolving `.` and `..` segments to prevent path traversal bypasses.
///
/// Security middlewares (CSRF, CAPTCHA) that use prefix matching for exemptions
/// must evaluate the normalized path to prevent attackers from using `..` to
/// match a prefix while actually routing to a protected endpoint.
pub fn clean_path(path: &str) -> String {
    let mut segments = Vec::new();
    for segment in path.split('/') {
        if segment == ".." {
            segments.pop();
        } else if segment != "." && !segment.is_empty() {
            segments.push(segment);
        }
    }

    let mut normalized = String::new();
    if path.starts_with('/') {
        normalized.push('/');
    }
    normalized.push_str(&segments.join("/"));
    if path.ends_with('/') && !normalized.ends_with('/') {
        normalized.push('/');
    }
    if path.starts_with('/') && normalized.is_empty() {
        "/".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_path() {
        assert_eq!(clean_path("/api/public"), "/api/public");
        assert_eq!(clean_path("/api/../protected"), "/protected");
        assert_eq!(clean_path("/api/v1/../../protected"), "/protected");
        assert_eq!(clean_path("/api/public/"), "/api/public/");
        assert_eq!(clean_path("/api/public/.."), "/api");
        assert_eq!(clean_path("/api/public/../"), "/api/");
        assert_eq!(clean_path("/"), "/");
        assert_eq!(clean_path(""), "");
    }
}
