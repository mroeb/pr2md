//! GitHub GraphQL client. One search request finds the PRs, one batched request
//! fetches everything about up to five PRs at once (comments, reviews, threads, checks, repo).

use crate::util::Res;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

// --------------------------------------------------------------------------- model

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Pr {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    /// open | draft | merged | closed
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub base: String,
    pub head: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub commits: u64,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    /// Pending review requests (users and @org/team).
    pub requested: Vec<String>,
    pub milestone: Option<String>,
    /// APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED | null
    pub review_decision: Option<String>,
    /// MERGEABLE | CONFLICTING | UNKNOWN
    pub mergeable: String,
    /// SUCCESS | FAILURE | ERROR | PENDING | EXPECTED | null (no checks)
    pub checks: Option<String>,
    pub failing_checks: Vec<Check>,
    pub comments: Vec<Comment>,
    pub reviews: Vec<Review>,
    pub threads: Vec<Thread>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Comment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Review {
    pub id: u64,
    pub author: String,
    /// APPROVED | CHANGES_REQUESTED | COMMENTED | DISMISSED
    pub state: String,
    pub body: String,
    pub submitted_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Thread {
    pub path: String,
    pub line: Option<u64>,
    pub resolved: bool,
    pub outdated: bool,
    pub diff_hunk: String,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Check {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Repo {
    pub name: String,
    pub url: String,
    pub description: String,
    pub stars: u64,
    pub forks: u64,
    pub language: Option<String>,
    pub default_branch: String,
    pub private: bool,
    pub archived: bool,
    pub open_prs: u64,
    pub open_issues: u64,
    pub pushed_at: String,
    pub topics: Vec<String>,
}

/// A search result: just enough to decide whether the PR needs a full refresh.
#[derive(Debug, Clone)]
pub struct Hit {
    pub repo: String,
    pub number: u64,
    pub fingerprint: String,
}

impl Pr {
    pub fn key(&self) -> String {
        format!("{}#{}", self.repo, self.number)
    }
    pub fn is_open(&self) -> bool {
        self.state == "open" || self.state == "draft"
    }
    pub fn thread_id(t: &Thread) -> u64 {
        t.comments.first().map(|c| c.id).unwrap_or(0)
    }
}

// --------------------------------------------------------------------------- client

pub struct Client {
    agent: ureq::Agent,
    graphql: String,
    token: String,
}

const PR_FIELDS: &str = r#"
fragment R on Repository {
  nameWithOwner url description stargazerCount forkCount isPrivate isArchived pushedAt
  primaryLanguage { name } defaultBranchRef { name }
  repositoryTopics(first: 10) { nodes { topic { name } } }
  pullRequests(states: OPEN) { totalCount } issues(states: OPEN) { totalCount }
}
fragment P on PullRequest {
  number title url body state isDraft createdAt updatedAt closedAt
  author { login } baseRefName headRefName additions deletions changedFiles
  labels(first: 20) { nodes { name } }
  assignees(first: 10) { nodes { login } }
  milestone { title }
  reviewDecision mergeable
  reviewRequests(first: 20) { nodes { requestedReviewer { __typename ... on User { login } ... on Team { combinedSlug } } } }
  commits(last: 1) { totalCount nodes { commit { statusCheckRollup { state contexts(first: 100) { nodes {
    __typename
    ... on CheckRun { name conclusion detailsUrl }
    ... on StatusContext { context state targetUrl }
  } } } } } }
  comments(last: 100) { totalCount nodes { databaseId author { login } body createdAt url } }
  reviews(last: 100) { totalCount nodes { databaseId author { login } state body submittedAt url } }
  reviewThreads(first: 100) { totalCount nodes {
    isResolved isOutdated path line originalLine
    comments(first: 50) { nodes { databaseId author { login } body createdAt url diffHunk } }
  } }
  repository { ...R }
}
"#;

impl Client {
    pub fn new(graphql_url: String, token: String) -> Client {
        Client {
            agent: agent(),
            graphql: graphql_url,
            token,
        }
    }

    pub fn query(&self, query: &str, variables: Value) -> Res<Value> {
        let body = json!({ "query": query, "variables": variables }).to_string();
        let mut resp = self
            .agent
            .post(&self.graphql)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .send(body.as_bytes())?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_string()?;
        match status {
            200 => {}
            401 => crate::bail!("GitHub rejected the token (401). Please sign in again."),
            403 | 429 => crate::bail!(
                "GitHub rate limit or permission problem ({status}): {}",
                short(&text)
            ),
            _ => crate::bail!("GitHub returned HTTP {status}: {}", short(&text)),
        }
        let v: Value = serde_json::from_str(&text)?;
        if let Some(errors) = v.get("errors").and_then(Value::as_array) {
            let msg: Vec<&str> = errors
                .iter()
                .filter_map(|e| e["message"].as_str())
                .collect();
            if v.get("data").is_none_or(Value::is_null) {
                crate::bail!("GitHub GraphQL error: {}", msg.join("; "));
            }
            crate::warn!("GraphQL partial errors: {}", msg.join("; "));
        }
        Ok(v["data"].clone())
    }

    pub fn viewer(&self) -> Res<String> {
        let d = self.query("query { viewer { login } }", json!({}))?;
        Ok(d["viewer"]["login"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// All PRs matching a GitHub search query (max 1000, GitHub's limit).
    pub fn search(&self, q: &str) -> Res<Vec<Hit>> {
        const Q: &str = r#"query($q: String!, $after: String) {
          search(query: $q, type: ISSUE, first: 100, after: $after) {
            pageInfo { hasNextPage endCursor }
            nodes { ... on PullRequest {
              number updatedAt state isDraft reviewDecision mergeable
              repository { nameWithOwner }
              commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
            } }
          }
        }"#;
        let mut hits = vec![];
        let mut after = Value::Null;
        for _ in 0..10 {
            let d = self.query(Q, json!({ "q": q, "after": after }))?;
            let s = &d["search"];
            for n in s["nodes"].as_array().into_iter().flatten() {
                let Some(number) = n["number"].as_u64() else {
                    continue;
                }; // issues come back empty
                let checks = &n["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"];
                hits.push(Hit {
                    repo: str_of(&n["repository"]["nameWithOwner"]),
                    number,
                    fingerprint: format!(
                        "{}|{}|{}|{}|{}|{}",
                        str_of(&n["updatedAt"]),
                        str_of(&n["state"]),
                        n["isDraft"],
                        str_of(&n["reviewDecision"]),
                        str_of(&n["mergeable"]),
                        str_of(checks)
                    ),
                });
            }
            if s["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
                break;
            }
            after = s["pageInfo"]["endCursor"].clone();
        }
        Ok(hits)
    }

    /// Fetch full details for many PRs: batches of 5, up to 4 requests in parallel.
    /// Returns one result per input (None = PR not found / no access).
    pub fn fetch_prs(&self, keys: &[(String, u64)]) -> Vec<Res<Option<(Pr, Repo)>>> {
        let chunks: Vec<&[(String, u64)]> = keys.chunks(5).collect();
        let mut results: Vec<Option<Res<Vec<Option<(Pr, Repo)>>>>> =
            (0..chunks.len()).map(|_| None).collect();
        for (group_idx, group) in chunks.chunks(4).enumerate() {
            std::thread::scope(|scope| {
                let handles: Vec<_> = group
                    .iter()
                    .map(|chunk| scope.spawn(|| self.fetch_batch(chunk)))
                    .collect();
                for (i, h) in handles.into_iter().enumerate() {
                    results[group_idx * 4 + i] = Some(
                        h.join()
                            .unwrap_or_else(|_| Err("fetch thread panicked".into())),
                    );
                }
            });
        }
        let mut out = vec![];
        for (chunk, res) in chunks.iter().zip(results) {
            match res.unwrap() {
                Ok(items) => out.extend(items.into_iter().map(Ok)),
                Err(e) => {
                    let msg = e.to_string();
                    out.extend(chunk.iter().map(|_| Err(msg.clone().into())));
                }
            }
        }
        out
    }

    fn fetch_batch(&self, chunk: &[(String, u64)]) -> Res<Vec<Option<(Pr, Repo)>>> {
        let mut q = String::from("query {\n");
        for (i, (repo, number)) in chunk.iter().enumerate() {
            let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
            q.push_str(&format!(
                "  p{i}: repository(owner: {}, name: {}) {{ pullRequest(number: {number}) {{ ...P }} }}\n",
                serde_json::to_string(owner)?,
                serde_json::to_string(name)?
            ));
        }
        q.push_str("}\n");
        q.push_str(PR_FIELDS);
        let d = self.query(&q, json!({}))?;
        Ok((0..chunk.len())
            .map(|i| {
                let node = &d[format!("p{i}")]["pullRequest"];
                (!node.is_null()).then(|| (parse_pr(node), parse_repo(&node["repository"])))
            })
            .collect())
    }
}

pub fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent(concat!("pr2md/", env!("CARGO_PKG_VERSION")))
        // OS certificate store: works behind corporate TLS-inspecting proxies.
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

fn short(s: &str) -> String {
    s.chars().take(300).collect()
}

// --------------------------------------------------------------------------- parsing

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn login(v: &Value) -> String {
    v["login"].as_str().unwrap_or("ghost").to_string()
}

fn nodes(v: &Value) -> impl Iterator<Item = &Value> {
    v["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| !n.is_null())
}

fn clean(s: &str) -> String {
    s.replace("\r\n", "\n").trim().to_string()
}

fn comment(n: &Value) -> Comment {
    Comment {
        id: n["databaseId"].as_u64().unwrap_or(0),
        author: login(&n["author"]),
        body: clean(n["body"].as_str().unwrap_or_default()),
        created_at: str_of(&n["createdAt"]),
        url: str_of(&n["url"]),
    }
}

pub fn parse_pr(n: &Value) -> Pr {
    let state = match (n["state"].as_str(), n["isDraft"].as_bool()) {
        (Some("MERGED"), _) => "merged",
        (Some("CLOSED"), _) => "closed",
        (_, Some(true)) => "draft",
        _ => "open",
    };
    let commit = &n["commits"]["nodes"][0]["commit"];
    let rollup = &commit["statusCheckRollup"];
    let failing_checks = nodes(&rollup["contexts"])
        .filter_map(|c| match c["__typename"].as_str() {
            Some("CheckRun") => matches!(
                c["conclusion"].as_str(),
                Some("FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE")
            )
            .then(|| Check {
                name: str_of(&c["name"]),
                url: str_of(&c["detailsUrl"]),
            }),
            Some("StatusContext") => {
                matches!(c["state"].as_str(), Some("FAILURE" | "ERROR")).then(|| Check {
                    name: str_of(&c["context"]),
                    url: str_of(&c["targetUrl"]),
                })
            }
            _ => None,
        })
        .collect();
    let threads: Vec<Thread> = nodes(&n["reviewThreads"])
        .map(|t| {
            let comments: Vec<Comment> = nodes(&t["comments"]).map(comment).collect();
            Thread {
                path: str_of(&t["path"]),
                line: t["line"].as_u64().or_else(|| t["originalLine"].as_u64()),
                resolved: t["isResolved"].as_bool().unwrap_or(false),
                outdated: t["isOutdated"].as_bool().unwrap_or(false),
                diff_hunk: nodes(&t["comments"])
                    .next()
                    .map(|c| clean(c["diffHunk"].as_str().unwrap_or_default()))
                    .unwrap_or_default(),
                comments,
            }
        })
        .filter(|t| !t.comments.is_empty())
        .collect();
    let total = |k: &str| n[k]["totalCount"].as_u64().unwrap_or(0);
    let count = |k: &str| n[k]["nodes"].as_array().map_or(0, |a| a.len() as u64);
    Pr {
        repo: str_of(&n["repository"]["nameWithOwner"]),
        number: n["number"].as_u64().unwrap_or(0),
        title: str_of(&n["title"]),
        url: str_of(&n["url"]),
        author: login(&n["author"]),
        state: state.into(),
        created_at: str_of(&n["createdAt"]),
        updated_at: str_of(&n["updatedAt"]),
        closed_at: opt_str(&n["closedAt"]),
        base: str_of(&n["baseRefName"]),
        head: str_of(&n["headRefName"]),
        additions: n["additions"].as_u64().unwrap_or(0),
        deletions: n["deletions"].as_u64().unwrap_or(0),
        changed_files: n["changedFiles"].as_u64().unwrap_or(0),
        commits: n["commits"]["totalCount"].as_u64().unwrap_or(0),
        body: clean(n["body"].as_str().unwrap_or_default()),
        labels: nodes(&n["labels"]).map(|l| str_of(&l["name"])).collect(),
        assignees: nodes(&n["assignees"]).map(login).collect(),
        requested: nodes(&n["reviewRequests"])
            .filter_map(|r| {
                let r = &r["requestedReviewer"];
                opt_str(&r["login"]).or_else(|| opt_str(&r["combinedSlug"]))
            })
            .collect(),
        milestone: opt_str(&n["milestone"]["title"]),
        review_decision: opt_str(&n["reviewDecision"]),
        mergeable: str_of(&n["mergeable"]),
        checks: opt_str(&rollup["state"]),
        failing_checks,
        comments: nodes(&n["comments"]).map(comment).collect(),
        reviews: nodes(&n["reviews"])
            .map(|r| Review {
                id: r["databaseId"].as_u64().unwrap_or(0),
                author: login(&r["author"]),
                state: str_of(&r["state"]),
                body: clean(r["body"].as_str().unwrap_or_default()),
                submitted_at: str_of(&r["submittedAt"]),
                url: str_of(&r["url"]),
            })
            .filter(|r| r.state != "PENDING")
            .collect(),
        truncated: total("comments") > count("comments")
            || total("reviews") > count("reviews")
            || total("reviewThreads") > count("reviewThreads"),
        threads,
    }
}

pub fn parse_repo(n: &Value) -> Repo {
    Repo {
        name: str_of(&n["nameWithOwner"]),
        url: str_of(&n["url"]),
        description: str_of(&n["description"]),
        stars: n["stargazerCount"].as_u64().unwrap_or(0),
        forks: n["forkCount"].as_u64().unwrap_or(0),
        language: opt_str(&n["primaryLanguage"]["name"]),
        default_branch: str_of(&n["defaultBranchRef"]["name"]),
        private: n["isPrivate"].as_bool().unwrap_or(false),
        archived: n["isArchived"].as_bool().unwrap_or(false),
        open_prs: n["pullRequests"]["totalCount"].as_u64().unwrap_or(0),
        open_issues: n["issues"]["totalCount"].as_u64().unwrap_or(0),
        pushed_at: str_of(&n["pushedAt"]),
        topics: nodes(&n["repositoryTopics"])
            .map(|t| str_of(&t["topic"]["name"]))
            .collect(),
    }
}
