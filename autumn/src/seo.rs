//! First-class SEO toolkit: sitemap.xml, robots.txt, and meta tag helpers.
//!
//! Autumn content apps need three artifacts for full crawl coverage:
//! a `sitemap.xml`, a `robots.txt`, and per-page meta tags. This module
//! provides builders and helpers for all three with sensible defaults.
//!
//! # Quick start
//!
//! ## Meta tags
//!
//! ```rust,ignore
//! use autumn_web::seo::SeoMeta;
//! use autumn_web::prelude::*;
//!
//! #[get("/posts/{slug}")]
//! async fn show(slug: Path<String>) -> Markup {
//!     let meta = SeoMeta::new()
//!         .title("My Blog Post")
//!         .description("A fascinating exploration of things")
//!         .canonical(format!("https://example.com/posts/{}", *slug))
//!         .og_image("https://example.com/og.jpg");
//!     html! {
//!         head { (meta.render()) }
//!     }
//! }
//! ```
//!
//! ## Sitemap
//!
//! Register a [`SitemapSource`] on the app builder for dynamic routes:
//!
//! ```rust,ignore
//! use autumn_web::seo::{SitemapEntry, SitemapSource};
//! use std::future::Future;
//! use std::pin::Pin;
//!
//! struct BlogSitemapSource;
//!
//! impl SitemapSource for BlogSitemapSource {
//!     fn entries(&self) -> Pin<Box<dyn Future<Output = Vec<SitemapEntry>> + Send + '_>> {
//!         Box::pin(async {
//!             vec![SitemapEntry::new("https://example.com/posts/hello")]
//!         })
//!     }
//! }
//!
//! // In main():
//! // autumn_web::app()
//! //     .routes(routes![...])
//! //     .seo_source(BlogSitemapSource)
//! //     .run()
//! //     .await;
//! ```
//!
//! ## Robots.txt
//!
//! Configure in `autumn.toml`:
//!
//! ```toml
//! [seo]
//! base_url = "https://example.com"
//!
//! [seo.robots]
//! additional_rules = ["Disallow: /admin"]
//! ```
//!
//! The framework defaults: `dev`/`test` → disallow all; `prod` → allow all.

use std::fmt::Write as _;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::Response;
use axum::routing::get;

#[cfg(feature = "maud")]
use maud::{Markup, html};

// ── SitemapEntry ─────────────────────────────────────────────────────────────

/// A single URL entry in a sitemap.
#[derive(Debug, Clone)]
pub struct SitemapEntry {
    /// The fully-qualified URL of the page.
    pub loc: String,
    /// Last modified date in `YYYY-MM-DD` format.
    pub lastmod: Option<String>,
    /// Suggested crawl frequency.
    pub changefreq: Option<SitemapChangefreq>,
    /// Relative priority (0.0–1.0). Clamped on construction.
    pub priority: Option<f32>,
}

impl SitemapEntry {
    /// Create a new entry with the given URL.
    pub fn new(loc: impl Into<String>) -> Self {
        Self {
            loc: loc.into(),
            lastmod: None,
            changefreq: None,
            priority: None,
        }
    }

    /// Set the last modified date (ISO 8601, e.g. `"2026-01-15"`).
    #[must_use]
    pub fn lastmod(mut self, lastmod: impl Into<String>) -> Self {
        self.lastmod = Some(lastmod.into());
        self
    }

    /// Set the suggested change frequency.
    #[must_use]
    pub const fn changefreq(mut self, changefreq: SitemapChangefreq) -> Self {
        self.changefreq = Some(changefreq);
        self
    }

    /// Set the priority (clamped to 0.0–1.0).
    #[must_use]
    pub const fn priority(mut self, priority: f32) -> Self {
        self.priority = Some(priority.clamp(0.0, 1.0));
        self
    }
}

// ── SitemapChangefreq ─────────────────────────────────────────────────────────

/// Suggested update frequency for a sitemap entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitemapChangefreq {
    Always,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    Never,
}

impl SitemapChangefreq {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Hourly => "hourly",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Yearly => "yearly",
            Self::Never => "never",
        }
    }
}

// ── SitemapSource ─────────────────────────────────────────────────────────────

/// Trait for providing dynamic sitemap entries (e.g. blog posts from a database).
///
/// Implement this trait and register the source with
/// [`AppBuilder::seo_source`](crate::app::AppBuilder::seo_source).
///
/// # Example
///
/// ```rust,ignore
/// use autumn_web::seo::{SitemapEntry, SitemapSource};
/// use std::pin::Pin;
/// use std::future::Future;
///
/// struct PostSitemapSource;
///
/// impl SitemapSource for PostSitemapSource {
///     fn entries(&self) -> Pin<Box<dyn Future<Output = Vec<SitemapEntry>> + Send + '_>> {
///         Box::pin(async {
///             vec![
///                 SitemapEntry::new("https://example.com/posts/hello-world")
///                     .lastmod("2026-01-15")
///                     .changefreq(autumn_web::seo::SitemapChangefreq::Weekly),
///             ]
///         })
///     }
/// }
/// ```
pub trait SitemapSource: Send + Sync {
    /// Return the sitemap entries for this source.
    fn entries(&self) -> Pin<Box<dyn Future<Output = Vec<SitemapEntry>> + Send + '_>>;
}

// ── Internal AppState extension newtypes ──────────────────────────────────────

/// Registered sitemap sources stored in AppState extensions.
#[doc(hidden)]
pub struct RegisteredSitemapSources(pub Vec<Arc<dyn SitemapSource>>);

/// Registered SEO config stored in AppState extensions.
#[doc(hidden)]
pub struct RegisteredSeoConfig(pub crate::config::SeoConfig);

// ── robots_txt() ──────────────────────────────────────────────────────────────

/// Generate an environment-aware `robots.txt` string.
///
/// - `dev`/`test` profiles → `Disallow: /` (blocks all crawlers)
/// - `prod`/`production` profiles → `Allow: /` (permits all crawlers)
///
/// # Arguments
///
/// * `profile` — The active profile (`"dev"`, `"test"`, or `"prod"`).
/// * `sitemap_url` — Optional sitemap URL to inject as a `Sitemap:` directive.
/// * `additional_rules` — Extra lines to append (e.g. `"Disallow: /admin"`).
#[must_use]
pub fn robots_txt(profile: &str, sitemap_url: Option<&str>, additional_rules: &[String]) -> String {
    let mut txt = String::new();

    let is_prod = matches!(profile, "prod" | "production");
    if is_prod {
        txt.push_str("User-agent: *\nAllow: /\n");
    } else {
        txt.push_str("User-agent: *\nDisallow: /\n");
    }

    for rule in additional_rules {
        txt.push_str(rule);
        txt.push('\n');
    }

    if let Some(url) = sitemap_url {
        txt.push('\n');
        txt.push_str("Sitemap: ");
        txt.push_str(url);
        txt.push('\n');
    }

    txt
}

// ── sitemap_xml() ─────────────────────────────────────────────────────────────

/// Generate a valid `sitemap.xml` string.
///
/// For sites with more than 50,000 URLs, generates a
/// `<sitemapindex>` document that references numbered sub-sitemaps
/// (`/sitemap-1.xml`, `/sitemap-2.xml`, …).
///
/// # Arguments
///
/// * `entries` — The sitemap entries to include.
/// * `base_url` — The site base URL (e.g. `"https://example.com"`), used to
///   build sub-sitemap URLs in sitemapindex mode.
#[must_use]
pub fn sitemap_xml(entries: &[SitemapEntry], base_url: Option<&str>) -> String {
    const CHUNK_SIZE: usize = 50_000;

    if entries.len() > CHUNK_SIZE {
        sitemap_index_xml(entries, base_url.unwrap_or(""), CHUNK_SIZE)
    } else {
        sitemap_urlset_xml(entries)
    }
}

/// Build a `<urlset>` sitemap.
#[must_use]
pub(crate) fn sitemap_urlset_xml(entries: &[SitemapEntry]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">",
    );
    for entry in entries {
        xml.push_str("\n  <url>");
        xml.push_str("\n    <loc>");
        xml.push_str(&xml_escape(&entry.loc));
        xml.push_str("</loc>");
        if let Some(lastmod) = &entry.lastmod {
            xml.push_str("\n    <lastmod>");
            xml.push_str(lastmod);
            xml.push_str("</lastmod>");
        }
        if let Some(freq) = entry.changefreq {
            xml.push_str("\n    <changefreq>");
            xml.push_str(freq.as_str());
            xml.push_str("</changefreq>");
        }
        if let Some(prio) = entry.priority {
            xml.push_str("\n    <priority>");
            write!(xml, "{prio:.1}").ok();
            xml.push_str("</priority>");
        }
        xml.push_str("\n  </url>");
    }
    xml.push_str("\n</urlset>");
    xml
}

/// Build a `<sitemapindex>` for sites with more than `chunk_size` URLs.
fn sitemap_index_xml(entries: &[SitemapEntry], base_url: &str, chunk_size: usize) -> String {
    let chunk_count = entries.len().div_ceil(chunk_size);
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">",
    );
    for i in 0..chunk_count {
        let idx = i + 1;
        xml.push_str("\n  <sitemap>\n    <loc>");
        xml.push_str(base_url);
        xml.push_str("/sitemap-");
        write!(xml, "{idx}").ok();
        xml.push_str(".xml</loc>\n  </sitemap>");
    }
    xml.push_str("\n</sitemapindex>");
    xml
}

/// Escape XML special characters in a single pass over the input.
fn xml_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

// ── SeoMeta builder ───────────────────────────────────────────────────────────

/// Builder for per-page SEO meta tags.
///
/// Generates `<title>`, `<meta>`, `<link rel="canonical">`, Open Graph,
/// and Twitter card tags from a fluent builder API.
///
/// # Example
///
/// ```rust,ignore
/// # #[cfg(feature = "maud")]
/// # {
/// use autumn_web::seo::SeoMeta;
///
/// let meta = SeoMeta::new()
///     .title("My Post")
///     .description("A great post")
///     .canonical("https://example.com/posts/my-post")
///     .og_image("https://example.com/og.jpg")
///     .twitter_card("summary_large_image");
///
/// // Embed in a Maud template:
/// // html! { head { (meta.render()) } }
/// # }
/// ```
#[derive(Debug, Default, Clone)]
pub struct SeoMeta {
    title: Option<String>,
    description: Option<String>,
    canonical: Option<String>,
    og_title: Option<String>,
    og_description: Option<String>,
    og_image: Option<String>,
    og_type: Option<String>,
    og_url: Option<String>,
    twitter_card: Option<String>,
    twitter_title: Option<String>,
    twitter_description: Option<String>,
    twitter_image: Option<String>,
    robots_directive: Option<String>,
}

impl SeoMeta {
    /// Create a new, empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the page `<title>` (also used as the default OG/Twitter title).
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the `<meta name="description">` content.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the `<link rel="canonical">` URL.
    ///
    /// Also used as the `og:url` fallback.
    #[must_use]
    pub fn canonical(mut self, url: impl Into<String>) -> Self {
        self.canonical = Some(url.into());
        self
    }

    /// Set the `og:image` URL.
    #[must_use]
    pub fn og_image(mut self, url: impl Into<String>) -> Self {
        self.og_image = Some(url.into());
        self
    }

    /// Set the `og:type` value (default: omitted; common: `"website"`, `"article"`).
    #[must_use]
    pub fn og_type(mut self, og_type: impl Into<String>) -> Self {
        self.og_type = Some(og_type.into());
        self
    }

    /// Override the `og:title` (defaults to `title()`).
    #[must_use]
    pub fn og_title(mut self, title: impl Into<String>) -> Self {
        self.og_title = Some(title.into());
        self
    }

    /// Override the `og:description` (defaults to `description()`).
    #[must_use]
    pub fn og_description(mut self, desc: impl Into<String>) -> Self {
        self.og_description = Some(desc.into());
        self
    }

    /// Set the `og:url` (defaults to `canonical()` if not set).
    #[must_use]
    pub fn og_url(mut self, url: impl Into<String>) -> Self {
        self.og_url = Some(url.into());
        self
    }

    /// Set the `twitter:card` type (e.g. `"summary_large_image"`).
    ///
    /// When set, `twitter:title` and `twitter:description` are also emitted.
    #[must_use]
    pub fn twitter_card(mut self, card_type: impl Into<String>) -> Self {
        self.twitter_card = Some(card_type.into());
        self
    }

    /// Override the `twitter:title` (defaults to `title()`).
    #[must_use]
    pub fn twitter_title(mut self, title: impl Into<String>) -> Self {
        self.twitter_title = Some(title.into());
        self
    }

    /// Override the `twitter:description` (defaults to `description()`).
    #[must_use]
    pub fn twitter_description(mut self, desc: impl Into<String>) -> Self {
        self.twitter_description = Some(desc.into());
        self
    }

    /// Set the `twitter:image` URL.
    #[must_use]
    pub fn twitter_image(mut self, url: impl Into<String>) -> Self {
        self.twitter_image = Some(url.into());
        self
    }

    /// Set the `<meta name="robots">` directive (e.g. `"noindex"`, `"nofollow"`).
    #[must_use]
    pub fn robots(mut self, directive: impl Into<String>) -> Self {
        self.robots_directive = Some(directive.into());
        self
    }

    /// Render all configured meta tags as Maud [`Markup`].
    ///
    /// Emits only the tags that have been configured. Empty builders produce
    /// no output.
    #[cfg(feature = "maud")]
    #[must_use]
    pub fn render(&self) -> Markup {
        let og_title = self.og_title.as_ref().or(self.title.as_ref());
        let og_desc = self.og_description.as_ref().or(self.description.as_ref());
        let twitter_title = self.twitter_title.as_ref().or(self.title.as_ref());
        let twitter_desc = self
            .twitter_description
            .as_ref()
            .or(self.description.as_ref());
        let og_url = self.og_url.as_ref().or(self.canonical.as_ref());
        let has_twitter = self.twitter_card.is_some();

        html! {
            @if let Some(title) = &self.title {
                title { (title) }
            }
            @if let Some(desc) = &self.description {
                meta name="description" content=(desc);
            }
            @if let Some(dir) = &self.robots_directive {
                meta name="robots" content=(dir);
            }
            @if let Some(url) = &self.canonical {
                link rel="canonical" href=(url);
            }
            @if let Some(t) = og_title {
                meta property="og:title" content=(t);
            }
            @if let Some(d) = og_desc {
                meta property="og:description" content=(d);
            }
            @if let Some(img) = &self.og_image {
                meta property="og:image" content=(img);
            }
            @if let Some(ot) = &self.og_type {
                meta property="og:type" content=(ot);
            }
            @if let Some(url) = og_url {
                meta property="og:url" content=(url);
            }
            @if let Some(card) = &self.twitter_card {
                meta name="twitter:card" content=(card);
            }
            @if has_twitter {
                @if let Some(t) = twitter_title {
                    meta name="twitter:title" content=(t);
                }
                @if let Some(d) = twitter_desc {
                    meta name="twitter:description" content=(d);
                }
            }
            @if let Some(img) = &self.twitter_image {
                meta name="twitter:image" content=(img);
            }
        }
    }
}

// ── HTTP route builders (used by AppBuilder::run) ─────────────────────────────

/// Build an axum [`Router`] serving `/robots.txt` and `/sitemap.xml`.
///
/// The router is generic over the application state `S`, making it compatible
/// with both bare test routers and full `AppState`-powered production routers.
///
/// Used by [`AppBuilder::seo_source`](crate::app::AppBuilder::seo_source) when
/// assembling the server. The `entries` parameter provides the initial set of
/// URLs to include in the sitemap; dynamic sources registered via `seo_source()`
/// can supply additional entries at request time.
pub fn build_seo_router<S>(
    profile: &str,
    base_url: Option<&str>,
    additional_rules: &[String],
) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    build_seo_router_with_entries(profile, base_url, additional_rules, &[])
}

/// Build a SEO router with a pre-populated list of sitemap entries.
///
/// # Panics
///
/// This function will not panic in practice. The `Response::builder()` calls
/// inside the route handlers use hard-coded, well-formed `Content-Type` header
/// values that can never produce an error.
pub fn build_seo_router_with_entries<S>(
    profile: &str,
    base_url: Option<&str>,
    additional_rules: &[String],
    entries: &[SitemapEntry],
) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let base_url = base_url.map(|u| u.trim_end_matches('/'));
    let sitemap_url = base_url.map(|b| format!("{b}/sitemap.xml"));
    let robots_body = robots_txt(profile, sitemap_url.as_deref(), additional_rules);
    let sitemap_body = sitemap_xml(entries, base_url);
    build_seo_router_from_bodies(robots_body, sitemap_body)
}

/// Build a SEO router from pre-rendered `robots.txt` and `sitemap.xml` bodies.
///
/// Use this when you need full control over how the bodies are generated
/// (e.g. to honour `[seo.robots] sitemap_url` or `allow_all` overrides).
///
/// # Panics
///
/// In practice this function cannot panic. The hard-coded `Content-Type`
/// header values are always valid.
pub fn build_seo_router_from_bodies<S>(robots_body: String, sitemap_body: String) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::<S>::new()
        .route(
            "/robots.txt",
            get(move || {
                let body = robots_body.clone();
                async move {
                    Response::builder()
                        .header("Content-Type", "text/plain; charset=utf-8")
                        .body(Body::from(body))
                        .unwrap()
                }
            }),
        )
        .route(
            "/sitemap.xml",
            get(move || {
                let body = sitemap_body.clone();
                async move {
                    Response::builder()
                        .header("Content-Type", "application/xml; charset=utf-8")
                        .body(Body::from(body))
                        .unwrap()
                }
            }),
        )
}

// ── App-level SEO helpers (shared by run() and run_build_mode()) ──────────────

/// Return `true` when the `[seo]` config section contains any non-default value.
pub(crate) fn has_seo_config(seo_cfg: &crate::config::SeoConfig) -> bool {
    seo_cfg.base_url.is_some()
        || !seo_cfg.robots.additional_rules.is_empty()
        || seo_cfg.robots.allow_all.is_some()
        || seo_cfg.robots.sitemap_url.is_some()
}

/// Resolve the effective robots.txt profile from `raw_profile` and the
/// optional `allow_all` override in `[seo.robots]`.
pub(crate) fn effective_seo_profile<'a>(raw_profile: &'a str, allow_all: Option<bool>) -> &'a str {
    match allow_all {
        Some(true) => "prod",
        Some(false) => "dev",
        None => raw_profile,
    }
}

/// Collect sitemap entries from dynamic sources and static path hints, then
/// build the `robots.txt` and `sitemap.xml` bodies.
///
/// Called by both `AppBuilder::run` (server mode) and
/// `AppBuilder::run_build_mode` (static build mode).
pub(crate) async fn assemble_seo_bodies(
    profile: &str,
    base_url: Option<&str>,
    sitemap_url_override: Option<&str>,
    additional_rules: &[String],
    sources: &[Arc<dyn SitemapSource>],
    static_paths: &[&str],
) -> (String, String) {
    let base_url = base_url.map(|u| u.trim_end_matches('/'));

    let mut sitemap_entries = Vec::new();
    for source in sources {
        let mut entries = source.entries().await;
        sitemap_entries.append(&mut entries);
    }

    if let Some(bu) = base_url {
        for path in static_paths {
            if !path.contains('{') {
                sitemap_entries.push(SitemapEntry::new(format!("{bu}{path}")));
            }
        }
    }

    let derived_sitemap_url = base_url.map(|b| format!("{b}/sitemap.xml"));
    let sitemap_url = sitemap_url_override.or(derived_sitemap_url.as_deref());
    let robots_body = robots_txt(profile, sitemap_url, additional_rules);
    let sitemap_body = sitemap_xml(&sitemap_entries, base_url);
    (robots_body, sitemap_body)
}

// ── Static build helpers ──────────────────────────────────────────────────────

/// Write `robots.txt` and `sitemap.xml` to `dist_dir` as part of `autumn build`.
///
/// Called by `AppBuilder::run_build_mode` after static routes are rendered.
///
/// # Arguments
///
/// * `dist_dir` — The output directory (e.g. `dist/`).
/// * `profile` — The active profile.
/// * `base_url` — The site base URL (auto-injects the `Sitemap:` directive).
/// * `additional_rules` — Extra robots.txt rules.
/// * `entries` — Sitemap entries to include (from registered sources + static metas).
///
/// # Errors
///
/// Returns `std::io::Error` if writing fails.
pub async fn write_seo_files(
    dist_dir: &Path,
    profile: &str,
    base_url: Option<&str>,
    sitemap_url_override: Option<&str>,
    additional_rules: &[String],
    entries: &[SitemapEntry],
) -> Result<(), std::io::Error> {
    let base_url = base_url.map(|u| u.trim_end_matches('/'));
    let derived_sitemap_url = base_url.map(|b| format!("{b}/sitemap.xml"));
    let sitemap_url = sitemap_url_override.or(derived_sitemap_url.as_deref());
    let robots = robots_txt(profile, sitemap_url, additional_rules);
    let sitemap = sitemap_xml(entries, base_url);

    tokio::fs::write(dist_dir.join("robots.txt"), robots).await?;
    tokio::fs::write(dist_dir.join("sitemap.xml"), sitemap).await?;

    Ok(())
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sitemap_entry_builder() {
        let e = SitemapEntry::new("https://example.com/")
            .lastmod("2026-01-01")
            .changefreq(SitemapChangefreq::Weekly)
            .priority(0.9);
        assert_eq!(e.loc, "https://example.com/");
        assert_eq!(e.lastmod.as_deref(), Some("2026-01-01"));
        assert_eq!(e.changefreq, Some(SitemapChangefreq::Weekly));
        assert!((e.priority.unwrap() - 0.9).abs() < 0.001);
    }

    #[test]
    fn sitemap_entry_priority_clamped() {
        let hi = SitemapEntry::new("https://example.com/").priority(1.5);
        let lo = SitemapEntry::new("https://example.com/").priority(-0.5);
        assert!((hi.priority.unwrap() - 1.0).abs() < 0.001);
        assert!((lo.priority.unwrap() - 0.0).abs() < 0.001);
    }

    #[test]
    fn xml_escape_replaces_special_chars() {
        assert_eq!(
            xml_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
    }

    #[test]
    fn robots_txt_staging_profile_disallows() {
        let txt = robots_txt("staging", None, &[]);
        assert!(txt.contains("Disallow: /"));
        assert!(!txt.contains("Allow: /"));
    }

    #[test]
    fn has_seo_config_false_when_empty() {
        let cfg = crate::config::SeoConfig::default();
        assert!(!has_seo_config(&cfg));
    }

    #[test]
    fn has_seo_config_true_when_base_url_set() {
        let mut cfg = crate::config::SeoConfig::default();
        cfg.base_url = Some("https://example.com".to_string());
        assert!(has_seo_config(&cfg));
    }

    #[test]
    fn has_seo_config_true_when_allow_all_set() {
        let mut cfg = crate::config::SeoConfig::default();
        cfg.robots.allow_all = Some(true);
        assert!(has_seo_config(&cfg));
    }

    #[test]
    fn has_seo_config_true_when_sitemap_url_set() {
        let mut cfg = crate::config::SeoConfig::default();
        cfg.robots.sitemap_url = Some("https://example.com/sitemap.xml".to_string());
        assert!(has_seo_config(&cfg));
    }

    #[test]
    fn has_seo_config_true_when_additional_rules_set() {
        let mut cfg = crate::config::SeoConfig::default();
        cfg.robots.additional_rules = vec!["Disallow: /admin".to_string()];
        assert!(has_seo_config(&cfg));
    }

    #[test]
    fn effective_seo_profile_respects_allow_all_true() {
        assert_eq!(effective_seo_profile("dev", Some(true)), "prod");
    }

    #[test]
    fn effective_seo_profile_respects_allow_all_false() {
        assert_eq!(effective_seo_profile("prod", Some(false)), "dev");
    }

    #[test]
    fn effective_seo_profile_falls_back_to_raw_when_none() {
        assert_eq!(effective_seo_profile("staging", None), "staging");
    }

    struct SimpleSitemapSource {
        entries: Vec<SitemapEntry>,
    }

    impl SitemapSource for SimpleSitemapSource {
        fn entries(
            &self,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Vec<SitemapEntry>> + Send + '_>,
        > {
            let entries = self.entries.clone();
            Box::pin(async move { entries })
        }
    }

    #[tokio::test]
    async fn assemble_seo_bodies_empty() {
        let (robots, sitemap) =
            assemble_seo_bodies("prod", None, None, &[], &[], &[]).await;
        assert!(robots.contains("Allow: /"));
        assert!(sitemap.contains("<urlset"));
    }

    #[tokio::test]
    async fn assemble_seo_bodies_collects_source_entries() {
        let source = Arc::new(SimpleSitemapSource {
            entries: vec![SitemapEntry::new("https://example.com/post/1")],
        }) as Arc<dyn SitemapSource>;
        let (_, sitemap) = assemble_seo_bodies(
            "prod",
            Some("https://example.com"),
            None,
            &[],
            &[source],
            &[],
        )
        .await;
        assert!(
            sitemap.contains("https://example.com/post/1"),
            "should include source entry; got:\n{sitemap}"
        );
    }

    #[tokio::test]
    async fn assemble_seo_bodies_includes_static_paths() {
        let (_, sitemap) = assemble_seo_bodies(
            "prod",
            Some("https://example.com"),
            None,
            &[],
            &[],
            &["/about", "/contact"],
        )
        .await;
        assert!(sitemap.contains("https://example.com/about"));
        assert!(sitemap.contains("https://example.com/contact"));
    }

    #[tokio::test]
    async fn assemble_seo_bodies_skips_dynamic_paths() {
        let (_, sitemap) = assemble_seo_bodies(
            "prod",
            Some("https://example.com"),
            None,
            &[],
            &[],
            &["/posts/{slug}"],
        )
        .await;
        assert!(
            !sitemap.contains("/posts/"),
            "should skip paths with params; got:\n{sitemap}"
        );
    }

    #[tokio::test]
    async fn assemble_seo_bodies_uses_sitemap_url_override() {
        let (robots, _) = assemble_seo_bodies(
            "prod",
            Some("https://example.com"),
            Some("https://cdn.example.com/sitemap.xml"),
            &[],
            &[],
            &[],
        )
        .await;
        assert!(
            robots.contains("Sitemap: https://cdn.example.com/sitemap.xml"),
            "should use override url; got:\n{robots}"
        );
    }

    #[tokio::test]
    async fn assemble_seo_bodies_trims_trailing_slash() {
        let (_, sitemap) = assemble_seo_bodies(
            "prod",
            Some("https://example.com/"),
            None,
            &[],
            &[],
            &["/about"],
        )
        .await;
        assert!(
            sitemap.contains("https://example.com/about"),
            "base_url trailing slash should be trimmed; got:\n{sitemap}"
        );
    }
}
