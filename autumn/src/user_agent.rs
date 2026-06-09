//! Lightweight `User-Agent` parsing for login-session device attribution.
//!
//! Powers the device list emitted by `autumn generate auth` (issue #819):
//! each login session stores the raw `User-Agent` plus the parsed browser
//! family, operating system, and device class so a "where am I signed in?"
//! page can render a human-readable device line without re-parsing on read.
//!
//! This is a deliberately small heuristic parser — no regex tables, no
//! external database — tuned for the major browser/OS families. Apps that
//! need exhaustive coverage (exotic devices, bot taxonomies) can swap in a
//! dedicated crate: the generated auth starter funnels every parse through
//! one call site, so replacing the parser is a one-line change. See the
//! generated `docs/guide/session-management.md` for the recipe.
//!
//! ## Example
//!
//! ```rust
//! use autumn_web::user_agent::{parse_user_agent, DeviceClass};
//!
//! let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
//!           (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
//! let parsed = parse_user_agent(ua);
//! assert_eq!(parsed.family, "Chrome");
//! assert_eq!(parsed.os, "macOS");
//! assert_eq!(parsed.device_class, DeviceClass::Desktop);
//! ```

use serde::{Deserialize, Serialize};

/// Coarse device classification derived from a `User-Agent` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeviceClass {
    /// Desktop or laptop browser.
    Desktop,
    /// Phone-class device.
    Mobile,
    /// Tablet-class device.
    Tablet,
    /// Crawler, monitoring agent, or scripted client.
    Bot,
    /// Could not be classified.
    #[default]
    Unknown,
}

impl DeviceClass {
    /// Stable lowercase label, suitable for storing in a TEXT column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Mobile => "mobile",
            Self::Tablet => "tablet",
            Self::Bot => "bot",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for DeviceClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The result of parsing a `User-Agent` header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ParsedUserAgent {
    /// Browser or client family, e.g. `"Firefox"`, `"Chrome"`, `"curl"`.
    /// `"Unknown"` when unrecognised.
    pub family: String,
    /// Operating system, e.g. `"Windows"`, `"macOS"`, `"iOS"`.
    /// `"Unknown"` when unrecognised.
    pub os: String,
    /// Coarse device class.
    pub device_class: DeviceClass,
}

/// Parse a `User-Agent` header value into family / OS / device class.
///
/// Heuristic, allocation-light, and total: never fails, returning
/// `Unknown` fields for anything it cannot classify (including the empty
/// string).
#[must_use]
pub fn parse_user_agent(ua: &str) -> ParsedUserAgent {
    let lower = ua.to_ascii_lowercase();
    if lower.trim().is_empty() {
        return ParsedUserAgent {
            family: "Unknown".to_owned(),
            os: "Unknown".to_owned(),
            device_class: DeviceClass::Unknown,
        };
    }

    let family = parse_family(&lower);
    let os = parse_os(&lower);
    let device_class = parse_device_class(&lower, family);

    ParsedUserAgent {
        family: family.to_owned(),
        os: os.to_owned(),
        device_class,
    }
}

/// Scripted clients and crawlers, matched before browser families because
/// many bots embed browser tokens (`"compatible; Googlebot/2.1"`).
const BOT_MARKERS: &[(&str, &str)] = &[
    ("googlebot", "Googlebot"),
    ("bingbot", "Bingbot"),
    ("duckduckbot", "DuckDuckBot"),
    ("yandexbot", "YandexBot"),
    ("baiduspider", "Baiduspider"),
    ("curl/", "curl"),
    ("wget/", "Wget"),
    ("python-requests", "python-requests"),
    ("python-urllib", "python-urllib"),
    ("go-http-client", "Go-http-client"),
    ("okhttp", "okhttp"),
    ("postmanruntime", "PostmanRuntime"),
    ("headlesschrome", "HeadlessChrome"),
];

fn bot_family(lower: &str) -> Option<&'static str> {
    BOT_MARKERS
        .iter()
        .find(|(marker, _)| lower.contains(marker))
        .map(|&(_, family)| family)
        .or_else(|| {
            // Generic crawler markers without a well-known name.
            (lower.contains("bot/")
                || lower.contains("bot;")
                || lower.contains("crawler")
                || lower.contains("spider"))
            .then_some("Bot")
        })
}

fn parse_family(lower: &str) -> &'static str {
    if let Some(bot) = bot_family(lower) {
        return bot;
    }
    // Order matters: Edge/Opera/Samsung Internet embed "chrome", and
    // Chrome embeds "safari", so check the most specific tokens first.
    if lower.contains("edg/") || lower.contains("edge/") {
        "Edge"
    } else if lower.contains("opr/") || lower.contains("opera") {
        "Opera"
    } else if lower.contains("samsungbrowser") {
        "Samsung Internet"
    } else if lower.contains("firefox/") || lower.contains("fxios/") {
        "Firefox"
    } else if lower.contains("crios/") || lower.contains("chrome/") || lower.contains("chromium/") {
        "Chrome"
    } else if lower.contains("safari/") {
        "Safari"
    } else {
        "Unknown"
    }
}

fn parse_os(lower: &str) -> &'static str {
    // iOS before macOS: iPhone UAs contain "like mac os x".
    if lower.contains("iphone") || lower.contains("ipad") || lower.contains("ipod") {
        "iOS"
    } else if lower.contains("android") {
        "Android"
    } else if lower.contains("windows") {
        "Windows"
    } else if lower.contains("mac os x") || lower.contains("macintosh") {
        "macOS"
    } else if lower.contains("cros") {
        "ChromeOS"
    } else if lower.contains("linux") || lower.contains("x11") {
        "Linux"
    } else {
        "Unknown"
    }
}

fn parse_device_class(lower: &str, family: &'static str) -> DeviceClass {
    if bot_family(lower).is_some() {
        return DeviceClass::Bot;
    }
    if lower.contains("ipad") || lower.contains("tablet") {
        return DeviceClass::Tablet;
    }
    if lower.contains("android") {
        // Android phones carry an explicit "mobile" token; tablets do not.
        return if lower.contains("mobile") {
            DeviceClass::Mobile
        } else {
            DeviceClass::Tablet
        };
    }
    if lower.contains("iphone") || lower.contains("ipod") || lower.contains("mobile") {
        return DeviceClass::Mobile;
    }
    if family == "Unknown" && parse_os(lower) == "Unknown" {
        return DeviceClass::Unknown;
    }
    DeviceClass::Desktop
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(ua: &str) -> (String, String, DeviceClass) {
        let p = parse_user_agent(ua);
        (p.family, p.os, p.device_class)
    }

    #[test]
    fn chrome_on_macos_desktop() {
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        assert_eq!(
            parsed(ua),
            ("Chrome".into(), "macOS".into(), DeviceClass::Desktop)
        );
    }

    #[test]
    fn firefox_on_windows_desktop() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:124.0) Gecko/20100101 Firefox/124.0";
        assert_eq!(
            parsed(ua),
            ("Firefox".into(), "Windows".into(), DeviceClass::Desktop)
        );
    }

    #[test]
    fn safari_on_iphone_is_mobile() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 \
                  (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1";
        assert_eq!(
            parsed(ua),
            ("Safari".into(), "iOS".into(), DeviceClass::Mobile)
        );
    }

    #[test]
    fn chrome_on_android_phone_is_mobile() {
        let ua = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/123.0.0.0 Mobile Safari/537.36";
        assert_eq!(
            parsed(ua),
            ("Chrome".into(), "Android".into(), DeviceClass::Mobile)
        );
    }

    #[test]
    fn android_without_mobile_token_is_tablet() {
        let ua = "Mozilla/5.0 (Linux; Android 13; SM-X906C) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/116.0.0.0 Safari/537.36";
        assert_eq!(
            parsed(ua),
            ("Chrome".into(), "Android".into(), DeviceClass::Tablet)
        );
    }

    #[test]
    fn ipad_is_tablet() {
        let ua = "Mozilla/5.0 (iPad; CPU OS 17_4 like Mac OS X) AppleWebKit/605.1.15 \
                  (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1";
        assert_eq!(
            parsed(ua),
            ("Safari".into(), "iOS".into(), DeviceClass::Tablet)
        );
    }

    #[test]
    fn edge_is_not_misread_as_chrome() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36 Edg/122.0.2365.92";
        assert_eq!(
            parsed(ua),
            ("Edge".into(), "Windows".into(), DeviceClass::Desktop)
        );
    }

    #[test]
    fn desktop_safari_is_not_misread_as_chrome() {
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                  (KHTML, like Gecko) Version/17.4 Safari/605.1.15";
        assert_eq!(
            parsed(ua),
            ("Safari".into(), "macOS".into(), DeviceClass::Desktop)
        );
    }

    #[test]
    fn linux_firefox_desktop() {
        let ua = "Mozilla/5.0 (X11; Linux x86_64; rv:124.0) Gecko/20100101 Firefox/124.0";
        assert_eq!(
            parsed(ua),
            ("Firefox".into(), "Linux".into(), DeviceClass::Desktop)
        );
    }

    #[test]
    fn curl_is_a_bot_class_client() {
        assert_eq!(
            parsed("curl/8.4.0"),
            ("curl".into(), "Unknown".into(), DeviceClass::Bot)
        );
    }

    #[test]
    fn googlebot_is_bot() {
        let ua = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
        let p = parse_user_agent(ua);
        assert_eq!(p.device_class, DeviceClass::Bot);
    }

    #[test]
    fn empty_string_is_unknown() {
        assert_eq!(
            parsed(""),
            ("Unknown".into(), "Unknown".into(), DeviceClass::Unknown)
        );
    }

    #[test]
    fn garbage_is_unknown_not_panicking() {
        assert_eq!(
            parsed("\u{0} \u{ffff} ~~~///"),
            ("Unknown".into(), "Unknown".into(), DeviceClass::Unknown)
        );
    }

    #[test]
    fn device_class_labels_are_stable() {
        assert_eq!(DeviceClass::Desktop.as_str(), "desktop");
        assert_eq!(DeviceClass::Mobile.as_str(), "mobile");
        assert_eq!(DeviceClass::Tablet.as_str(), "tablet");
        assert_eq!(DeviceClass::Bot.as_str(), "bot");
        assert_eq!(DeviceClass::Unknown.as_str(), "unknown");
    }
}
