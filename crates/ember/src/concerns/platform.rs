//! `ApplicationPlatform` (reference/app/models/application_platform.rb, over platform_agent 1.0.1)
//! and the `allow_browser` check from reference/app/controllers/concerns/allow_browser.rb.

use super::user_agent::{self, Agent, Raised, Rb, Version};

/// `ApplicationPlatform.new(request.user_agent)`. Predicates marked "raises" in Ruby (a nil
/// `user_agent.browser`, which Rails turns into a 500) answer false here.
#[derive(Debug, Clone)]
pub struct ApplicationPlatform {
    user_agent_string: String,
    user_agent: Agent,
}

impl ApplicationPlatform {
    pub fn new(user_agent: Option<&str>) -> Self {
        // `match?` works on `user_agent_string.to_s`, and UserAgent.parse treats nil like "".
        let user_agent_string = user_agent.unwrap_or("").to_string();
        let user_agent = user_agent::parse(&user_agent_string);
        Self { user_agent_string, user_agent }
    }

    fn matches(&self, needle: &str) -> bool {
        self.user_agent_string.contains(needle)
    }

    pub fn ios(&self) -> bool {
        self.matches("iPhone") || self.matches("iPad")
    }

    pub fn android(&self) -> bool {
        self.matches("Android")
    }

    pub fn mac(&self) -> bool {
        self.matches("Macintosh")
    }

    /// Apple Messages link previews claim to be both the Facebook and Twitter bots.
    pub fn apple_messages(&self) -> bool {
        let lowercased = self.user_agent_string.to_lowercase();
        lowercased.contains("facebookexternalhit") && lowercased.contains("twitterbot")
    }

    pub fn mobile(&self) -> bool {
        self.ios() || self.android()
    }

    pub fn desktop(&self) -> bool {
        !self.mobile()
    }

    /// `operating_system`: nil when the gem's `os` is nil.
    fn try_operating_system(&self) -> Rb<Option<String>> {
        let platform = self.user_agent.try_platform()?.unwrap_or_default();
        let named = [
            ("Android", "Android"),
            ("iPad", "iPad"),
            ("iPhone", "iPhone"),
            ("Macintosh", "macOS"),
            ("Windows", "Windows"),
            ("CrOS", "ChromeOS"),
        ]
        .into_iter()
        .find(|(needle, _)| platform.contains(needle));

        Ok(match named {
            Some((_, name)) => Some(name.to_string()),
            None => self.user_agent.try_os()?.map(|os| if os.contains("Linux") { "Linux".into() } else { os }),
        })
    }

    /// The platform as the views see it. `chrome?`, `firefox?`, `safari?` and `edge?` all read the
    /// gem's `browser`, and `windows?` reads `operating_system`, so each is worked out once here.
    /// `browser` (delegated to the gem) and `operating_system` are "" when nil or raised.
    #[allow(clippy::needless_update)]
    pub fn to_view(&self) -> ember_views::Platform {
        let browser = self.user_agent.try_browser();
        let operating_system = self.try_operating_system();
        let browser_is = |names: &[&str]| browser_matches(&browser, names).unwrap_or(false);

        ember_views::Platform {
            ios: self.ios(),
            android: self.android(),
            mac: self.mac(),
            windows: is_windows(&operating_system).unwrap_or(false),
            chrome: browser_is(CHROME),
            firefox: browser_is(FIREFOX),
            safari: browser_is(SAFARI),
            edge: browser_is(EDGE),
            mobile: self.mobile(),
            desktop: self.desktop(),
            apple_messages: self.apple_messages(),
            browser: browser.ok().flatten().unwrap_or_default(),
            operating_system: operating_system.ok().flatten().unwrap_or_default(),
            ..Default::default()
        }
    }
}

/// What `chrome?`, `firefox?`, `safari?` and `edge?` look for in the gem's `browser`.
const CHROME: &[&str] = &["Chrome"];
const FIREFOX: &[&str] = &["Firefox", "FxiOS"];
const SAFARI: &[&str] = &["Safari"];
const EDGE: &[&str] = &["Edg"];

/// `user_agent.browser.match?(/A|B/)`, which raises for a nil browser.
fn browser_matches(browser: &Rb<Option<String>>, names: &[&str]) -> Rb<bool> {
    let browser = browser.as_ref().map_err(|_| Raised)?.as_deref().ok_or(Raised)?;
    Ok(names.iter().any(|name| browser.contains(name)))
}

/// `windows?`: `operating_system == "Windows"`.
fn is_windows(operating_system: &Rb<Option<String>>) -> Rb<bool> {
    Ok(operating_system.as_ref().map_err(|_| Raised)?.as_deref() == Some("Windows"))
}

impl ApplicationPlatform {
    /// `ActionController::AllowBrowser::BrowserBlocker#blocked?` with Ember's
    /// `AllowBrowser::VERSIONS = { safari: 17.2, chrome: 120, firefox: 121, opera: 104, ie: false }`.
    /// The blocker parses the header itself; this reads the platform's parse of the same string.
    /// Rails raises (a 500) for a versioned agent with a nil browser; that is not blocked here.
    pub fn browser_blocked(&self) -> bool {
        self.try_browser_blocked().unwrap_or(false)
    }

    fn try_browser_blocked(&self) -> Rb<bool> {
        if !user_agent::is_present(&self.user_agent_string) {
            return Ok(false);
        }
        let agent = &self.user_agent;
        let Some(version) = agent.try_version()?.filter(Version::is_present) else {
            return Ok(false);
        };

        let browser = agent.try_browser()?.ok_or(Raised)?.to_lowercase();
        // `nil` means the browser isn't version-guarded; `Some(None)` is `ie: false`, always blocked.
        let minimum = match browser.as_str() {
            "safari" => Some(Some("17.2")),
            "chrome" => Some(Some("120")),
            "firefox" => Some(Some("121")),
            "opera" => Some(Some("104")),
            "internet explorer" => Some(None),
            _ => None,
        };

        let Some(minimum) = minimum else {
            return Ok(false);
        };
        let below_minimum = minimum.is_none_or(|minimum| version < Version::new(minimum));
        Ok(below_minimum && !agent.is_bot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concerns::user_agent::tests::{check, vectors};
    use serde_json::json;

    #[test]
    fn matches_application_platform_and_allow_browser() {
        let vectors = vectors();
        let mut failures = Vec::new();

        for case in vectors["user_agents"].as_array().unwrap() {
            let ua = case["ua"].as_str();
            let platform = ApplicationPlatform::new(ua);
            let browser = platform.user_agent.try_browser();
            let operating_system = platform.try_operating_system();
            let expected = &case["application_platform"];
            let label = &case["ua"];
            let mut field = |name: &str, actual: Rb<serde_json::Value>| {
                check(&mut failures, &format!("{label} {name}"), &expected[name], actual);
            };

            field("ios", Ok(json!(platform.ios())));
            field("android", Ok(json!(platform.android())));
            field("mac", Ok(json!(platform.mac())));
            field("chrome", browser_matches(&browser, CHROME).map(|v| json!(v)));
            field("firefox", browser_matches(&browser, FIREFOX).map(|v| json!(v)));
            field("safari", browser_matches(&browser, SAFARI).map(|v| json!(v)));
            field("edge", browser_matches(&browser, EDGE).map(|v| json!(v)));
            field("apple_messages", Ok(json!(platform.apple_messages())));
            field("mobile", Ok(json!(platform.mobile())));
            field("desktop", Ok(json!(platform.desktop())));
            field("windows", is_windows(&operating_system).map(|v| json!(v)));
            field("operating_system", operating_system.map(|v| json!(v)));
            field("browser", browser.map(|v| json!(v)));

            // The view answers false, or "", where Ruby raises.
            let flag = |name: &str| expected[name].as_bool().unwrap_or(false);
            let text = |name: &str| expected[name].as_str().unwrap_or_default().to_string();
            let expected_view = ember_views::Platform {
                ios: flag("ios"),
                android: flag("android"),
                mac: flag("mac"),
                windows: flag("windows"),
                chrome: flag("chrome"),
                firefox: flag("firefox"),
                safari: flag("safari"),
                edge: flag("edge"),
                mobile: flag("mobile"),
                desktop: flag("desktop"),
                apple_messages: flag("apple_messages"),
                browser: text("browser"),
                operating_system: text("operating_system"),
            };
            let (view, expected_view) = (format!("{:?}", platform.to_view()), format!("{expected_view:?}"));
            if view != expected_view {
                failures.push(format!("{label} view: expected {expected_view}, got {view}"));
            }

            check(&mut failures, &format!("{label} blocked"), &case["blocked"], platform.try_browser_blocked().map(|v| json!(v)));
        }

        assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
    }

    #[test]
    fn view_platform_uses_empty_strings_for_nil() {
        let view = ApplicationPlatform::new(Some("curl/8.4.0")).to_view();
        assert_eq!(view.operating_system, "");
        assert_eq!(view.browser, "curl");
        assert!(view.desktop);
    }

    /// `allow_browser`'s User-Agent work: it parses and checks a present header, and keeps the
    /// platform.
    fn allow_browser(user_agent: Option<&str>) -> Option<(bool, ApplicationPlatform)> {
        let platform = ApplicationPlatform::new(Some(user_agent.filter(|header| user_agent::is_present(header))?));
        Some((platform.browser_blocked(), platform))
    }

    /// The User-Agent work of one page request: `allow_browser`, then the layout's `platform`,
    /// which reads the platform `allow_browser` kept or builds its own.
    fn page_request(user_agent: Option<&str>) -> ember_views::Platform {
        match allow_browser(user_agent) {
            Some((_, platform)) => platform.to_view(),
            None => ApplicationPlatform::new(user_agent).to_view(),
        }
    }

    /// Times the User-Agent work of a page request for a few common agents, in ns per call (the
    /// median of `RUNS` runs of `ITERATIONS` calls):
    /// `cargo test --release -p ember --bin ember time_user_agent_work -- --ignored --nocapture`
    #[test]
    #[ignore = "timing harness; run by hand with --release"]
    fn time_user_agent_work() {
        use std::hint::black_box;

        const RUNS: usize = 9;
        const ITERATIONS: u32 = 100_000;
        let agents = [
            (
                "Chrome, macOS",
                Some(
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
                ),
            ),
            (
                "Chrome, Windows",
                Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36"),
            ),
            (
                "Chrome, Android",
                Some("Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36"),
            ),
            (
                "Safari, macOS",
                Some(
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
                ),
            ),
            (
                "Safari, iPhone",
                Some(
                    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1",
                ),
            ),
            ("Firefox, Windows", Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:143.0) Gecko/20100101 Firefox/143.0")),
            (
                "Edge, Windows",
                Some(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0",
                ),
            ),
            ("Googlebot", Some("Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)")),
            ("none", None),
        ];

        let median_ns = |f: &dyn Fn()| {
            let mut runs: Vec<u128> = (0..RUNS)
                .map(|_| {
                    let start = std::time::Instant::now();
                    for _ in 0..ITERATIONS {
                        f();
                    }
                    start.elapsed().as_nanos() / u128::from(ITERATIONS)
                })
                .collect();
            runs.sort_unstable();
            runs[RUNS / 2]
        };

        println!("{:<18} {:>8} {:>8} {:>8} {:>8} {:>12}", "agent", "parse", "blocked", "platform", "to_view", "page request");
        for (label, user_agent) in agents {
            let platform = ApplicationPlatform::new(user_agent);
            let parse = median_ns(&|| {
                black_box(user_agent::parse(black_box(user_agent).unwrap_or("")));
            });
            let blocked = median_ns(&|| {
                black_box(allow_browser(black_box(user_agent)));
            });
            let new = median_ns(&|| {
                black_box(ApplicationPlatform::new(black_box(user_agent)));
            });
            let to_view = median_ns(&|| {
                black_box(black_box(&platform).to_view());
            });
            let request = median_ns(&|| {
                black_box(page_request(black_box(user_agent)));
            });
            println!("{label:<18} {parse:>8} {blocked:>8} {new:>8} {to_view:>8} {request:>12}");
        }
    }
}
