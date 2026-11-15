//! The browser process's arguments (research #6). One environment, so these apply to every bot's browser.

/// Chromium features that only cost memory in a bot's browser: a spare renderer, the back/forward cache, the
/// translate bar, Cast, the optimization-hints download, Edge collections and the Edge web-overlay UI.
const DISABLED_FEATURES: [&str; 7] = [
    "SpareRendererForSitePerProcess",
    "BackForwardCache",
    "Translate",
    "MediaRouter",
    "OptimizationHints",
    "EdgeCollections",
    "msWebOOUI",
];

/// What `AdditionalBrowserArguments` is set to.
pub fn browser_arguments() -> String {
    format!("--disable-features={}", DISABLED_FEATURES.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_exact() {
        assert_eq!(
            browser_arguments(),
            "--disable-features=SpareRendererForSitePerProcess,BackForwardCache,Translate,MediaRouter,OptimizationHints,EdgeCollections,msWebOOUI"
        );
    }

    #[test]
    fn never_weakens_isolation_or_changes_the_process_model() {
        let arguments = browser_arguments();
        for forbidden in [
            "--disable-site-isolation-trials",
            "IsolateOrigins",
            "site-per-process",
            "--single-process",
            "--in-process-gpu",
            "--disable-gpu",
            "--renderer-process-limit",
        ] {
            // "SpareRendererForSitePerProcess" contains "SitePerProcess" but not the lower-case flag.
            assert!(
                !arguments.contains(forbidden),
                "{forbidden} must not be set"
            );
        }
    }
}
