//! The providers Pindot's own loop knows: local servers and API services
//! that speak OpenAI's Chat Completions. Check a URL against the
//! provider's current documentation before changing it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    /// Empty for Custom.
    pub base_url: &'static str,
    /// Runs on this computer: looked for on loopback, no key.
    pub local: bool,
    pub needs_key: bool,
    /// How its keys usually start, to catch a key pasted for another one.
    pub key_prefix: Option<&'static str>,
}

const fn p(id: &'static str, name: &'static str, base_url: &'static str, local: bool, key_prefix: Option<&'static str>) -> Provider {
    Provider { id, name, base_url, local, needs_key: key_prefix.is_some(), key_prefix }
}

pub const PROVIDERS: &[Provider] = &[
    p("ollama", "Ollama", "http://127.0.0.1:11434/v1", true, None),
    p("lmstudio", "LM Studio", "http://127.0.0.1:1234/v1", true, None),
    p("llamacpp", "llama.cpp", "http://127.0.0.1:8080/v1", true, None),
    p("openai", "OpenAI", "https://api.openai.com/v1", false, Some("sk-")),
    p("openrouter", "OpenRouter", "https://openrouter.ai/api/v1", false, Some("sk-or-")),
    p("deepseek", "DeepSeek", "https://api.deepseek.com/v1", false, Some("sk-")),
    p("groq", "Groq", "https://api.groq.com/openai/v1", false, Some("gsk_")),
    p("custom", "Other (OpenAI-compatible)", "", false, None),
];

pub fn find(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

fn article(name: &str) -> &'static str {
    if name.starts_with(['A', 'E', 'I', 'O', 'U']) { "an" } else { "a" }
}

/// A warning when `key` looks like it belongs to another provider, or not
/// like this provider's keys at all.
pub fn key_warning(provider: &Provider, key: &str) -> Option<String> {
    let key = key.trim();
    let mine = provider.key_prefix.filter(|k| key.starts_with(k)).map_or(0, str::len);
    let matches: Vec<(usize, &Provider)> =
        PROVIDERS.iter().filter_map(|p| p.key_prefix.filter(|k| key.starts_with(k)).map(|k| (k.len(), p))).collect();
    let best = matches.iter().map(|m| m.0).max().unwrap_or(0);
    let owners: Vec<&Provider> = matches.iter().filter(|m| m.0 == best).map(|m| m.1).collect();
    if best > mine && owners.len() == 1 && owners[0].id != provider.id {
        let o = owners[0];
        return Some(format!(
            "This looks like {} {} key, not {} {} one.",
            article(o.name),
            o.name,
            article(provider.name),
            provider.name
        ));
    }
    match provider.key_prefix {
        Some(k) if mine == 0 => Some(format!("{} keys usually start with \"{k}\".", provider.name)),
        _ => None,
    }
}

/// An `http://` address of another computer: a key must never go there in
/// the clear.
pub fn plain_http_remote(base_url: &str) -> bool {
    let Ok(u) = url::Url::parse(base_url.trim()) else { return false };
    if u.scheme() != "http" {
        return false;
    }
    match u.host() {
        Some(url::Host::Ipv4(ip)) => !ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => !ip.is_loopback(),
        Some(url::Host::Domain(d)) => d != "localhost",
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_presets_are_loopback_http_and_need_no_key() {
        for p in PROVIDERS.iter().filter(|p| p.local) {
            assert!(p.base_url.starts_with("http://127.0.0.1:"), "{}", p.id);
            assert!(!p.needs_key && p.key_prefix.is_none(), "{}", p.id);
        }
        assert_eq!(find("ollama").unwrap().base_url, "http://127.0.0.1:11434/v1");
        assert!(find("nope").is_none());
    }

    #[test]
    fn a_key_for_another_provider_is_caught() {
        let openai = find("openai").unwrap();
        let openrouter = find("openrouter").unwrap();
        assert_eq!(key_warning(openai, "sk-proj-abc"), None);
        assert_eq!(key_warning(openai, "sk-or-v1-abc"), Some("This looks like an OpenRouter key, not an OpenAI one.".into()));
        assert_eq!(key_warning(openrouter, "sk-proj-abc"), Some("OpenRouter keys usually start with \"sk-or-\".".into()));
        assert_eq!(key_warning(find("groq").unwrap(), "gsk_x"), None);
        assert_eq!(key_warning(find("custom").unwrap(), "anything"), None);
    }

    #[test]
    fn plain_http_to_another_computer_is_spotted() {
        assert!(!plain_http_remote("http://127.0.0.1:11434/v1"));
        assert!(!plain_http_remote("http://localhost:1234/v1"));
        assert!(!plain_http_remote("http://[::1]:8080/v1"));
        assert!(!plain_http_remote("https://api.openai.com/v1"));
        assert!(plain_http_remote("http://192.168.1.20:11434/v1"));
    }
}
