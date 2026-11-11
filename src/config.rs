//! `config.toml` - a flat, commented file that is easy to edit by hand or via the settings window.

use crate::util::{self, Res};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub vault: String,
    pub folder: String,
    pub interval_minutes: u64,
    pub queries: Vec<String>,
    pub repos: Vec<String>,
    pub exclude_repos: Vec<String>,
    pub notifications: bool,
    pub notify_own_activity: bool,
    pub keep_closed_days: u64,
    pub api_url: String,
    pub client_id: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            vault: String::new(),
            folder: "GitHub".into(),
            interval_minutes: 5,
            queries: vec![
                "is:pr is:open involves:@me".into(),
                "is:pr is:open review-requested:@me".into(),
            ],
            repos: vec![],
            exclude_repos: vec![],
            notifications: true,
            notify_own_activity: false,
            keep_closed_days: 14,
            api_url: "https://api.github.com".into(),
            client_id: String::new(),
        }
    }
}

/// OAuth app used for "Sign in with GitHub" (device flow). Can be baked in at build time
/// with `PR2MD_CLIENT_ID=... cargo build --release` or set as `client_id` in the config.
pub const BUILTIN_CLIENT_ID: Option<&str> = option_env!("PR2MD_CLIENT_ID");

impl Config {
    pub fn path() -> PathBuf {
        util::config_path()
    }

    pub fn exists() -> bool {
        Self::path().is_file()
    }

    pub fn load() -> Res<Config> {
        let path = Self::path();
        if !path.is_file() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)?;
        let mut cfg: Config =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        cfg.normalize();
        Ok(cfg)
    }

    fn normalize(&mut self) {
        let clean = |v: &mut Vec<String>| {
            for s in v.iter_mut() {
                *s = s.trim().to_string();
            }
            v.retain(|s| !s.is_empty());
            v.dedup();
        };
        clean(&mut self.queries);
        clean(&mut self.repos);
        clean(&mut self.exclude_repos);
        // Accept full URLs in the repo lists: https://github.com/owner/repo -> owner/repo
        for r in self.repos.iter_mut().chain(self.exclude_repos.iter_mut()) {
            *r = normalize_repo(r);
        }
        self.interval_minutes = self.interval_minutes.max(1);
        self.folder = self.folder.trim().trim_matches(['/', '\\']).to_string();
        if self.folder.is_empty() {
            self.folder = "GitHub".into();
        }
        self.api_url = self.api_url.trim().trim_end_matches('/').to_string();
    }

    pub fn client_id(&self) -> Option<String> {
        let id = self.client_id.trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
        BUILTIN_CLIENT_ID
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// All search queries: the custom ones plus one per followed repository.
    pub fn all_queries(&self) -> Vec<String> {
        let mut q = self.queries.clone();
        q.extend(self.repos.iter().map(|r| format!("is:pr is:open repo:{r}")));
        q
    }

    pub fn is_excluded(&self, repo: &str) -> bool {
        self.exclude_repos.iter().any(|e| {
            let (e, repo) = (e.to_lowercase(), repo.to_lowercase());
            match e.strip_suffix('*') {
                Some(prefix) => repo.starts_with(prefix),
                None => e == repo,
            }
        })
    }

    /// Root folder for generated notes inside the vault.
    pub fn out_dir(&self) -> PathBuf {
        PathBuf::from(&self.vault).join(&self.folder)
    }

    /// `https://api.github.com` -> `https://github.com`, `https://ghe/api/v3` -> `https://ghe`.
    pub fn web_url(&self) -> String {
        let api = self.api_url.trim_end_matches('/');
        if api == "https://api.github.com" {
            "https://github.com".into()
        } else {
            api.trim_end_matches("/api/v3").to_string()
        }
    }

    pub fn graphql_url(&self) -> String {
        let api = self.api_url.trim_end_matches('/');
        match api.strip_suffix("/v3") {
            Some(base) => format!("{base}/graphql"),
            None => format!("{api}/graphql"),
        }
    }

    /// Problems that stop a sync, in plain words. Empty = good to go.
    pub fn problems(&self) -> Vec<String> {
        let mut p = vec![];
        if self.vault.trim().is_empty() {
            p.push("Choose your Obsidian vault folder.".into());
        } else if !PathBuf::from(&self.vault).is_dir() {
            p.push(format!("Vault folder does not exist: {}", self.vault));
        }
        if self.all_queries().is_empty() {
            p.push("Add at least one search or repository to follow.".into());
        }
        p
    }

    pub fn save(&self) -> Res<()> {
        let s = |v: &str| serde_json::to_string(v).unwrap(); // JSON strings are valid TOML strings
        let list = |v: &[String]| {
            if v.is_empty() {
                "[]".to_string()
            } else {
                format!(
                    "[\n{}\n]",
                    v.iter()
                        .map(|x| format!("  {},", s(x)))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            }
        };
        let text = format!(
            r#"# pr2md configuration - edit here or via "Settings..." in the tray menu.

# Your Obsidian vault and the folder inside it where notes are written.
vault = {vault}
folder = {folder}

# How often to check GitHub, in minutes.
interval_minutes = {interval}

# GitHub searches for pull requests to follow. "@me" means you.
# Syntax: https://docs.github.com/search-github/searching-on-github/searching-issues-and-pull-requests
queries = {queries}

# Follow every open pull request in these repositories ("owner/repo").
repos = {repos}

# Never show pull requests from these repositories ("owner/repo" or "owner/*").
exclude_repos = {exclude}

# Desktop notifications for new comments, reviews and state changes.
notifications = {notify}
notify_own_activity = {own}

# Keep merged / closed pull requests on the dashboard for this many days.
keep_closed_days = {keep}

# GitHub Enterprise: "https://github.example.com/api/v3"
api_url = {api}

# OAuth app client id for "Sign in with GitHub" (leave empty to use the built-in one).
client_id = {client}
"#,
            vault = s(&self.vault),
            folder = s(&self.folder),
            interval = self.interval_minutes,
            queries = list(&self.queries),
            repos = list(&self.repos),
            exclude = list(&self.exclude_repos),
            notify = self.notifications,
            own = self.notify_own_activity,
            keep = self.keep_closed_days,
            api = s(&self.api_url),
            client = s(&self.client_id),
        );
        util::write_atomic(&Self::path(), text.as_bytes())?;
        Ok(())
    }
}

pub fn normalize_repo(r: &str) -> String {
    let r = r.trim().trim_end_matches('/').trim_end_matches(".git");
    let r = r.split_once("://").map(|(_, rest)| rest).unwrap_or(r);
    let parts: Vec<&str> = r.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3 && parts[0].contains('.') {
        // host/owner/repo[/...]
        return format!("{}/{}", parts[1], parts[2]);
    }
    if parts.len() >= 2 {
        return format!("{}/{}", parts[0], parts[1]);
    }
    r.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut c = Config::default();
        c.vault = r#"C:\Users\me\Vault "x""#.into();
        c.repos = vec!["a/b".into()];
        let dir = std::env::temp_dir().join(format!("pr2md-test-{}", std::process::id()));
        unsafe { std::env::set_var("PR2MD_HOME", &dir) };
        c.save().unwrap();
        assert_eq!(Config::load().unwrap(), c);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_urls() {
        assert_eq!(
            normalize_repo("https://github.com/rust-lang/rust/pulls"),
            "rust-lang/rust"
        );
        assert_eq!(normalize_repo("rust-lang/rust.git"), "rust-lang/rust");
        assert_eq!(normalize_repo("owner/*"), "owner/*");
    }

    #[test]
    fn excludes() {
        let c = Config {
            exclude_repos: vec!["Org/*".into(), "a/noisy-*".into(), "x/y".into()],
            ..Default::default()
        };
        assert!(c.is_excluded("org/repo"));
        assert!(c.is_excluded("a/noisy-bot"));
        assert!(!c.is_excluded("a/quiet"));
        assert!(c.is_excluded("X/Y"));
        assert!(!c.is_excluded("x/yz"));
    }

    #[test]
    fn urls() {
        let mut c = Config::default();
        assert_eq!(c.graphql_url(), "https://api.github.com/graphql");
        assert_eq!(c.web_url(), "https://github.com");
        c.api_url = "https://ghe.corp/api/v3".into();
        assert_eq!(c.graphql_url(), "https://ghe.corp/api/graphql");
        assert_eq!(c.web_url(), "https://ghe.corp");
    }
}
