/// Convert a title to a URL-safe slug.
///
/// "Hello World!" -> "hello-world"
/// "Ask Rust: What's new?" -> "ask-rust-what-s-new"
pub fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_title() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn special_characters() {
        assert_eq!(slugify("What's new in Rust?"), "what-s-new-in-rust");
    }

    #[test]
    fn already_slug() {
        assert_eq!(slugify("already-a-slug"), "already-a-slug");
    }
}
