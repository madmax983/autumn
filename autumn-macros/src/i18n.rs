//! Compile-time key-existence validation for the i18n `t!()` macro.
//!
//! The macro reads the default locale's `.ftl` file from the project's
//! manifest directory at compile time and emits a `compile_error!` if the
//! requested key is absent. When the file doesn't exist (e.g. an app that
//! enables the `i18n` feature flag but hasn't yet authored translations),
//! the macro degrades gracefully to a runtime-only call so the build
//! still succeeds — the runtime fallback path will produce the visible
//! `{$key}` marker.
//!
//! ## How the file is located
//!
//! 1. `AUTUMN_I18N_FILE` env var (absolute path) — set by `build.rs` for
//!    apps that want a non-default location, takes priority over (2).
//! 2. `$CARGO_MANIFEST_DIR/i18n/<default_locale>.ftl`, where
//!    `<default_locale>` is the value of the `AUTUMN_I18N_DEFAULT_LOCALE`
//!    env var, defaulting to `"en"`.
//!
//! Both env vars are read at proc-macro expansion time. Because they are
//! `cargo:rerun-if-env-changed`-friendly, a `build.rs` change will
//! correctly invalidate the macro's cached parse on the next build.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, Ident, LitStr, Token};

/// Parsed input for `t!(locale, "key" [, arg = value]...)`.
struct TMacroInput {
    locale: Expr,
    key: LitStr,
    args: Punctuated<KvArg, Token![,]>,
}

struct KvArg {
    name: Ident,
    value: Expr,
}

impl Parse for KvArg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let _eq: Token![=] = input.parse()?;
        let value: Expr = input.parse()?;
        Ok(Self { name, value })
    }
}

impl Parse for TMacroInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let locale: Expr = input.parse()?;
        let _comma: Token![,] = input.parse()?;
        let key: LitStr = input.parse()?;
        let args = if input.is_empty() {
            Punctuated::new()
        } else {
            let _comma: Token![,] = input.parse()?;
            Punctuated::parse_terminated(input)?
        };
        Ok(Self { locale, key, args })
    }
}

/// Expand `t!(locale, "key", arg = value, ...)`.
///
/// Generates:
///
/// ```ignore
/// {
///     let args: &[(&str, &str)] = &[(stringify!(arg), value), ...];
///     ::autumn_web::i18n::Locale::t_with(&locale, "key", args)
/// }
/// ```
///
/// Plus, at compile time, validates that `"key"` exists in the default
/// locale's bundle (when discoverable). If validation fails, returns a
/// `compile_error!` invocation with a helpful diagnostic.
pub fn t_macro(input: TokenStream) -> TokenStream {
    let parsed = match syn::parse2::<TMacroInput>(input) {
        Ok(p) => p,
        Err(err) => return err.to_compile_error(),
    };

    if let Some(err) = validate_key(&parsed.key) {
        return err;
    }

    let TMacroInput { locale, key, args } = parsed;
    if args.is_empty() {
        quote! {
            ::autumn_web::i18n::Locale::t(&#locale, #key)
        }
    } else {
        let arg_pairs = args.iter().map(|KvArg { name, value }| {
            let name_str = name.to_string();
            quote! { (#name_str, #value) }
        });
        quote! {{
            let __autumn_i18n_args: &[(&str, &str)] = &[ #( #arg_pairs ),* ];
            ::autumn_web::i18n::Locale::t_with(&#locale, #key, __autumn_i18n_args)
        }}
    }
}

/// Returns `Some(compile_error_tokens)` when the key is **definitely**
/// missing from the discovered default-locale bundle, `None` otherwise
/// (key found, OR no bundle was discoverable so we degrade to runtime).
fn validate_key(key_lit: &LitStr) -> Option<TokenStream> {
    let bundle = match load_default_bundle() {
        BundleLookup::Loaded(map) => map,
        BundleLookup::NoFile => return None,
    };
    let key = key_lit.value();
    if bundle.contains_key(&key) {
        return None;
    }
    let suggestion = closest_key(&key, bundle.keys()).map_or_else(String::new, |closest| {
        format!("\n  hint: did you mean `{closest}`?")
    });
    let msg = format!("i18n key `{key}` is not defined in the default locale bundle{suggestion}");
    Some(quote_spanned! { key_lit.span() =>
        compile_error!(#msg)
    })
}

enum BundleLookup {
    Loaded(&'static HashMap<String, String>),
    NoFile,
}

fn load_default_bundle() -> BundleLookup {
    static CACHE: OnceLock<Option<HashMap<String, String>>> = OnceLock::new();
    let cached = CACHE.get_or_init(read_and_parse_default_bundle);
    cached
        .as_ref()
        .map_or(BundleLookup::NoFile, BundleLookup::Loaded)
}

fn read_and_parse_default_bundle() -> Option<HashMap<String, String>> {
    let path = locate_default_bundle()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    Some(parse_keys(&raw))
}

fn locate_default_bundle() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("AUTUMN_I18N_FILE") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let default_locale =
        std::env::var("AUTUMN_I18N_DEFAULT_LOCALE").unwrap_or_else(|_| "en".to_owned());
    let candidate = PathBuf::from(manifest)
        .join("i18n")
        .join(format!("{default_locale}.ftl"));
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// Minimal FTL key extractor — duplicates only the keys, not the values, to
/// keep the proc-macro's footprint tiny. Mirrors the parser in
/// `autumn-web/src/i18n.rs` for the entries the validation cares about.
fn parse_keys(src: &str) -> HashMap<String, String> {
    let mut keys = HashMap::new();
    for line in src.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Skip indented continuation lines.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        if let Some((raw_key, _)) = trimmed.split_once('=') {
            let key = raw_key.trim();
            if !key.is_empty() {
                keys.insert(key.to_owned(), String::new());
            }
        }
    }
    keys
}

/// Pick the lexically-closest key name (Levenshtein distance ≤ 3) for a
/// helpful "did you mean" diagnostic. Returns `None` if nothing is close.
fn closest_key<'a, I>(target: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a String>,
{
    let mut best: Option<(&'a str, usize)> = None;
    for cand in candidates {
        let d = levenshtein(target, cand);
        if d <= 3 && best.is_none_or(|(_, current)| d < current) {
            best = Some((cand.as_str(), d));
        }
    }
    best.map(|(s, _)| s)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ac) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            let cost = usize::from(ac != bc);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keys_extracts_basic_entries() {
        let src = "# comment\nfoo = bar\nbaz = qux\n";
        let keys = parse_keys(src);
        assert!(keys.contains_key("foo"));
        assert!(keys.contains_key("baz"));
    }

    #[test]
    fn parse_keys_skips_continuation_lines() {
        let src = "long = first\n  continued\n  more\nshort = ok\n";
        let keys = parse_keys(src);
        assert!(keys.contains_key("long"));
        assert!(keys.contains_key("short"));
        assert!(!keys.contains_key("continued"));
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        assert_eq!(levenshtein("abc", "ac"), 1);
        assert_eq!(levenshtein("welcome.tite", "welcome.title"), 1);
    }

    #[test]
    fn closest_key_finds_typo() {
        let candidates = ["welcome.title".to_owned(), "welcome.greeting".to_owned()];
        let got = closest_key("welcome.tite", candidates.iter());
        assert_eq!(got, Some("welcome.title"));
    }

    #[test]
    fn closest_key_returns_none_when_nothing_close() {
        let candidates = ["hi".to_owned()];
        let got = closest_key("completely.unrelated.key", candidates.iter());
        assert!(got.is_none());
    }
}
