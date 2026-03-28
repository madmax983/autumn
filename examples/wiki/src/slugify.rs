/// Convert a title to a URL-safe slug.
///
/// "Hello World!" -> "hello-world"
/// "Rust & WebAssembly" -> "rust-webassembly"
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
        assert_eq!(slugify("Rust & WebAssembly!"), "rust-webassembly");
    }

    #[test]
    fn already_slug() {
        assert_eq!(slugify("already-a-slug"), "already-a-slug");
    }

    #[test]
    fn leading_trailing_spaces() {
        assert_eq!(slugify("  spaced out  "), "spaced-out");
    }
}
