//! Where a granted permission applies: only some chats, hosts, or accounts. A permission without
//! a scope applies everywhere, as before scopes existed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    /// Chat ids in the Bot API format, as modules see them in `msg.chat_id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chats: Vec<i64>,
    /// Host names; `*.example.com` also matches `example.com` and every subdomain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// Session files of the accounts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<String>,
}

impl Scope {
    pub fn is_empty(&self) -> bool {
        self.chats.is_empty() && self.hosts.is_empty() && self.accounts.is_empty()
    }

    pub fn normalized(mut self) -> Self {
        self.chats.sort_unstable();
        self.chats.dedup();
        self.hosts = self
            .hosts
            .iter()
            .map(|host| host.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|host| !host.is_empty())
            .collect();
        self.hosts.sort();
        self.hosts.dedup();
        self.accounts = self
            .accounts
            .iter()
            .map(|account| account.trim().to_string())
            .filter(|account| !account.is_empty())
            .collect();
        self.accounts.sort();
        self.accounts.dedup();
        self
    }

    /// Whether the scope covers a call from `account` in `chat`. A call outside any chat, such
    /// as a scheduled one, is outside a scope that names chats.
    pub fn covers(&self, account: &str, chat: Option<i64>) -> bool {
        (self.accounts.is_empty() || self.accounts.iter().any(|a| a == account))
            && (self.chats.is_empty() || chat.is_some_and(|chat| self.chats.contains(&chat)))
    }
}

pub fn host_matches(pattern: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    match pattern.strip_prefix("*.") {
        Some(domain) => host == domain || host.ends_with(&format!(".{domain}")),
        None => host == pattern,
    }
}

/// Whether a module may use `permission` for a call from `account` in `chat`.
pub fn allows(
    trusted: bool,
    permissions: &[String],
    scopes: &BTreeMap<String, Scope>,
    permission: &str,
    account: &str,
    chat: Option<i64>,
) -> bool {
    trusted
        || (permissions.iter().any(|p| p == permission)
            && scopes
                .get(permission)
                .is_none_or(|scope| scope.covers(account, chat)))
}

/// Scopes worth keeping: normalized, not empty, and only for granted permissions.
pub fn clean(scopes: BTreeMap<String, Scope>, permissions: &[String]) -> BTreeMap<String, Scope> {
    scopes
        .into_iter()
        .filter(|(permission, _)| permissions.contains(permission))
        .map(|(permission, scope)| (permission, scope.normalized()))
        .filter(|(_, scope)| !scope.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scoped(permission: &str, scope: Scope) -> BTreeMap<String, Scope> {
        BTreeMap::from([(permission.to_string(), scope)])
    }

    #[test]
    fn hosts_match_exactly_or_by_wildcard() {
        assert!(host_matches("api.example.com", "API.example.com."));
        assert!(!host_matches("api.example.com", "evil-api.example.com"));
        assert!(host_matches("*.example.com", "example.com"));
        assert!(host_matches("*.example.com", "a.b.example.com"));
        assert!(!host_matches("*.example.com", "example.com.evil.org"));
        assert!(!host_matches("*.example.com", "badexample.com"));
    }

    #[test]
    fn a_scope_limits_chats_and_accounts() {
        let permissions = vec!["telegram.read".to_string()];
        let scopes = scoped(
            "telegram.read",
            Scope {
                chats: vec![-100, 5],
                accounts: vec!["main.session".into()],
                ..Scope::default()
            },
        );
        let allows = |account: &str, chat: Option<i64>| {
            allows(false, &permissions, &scopes, "telegram.read", account, chat)
        };
        assert!(allows("main.session", Some(5)));
        assert!(!allows("main.session", Some(6)));
        assert!(!allows("work.session", Some(5)));
        assert!(!allows("main.session", None));
        assert!(super::allows(
            true,
            &[],
            &BTreeMap::new(),
            "shell",
            "any",
            None
        ));
        assert!(super::allows(
            false,
            &permissions,
            &BTreeMap::new(),
            "telegram.read",
            "any",
            None
        ));
        assert!(!super::allows(
            false,
            &permissions,
            &BTreeMap::new(),
            "network",
            "any",
            None
        ));
    }

    #[test]
    fn clean_drops_empty_and_ungranted_scopes() {
        let mut scopes = scoped(
            "network",
            Scope {
                hosts: vec![" API.X.com. ".into(), "api.x.com".into()],
                ..Scope::default()
            },
        );
        scopes.insert(
            "shell".into(),
            Scope {
                chats: vec![1],
                ..Scope::default()
            },
        );
        scopes.insert("telegram.read".into(), Scope::default());
        let permissions = vec!["network".to_string(), "telegram.read".to_string()];
        let cleaned = clean(scopes, &permissions);
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned["network"].hosts, vec!["api.x.com"]);
    }
}
