//! The updater's one way onto the network, behind a trait so the state machine is tested with a fake and never
//! reaches a real server. The real client follows redirects itself, one hop at a time, and checks every hop's address
//! against the `Policy` before asking for it.

use std::path::Path;
use std::time::Duration;

use futures_util::future::BoxFuture;
use url::{Host, Url};

/// GitHub answers a release download with a redirect to its asset host, so a hop or two is normal.
const MAX_REDIRECTS: usize = 5;

/// Where the updater may ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// The production feed: HTTPS to github.com or its asset hosts, on the default port, with no credentials in the
    /// address.
    GitHub,
    /// The speech model: HTTPS to huggingface.co or its `*.hf.co` download hosts, on the default port, with no
    /// credentials in the address.
    HuggingFace,
    /// The update test's local feed: this PC only.
    Loopback,
    /// This PC, one port: lets a test show that a redirect to anything else is never asked for, even a local address.
    #[cfg(test)]
    LoopbackPort(u16),
}

fn is_loopback(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && match url.host() {
            Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
            Some(Host::Ipv4(address)) => address.is_loopback(),
            Some(Host::Ipv6(address)) => address.is_loopback(),
            None => false,
        }
}

fn is_github(url: &Url) -> bool {
    let Some(Host::Domain(name)) = url.host() else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none_or(|port| port == 443)
        && (name == "github.com" || name.ends_with(".githubusercontent.com"))
}

/// Where the speech model may come from: Hugging Face and its download hosts (`huggingface.co`, `*.hf.co`).
pub fn is_model_host(url: &Url) -> bool {
    let Some(Host::Domain(name)) = url.host() else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none_or(|port| port == 443)
        && (name == "huggingface.co"
            || (name.ends_with(".hf.co")
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')))
}

impl Policy {
    /// The policy a feed address implies. An address that is neither GitHub nor this PC is not a feed.
    pub fn for_feed(feed: &Url) -> Result<Policy, String> {
        if is_loopback(feed) {
            Ok(Policy::Loopback)
        } else if is_github(feed) {
            Ok(Policy::GitHub)
        } else {
            Err("The update feed isn't an address the updater may use.".into())
        }
    }

    pub fn check(self, url: &Url) -> Result<(), String> {
        // Tests never reach a real server, whatever policy they hold.
        #[cfg(test)]
        {
            if !is_loopback(url) {
                return Err("Tests never contact the network.".into());
            }
        }
        let allowed = match self {
            Policy::GitHub => is_github(url),
            Policy::HuggingFace => is_model_host(url),
            Policy::Loopback => is_loopback(url),
            #[cfg(test)]
            Policy::LoopbackPort(port) => is_loopback(url) && url.port() == Some(port),
        };
        if allowed {
            Ok(())
        } else {
            Err("The update asked for an address the updater may not use.".into())
        }
    }
}

pub type Progress<'a> = &'a (dyn Fn(u64, Option<u64>) + Send + Sync);

pub trait Http: Send + Sync {
    /// The body of `url` as text, at most `max_bytes` of it.
    fn get_text<'a>(
        &'a self,
        url: &'a Url,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<String, String>>;

    /// Writes the body of `url` to `destination`, at most `max_bytes` of it, reporting (received, total).
    fn download<'a>(
        &'a self,
        url: &'a Url,
        destination: &'a Path,
        max_bytes: u64,
        progress: Progress<'a>,
    ) -> BoxFuture<'a, Result<(), String>>;
}

pub struct ReqwestHttp {
    client: reqwest::Client,
    policy: Policy,
}

impl ReqwestHttp {
    pub fn new(policy: Policy) -> Result<ReqwestHttp, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .user_agent("Tjommie-updater")
            .build()
            .map_err(|_| "The update check could not start.".to_string())?;
        Ok(ReqwestHttp { client, policy })
    }

    /// Asks for `url`, following redirects only to addresses the policy allows.
    async fn open(&self, url: &Url) -> Result<reqwest::Response, String> {
        let mut url = url.clone();
        for _ in 0..=MAX_REDIRECTS {
            self.policy.check(&url)?;
            let response = self
                .client
                .get(url.clone())
                .send()
                .await
                .map_err(|_| "The update server could not be reached.".to_string())?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or("The update server sent a bad redirect.")?;
                url = url
                    .join(location)
                    .map_err(|_| "The update server sent a bad redirect.".to_string())?;
                continue;
            }
            return if response.status().is_success() {
                Ok(response)
            } else {
                Err(format!(
                    "The update server answered {}.",
                    response.status().as_u16()
                ))
            };
        }
        Err("The update server redirected too many times.".into())
    }
}

/// A stalled connection must not hold an update check up for ever.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

impl Http for ReqwestHttp {
    fn get_text<'a>(
        &'a self,
        url: &'a Url,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let mut response = self.open(url).await?;
            let mut body = Vec::new();
            while let Some(chunk) = tokio::time::timeout(CHUNK_TIMEOUT, response.chunk())
                .await
                .map_err(|_| "The update server stopped answering.".to_string())?
                .map_err(|_| "The update server dropped the connection.".to_string())?
            {
                if body.len() + chunk.len() > max_bytes {
                    return Err("The update server's answer is too large.".into());
                }
                body.extend_from_slice(&chunk);
            }
            String::from_utf8(body).map_err(|_| "The update server's answer isn't text.".into())
        })
    }

    fn download<'a>(
        &'a self,
        url: &'a Url,
        destination: &'a Path,
        max_bytes: u64,
        progress: Progress<'a>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            use tokio::io::AsyncWriteExt as _;
            let mut response = self.open(url).await?;
            let total = response.content_length();
            if total.is_some_and(|total| total > max_bytes) {
                return Err("The update is larger than expected.".into());
            }
            let io = |_| "The update could not be saved.".to_string();
            let mut file = tokio::fs::File::create(destination).await.map_err(io)?;
            let mut received = 0u64;
            while let Some(chunk) = tokio::time::timeout(CHUNK_TIMEOUT, response.chunk())
                .await
                .map_err(|_| "The update server stopped answering.".to_string())?
                .map_err(|_| "The update server dropped the connection.".to_string())?
            {
                received += chunk.len() as u64;
                if received > max_bytes {
                    return Err("The update is larger than expected.".into());
                }
                file.write_all(&chunk).await.map_err(io)?;
                progress(received, total);
            }
            file.flush().await.map_err(io)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn a_feed_is_github_or_this_pc_and_nothing_else() {
        assert_eq!(
            Policy::for_feed(&url(
                "https://github.com/a/b/releases/latest/download/latest.yml"
            )),
            Ok(Policy::GitHub)
        );
        assert_eq!(
            Policy::for_feed(&url("http://127.0.0.1:8765/latest.yml")),
            Ok(Policy::Loopback)
        );
        assert_eq!(
            Policy::for_feed(&url("http://localhost:1/latest.yml")),
            Ok(Policy::Loopback)
        );
        assert_eq!(
            Policy::for_feed(&url("http://[::1]:1/latest.yml")),
            Ok(Policy::Loopback)
        );
        for bad in [
            "http://github.com/a/latest.yml",
            "https://evil.example/latest.yml",
            "https://github.com.evil.example/latest.yml",
            "https://evilgithub.com/latest.yml",
            "https://user:pw{AT}github.com/latest.yml",
            "https://github.com:8443/latest.yml",
            "ftp://127.0.0.1/latest.yml",
            "file:///C:/latest.yml",
            "http://192.168.1.5/latest.yml",
            "http://0.0.0.0/latest.yml",
        ] {
            // Credentials in the address: the at sign is put in here so no address-like text is in the source.
            let bad = bad.replace("{AT}", "@");
            assert!(
                Policy::for_feed(&url(&bad)).is_err(),
                "{bad} must not be a feed"
            );
        }
    }

    // The policy is what `check` enforces on every redirect hop. (In the test build `check` also refuses anything off
    // this PC, so these use the pure predicates for GitHub's addresses.)
    #[test]
    fn redirects_may_go_to_githubs_asset_hosts_only() {
        for good in [
            "https://github.com/x/y/releases/download/v1/a.exe",
            "https://objects.githubusercontent.com/github-production-release-asset/1",
            "https://release-assets.githubusercontent.com/a?x=1",
        ] {
            assert!(is_github(&url(good)), "{good}");
        }
        for bad in [
            "http://github.com/x",
            "https://githubusercontent.com.evil.example/x",
            "https://evil.example/github.com",
            "https://127.0.0.1/x",
            "https://api.github.com:444/x",
            "https://user{AT}github.com/x",
        ] {
            let bad = bad.replace("{AT}", "@");
            assert!(!is_github(&url(&bad)), "{bad}");
        }
        // And in the test build the real policy check refuses every real address, so no test can reach the network.
        assert!(Policy::GitHub.check(&url("https://github.com/x")).is_err());
        assert!(Policy::Loopback.check(&url("http://127.0.0.1:9/x")).is_ok());
        assert!(
            Policy::Loopback
                .check(&url("http://example.com/x"))
                .is_err()
        );
    }
}
