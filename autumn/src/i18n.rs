//! Locale-aware text resolution (opt-in via the `i18n` feature flag).
//!
//! RED-phase stub: the public API surface is in place so tests compile and
//! exercise the contract, but every function is `unimplemented!()`. The
//! GREEN-phase commit fills these in.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct I18nConfig {
    pub default_locale: String,
    pub supported_locales: Vec<String>,
    pub fallback_chain: Vec<String>,
    pub dir: String,
}

impl Default for I18nConfig {
    fn default() -> Self {
        Self {
            default_locale: String::new(),
            supported_locales: Vec::new(),
            fallback_chain: Vec::new(),
            dir: String::new(),
        }
    }
}

impl I18nConfig {
    #[must_use]
    pub fn resolved_fallback_chain(&self) -> Vec<String> {
        unimplemented!("RED phase")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("i18n directory `{path}` could not be read: {source}")]
    DirectoryRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("i18n default locale `{locale}` is missing — expected file at `{path}`")]
    MissingDefaultLocale { locale: String, path: PathBuf },
    #[error("failed to parse `{path}` at line {line}: {message}")]
    Parse {
        path: PathBuf,
        line: usize,
        message: String,
    },
    #[error("file `{path}` is not a recognizable locale tag")]
    InvalidLocaleFilename { path: PathBuf },
}

#[derive(Debug, Clone)]
pub struct Locale {
    tag: String,
    bundle: Option<Arc<Bundle>>,
}

impl Locale {
    #[must_use]
    pub fn new(_tag: impl Into<String>) -> Self {
        unimplemented!("RED phase")
    }

    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    #[must_use]
    pub fn with_bundle(mut self, bundle: Arc<Bundle>) -> Self {
        self.bundle = Some(bundle);
        self
    }

    #[must_use]
    pub fn bundle(&self) -> Option<&Arc<Bundle>> {
        self.bundle.as_ref()
    }

    #[must_use]
    pub fn t(&self, _key: &str) -> String {
        unimplemented!("RED phase")
    }

    #[must_use]
    pub fn t_with(&self, _key: &str, _args: &[(&str, &str)]) -> String {
        unimplemented!("RED phase")
    }
}

#[must_use]
pub fn negotiate<'a>(_requested: &str, _supported: &'a [String]) -> Option<&'a str> {
    unimplemented!("RED phase")
}

#[must_use]
pub fn parse_accept_language<'a>(_header: &str, _supported: &'a [String]) -> Option<&'a str> {
    unimplemented!("RED phase")
}

pub struct Bundle {
    _messages: HashMap<String, HashMap<String, String>>,
    _default_locale: String,
}

impl fmt::Debug for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bundle").finish()
    }
}

impl Bundle {
    pub fn load_from_dir(_dir: &Path, _config: &I18nConfig) -> Result<Self, LoadError> {
        unimplemented!("RED phase")
    }

    #[must_use]
    pub fn from_messages(
        _messages: HashMap<String, HashMap<String, String>>,
        _config: &I18nConfig,
    ) -> Self {
        unimplemented!("RED phase")
    }

    #[must_use]
    pub fn locales(&self) -> Vec<&str> {
        unimplemented!("RED phase")
    }

    #[must_use]
    pub fn supported_locales(&self) -> &[String] {
        unimplemented!("RED phase")
    }

    #[must_use]
    pub fn default_locale(&self) -> &str {
        unimplemented!("RED phase")
    }

    #[must_use]
    pub fn fallback_chain(&self) -> &[String] {
        unimplemented!("RED phase")
    }

    #[must_use]
    pub fn miss_count(&self) -> u64 {
        unimplemented!("RED phase")
    }

    pub fn translate(&self, _locale: &str, _key: &str, _args: &[(&str, &str)]) -> String {
        unimplemented!("RED phase")
    }
}

impl<S> FromRequestParts<S> for Locale
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(_parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        unimplemented!("RED phase")
    }
}

#[must_use]
pub fn set_locale_cookie(_locale: &str) -> String {
    unimplemented!("RED phase")
}

#[macro_export]
macro_rules! t {
    ($locale:expr, $key:expr) => {
        $crate::i18n::Locale::t(&$locale, $key)
    };
    ($locale:expr, $key:expr, $($arg:ident = $val:expr),+ $(,)?) => {{
        let args: &[(&str, &str)] = &[$((stringify!($arg), $val)),+];
        $crate::i18n::Locale::t_with(&$locale, $key, args)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, header};

    fn build_parts(uri: &str, headers: &[(&str, &str)]) -> Parts {
        let mut req = Request::builder().uri(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = req.body(Body::empty()).unwrap();
        let (parts, _) = req.into_parts();
        parts
    }

    fn cfg(default: &str, supported: &[&str]) -> I18nConfig {
        I18nConfig {
            default_locale: default.to_owned(),
            supported_locales: supported.iter().map(|s| (*s).to_owned()).collect(),
            fallback_chain: vec![],
            dir: "i18n".to_owned(),
        }
    }

    fn bundle_with(locales: &[(&str, &[(&str, &str)])], cfg: &I18nConfig) -> Bundle {
        let mut messages = HashMap::new();
        for (loc, kvs) in locales {
            let mut m = HashMap::new();
            for (k, v) in *kvs {
                m.insert((*k).to_owned(), (*v).to_owned());
            }
            messages.insert((*loc).to_owned(), m);
        }
        Bundle::from_messages(messages, cfg)
    }

    #[test]
    fn i18n_config_default_is_english_only() {
        let cfg = I18nConfig::default();
        assert_eq!(cfg.default_locale, "en");
        assert_eq!(cfg.supported_locales, vec!["en".to_owned()]);
        assert_eq!(cfg.dir, "i18n");
    }

    #[test]
    fn fallback_chain_defaults_to_default_locale() {
        let cfg = cfg("en", &["en", "es"]);
        assert_eq!(cfg.resolved_fallback_chain(), vec!["en".to_owned()]);
    }

    #[test]
    fn fallback_chain_appends_default_when_missing() {
        let mut cfg = cfg("en", &["en", "es", "pt-BR"]);
        cfg.fallback_chain = vec!["pt".to_owned(), "es".to_owned()];
        let chain = cfg.resolved_fallback_chain();
        assert_eq!(chain, vec!["pt".to_owned(), "es".to_owned(), "en".to_owned()]);
    }

    #[test]
    fn fallback_chain_keeps_user_order_when_default_present() {
        let mut cfg = cfg("en", &["en", "es"]);
        cfg.fallback_chain = vec!["es".to_owned(), "en".to_owned()];
        assert_eq!(
            cfg.resolved_fallback_chain(),
            vec!["es".to_owned(), "en".to_owned()]
        );
    }

    #[test]
    fn load_from_dir_errors_when_default_locale_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("es.ftl"), "hi = Hola\n").unwrap();
        let cfg = cfg("en", &["en", "es"]);
        let err = Bundle::load_from_dir(tmp.path(), &cfg).unwrap_err();
        match err {
            LoadError::MissingDefaultLocale { locale, path } => {
                assert_eq!(locale, "en");
                assert!(path.ends_with("en.ftl"));
            }
            other => panic!("expected MissingDefaultLocale, got {other:?}"),
        }
    }

    #[test]
    fn load_from_dir_loads_all_ftl_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("en.ftl"), "hi = Hi\n").unwrap();
        std::fs::write(tmp.path().join("es.ftl"), "hi = Hola\n").unwrap();
        std::fs::write(tmp.path().join("README.md"), "ignore me").unwrap();
        let cfg = cfg("en", &["en", "es"]);
        let bundle = Bundle::load_from_dir(tmp.path(), &cfg).unwrap();
        let mut locales = bundle.locales();
        locales.sort();
        assert_eq!(locales, vec!["en", "es"]);
    }

    #[test]
    fn load_from_dir_propagates_parse_errors() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("en.ftl"), "no equals here\n").unwrap();
        let cfg = cfg("en", &["en"]);
        let err = Bundle::load_from_dir(tmp.path(), &cfg).unwrap_err();
        assert!(matches!(err, LoadError::Parse { .. }));
    }

    #[test]
    fn load_from_dir_missing_directory_is_treated_as_missing_default() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let cfg = cfg("en", &["en"]);
        let err = Bundle::load_from_dir(&missing, &cfg).unwrap_err();
        assert!(matches!(err, LoadError::MissingDefaultLocale { .. }));
    }

    #[test]
    fn translate_returns_translated_string() {
        let cfg = cfg("en", &["en", "es"]);
        let bundle = bundle_with(
            &[("en", &[("hi", "Hi")]), ("es", &[("hi", "Hola")])],
            &cfg,
        );
        assert_eq!(bundle.translate("es", "hi", &[]), "Hola");
        assert_eq!(bundle.translate("en", "hi", &[]), "Hi");
    }

    #[test]
    fn translate_falls_back_to_default_locale_on_miss() {
        let cfg = cfg("en", &["en", "es"]);
        let bundle = bundle_with(
            &[
                ("en", &[("only_in_en", "english only")]),
                ("es", &[("hi", "Hola")]),
            ],
            &cfg,
        );
        assert_eq!(bundle.translate("es", "only_in_en", &[]), "english only");
    }

    #[test]
    fn translate_uses_explicit_fallback_chain() {
        let mut cfg = cfg("en", &["en", "es", "pt-BR"]);
        cfg.fallback_chain = vec!["pt".to_owned(), "es".to_owned(), "en".to_owned()];
        let bundle = bundle_with(
            &[
                ("en", &[]),
                ("pt", &[("howdy", "Olá")]),
                ("es", &[("howdy", "Hola")]),
            ],
            &cfg,
        );
        assert_eq!(bundle.translate("pt-BR", "howdy", &[]), "Olá");
    }

    #[test]
    fn translate_returns_marker_on_total_miss_and_records() {
        let cfg = cfg("en", &["en"]);
        let bundle = bundle_with(&[("en", &[])], &cfg);
        let result = bundle.translate("en", "definitely.missing", &[]);
        assert_eq!(result, "{$definitely.missing}");
        assert_eq!(bundle.miss_count(), 1);
    }

    #[test]
    fn translate_dedups_warnings_for_same_key() {
        let cfg = cfg("en", &["en"]);
        let bundle = bundle_with(&[("en", &[])], &cfg);
        for _ in 0..5 {
            let _ = bundle.translate("en", "missing", &[]);
        }
        assert_eq!(bundle.miss_count(), 5);
    }

    #[test]
    fn translate_substitutes_named_args() {
        let cfg = cfg("en", &["en"]);
        let bundle = bundle_with(&[("en", &[("greeting", "Hello, { $name }!")])], &cfg);
        assert_eq!(
            bundle.translate("en", "greeting", &[("name", "Ada")]),
            "Hello, Ada!"
        );
    }

    #[test]
    fn translate_leaves_unknown_args_visible() {
        let cfg = cfg("en", &["en"]);
        let bundle = bundle_with(&[("en", &[("greeting", "Hello, { $name }!")])], &cfg);
        let out = bundle.translate("en", "greeting", &[]);
        assert!(out.contains("{ $name }"), "got: {out}");
    }

    #[test]
    fn negotiate_exact_match() {
        let supported = vec!["en".to_owned(), "es".to_owned()];
        assert_eq!(negotiate("es", &supported), Some("es"));
    }

    #[test]
    fn negotiate_falls_back_to_primary_subtag() {
        let supported = vec!["en".to_owned(), "es".to_owned()];
        assert_eq!(negotiate("es-MX", &supported), Some("es"));
    }

    #[test]
    fn negotiate_returns_none_when_unsupported() {
        let supported = vec!["en".to_owned()];
        assert_eq!(negotiate("ja", &supported), None);
    }

    #[test]
    fn parse_accept_language_picks_highest_q() {
        let supported = vec!["en".to_owned(), "es".to_owned()];
        let header = "fr;q=0.9, es;q=0.8, en;q=0.7";
        assert_eq!(parse_accept_language(header, &supported), Some("es"));
    }

    #[test]
    fn parse_accept_language_skips_wildcard() {
        let supported = vec!["en".to_owned(), "es".to_owned()];
        assert_eq!(parse_accept_language("*", &supported), None);
    }

    #[tokio::test]
    async fn locale_extractor_uses_query_override() {
        let cfg = cfg("en", &["en", "es"]);
        let bundle = Arc::new(bundle_with(&[("en", &[]), ("es", &[])], &cfg));
        let mut parts =
            build_parts("/?locale=es", &[(header::ACCEPT_LANGUAGE.as_str(), "en")]);
        parts.extensions.insert(bundle.clone());
        let locale = Locale::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(locale.tag(), "es");
    }

    #[tokio::test]
    async fn locale_extractor_uses_cookie_when_no_query() {
        let cfg = cfg("en", &["en", "es"]);
        let bundle = Arc::new(bundle_with(&[("en", &[]), ("es", &[])], &cfg));
        let mut parts = build_parts(
            "/",
            &[
                (header::COOKIE.as_str(), "autumn_locale=es; other=foo"),
                (header::ACCEPT_LANGUAGE.as_str(), "en"),
            ],
        );
        parts.extensions.insert(bundle.clone());
        let locale = Locale::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(locale.tag(), "es");
    }

    #[tokio::test]
    async fn locale_extractor_negotiates_accept_language() {
        let cfg = cfg("en", &["en", "es"]);
        let bundle = Arc::new(bundle_with(&[("en", &[]), ("es", &[])], &cfg));
        let mut parts = build_parts(
            "/",
            &[(
                header::ACCEPT_LANGUAGE.as_str(),
                "es-MX,es;q=0.9,en;q=0.8",
            )],
        );
        parts.extensions.insert(bundle.clone());
        let locale = Locale::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(locale.tag(), "es");
    }

    #[tokio::test]
    async fn locale_extractor_falls_through_to_default() {
        let cfg = cfg("en", &["en", "es"]);
        let bundle = Arc::new(bundle_with(&[("en", &[]), ("es", &[])], &cfg));
        let mut parts = build_parts(
            "/?locale=ja",
            &[(header::ACCEPT_LANGUAGE.as_str(), "ja-JP")],
        );
        parts.extensions.insert(bundle.clone());
        let locale = Locale::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(locale.tag(), "en");
    }

    #[tokio::test]
    async fn locale_extractor_resolution_order_is_query_then_cookie() {
        let cfg = cfg("en", &["en", "es"]);
        let bundle = Arc::new(bundle_with(&[("en", &[]), ("es", &[])], &cfg));
        let mut parts = build_parts(
            "/?locale=es",
            &[(header::COOKIE.as_str(), "autumn_locale=en")],
        );
        parts.extensions.insert(bundle.clone());
        let locale = Locale::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(locale.tag(), "es");
    }

    #[test]
    fn set_locale_cookie_format() {
        let cookie = set_locale_cookie("es");
        assert!(cookie.starts_with("autumn_locale=es"));
        assert!(cookie.contains("Path=/"));
        assert!(cookie.contains("SameSite=Lax"));
    }

    #[test]
    fn t_macro_basic_lookup() {
        let cfg = cfg("en", &["en"]);
        let bundle = Arc::new(bundle_with(&[("en", &[("hi", "Hi")])], &cfg));
        let locale = Locale::new("en").with_bundle(bundle);
        assert_eq!(t!(locale, "hi"), "Hi");
    }

    #[test]
    fn t_macro_with_named_args() {
        let cfg = cfg("en", &["en"]);
        let bundle = Arc::new(bundle_with(
            &[("en", &[("g", "Hello, { $name }!")])],
            &cfg,
        ));
        let locale = Locale::new("en").with_bundle(bundle);
        assert_eq!(t!(locale, "g", name = "Ada"), "Hello, Ada!");
    }
}
