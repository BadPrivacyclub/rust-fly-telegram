//! SOCKS5 proxy helpers, including per-account circuit isolation for Tor.

use sha2::{Digest, Sha256};

const TOR_PORTS: [&str; 2] = ["9050", "9150"];
const LOCAL_HOSTS: [&str; 4] = ["127.0.0.1", "localhost", "[::1]", "::1"];

/// Splits `socks5://[user[:pass]@]host:port` into (userinfo, host, port).
fn split(url: &str) -> Option<(Option<&str>, &str, &str)> {
    let rest = url.trim().strip_prefix(crate::config::SOCKS5_SCHEME)?;
    let rest = rest.trim_end_matches('/');
    let (userinfo, hostport) = match rest.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(userinfo), hostport),
        None => (None, rest),
    };
    let (host, port) = hostport.rsplit_once(':')?;
    Some((userinfo, host, port))
}

/// True for the local Tor SOCKS ports (`socks5://127.0.0.1:9050`, or 9150 for Tor Browser).
pub fn is_tor(url: &str) -> bool {
    split(url).is_some_and(|(_, host, port)| {
        LOCAL_HOSTS.contains(&host.to_ascii_lowercase().as_str()) && TOR_PORTS.contains(&port)
    })
}

/// Proxy URL used for one account's connections.
///
/// Tor gives every distinct SOCKS username/password its own circuit (`IsolateSOCKSAuth`), so
/// a credential derived from the session file puts each account on a separate circuit and,
/// in practice, a different exit IP. Proxies that are not Tor, and URLs that already carry
/// credentials, are used unchanged.
pub fn for_account(url: Option<String>, session_file: &str) -> Option<String> {
    let url = url?;
    if !is_tor(&url) || split(&url).is_some_and(|(userinfo, _, _)| userinfo.is_some()) {
        return Some(url);
    }
    let digest = Sha256::digest(session_file.trim().replace('\\', "/").as_bytes());
    let id = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let (_, host, port) = split(&url)?;
    Some(format!(
        "{}fly-{id}:isolated@{host}:{port}",
        crate::config::SOCKS5_SCHEME
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_tor_ports_only_on_localhost() {
        assert!(is_tor("socks5://127.0.0.1:9050"));
        assert!(is_tor("socks5://localhost:9150/"));
        assert!(!is_tor("socks5://127.0.0.1:1080"));
        assert!(!is_tor("socks5://10.0.0.5:9050"));
        assert!(!is_tor("http://127.0.0.1:9050"));
    }

    #[test]
    fn isolates_accounts_on_tor() {
        let url = Some("socks5://127.0.0.1:9050".to_string());
        let a = for_account(url.clone(), "sessions/a.session").unwrap();
        let b = for_account(url.clone(), "sessions/b.session").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with("socks5://fly-") && a.ends_with("@127.0.0.1:9050"));
        assert_eq!(a, for_account(url, "sessions\\a.session").unwrap());
    }

    #[test]
    fn leaves_other_proxies_alone() {
        for url in ["socks5://127.0.0.1:1080", "socks5://u:p@127.0.0.1:9050"] {
            assert_eq!(for_account(Some(url.into()), "s").as_deref(), Some(url));
        }
        assert_eq!(for_account(None, "s"), None);
    }
}
