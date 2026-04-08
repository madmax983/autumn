/// Convert a title to a URL-safe slug.
///
/// "Hello World!" -> "hello-world"
/// "Rust & WebAssembly" -> "rust-webassembly"
///
/// ⚡ Bolt Optimization:
/// This avoids multiple heap allocations (an intermediate `String` and `Vec`)
/// by iterating through characters in a single pass and pushing to a pre-allocated String.
pub fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_dash = true; // Start true to prevent leading dashes

    for c in title.chars() {
        if c.is_alphanumeric() {
            // Using flat_map to handle potential multiple chars from lowercase conversion
            for lc in c.to_lowercase() {
                slug.push(lc);
            }
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }

    if slug.ends_with('-') {
        slug.pop();
    }

    slug
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
