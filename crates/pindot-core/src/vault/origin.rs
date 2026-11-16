//! Site origins: scheme + host + port, nothing else. A saved login works on
//! exactly one origin. There is no suffix, substring or "same site" matching:
//! two origins match only when their normalised strings are equal.

use url::{Host, Url};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OriginError {
    #[error("enter a website address, such as https://example.com")]
    Empty,
    #[error("that is not a valid website address")]
    Invalid,
    #[error("only http and https sites are supported")]
    Scheme,
    #[error("the address must not contain a user name or password")]
    UserInfo,
    #[error("use https:// (plain http is only accepted for localhost)")]
    InsecureHttp,
}

/// Is the host this computer itself?
fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(d) => *d == "localhost",
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    }
}

fn parse_url(input: &str) -> Result<Url, OriginError> {
    let t = input.trim();
    if t.is_empty() {
        return Err(OriginError::Empty);
    }
    if t.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(OriginError::Invalid);
    }
    // A bare domain such as "example.com" or "example.com:8443" means https.
    let with_scheme = if t.contains("://") { t.to_string() } else { format!("https://{t}") };
    let url = Url::parse(&with_scheme).map_err(|_| OriginError::Invalid)?;
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err(OriginError::Scheme),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(OriginError::UserInfo);
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(OriginError::Invalid);
    }
    Ok(url)
}

fn origin_of(url: &Url) -> String {
    // `Url::origin().ascii_serialization()` drops default ports and
    // lower-cases and punycodes the host.
    url.origin().ascii_serialization()
}

/// The origin to save for what the user typed or an import gave:
/// `https://www.example.com/uk/?x=1` becomes `https://www.example.com`.
/// Plain http is accepted only for loopback addresses.
pub fn parse_saved_origin(input: &str) -> Result<String, OriginError> {
    let url = parse_url(input)?;
    if url.scheme() == "http" && !url.host().as_ref().is_some_and(is_loopback) {
        return Err(OriginError::InsecureHttp);
    }
    Ok(origin_of(&url))
}

/// The origin of the page the browser shows, or `None` when the address is
/// missing, not http(s), carries a user name, or does not parse. A caller
/// treats `None` as "cannot tell" and refuses.
pub fn page_origin(url: &str) -> Option<String> {
    if !url.contains("://") {
        return None;
    }
    parse_url(url).ok().map(|u| origin_of(&u))
}

/// True only when the page really is at the saved origin.
pub fn matches(saved: &str, page_url: &str) -> bool {
    let Ok(saved) = parse_saved_origin(saved) else { return false };
    page_origin(page_url).is_some_and(|p| p == saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_origin_keeps_only_scheme_host_port() {
        assert_eq!(parse_saved_origin("https://www.example.com/uk/?a=1#x").unwrap(), "https://www.example.com");
        assert_eq!(parse_saved_origin("Example.COM").unwrap(), "https://example.com");
        assert_eq!(parse_saved_origin("https://example.com:443/").unwrap(), "https://example.com");
        assert_eq!(parse_saved_origin("https://example.com:8443").unwrap(), "https://example.com:8443");
        assert_eq!(parse_saved_origin("http://localhost:4310/x").unwrap(), "http://localhost:4310");
        assert_eq!(parse_saved_origin("http://127.0.0.1:8080").unwrap(), "http://127.0.0.1:8080");
        assert_eq!(parse_saved_origin("  example.com:8443  ").unwrap(), "https://example.com:8443");
    }

    #[test]
    fn saved_origin_refusals() {
        assert_eq!(parse_saved_origin(""), Err(OriginError::Empty));
        assert_eq!(parse_saved_origin("   "), Err(OriginError::Empty));
        assert_eq!(parse_saved_origin("http://example.com"), Err(OriginError::InsecureHttp));
        assert_eq!(parse_saved_origin("ftp://example.com"), Err(OriginError::Scheme));
        assert_eq!(parse_saved_origin("android://abc@com.example.app"), Err(OriginError::Scheme));
        assert_eq!(parse_saved_origin("https://example.com@evil.test"), Err(OriginError::UserInfo));
        assert_eq!(parse_saved_origin("https://user:pw@example.com"), Err(OriginError::UserInfo));
        assert_eq!(parse_saved_origin("https://exa mple.com"), Err(OriginError::Invalid));
        assert_eq!(parse_saved_origin("https://"), Err(OriginError::Invalid));
    }

    #[test]
    fn exact_matches() {
        let saved = "https://example.com";
        assert!(matches(saved, "https://example.com/login?next=/a#b"));
        assert!(matches(saved, "https://EXAMPLE.com:443/"));
        assert!(matches("https://example.com:8443", "https://example.com:8443/x"));
        assert!(matches("http://localhost:4310", "http://localhost:4310/login.html"));
    }

    #[test]
    fn lookalikes_do_not_match() {
        let saved = "https://example.com";
        for page in [
            "https://example.com.evil.test/",
            "https://evil-example.com/",
            "https://evilexample.com/",
            "https://example.com.evil.test:443/",
            "http://example.com/",
            "https://example.com:8443/",
            "https://example.com@evil.test/",
            "https://user@example.com/",
            "https://www.example.com/",
            "https://sub.example.com/",
            "https://example.com./",
            "https://example.co/",
            "https://example.com%2eevil.test/",
            "https://xn--exmple-cua.com/",
            "javascript:alert(1)",
            "about:blank",
            "data:text/html,example.com",
            "file:///example.com",
            "example.com",
            "",
        ] {
            assert!(!matches(saved, page), "{page} must not match");
        }
        // Different port the other way round, and http vs https.
        assert!(!matches("https://example.com:8443", "https://example.com/"));
        assert!(!matches("http://localhost:4310", "http://localhost:4311/"));
        assert!(!matches("http://localhost:4310", "https://localhost:4310/"));
    }

    #[test]
    fn a_bad_saved_origin_never_matches() {
        assert!(!matches("", "https://example.com"));
        assert!(!matches("http://example.com", "http://example.com"));
    }

    #[test]
    fn page_origin_needs_a_scheme() {
        assert_eq!(page_origin("https://a.test/x").as_deref(), Some("https://a.test"));
        assert_eq!(page_origin("a.test"), None);
        assert_eq!(page_origin("https://u@a.test/"), None);
    }
}
