//! Markdown rendering, tuned for Obsidian: properties (front matter), callouts, wikilinks,
//! tags, and checkboxes with block ids (`^t123`) so ticks survive re-rendering.

use crate::config::Config;
use crate::github::{Pr, Repo, Thread};
use crate::state::{State, Tracked};
use crate::util::{fmt_date, fmt_time, parse_ts};
use std::collections::BTreeSet;
use std::fmt::Write;

/// Everything the renderers need besides the PR itself.
pub struct Ctx<'a> {
    pub cfg: &'a Config,
    pub me: &'a str,
    pub now: i64,
    /// Ids the user ticked locally in Obsidian.
    pub local_done: &'a BTreeSet<String>,
}

const NEW_WINDOW_SECS: i64 = 24 * 3600;

// --------------------------------------------------------------------------- paths & links

pub fn safe_name(text: &str, limit: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*#^[]".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = collapsed.chars().take(limit).collect();
    cut.trim_matches([' ', '.']).to_string()
}

fn repo_dir(cfg: &Config, repo: &str) -> String {
    let (owner, name) = repo.split_once('/').unwrap_or((repo, repo));
    format!(
        "{}/{}/{}",
        cfg.folder.replace('\\', "/"),
        safe_name(owner, 60),
        safe_name(name, 80)
    )
}

pub fn note_file(cfg: &Config, pr: &Pr) -> String {
    format!(
        "{}/{} - {}.md",
        repo_dir(cfg, &pr.repo),
        pr.number,
        safe_name(&pr.title, 80)
    )
}

pub fn repo_file(cfg: &Config, repo: &str) -> String {
    let name = repo.split_once('/').map_or(repo, |(_, n)| n);
    format!("{}/{}.md", repo_dir(cfg, repo), safe_name(name, 80))
}

pub fn dashboard_file(cfg: &Config) -> String {
    format!("{}/Dashboard.md", cfg.folder.replace('\\', "/"))
}

pub fn activity_file(cfg: &Config) -> String {
    format!("{}/Activity.md", cfg.folder.replace('\\', "/"))
}

/// `[[path/without/ext|text]]`
fn wikilink(file: &str, text: &str) -> String {
    let text = text.replace(['[', ']'], "").replace('|', "/");
    format!("[[{}|{}]]", file.trim_end_matches(".md"), text)
}

/// Same as [`wikilink`] but safe inside a table cell.
fn wikilink_cell(file: &str, text: &str) -> String {
    wikilink(file, text).replace('|', "\\|")
}

fn user_link(cfg: &Config, login: &str) -> String {
    if login.ends_with("[bot]") || login == "ghost" {
        format!("@{login}")
    } else {
        format!("[@{login}]({}/{login})", cfg.web_url())
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn yaml(v: impl serde::Serialize) -> String {
    serde_json::to_string(&v).unwrap_or_default() // JSON is valid YAML
}

fn local_iso(iso: &str) -> String {
    parse_ts(iso)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%dT%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

// --------------------------------------------------------------------------- text helpers

/// Remove HTML comments (PR templates, bot metadata).
fn strip_html_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// First meaningful line of a comment, shortened - for one-line to-do items.
pub fn snippet(body: &str, max: usize) -> String {
    let body = strip_html_comments(body);
    let mut in_code = false;
    let mut suggestion = false;
    let mut first_code: Option<String> = None;
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_code = !in_code;
            suggestion |= t == "```suggestion";
            continue;
        }
        if in_code && first_code.is_none() && !t.is_empty() {
            first_code = Some(t.replace('`', "'"));
        }
        if in_code
            || t.is_empty()
            || t.starts_with('<')
            || t.starts_with('|')
            || t.starts_with("---")
            || t.starts_with('>')
        {
            continue;
        }
        let t = t.trim_start_matches(['#', '*', '-', ' ']);
        let t = t
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace(['[', ']'], "");
        if t.is_empty() {
            continue;
        }
        return shorten(&t, max);
    }
    match first_code {
        Some(code) if suggestion => format!("💡 suggestion: `{}`", shorten(&code, max)),
        Some(code) => format!("`{}`", shorten(&code, max)),
        None if suggestion => "💡 suggestion: delete lines".into(),
        None => String::new(),
    }
}

fn shorten(t: &str, max: usize) -> String {
    if t.chars().count() > max {
        format!("{}…", t.chars().take(max).collect::<String>().trim_end())
    } else {
        t.to_string()
    }
}

/// Make GitHub markdown sit nicely inside a note: demote headings so they don't fight ours.
fn demote_headings(body: &str) -> String {
    let body = strip_html_comments(body);
    let mut in_code = false;
    let mut out = vec![];
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            if in_code && line.trim() == "```suggestion" {
                out.push("💡 **Suggested change:**".to_string());
                out.push("```".to_string());
                continue;
            }
        }
        if !in_code && line.starts_with('#') {
            let level = line.chars().take_while(|c| *c == '#').count();
            if level <= 6 && line[level..].starts_with(' ') {
                out.push(format!(
                    "{}{}",
                    "#".repeat((level + 2).min(6)),
                    &line[level..]
                ));
                continue;
            }
        }
        out.push(line.to_string());
    }
    out.join("\n").trim().to_string()
}

fn quote(body: &str) -> String {
    let body = demote_headings(body);
    if body.is_empty() {
        return String::new();
    }
    body.lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn mentions(body: &str, me: &str) -> bool {
    let needle = format!("@{}", me.to_lowercase());
    let lower = body.to_lowercase();
    lower.match_indices(&needle).any(|(i, _)| {
        let after = lower[i + needle.len()..].chars().next();
        !after.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_')
    })
}

// --------------------------------------------------------------------------- badges

pub fn state_icon(pr: &Pr) -> &'static str {
    match pr.state.as_str() {
        "draft" => "⚪",
        "merged" => "🟣",
        "closed" => "🔴",
        _ => "🟢",
    }
}

fn state_label(pr: &Pr) -> &'static str {
    match pr.state.as_str() {
        "draft" => "Draft",
        "merged" => "Merged",
        "closed" => "Closed",
        _ => "Open",
    }
}

fn review_badge(pr: &Pr) -> &'static str {
    match pr.review_decision.as_deref() {
        Some("APPROVED") => "✅ Approved",
        Some("CHANGES_REQUESTED") => "🛠️ Changes requested",
        Some("REVIEW_REQUIRED") => "⏳ Review required",
        _ => "—",
    }
}

fn checks_badge(pr: &Pr) -> &'static str {
    match pr.checks.as_deref() {
        Some("SUCCESS") => "✔️ Passing",
        Some("FAILURE" | "ERROR") => "❌ Failing",
        Some("PENDING" | "EXPECTED") => "⏳ Running",
        _ => "—",
    }
}

fn merge_badge(pr: &Pr) -> &'static str {
    match pr.mergeable.as_str() {
        "CONFLICTING" => "⚔️ Conflicts",
        "MERGEABLE" => "✅ Clean",
        _ => "—",
    }
}

// --------------------------------------------------------------------------- to-dos

#[derive(Debug, Clone)]
pub struct Todo {
    /// Obsidian block id - stable across syncs.
    pub id: String,
    pub text: String,
    /// Done according to GitHub (thread resolved, re-approved, ...).
    pub github_done: bool,
    /// Something *you* should act on.
    pub mine: bool,
    pub kind: TodoKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TodoKind {
    ReviewRequest,
    ChangesRequested,
    Thread,
    Checks,
    Conflict,
    Mention,
}

impl Todo {
    pub fn checked(&self, done: &BTreeSet<String>) -> bool {
        self.github_done || done.contains(&self.id)
    }
}

pub fn todos(pr: &Pr, me: &str) -> Vec<Todo> {
    let is_me = |l: &str| l.eq_ignore_ascii_case(me);
    let mine_pr = is_me(&pr.author);
    let open = pr.is_open();
    let key_slug = slug(&format!("{}-{}", pr.repo, pr.number));
    let mut out = vec![];

    if open && pr.requested.iter().any(|r| is_me(r)) {
        out.push(Todo {
            id: format!("rq-{key_slug}"),
            text: format!(
                "👀 **Review requested** from you · [Review on GitHub]({}/files)",
                pr.url
            ),
            github_done: false,
            mine: true,
            kind: TodoKind::ReviewRequest,
        });
    }

    // Changes requested: done once that reviewer's latest verdict is no longer "changes requested".
    for r in pr.reviews.iter().filter(|r| r.state == "CHANGES_REQUESTED") {
        let latest = pr
            .reviews
            .iter()
            .filter(|x| {
                x.author == r.author
                    && matches!(
                        x.state.as_str(),
                        "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED"
                    )
            })
            .max_by(|a, b| a.submitted_at.cmp(&b.submitted_at));
        let superseded = latest.is_some_and(|l| l.id != r.id);
        let text = snippet(&r.body, 120);
        out.push(Todo {
            id: format!("r{}", r.id),
            text: format!(
                "🛠️ **@{}** requested changes{} · [↗]({})",
                r.author,
                if text.is_empty() {
                    String::new()
                } else {
                    format!(": {text}")
                },
                r.url
            ),
            github_done: superseded || !open,
            mine: mine_pr && !is_me(&r.author),
            kind: TodoKind::ChangesRequested,
        });
    }

    let mut threads: Vec<&Thread> = pr.threads.iter().collect();
    threads.sort_by_key(|t| t.outdated);
    for t in threads {
        let first = &t.comments[0];
        let last = t.comments.last().unwrap();
        let involved = t
            .comments
            .iter()
            .any(|c| is_me(&c.author) || mentions(&c.body, me));
        let replies = t.comments.len() - 1;
        let mut text = format!(
            "💬 **@{}** · `{}{}` — {}",
            first.author,
            t.path,
            t.line.map(|l| format!(":{l}")).unwrap_or_default(),
            snippet(&first.body, 120)
        );
        if replies > 0 {
            let _ = write!(
                text,
                " · {replies} {}",
                if replies == 1 { "reply" } else { "replies" }
            );
            if last.author != first.author {
                let _ = write!(text, ", last by **@{}**", last.author);
            }
        }
        if t.outdated {
            text.push_str(" · _outdated_");
        }
        let _ = write!(text, " · [↗]({})", last.url);
        out.push(Todo {
            id: format!("t{}", Pr::thread_id(t)),
            text,
            github_done: t.resolved || !open,
            mine: !is_me(&last.author) && (mine_pr || involved),
            kind: TodoKind::Thread,
        });
    }

    if open && mine_pr && matches!(pr.checks.as_deref(), Some("FAILURE" | "ERROR")) {
        let names: Vec<String> = pr
            .failing_checks
            .iter()
            .take(5)
            .map(|c| format!("[{}]({})", c.name, c.url))
            .collect();
        out.push(Todo {
            id: format!("ci-{key_slug}"),
            text: format!(
                "❌ **Checks failing**: {}",
                if names.is_empty() {
                    "see GitHub".into()
                } else {
                    names.join(", ")
                }
            ),
            github_done: false,
            mine: true,
            kind: TodoKind::Checks,
        });
    }
    if open && mine_pr && pr.mergeable == "CONFLICTING" {
        out.push(Todo {
            id: format!("mc-{key_slug}"),
            text: format!("⚔️ **Merge conflicts** with `{}`", pr.base),
            github_done: false,
            mine: true,
            kind: TodoKind::Conflict,
        });
    }

    // Mentions in the conversation that you haven't answered yet.
    for c in pr
        .comments
        .iter()
        .filter(|c| !is_me(&c.author) && mentions(&c.body, me))
    {
        let answered = pr
            .comments
            .iter()
            .any(|x| is_me(&x.author) && x.created_at > c.created_at)
            || pr
                .reviews
                .iter()
                .any(|x| is_me(&x.author) && x.submitted_at > c.created_at);
        out.push(Todo {
            id: format!("m{}", c.id),
            text: format!(
                "📣 **@{}** mentioned you: {} · [↗]({})",
                c.author,
                snippet(&c.body, 120),
                c.url
            ),
            github_done: answered || !open,
            mine: true,
            kind: TodoKind::Mention,
        });
    }
    out
}

fn checkbox(todo: &Todo, done: &BTreeSet<String>) -> String {
    format!(
        "- [{}] {} ^{}",
        if todo.checked(done) { "x" } else { " " },
        todo.text,
        todo.id
    )
}

/// Parse `- [x] ... ^id` lines from an existing note: id -> ticked.
pub fn read_ticks(text: &str) -> Vec<(String, bool)> {
    text.lines()
        .filter_map(|line| {
            let t = line.trim_start();
            let rest = t.strip_prefix("- [").or_else(|| t.strip_prefix("* ["))?;
            let mark = rest.chars().next()?;
            let (_, id) = t.trim_end().rsplit_once(" ^")?;
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return None;
            }
            Some((id.to_string(), mark != ' '))
        })
        .collect()
}

// --------------------------------------------------------------------------- PR note

fn is_new(t: &Tracked, id: &str, now: i64) -> bool {
    t.first_seen
        .get(id)
        .is_some_and(|&ts| ts > 0 && now - ts < NEW_WINDOW_SECS)
}

pub fn pr_note(ctx: &Ctx, t: &Tracked, repo: Option<&Repo>) -> (String, Vec<Todo>) {
    let cfg = ctx.cfg;
    let pr = &t.pr;
    let todos = todos(pr, ctx.me);
    let mut o = String::new();
    let open_threads = pr.threads.iter().filter(|t| !t.resolved).count();

    // Properties
    o.push_str("---\n");
    let props: Vec<(&str, String)> = vec![
        ("type", yaml("pull-request")),
        ("repo", yaml(&pr.repo)),
        ("number", pr.number.to_string()),
        ("title", yaml(&pr.title)),
        ("state", yaml(&pr.state)),
        ("author", yaml(&pr.author)),
        ("url", yaml(&pr.url)),
        ("base", yaml(&pr.base)),
        ("head", yaml(&pr.head)),
        (
            "review",
            yaml(pr.review_decision.as_deref().unwrap_or("").to_lowercase()),
        ),
        (
            "checks",
            yaml(pr.checks.as_deref().unwrap_or("").to_lowercase()),
        ),
        ("mergeable", yaml(pr.mergeable.to_lowercase())),
        ("labels", yaml(&pr.labels)),
        ("assignees", yaml(&pr.assignees)),
        ("reviewers", yaml(&pr.requested)),
        ("milestone", yaml(pr.milestone.as_deref().unwrap_or(""))),
        ("open_threads", open_threads.to_string()),
        (
            "todo",
            todos
                .iter()
                .filter(|x| x.mine && !x.checked(ctx.local_done))
                .count()
                .to_string(),
        ),
        ("created", yaml(local_iso(&pr.created_at))),
        ("updated", yaml(local_iso(&pr.updated_at))),
        (
            "closed",
            yaml(pr.closed_at.as_deref().map(local_iso).unwrap_or_default()),
        ),
        ("aliases", yaml([pr.key()])),
        (
            "tags",
            yaml([
                "pr".to_string(),
                format!("pr/{}", pr.state),
                format!("repo/{}", slug(&pr.repo)),
            ]),
        ),
    ];
    for (k, v) in props {
        let _ = writeln!(o, "{k}: {v}");
    }
    o.push_str("---\n\n");

    // Header
    let _ = writeln!(o, "# {}\n", pr.title);
    let _ = writeln!(
        o,
        "> [!abstract] [{}]({}) · {} {} · by {}",
        pr.key(),
        pr.url,
        state_icon(pr),
        state_label(pr),
        user_link(cfg, &pr.author)
    );
    let _ = writeln!(
        o,
        "> `{}` → `{}` · **+{} / −{}** in {} files · {} commits · opened {}{}",
        pr.head,
        pr.base,
        pr.additions,
        pr.deletions,
        pr.changed_files,
        pr.commits,
        fmt_date(&pr.created_at),
        pr.closed_at
            .as_deref()
            .map(|c| format!(" · {} {}", state_label(pr).to_lowercase(), fmt_date(c)))
            .unwrap_or_default()
    );
    let _ = writeln!(
        o,
        "> Review: {} · Checks: {} · Merge: {}",
        review_badge(pr),
        checks_badge(pr),
        merge_badge(pr)
    );
    let mut meta = vec![];
    if !pr.labels.is_empty() {
        meta.push(format!(
            "Labels: {}",
            pr.labels
                .iter()
                .map(|l| format!("`{l}`"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    if !pr.requested.is_empty() {
        meta.push(format!(
            "Waiting for: {}",
            pr.requested
                .iter()
                .map(|r| format!("@{r}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !pr.assignees.is_empty() {
        meta.push(format!(
            "Assignees: {}",
            pr.assignees
                .iter()
                .map(|r| format!("@{r}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(m) = &pr.milestone {
        meta.push(format!("Milestone: {m}"));
    }
    if !meta.is_empty() {
        let _ = writeln!(o, "> {}", meta.join(" · "));
    }
    let repo_name = repo.map_or(pr.repo.as_str(), |r| r.name.as_str());
    let _ = writeln!(
        o,
        "> 📦 {} · [Files]({u}/files) · [Commits]({u}/commits) · [Checks]({u}/checks) · {}\n",
        wikilink(&repo_file(cfg, &pr.repo), repo_name),
        wikilink(&dashboard_file(cfg), "Dashboard"),
        u = pr.url,
    );
    if pr.truncated {
        o.push_str("> [!warning] Large pull request: only the latest 100 comments / reviews / threads are shown.\n\n");
    }

    // To-dos
    o.push_str("## ✅ To-do\n\n");
    if todos.is_empty() {
        o.push_str("_Nothing to do._\n");
    } else {
        let (open_items, done_items): (Vec<_>, Vec<_>) =
            todos.iter().partition(|x| !x.checked(ctx.local_done));
        for x in open_items.iter().chain(done_items.iter()) {
            let _ = writeln!(o, "{}", checkbox(x, ctx.local_done));
        }
    }
    o.push('\n');

    if !pr.failing_checks.is_empty() {
        o.push_str("## ❌ Failing checks\n\n");
        for c in &pr.failing_checks {
            let _ = writeln!(o, "- [{}]({})", c.name, c.url);
        }
        o.push('\n');
    }

    // Description
    o.push_str("## 📝 Description\n\n");
    let body = demote_headings(&pr.body);
    o.push_str(if body.is_empty() {
        "_No description._"
    } else {
        &body
    });
    o.push_str("\n\n");

    // Conversation: comments + reviews, oldest first
    enum Ev<'a> {
        C(&'a crate::github::Comment),
        R(&'a crate::github::Review),
    }
    let mut events: Vec<(&str, Ev)> = pr
        .comments
        .iter()
        .map(|c| (c.created_at.as_str(), Ev::C(c)))
        .collect();
    events.extend(
        pr.reviews
            .iter()
            .filter(|r| r.state != "COMMENTED" || !r.body.is_empty())
            .map(|r| (r.submitted_at.as_str(), Ev::R(r))),
    );
    events.sort_by(|a, b| a.0.cmp(b.0));
    o.push_str("## 💬 Conversation\n\n");
    if events.is_empty() {
        o.push_str("_No comments yet._\n\n");
    }
    for (_, ev) in events {
        let (kind, fold, title, body) = match ev {
            Ev::C(c) => {
                let new = if is_new(t, &format!("c{}", c.id), ctx.now) {
                    " 🆕"
                } else {
                    ""
                };
                let bot = c.author.ends_with("[bot]");
                (
                    "quote",
                    bot,
                    format!(
                        "💬 **{}** · {} · [↗]({}){new}",
                        user_link(cfg, &c.author),
                        fmt_time(&c.created_at),
                        c.url
                    ),
                    &c.body,
                )
            }
            Ev::R(r) => {
                let new = if is_new(t, &format!("r{}", r.id), ctx.now) {
                    " 🆕"
                } else {
                    ""
                };
                let (kind, verb, fold) = match r.state.as_str() {
                    "APPROVED" => ("success", "✅ approved", false),
                    "CHANGES_REQUESTED" => ("failure", "🛠️ requested changes", false),
                    "DISMISSED" => ("quote", "🚫 review dismissed", true),
                    _ => ("note", "📝 reviewed", false),
                };
                (
                    kind,
                    fold,
                    format!(
                        "**{}** {verb} · {} · [↗]({}){new}",
                        user_link(cfg, &r.author),
                        fmt_time(&r.submitted_at),
                        r.url
                    ),
                    &r.body,
                )
            }
        };
        let _ = writeln!(o, "> [!{kind}]{} {title}", if fold { "-" } else { "" });
        let q = quote(body);
        if !q.is_empty() {
            let _ = writeln!(o, "{q}");
        }
        o.push('\n');
    }

    // Review threads: open first
    if !pr.threads.is_empty() {
        o.push_str("## 🧵 Review threads\n\n");
        let mut threads: Vec<&Thread> = pr.threads.iter().collect();
        threads.sort_by_key(|t| t.resolved);
        for th in threads {
            let loc = format!(
                "`{}{}`",
                th.path,
                th.line.map(|l| format!(":{l}")).unwrap_or_default()
            );
            let (kind, fold, status) = if th.resolved {
                ("success", "-", "✅ resolved")
            } else if th.outdated {
                ("question", "", "🔴 open · outdated")
            } else {
                ("question", "", "🔴 open")
            };
            let _ = writeln!(
                o,
                "> [!{kind}]{fold} {loc} · {status} · {} comment(s)",
                th.comments.len()
            );
            if !th.diff_hunk.is_empty() {
                o.push_str("> ```diff\n");
                let lines: Vec<&str> = th.diff_hunk.lines().collect();
                for l in &lines[lines.len().saturating_sub(6)..] {
                    let _ = writeln!(o, "> {l}");
                }
                o.push_str("> ```\n");
            }
            for c in &th.comments {
                let new = if is_new(t, &format!("t{}", c.id), ctx.now) {
                    " 🆕"
                } else {
                    ""
                };
                let _ = writeln!(
                    o,
                    ">\n> **{}** · {} · [↗]({}){new}",
                    user_link(cfg, &c.author),
                    fmt_time(&c.created_at),
                    c.url
                );
                let q = quote(&c.body);
                if !q.is_empty() {
                    // nested quote inside the callout
                    for l in q.lines() {
                        let _ = writeln!(o, "> {l}");
                    }
                }
            }
            o.push('\n');
        }
    }

    let _ = writeln!(
        o,
        "---\n_Generated by pr2md from GitHub ({}). Edits are overwritten on the next sync — ticked checkboxes are kept._",
        fmt_time(&pr.updated_at)
    );
    (o, todos)
}

// --------------------------------------------------------------------------- repo note

pub fn repo_note(ctx: &Ctx, repo: &Repo, prs: &[&Tracked]) -> String {
    let cfg = ctx.cfg;
    let mut o = String::new();
    let _ = writeln!(
        o,
        "---\ntype: repository\nrepo: {}\nurl: {}\nlanguage: {}\nstars: {}\nprivate: {}\ntags: {}\n---\n",
        yaml(&repo.name),
        yaml(&repo.url),
        yaml(repo.language.as_deref().unwrap_or("")),
        repo.stars,
        repo.private,
        yaml([
            "pr2md/repo".to_string(),
            format!("repo/{}", slug(&repo.name))
        ])
    );
    let _ = writeln!(o, "# 📦 {}\n", repo.name);
    let _ = writeln!(
        o,
        "> [!abstract] {}",
        if repo.description.is_empty() {
            "_No description._"
        } else {
            &repo.description
        }
    );
    let _ = writeln!(o, "> {}", repo_meta(repo));
    let _ = writeln!(
        o,
        "> [GitHub]({u}) · [Pull requests]({u}/pulls) · [Issues]({u}/issues) · [Actions]({u}/actions) · {}",
        wikilink(&dashboard_file(cfg), "Dashboard"),
        u = repo.url
    );
    if !repo.topics.is_empty() {
        let _ = writeln!(
            o,
            "> Topics: {}",
            repo.topics
                .iter()
                .map(|t| format!("`{t}`"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    o.push('\n');

    let (open, closed): (Vec<&&Tracked>, Vec<&&Tracked>) = prs.iter().partition(|t| t.pr.is_open());
    o.push_str("## Open pull requests\n\n");
    if open.is_empty() {
        o.push_str("_None tracked._\n");
    } else {
        o.push_str("| | PR | Author | Review | Checks | To-do | Updated |\n|---|---|---|---|---|---|---|\n");
        for t in open {
            let todo = todos(&t.pr, ctx.me)
                .iter()
                .filter(|x| x.mine && !x.checked(ctx.local_done))
                .count();
            let _ = writeln!(
                o,
                "| {} | {} | @{} | {} | {} | {} | {} |",
                state_icon(&t.pr),
                wikilink_cell(&t.file, &format!("#{} {}", t.pr.number, t.pr.title)),
                t.pr.author,
                review_badge(&t.pr),
                checks_badge(&t.pr),
                if todo > 0 {
                    todo.to_string()
                } else {
                    String::new()
                },
                fmt_date(&t.pr.updated_at)
            );
        }
    }
    if !closed.is_empty() {
        o.push_str("\n## Recently closed\n\n| | PR | Author | Closed |\n|---|---|---|---|\n");
        for t in closed {
            let _ = writeln!(
                o,
                "| {} | {} | @{} | {} |",
                state_icon(&t.pr),
                wikilink_cell(&t.file, &format!("#{} {}", t.pr.number, t.pr.title)),
                t.pr.author,
                t.pr.closed_at.as_deref().map(fmt_date).unwrap_or_default()
            );
        }
    }
    o
}

fn repo_meta(repo: &Repo) -> String {
    let mut m = vec![if repo.private {
        "🔒 Private".to_string()
    } else {
        "🌐 Public".to_string()
    }];
    if repo.archived {
        m.push("🗄️ Archived".into());
    }
    if let Some(l) = &repo.language {
        m.push(l.clone());
    }
    m.push(format!("⭐ {}", repo.stars));
    m.push(format!("🍴 {}", repo.forks));
    if !repo.default_branch.is_empty() {
        m.push(format!("`{}`", repo.default_branch));
    }
    m.push(format!("{} open PRs", repo.open_prs));
    m.push(format!("{} open issues", repo.open_issues));
    if !repo.pushed_at.is_empty() {
        m.push(format!("last push {}", fmt_date(&repo.pushed_at)));
    }
    m.join(" · ")
}

// --------------------------------------------------------------------------- dashboard

pub fn dashboard(ctx: &Ctx, state: &State) -> (String, Vec<Todo>, usize) {
    let cfg = ctx.cfg;
    let mut o = String::new();
    let open: Vec<&Tracked> = state
        .prs
        .values()
        .filter(|t| !t.archived && t.pr.is_open())
        .collect();
    let mut repos: Vec<&str> = open.iter().map(|t| t.pr.repo.as_str()).collect();
    repos.sort_by_key(|r| r.to_lowercase());
    repos.dedup();

    // Per PR: the to-dos that are yours and still open.
    let mut all_todos = vec![];
    let my_todos = |t: &Tracked| -> Vec<Todo> {
        todos(&t.pr, ctx.me)
            .into_iter()
            .filter(|x| x.mine && !x.checked(ctx.local_done))
            .collect()
    };
    let todo_total: usize = open.iter().map(|t| my_todos(t).len()).sum();

    let _ = writeln!(
        o,
        "---\ntype: pr2md-dashboard\ntodo: {todo_total}\nopen_prs: {}\ntags: [\"pr2md\"]\n---\n",
        open.len()
    );
    o.push_str("# 🔀 Pull Requests\n\n");
    let _ = writeln!(
        o,
        "> [!info] Synced **{}** as {}\n> **{}** open PRs in **{}** repos · **{}** to-dos · {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        user_link(cfg, ctx.me),
        open.len(),
        repos.len(),
        todo_total,
        wikilink(&activity_file(cfg), "Activity log")
    );

    // Needs attention: one line per PR, most urgent first (no checkboxes - those live below).
    let mut attention: Vec<(&Tracked, Vec<Todo>)> = open
        .iter()
        .map(|t| (*t, my_todos(t)))
        .filter(|(_, x)| !x.is_empty())
        .collect();
    attention.sort_by_key(|(t, x)| {
        (
            !x.iter().any(|x| x.kind == TodoKind::ReviewRequest),
            std::cmp::Reverse(t.pr.updated_at.clone()),
        )
    });
    o.push_str("## 🎯 Needs your attention\n\n");
    if attention.is_empty() {
        o.push_str("_Nothing waiting for you_ 🎉\n");
    }
    for (t, x) in &attention {
        let count = |k: TodoKind| x.iter().filter(|x| x.kind == k).count();
        let mut why = vec![];
        if count(TodoKind::ReviewRequest) > 0 {
            why.push("👀 review requested".to_string());
        }
        if count(TodoKind::ChangesRequested) > 0 {
            why.push("🛠️ changes requested".into());
        }
        match count(TodoKind::Thread) {
            0 => {}
            1 => why.push("💬 1 open thread".into()),
            n => why.push(format!("💬 {n} open threads")),
        }
        if count(TodoKind::Checks) > 0 {
            why.push("❌ checks failing".into());
        }
        if count(TodoKind::Conflict) > 0 {
            why.push("⚔️ conflicts".into());
        }
        if count(TodoKind::Mention) > 0 {
            why.push("📣 mentioned".into());
        }
        let _ = writeln!(
            o,
            "- {} {} — {}",
            wikilink(&t.file, &t.pr.key()),
            t.pr.title,
            why.join(" · ")
        );
    }
    o.push('\n');

    // Recent activity
    let day_ago = ctx.now - 24 * 3600;
    let recent: Vec<_> = state
        .activity
        .iter()
        .filter(|a| parse_ts(&a.time).is_some_and(|d| d.timestamp() >= day_ago))
        .take(15)
        .collect();
    if !recent.is_empty() {
        o.push_str("## 🔔 Last 24 hours\n\n");
        for a in recent {
            let link = state
                .prs
                .get(&a.key)
                .map(|t| wikilink(&t.file, &a.key))
                .unwrap_or_else(|| a.key.clone());
            let time = parse_ts(&a.time)
                .map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string())
                .unwrap_or_default();
            let _ = writeln!(o, "- {time} · {link} — {}", a.text);
        }
        o.push('\n');
    }

    // Repos -> PRs -> to-dos
    for repo in &repos {
        let meta = state.repos.get(*repo);
        o.push_str(&format!(
            "## 📦 {}\n\n",
            wikilink(&repo_file(cfg, repo), repo)
        ));
        if let Some(r) = meta {
            let _ = writeln!(o, "{} · [GitHub]({})\n", repo_meta(r), r.url);
        }
        let mut prs: Vec<&Tracked> = open
            .iter()
            .copied()
            .filter(|t| t.pr.repo == *repo)
            .collect();
        prs.sort_by_key(|t| {
            (
                my_todos(t).is_empty(),
                std::cmp::Reverse(t.pr.updated_at.clone()),
            )
        });
        for t in prs {
            let pr = &t.pr;
            let role = if pr.author.eq_ignore_ascii_case(ctx.me) {
                " · _yours_"
            } else {
                ""
            };
            let _ = writeln!(
                o,
                "### {} {}\n",
                state_icon(pr),
                wikilink(&t.file, &format!("#{} {}", pr.number, pr.title))
            );
            let comments =
                pr.comments.len() + pr.threads.iter().map(|t| t.comments.len()).sum::<usize>();
            let _ = writeln!(
                o,
                "by @{}{role} · `{}` → `{}` · {} · {} · 💬 {} · updated {} · [↗ GitHub]({})\n",
                pr.author,
                pr.head,
                pr.base,
                review_badge(pr),
                checks_badge(pr),
                comments,
                fmt_time(&pr.updated_at),
                pr.url
            );
            let x = my_todos(t);
            let mut wrote = !x.is_empty();
            const MAX_PER_PR: usize = 12;
            for todo in x.iter().take(MAX_PER_PR) {
                let _ = writeln!(o, "{}", checkbox(todo, ctx.local_done));
            }
            if x.len() > MAX_PER_PR {
                let _ = writeln!(
                    o,
                    "- … {} more in {}",
                    x.len() - MAX_PER_PR,
                    wikilink(&t.file, "the note")
                );
            }
            // fresh conversation comments (not to-dos, but worth seeing)
            for c in pr.comments.iter().filter(|c| {
                is_new(t, &format!("c{}", c.id), ctx.now) && !c.author.eq_ignore_ascii_case(ctx.me)
            }) {
                if x.iter().any(|x| x.id == format!("m{}", c.id)) {
                    continue;
                }
                let _ = writeln!(
                    o,
                    "- 🆕 **@{}**: {} · [↗]({})",
                    c.author,
                    snippet(&c.body, 120),
                    c.url
                );
                wrote = true;
            }
            if wrote {
                o.push('\n');
            }
            all_todos.extend(x);
        }
    }

    // Recently closed
    let mut closed: Vec<&Tracked> = state.prs.values().filter(|t| !t.pr.is_open()).collect();
    closed.sort_by(|a, b| b.pr.closed_at.cmp(&a.pr.closed_at));
    if !closed.is_empty() {
        o.push_str(
            "## 🗄️ Recently closed\n\n| | PR | Repo | Author | Closed |\n|---|---|---|---|---|\n",
        );
        for t in closed {
            let _ = writeln!(
                o,
                "| {} | {} | {} | @{} | {} |",
                state_icon(&t.pr),
                wikilink_cell(&t.file, &format!("#{} {}", t.pr.number, t.pr.title)),
                t.pr.repo,
                t.pr.author,
                t.pr.closed_at.as_deref().map(fmt_date).unwrap_or_default()
            );
        }
        o.push('\n');
    }
    o.push_str("---\n_Generated by pr2md. Tick a checkbox to mark it done — it stays ticked across syncs and disappears from here. Untick it in the PR note to bring it back._\n");
    (o, all_todos, todo_total)
}

// --------------------------------------------------------------------------- activity log

pub fn activity(ctx: &Ctx, state: &State) -> String {
    let mut o =
        String::from("---\ntype: pr2md-activity\ntags: [\"pr2md\"]\n---\n\n# 🔔 Activity\n");
    let mut day = String::new();
    for a in &state.activity {
        let Some(t) = parse_ts(&a.time) else { continue };
        let local = t.with_timezone(&chrono::Local);
        let d = local.format("%Y-%m-%d · %A").to_string();
        if d != day {
            let _ = writeln!(o, "\n## {d}\n");
            day = d;
        }
        let link = state
            .prs
            .get(&a.key)
            .map(|t| wikilink(&t.file, &a.key))
            .unwrap_or_else(|| a.key.clone());
        let _ = writeln!(o, "- {} · {link} — {}", local.format("%H:%M"), a.text);
    }
    if state.activity.is_empty() {
        o.push_str("\n_No activity yet._\n");
    }
    let _ = ctx;
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks() {
        let t = "- [x] done ^t123\n- [ ] open ^rq-a-b-1\n  - [X] nested ^m9\n- [x] no id\n- [ ] weird ^a_b";
        assert_eq!(
            read_ticks(t),
            vec![
                ("t123".into(), true),
                ("rq-a-b-1".into(), false),
                ("m9".into(), true)
            ]
        );
    }

    #[test]
    fn mention_matching() {
        assert!(mentions("hey @Me please", "me"));
        assert!(mentions("@me.", "me"));
        assert!(!mentions("@meow", "me"));
    }

    #[test]
    fn snippets() {
        assert_eq!(
            snippet(
                "<!-- x -->\n```rust\ncode\n```\n## Please [fix] this\nmore",
                50
            ),
            "Please fix this"
        );
        assert_eq!(snippet("aaaa bbbb", 4), "aaaa…");
        assert_eq!(
            snippet(
                "```suggestion
  if (x)
```",
                50
            ),
            "💡 suggestion: `if (x)`"
        );
    }

    #[test]
    fn headings_demoted() {
        assert_eq!(
            demote_headings("# A\n```\n# not\n```\n#tag"),
            "### A\n```\n# not\n```\n#tag"
        );
    }

    #[test]
    fn names() {
        assert_eq!(safe_name("Fix: a/b [wip] #12?", 80), "Fix a b wip 12");
    }
}
