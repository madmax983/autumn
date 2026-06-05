//! First-class system tests with a headless Chromium browser.
//!
//! This module provides [`SystemTest`] — a builder that boots your Autumn
//! application on an ephemeral TCP port, launches a managed headless Chromium
//! instance, and gives you a [`Page`] with htmx-aware auto-waiting assertions.
//!
//! # Feature flag
//!
//! Gated behind `autumn-web = { features = ["system-tests"] }`.  Add it as a
//! **dev-dependency** only:
//!
//! ```toml
//! [dev-dependencies]
//! autumn-web = { version = "0.4", features = ["system-tests"] }
//! ```
//!
//! # Quick start
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//! use autumn_web::system_test::SystemTest;
//!
//! #[get("/")]
//! async fn index() -> &'static str {
//!     "<html><body><h1>Hello</h1></body></html>"
//! }
//!
//! #[tokio::test]
//! #[ignore = "requires Chromium"]
//! async fn hello_renders() {
//!     let mut runner = SystemTest::new()
//!         .routes(routes![index])
//!         .build()
//!         .await
//!         .expect("start runner");
//!
//!     let page = runner.page().await.expect("open page");
//!     page.visit("/").await.expect("visit");
//!     page.expect_text("Hello").await.expect("text");
//! }
//! ```
//!
//! # Browser resolution order
//!
//! 1. `AUTUMN_CHROMIUM` environment variable (full binary path)
//! 2. `PLAYWRIGHT_BROWSERS_PATH` — scans `<path>/chromium-*/chrome-linux/chrome`
//! 3. Common system paths: `/usr/bin/chromium-browser`, `/usr/bin/chromium`,
//!    `/usr/bin/google-chrome`, `/usr/bin/google-chrome-stable`,
//!    `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`,
//!    `/Applications/Chromium.app/Contents/MacOS/Chromium`
//!
//! When the browser cannot be found the error message prints all searched
//! paths and the `apt-get install chromium-browser` remediation hint.
//!
//! # Failure artifacts
//!
//! On any assertion failure, autumn writes a `.png` screenshot and `.html`
//! dump to `target/system-tests/<test-name>/` so you can post-mortem the
//! failure without rerunning the test.
//!
//! # htmx settle detection
//!
//! All page-mutating helpers (`click`, form submits) auto-wait for htmx to
//! finish its settle phase before returning.  This is implemented by polling
//! `document.querySelectorAll('.htmx-request').length === 0` with a
//! configurable timeout (default 2 s).  Use [`Page::expect_hx_settle`] when
//! you need an explicit fence.

#![cfg(feature = "system-tests")]

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chromiumoxide::{Browser, BrowserConfig};
use futures::StreamExt as _;

use crate::config::AutumnConfig;
use crate::route::Route;

// ── Constants ──────────────────────────────────────────────────────────────

/// Default timeout while waiting for the browser binary to launch and connect.
const DEFAULT_BROWSER_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for htmx settle auto-wait after every mutating action.
const DEFAULT_HX_SETTLE_TIMEOUT: Duration = Duration::from_millis(2000);

/// Default assertion polling interval.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

// ── BrowserCheck ───────────────────────────────────────────────────────────

/// Result of probing the host for a usable Chromium binary.
///
/// Returned by [`BrowserCheck::run`] and shown by `autumn doctor`.
#[derive(Debug, Clone)]
pub enum BrowserCheck {
    /// A usable binary was found at `path` with the reported `version` string.
    Found {
        /// Absolute path to the Chromium binary.
        path: PathBuf,
        /// Version string reported by `--version` (e.g. `"Chromium 122.0.6261.111"`).
        version: String,
    },
    /// No usable binary could be found; `searched_paths` lists every path that
    /// was probed so the user knows what to add.
    NotFound {
        /// Every path that was checked and did not yield a working binary.
        searched_paths: Vec<PathBuf>,
    },
}

impl BrowserCheck {
    /// Probe the host for a Chromium binary using the documented resolution
    /// order and return the result.
    #[must_use]
    pub fn run() -> Self {
        let candidates = browser_candidates();
        let mut searched = Vec::new();
        for path in &candidates {
            if path.is_file() {
                if let Some(version) = probe_version(path) {
                    return BrowserCheck::Found {
                        path: path.clone(),
                        version,
                    };
                }
            }
            searched.push(path.clone());
        }
        BrowserCheck::NotFound {
            searched_paths: searched,
        }
    }

    /// `true` when a browser was found.
    #[must_use]
    pub fn is_found(&self) -> bool {
        matches!(self, BrowserCheck::Found { .. })
    }
}

impl fmt::Display for BrowserCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BrowserCheck::Found { path, version } => {
                write!(f, "Chromium found: {} ({})", path.display(), version)
            }
            BrowserCheck::NotFound { searched_paths } => {
                write!(
                    f,
                    "Chromium not found. Searched:\n{}",
                    searched_paths
                        .iter()
                        .map(|p| format!("  {}", p.display()))
                        .collect::<Vec<_>>()
                        .join("\n")
                )?;
                write!(
                    f,
                    "\n\nTo install on Ubuntu/Debian: apt-get install chromium-browser\n\
                     Or set the AUTUMN_CHROMIUM environment variable to the full binary path."
                )
            }
        }
    }
}

// ── SystemTestError ────────────────────────────────────────────────────────

/// Errors produced by the system-test harness.
#[derive(Debug, thiserror::Error)]
pub enum SystemTestError {
    /// Chromium binary could not be located on this host.
    #[error(
        "Chromium browser not found. Searched:\n{}\n\n\
         To install: apt-get install chromium-browser\n\
         Or set AUTUMN_CHROMIUM=/path/to/chrome",
        searched.iter().map(|p| format!("  {}", p.display())).collect::<Vec<_>>().join("\n")
    )]
    BrowserNotFound {
        /// Paths that were checked.
        searched: Vec<PathBuf>,
    },

    /// An assertion on the page content or URL failed.
    #[error("{message}")]
    AssertionFailed {
        /// Human-readable description of what was expected vs. found.
        message: String,
        /// Path prefix for the `.png` and `.html` artifacts (if they were
        /// written successfully).
        artifact_path: Option<String>,
    },

    /// The assertion did not resolve within the allowed time.
    #[error("assertion timed out after {timeout:?}: {message}")]
    Timeout {
        /// Human-readable description of the assertion.
        message: String,
        /// How long we waited.
        timeout: Duration,
    },

    /// An I/O error while writing failure artifacts.
    #[error("artifact write error: {0}")]
    ArtifactIo(#[from] std::io::Error),

    /// An error from the underlying chromiumoxide browser library.
    #[error("browser error: {0}")]
    Browser(#[from] chromiumoxide::error::CdpError),
}

// ── Artifact directory ─────────────────────────────────────────────────────

/// Return the canonical artifact directory for a given test name.
///
/// Output: `target/system-tests/<test_name>/`
#[must_use]
pub fn artifact_dir(test_name: &str) -> PathBuf {
    // Walk up from the crate root (or use CARGO_TARGET_DIR if set).
    let base = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target"));
    base.join("system-tests").join(test_name)
}

// ── SystemTest builder ─────────────────────────────────────────────────────

/// Builder for a system test run.
///
/// Boots an Autumn application on an ephemeral port, launches a managed
/// headless Chromium, and returns a [`SystemTestRunner`].
///
/// # Example
///
/// ```rust,no_run
/// # use autumn_web::system_test::SystemTest;
/// # use autumn_web::prelude::*;
/// # #[get("/")] async fn index() -> &'static str { "" }
/// # async fn example() {
/// let runner = SystemTest::new()
///     .routes(routes![index])
///     .build()
///     .await
///     .expect("start runner");
/// # }
/// ```
#[must_use]
pub struct SystemTest {
    routes: Vec<Route>,
    #[allow(dead_code)]
    config: AutumnConfig,
    artifact_dir_override: Option<PathBuf>,
    browser_timeout: Duration,
    hx_settle_timeout: Duration,
}

impl Default for SystemTest {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemTest {
    /// Create a new builder with default configuration.
    pub fn new() -> Self {
        let mut config = AutumnConfig::default();
        config.profile = Some("test".into());
        config.security.csrf.enabled = false;

        Self {
            routes: Vec::new(),
            config,
            artifact_dir_override: None,
            browser_timeout: DEFAULT_BROWSER_TIMEOUT,
            hx_settle_timeout: DEFAULT_HX_SETTLE_TIMEOUT,
        }
    }

    /// Register routes to serve.
    pub fn routes(mut self, routes: impl Into<Vec<Route>>) -> Self {
        self.routes = routes.into();
        self
    }

    /// Override the directory where failure artifacts are written.
    ///
    /// Defaults to `target/system-tests/<test_name>/`.
    pub fn artifact_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.artifact_dir_override = Some(dir.into());
        self
    }

    /// Override how long to wait for the browser to launch.
    pub fn browser_timeout(mut self, t: Duration) -> Self {
        self.browser_timeout = t;
        self
    }

    /// Override how long to wait for htmx to finish settling after each action.
    pub fn hx_settle_timeout(mut self, t: Duration) -> Self {
        self.hx_settle_timeout = t;
        self
    }

    /// Boot the server and launch the browser, returning a [`SystemTestRunner`].
    ///
    /// # Errors
    /// - [`SystemTestError::BrowserNotFound`] when no Chromium binary is available.
    /// - [`SystemTestError::Browser`] for CDP launch/connect errors.
    pub async fn build(self) -> Result<SystemTestRunner, SystemTestError> {
        // 1. Locate browser binary.
        let browser_path = find_chromium().ok_or_else(|| {
            let searched = browser_candidates();
            SystemTestError::BrowserNotFound { searched }
        })?;

        // 2. Bind the app to an ephemeral port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| SystemTestError::ArtifactIo(e))?;
        let addr = listener.local_addr().map_err(SystemTestError::ArtifactIo)?;
        let base_url = format!("http://127.0.0.1:{}", addr.port());

        // 3. Build the axum router from the registered routes.
        let router = build_router_for_system_test(self.routes);

        // 4. Spawn the server in a background task.
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server_handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        // 5. Launch Chromium.
        let config = BrowserConfig::builder()
            .chrome_executable(browser_path)
            .arg("--no-sandbox")
            .arg("--disable-dev-shm-usage")
            .arg("--disable-gpu")
            .arg("--headless")
            .build()
            .map_err(|msg| SystemTestError::Browser(chromiumoxide::error::CdpError::msg(msg)))?;

        let (browser, handler) =
            tokio::time::timeout(self.browser_timeout, Browser::launch(config))
                .await
                .map_err(|_| SystemTestError::Timeout {
                    message: "timed out waiting for Chromium to launch".into(),
                    timeout: self.browser_timeout,
                })??;

        // Drive the CDP event loop in a background task.
        tokio::spawn(async move {
            handler.for_each(|_| async {}).await;
        });

        let artifact_dir = self
            .artifact_dir_override
            .unwrap_or_else(|| artifact_dir("system_test"));

        Ok(SystemTestRunner {
            base_url,
            browser,
            artifact_dir,
            hx_settle_timeout: self.hx_settle_timeout,
            _shutdown: shutdown_tx,
            _server_handle: server_handle,
        })
    }
}

// ── SystemTestRunner ───────────────────────────────────────────────────────

/// A running system-test session.
///
/// Returned by [`SystemTest::build`].  Shuts down the embedded HTTP server
/// when dropped.
pub struct SystemTestRunner {
    base_url: String,
    browser: Browser,
    artifact_dir: PathBuf,
    hx_settle_timeout: Duration,
    _shutdown: tokio::sync::oneshot::Sender<()>,
    _server_handle: tokio::task::JoinHandle<()>,
}

impl SystemTestRunner {
    /// Open a new browser page connected to the running application.
    ///
    /// # Errors
    /// Propagates CDP errors from `chromiumoxide`.
    pub async fn page(&mut self) -> Result<Page, SystemTestError> {
        let cdp_page = self.browser.new_page("about:blank").await?;
        Ok(Page {
            inner: cdp_page,
            base_url: self.base_url.clone(),
            artifact_dir: self.artifact_dir.clone(),
            hx_settle_timeout: self.hx_settle_timeout,
        })
    }

    /// The base URL of the embedded application, e.g. `http://127.0.0.1:49832`.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

// ── Page ───────────────────────────────────────────────────────────────────

/// Browser page with htmx-aware assertions.
///
/// Obtained from [`SystemTestRunner::page`].
pub struct Page {
    inner: chromiumoxide::page::Page,
    base_url: String,
    artifact_dir: PathBuf,
    hx_settle_timeout: Duration,
}

impl Page {
    // ── Navigation ────────────────────────────────────────────────────────

    /// Navigate to `path` (relative to the app root, e.g. `"/todos"`).
    ///
    /// # Errors
    /// CDP navigation errors.
    pub async fn visit(&self, path: &str) -> Result<&Self, SystemTestError> {
        let url = format!("{}{path}", self.base_url);
        self.inner.goto(url).await?;
        Ok(self)
    }

    // ── Interaction ───────────────────────────────────────────────────────

    /// Fill a form field identified by `selector` with `value`.
    ///
    /// Clears any existing content then types the new value character by
    /// character.  Auto-waits for htmx settle after the field change.
    ///
    /// # Errors
    /// CDP or assertion errors.
    pub async fn fill(&self, selector: &str, value: &str) -> Result<&Self, SystemTestError> {
        let element = self.inner.find_element(selector).await?;
        element.click().await?;
        // Clear via JS and dispatch events so htmx/frameworks detect the change.
        self.inner
            .evaluate(format!(
                "(function() {{ var el = document.querySelector({}); \
                 if (el) {{ el.value = ''; \
                 el.dispatchEvent(new Event('input', {{ bubbles: true }})); \
                 el.dispatchEvent(new Event('change', {{ bubbles: true }})); }} }})()",
                js_string_literal(selector)
            ))
            .await?;
        element.type_str(value).await?;
        // Dispatch a final change event so `hx-trigger="change"` and validation
        // listeners see the fully typed value.
        self.inner
            .evaluate(format!(
                "(function() {{ var el = document.querySelector({}); \
                 if (el) {{ el.dispatchEvent(new Event('change', {{ bubbles: true }})); }} }})()",
                js_string_literal(selector)
            ))
            .await?;
        self.wait_for_hx_settle().await?;
        Ok(self)
    }

    /// Click the element identified by `selector_or_label`.
    ///
    /// Supports CSS selectors (e.g. `"button[type=submit]"`) or accessible
    /// text labels (e.g. `"Save"` matches `<button>Save</button>`).
    /// After clicking, auto-waits for htmx to settle.
    ///
    /// # Errors
    /// CDP or assertion errors.
    pub async fn click(&self, selector_or_label: &str) -> Result<&Self, SystemTestError> {
        // Try CSS selector first; fall back to XPath text match via JS.
        if let Ok(element) = self.inner.find_element(selector_or_label).await {
            element.click().await?;
        } else {
            let js = format!(
                "(function() {{ \
                 var label = {}; \
                 var q = label.indexOf(\"'\") >= 0 ? '\"' : \"'\"; \
                 var xpath = \"//*[normalize-space(text())=\" + q + label + q + \"]\"; \
                 var result = document.evaluate(xpath, document, null, \
                   XPathResult.FIRST_ORDERED_NODE_TYPE, null); \
                 var el = result.singleNodeValue; \
                 if (el) {{ el.click(); return true; }} \
                 return false; \
                 }})()",
                js_string_literal(selector_or_label)
            );
            let clicked: bool = self.inner.evaluate(js).await?.into_value().unwrap_or(false);
            if !clicked {
                return Err(SystemTestError::AssertionFailed {
                    message: format!("element not found by selector or text: {selector_or_label}"),
                    artifact_path: None,
                });
            }
        }
        self.wait_for_hx_settle().await?;
        Ok(self)
    }

    // ── Assertions ────────────────────────────────────────────────────────

    /// Assert that `text` appears somewhere in the visible page content.
    ///
    /// Polls until the text appears or the default assertion timeout (5 s)
    /// elapses.  On failure writes a screenshot + HTML artifact.
    ///
    /// # Errors
    /// [`SystemTestError::Timeout`] or [`SystemTestError::AssertionFailed`].
    pub async fn expect_text(&self, text: &str) -> Result<&Self, SystemTestError> {
        let timeout = Duration::from_secs(5);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let result = self
                .inner
                .evaluate(format!(
                    "document.body && document.body.innerText.includes({})",
                    js_string_literal(text)
                ))
                .await?;

            let found: bool = result.into_value().unwrap_or(false);
            if found {
                return Ok(self);
            }

            if tokio::time::Instant::now() >= deadline {
                let artifact = self.write_failure_artifacts("expect_text").await.ok();
                return Err(SystemTestError::AssertionFailed {
                    message: format!("expected text {text:?} in page body"),
                    artifact_path: artifact,
                });
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Assert that the current page URL ends with or contains `pattern`.
    ///
    /// # Errors
    /// [`SystemTestError::Timeout`] or [`SystemTestError::AssertionFailed`].
    pub async fn expect_url(&self, pattern: &str) -> Result<&Self, SystemTestError> {
        let timeout = Duration::from_secs(5);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let result = self
                .inner
                .evaluate(format!(
                    "window.location.href.includes({})",
                    js_string_literal(pattern)
                ))
                .await?;

            let found: bool = result.into_value().unwrap_or(false);
            if found {
                return Ok(self);
            }

            if tokio::time::Instant::now() >= deadline {
                let current_url: String = self
                    .inner
                    .evaluate("window.location.href")
                    .await
                    .ok()
                    .and_then(|v| v.into_value::<String>().ok())
                    .unwrap_or_else(|| "<unknown>".into());
                let artifact = self.write_failure_artifacts("expect_url").await.ok();
                return Err(SystemTestError::AssertionFailed {
                    message: format!("expected URL to contain {pattern:?}, got {current_url:?}"),
                    artifact_path: artifact,
                });
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Assert that an element matching `selector` has attribute `attr` equal
    /// to `value`.
    ///
    /// # Errors
    /// [`SystemTestError::AssertionFailed`] on mismatch.
    pub async fn expect_attribute(
        &self,
        selector: &str,
        attr: &str,
        value: &str,
    ) -> Result<&Self, SystemTestError> {
        let timeout = Duration::from_secs(5);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let js = format!(
                "(function() {{ \
                   var el = document.querySelector({sel}); \
                   return el && el.getAttribute({attr}) === {val}; \
                 }})()",
                sel = js_string_literal(selector),
                attr = js_string_literal(attr),
                val = js_string_literal(value),
            );
            let result = self.inner.evaluate(js).await?;
            let found: bool = result.into_value().unwrap_or(false);
            if found {
                return Ok(self);
            }

            if tokio::time::Instant::now() >= deadline {
                let artifact = self.write_failure_artifacts("expect_attribute").await.ok();
                return Err(SystemTestError::AssertionFailed {
                    message: format!("expected [{attr}={value:?}] on {selector:?}"),
                    artifact_path: artifact,
                });
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    // ── htmx helpers ──────────────────────────────────────────────────────

    /// Explicitly wait for htmx to finish all in-flight requests and settle.
    ///
    /// Polls `document.querySelectorAll('.htmx-request').length === 0` until
    /// the page is idle.  Use this as an explicit fence; `click()` already
    /// calls it implicitly.
    ///
    /// # Errors
    /// [`SystemTestError::Timeout`] if htmx does not settle within the
    /// configured [`SystemTest::hx_settle_timeout`].
    pub async fn expect_hx_settle(&self) -> Result<&Self, SystemTestError> {
        self.wait_for_hx_settle().await?;
        Ok(self)
    }

    // ── SSE helper ────────────────────────────────────────────────────────

    /// Wait until an element addressed by `stream_id` contains content
    /// satisfying `predicate`, indicating that an SSE stream has rendered new
    /// content into the DOM.
    ///
    /// `stream_id` may be a CSS `id` attribute or selector.
    ///
    /// # Errors
    /// [`SystemTestError::Timeout`] or [`SystemTestError::AssertionFailed`].
    pub async fn expect_sse_event(
        &self,
        stream_id: &str,
        predicate: impl Fn(&str) -> bool,
    ) -> Result<&Self, SystemTestError> {
        let timeout = Duration::from_secs(10);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            // Derive a CSS selector: treat bare word as an id, otherwise use
            // as-is.
            let selector = if stream_id.starts_with('#')
                || stream_id.starts_with('.')
                || stream_id.contains('[')
            {
                stream_id.to_owned()
            } else {
                format!("#{stream_id}")
            };

            let result = self
                .inner
                .evaluate(format!(
                    "(function() {{ \
                       var el = document.querySelector({sel}); \
                       return el ? el.innerText : null; \
                     }})()",
                    sel = js_string_literal(&selector)
                ))
                .await?;

            let text: Option<String> = result.into_value().ok();
            if let Some(ref t) = text {
                if predicate(t) {
                    return Ok(self);
                }
            }

            if tokio::time::Instant::now() >= deadline {
                let artifact = self.write_failure_artifacts("expect_sse_event").await.ok();
                return Err(SystemTestError::AssertionFailed {
                    message: format!(
                        "SSE event: element {stream_id:?} content {:?} did not satisfy predicate",
                        text.as_deref().unwrap_or("<not found>")
                    ),
                    artifact_path: artifact,
                });
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    // ── Screenshot / snapshot ─────────────────────────────────────────────

    /// Take a screenshot of the current page and save it to the artifact
    /// directory.
    ///
    /// Returns the path to the saved `.png` file.
    ///
    /// # Errors
    /// CDP or I/O errors.
    pub async fn snapshot(&self) -> Result<PathBuf, SystemTestError> {
        self.write_screenshot("snapshot").await
    }

    /// Evaluate a JavaScript expression on the page and return the result.
    ///
    /// # Errors
    /// Propagates CDP errors.
    pub async fn evaluate(
        &self,
        js: impl Into<String>,
    ) -> Result<chromiumoxide::js::EvaluationResult, SystemTestError> {
        let js_str: String = js.into();
        let res = self.inner.evaluate(js_str).await?;
        Ok(res)
    }

    // ── Internal helpers ──────────────────────────────────────────────────

    async fn wait_for_hx_settle(&self) -> Result<(), SystemTestError> {
        let deadline = tokio::time::Instant::now() + self.hx_settle_timeout;
        loop {
            let result = self
                .inner
                .evaluate("document.querySelectorAll('.htmx-request,.htmx-settling').length === 0")
                .await?;
            let settled: bool = result.into_value().unwrap_or(true);
            if settled {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(SystemTestError::Timeout {
                    message: "htmx did not settle".into(),
                    timeout: self.hx_settle_timeout,
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Write screenshot + HTML artifacts and return the base path (without
    /// extension) on success.
    async fn write_failure_artifacts(&self, label: &str) -> Result<String, SystemTestError> {
        let dir = &self.artifact_dir;
        tokio::fs::create_dir_all(dir).await?;

        let base = dir.join(label);
        let base_str = base.to_string_lossy().into_owned();

        // Screenshot
        let png_path = base.with_extension("png");
        if let Ok(bytes) = self
            .inner
            .screenshot(chromiumoxide::page::ScreenshotParams::builder().build())
            .await
        {
            let _ = tokio::fs::write(&png_path, bytes).await;
        }

        // HTML dump
        let html_path = base.with_extension("html");
        if let Ok(html) = self.inner.content().await {
            let _ = tokio::fs::write(&html_path, html).await;
        }

        Ok(base_str)
    }

    async fn write_screenshot(&self, label: &str) -> Result<PathBuf, SystemTestError> {
        let dir = &self.artifact_dir;
        tokio::fs::create_dir_all(dir).await?;
        let png_path = dir.join(label).with_extension("png");
        let bytes = self
            .inner
            .screenshot(chromiumoxide::page::ScreenshotParams::builder().build())
            .await?;
        tokio::fs::write(&png_path, bytes).await?;
        Ok(png_path)
    }
}

// ── system_test! macro ─────────────────────────────────────────────────────

/// Convenience macro that wraps a [`SystemTest`] builder call and ensures
/// the runner is dropped (shutting down the embedded server) at test end.
///
/// ```rust,ignore
/// system_test!(SystemTest::new().routes(routes![index]))
///     .page().await.expect("page")
/// ```
#[macro_export]
macro_rules! system_test {
    ($builder:expr) => {{
        $builder
            .build()
            .await
            .expect("system_test! failed to start runner")
    }};
}

// ── Internal utilities ─────────────────────────────────────────────────────

/// All paths to probe for a Chromium binary, in resolution order.
fn browser_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1. Explicit override.
    if let Ok(p) = std::env::var("AUTUMN_CHROMIUM") {
        candidates.push(PathBuf::from(p));
    }

    // 2. Playwright browsers directory.
    if let Ok(base) = std::env::var("PLAYWRIGHT_BROWSERS_PATH") {
        let base = PathBuf::from(base);
        if let Ok(entries) = std::fs::read_dir(&base) {
            let mut pw_paths: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("chromium-"))
                .map(|e| {
                    if cfg!(target_os = "macos") {
                        e.path()
                            .join("chrome-mac")
                            .join("Chromium.app")
                            .join("Contents")
                            .join("MacOS")
                            .join("Chromium")
                    } else if cfg!(target_os = "windows") {
                        e.path().join("chrome-win").join("chrome.exe")
                    } else {
                        e.path().join("chrome-linux").join("chrome")
                    }
                })
                .collect();
            pw_paths.sort();
            pw_paths.reverse(); // highest revision first
            candidates.extend(pw_paths);
        }
    }

    // 3. Well-known system paths.
    candidates.extend(
        [
            "/usr/bin/chromium-browser",
            "/usr/bin/chromium",
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/snap/bin/chromium",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
        .map(PathBuf::from),
    );

    candidates
}

/// Return the first candidate that exists and reports a version.
fn find_chromium() -> Option<PathBuf> {
    for path in browser_candidates() {
        if path.is_file() && probe_version(&path).is_some() {
            return Some(path);
        }
    }
    None
}

/// Run `<path> --version` and return the output if successful.
fn probe_version(path: &Path) -> Option<String> {
    std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// Build a minimal axum `Router` from a list of registered `Route`s.
fn build_router_for_system_test(routes: Vec<Route>) -> axum::Router {
    let mut config = AutumnConfig::default();
    config.profile = Some("test".into());
    config.security.csrf.enabled = false;
    let state = crate::state::AppState::for_test();
    crate::router::build_router(routes, &config, state)
}

/// Escape a string as a JSON-safe JavaScript string literal.
fn js_string_literal(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

// ── Tests (non-browser, always run) ───────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_string_literal_escapes_quotes() {
        assert_eq!(js_string_literal(r#"say "hi""#), r#""say \"hi\"""#);
    }

    #[test]
    fn js_string_literal_escapes_backslashes() {
        assert_eq!(js_string_literal(r"a\b"), r#""a\\b""#);
    }

    #[test]
    fn artifact_dir_contains_test_name() {
        let d = artifact_dir("my_test_name");
        assert!(d.to_string_lossy().contains("my_test_name"));
        assert!(d.to_string_lossy().contains("system-tests"));
    }

    #[test]
    fn browser_check_not_found_message_has_hints() {
        let check = BrowserCheck::NotFound {
            searched_paths: vec![PathBuf::from("/no/such/path")],
        };
        let msg = check.to_string();
        assert!(msg.contains("apt-get") || msg.contains("AUTUMN_CHROMIUM"));
    }

    #[test]
    fn browser_candidates_includes_common_paths() {
        let candidates = browser_candidates();
        let as_strings: Vec<_> = candidates
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert!(
            as_strings
                .iter()
                .any(|s| s.contains("chromium") || s.contains("chrome")),
            "should have at least one chrome path; got {as_strings:?}"
        );
    }
}
