//! One sync: search GitHub, refresh changed PRs, write notes, notify.

use crate::config::Config;
use crate::github::{Client, Pr};
use crate::render::{self, Ctx};
use crate::state::{Activity, State, Tracked};
use crate::util::{self, Lock, Res, now_ts};
use crate::{auth, notify};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// Re-fetch open PRs at least this often, even when the search says nothing changed
/// (resolving a review thread does not bump `updatedAt`).
const MAX_AGE_SECS: i64 = 30 * 60;
const MAX_ACTIVITY: usize = 500;

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub open_prs: usize,
    pub todos: usize,
    pub changes: usize,
}

pub struct Options {
    /// Only sync this PR (owner/repo, number) - handy for testing.
    pub single: Option<(String, u64)>,
    pub notify: bool,
}

pub fn run(cfg: &Config, opts: &Options) -> Res<Summary> {
    let result = run_inner(cfg, opts);
    // Remember the outcome for the tray / settings window.
    if let Some(mut st) = State::load() {
        st.last_error = result.as_ref().err().map(|e| e.to_string());
        let _ = st.save();
    } else if let Err(e) = &result {
        let st = State {
            last_error: Some(e.to_string()),
            ..Default::default()
        };
        let _ = st.save();
    }
    match &result {
        Ok(s) => crate::info!(
            "Sync done: {} open PR(s), {} to-do(s), {} change(s)",
            s.open_prs,
            s.todos,
            s.changes
        ),
        Err(e) => crate::warn!("Sync failed: {e}"),
    }
    result
}

fn run_inner(cfg: &Config, opts: &Options) -> Res<Summary> {
    let problems = cfg.problems();
    if !problems.is_empty() {
        crate::bail!("{}", problems.join(" "));
    }
    let Some((token, _)) = auth::token() else {
        crate::bail!("Not signed in to GitHub. Open Settings and sign in (or run `pr2md login`).");
    };
    let _lock = Lock::acquire("sync.lock")?;
    let gh = Client::new(cfg.graphql_url(), token);
    let loaded = State::load();
    let first_run = loaded.as_ref().is_none_or(|s| s.last_sync.is_none());
    let mut state = loaded.unwrap_or_default();
    let now = now_ts();

    // 1. Who am I, and what should be tracked? (all requests in parallel)
    let queries = if opts.single.is_some() {
        vec![]
    } else {
        cfg.all_queries()
    };
    let (me, searches) = std::thread::scope(|s| {
        let me = s.spawn(|| gh.viewer());
        let searches: Vec<_> = queries
            .iter()
            .map(|q| (q, s.spawn(|| gh.search(q))))
            .collect();
        let searches: Vec<_> = searches
            .into_iter()
            .map(|(q, h)| {
                (
                    q,
                    h.join().unwrap_or_else(|_| Err("search panicked".into())),
                )
            })
            .collect();
        (
            me.join().unwrap_or_else(|_| Err("viewer panicked".into())),
            searches,
        )
    });
    let me = me?;
    state.login = me.clone();
    let mut targets: BTreeMap<String, (String, u64, String)> = BTreeMap::new(); // key -> (repo, n, fingerprint)
    if let Some((repo, n)) = &opts.single {
        targets.insert(format!("{repo}#{n}"), (repo.clone(), *n, String::new()));
    }
    for (q, hits) in searches {
        for hit in hits.map_err(|e| format!("Search \"{q}\" failed: {e}"))? {
            if !cfg.is_excluded(&hit.repo) {
                targets.insert(
                    format!("{}#{}", hit.repo, hit.number),
                    (hit.repo, hit.number, hit.fingerprint),
                );
            }
        }
    }

    // 2. Which need a (re)fetch? Changed, stale, new, or just dropped out of the search.
    let mut fetch: Vec<(String, u64)> = vec![];
    let mut fetch_keys: Vec<(String, bool)> = vec![]; // (key, dropped)
    for (key, (repo, n, fp)) in &targets {
        let need = match state.prs.get(key) {
            None => true,
            Some(t) => {
                t.archived
                    || t.fingerprint != *fp
                    || fp.is_empty()
                    || now - t.fetched_at > MAX_AGE_SECS
            }
        };
        if need {
            fetch.push((repo.clone(), *n));
            fetch_keys.push((key.clone(), false));
        }
    }
    if opts.single.is_none() {
        for (key, t) in &state.prs {
            if !targets.contains_key(key) && !t.archived {
                fetch.push((t.pr.repo.clone(), t.pr.number));
                fetch_keys.push((key.clone(), true));
            }
        }
    }
    if !fetch.is_empty() {
        crate::info!("Fetching {} pull request(s)", fetch.len());
    }

    // 3. Fetch + diff
    let mut changes: Vec<(String, String)> = vec![]; // key, text
    let results = gh.fetch_prs(&fetch);
    for ((key, dropped), result) in fetch_keys.into_iter().zip(results) {
        match result {
            Ok(Some((pr, repo))) => {
                let prev = state.prs.get(&key);
                for text in diff(prev, &pr, &me, cfg, first_run) {
                    changes.push((key.clone(), text));
                }
                let first_seen = merge_seen(prev, &pr, now, first_run);
                let fingerprint = targets.get(&key).map(|t| t.2.clone()).unwrap_or_default();
                let file = prev.map(|p| p.file.clone()).unwrap_or_default();
                if dropped && pr.is_open() {
                    // No longer matched (repo excluded, query changed...) - forget it.
                    remove_note(cfg, &file);
                    state.prs.remove(&key);
                    continue;
                }
                state.repos.insert(repo.name.clone(), repo);
                state.prs.insert(
                    key,
                    Tracked {
                        pr,
                        fingerprint,
                        fetched_at: now,
                        file,
                        archived: dropped,
                        first_seen,
                    },
                );
            }
            Ok(None) => {
                crate::warn!("{key}: not found or no access - no longer tracked");
                if let Some(t) = state.prs.remove(&key) {
                    remove_note(cfg, &t.file);
                }
            }
            Err(e) => {
                if state.prs.contains_key(&key) {
                    crate::warn!("{key}: refresh failed, keeping old data: {e}");
                } else {
                    return Err(e);
                }
            }
        }
    }

    // 4. Forget PRs closed long ago (their notes stay in the vault).
    let keep = cfg.keep_closed_days as i64 * 86400;
    state.prs.retain(|_, t| {
        t.pr.is_open()
            || t.pr
                .closed_at
                .as_deref()
                .and_then(util::parse_ts)
                .is_none_or(|c| now - c.timestamp() < keep)
    });
    let live: HashSet<&str> = state.prs.values().map(|t| t.pr.repo.as_str()).collect();
    state.repos.retain(|r, _| live.contains(r.as_str()));

    // 5. Activity log
    let stamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    for (key, text) in changes.iter().rev() {
        state.activity.insert(
            0,
            Activity {
                time: stamp.clone(),
                key: key.clone(),
                text: text.clone(),
            },
        );
    }
    state.activity.truncate(MAX_ACTIVITY);

    // 6. Write everything
    let todos = write_all(cfg, &mut state, &me, now)?;
    state.todo_count = todos;
    state.last_sync = Some(stamp);
    state.save()?;

    // 7. Notify
    let summary = Summary {
        open_prs: state
            .prs
            .values()
            .filter(|t| t.pr.is_open() && !t.archived)
            .count(),
        todos,
        changes: changes.len(),
    };
    if opts.notify && cfg.notifications {
        send_notifications(
            cfg,
            &state,
            &changes,
            first_run && opts.single.is_none(),
            &summary,
        );
    }
    Ok(summary)
}

/// Event ids (comments, reviews, thread comments) with their author.
fn events(pr: &Pr) -> Vec<(String, &str)> {
    let mut v: Vec<(String, &str)> = pr
        .comments
        .iter()
        .map(|c| (format!("c{}", c.id), c.author.as_str()))
        .collect();
    v.extend(
        pr.reviews
            .iter()
            .map(|r| (format!("r{}", r.id), r.author.as_str())),
    );
    v.extend(
        pr.threads
            .iter()
            .flat_map(|t| t.comments.iter())
            .map(|c| (format!("t{}", c.id), c.author.as_str())),
    );
    v
}

fn merge_seen(prev: Option<&Tracked>, pr: &Pr, now: i64, first_run: bool) -> BTreeMap<String, i64> {
    events(pr)
        .into_iter()
        .map(|(id, _)| {
            let ts = match prev {
                Some(p) => p.first_seen.get(&id).copied().unwrap_or(now),
                None => 0, // newly tracked PR: its history is not "new"
            };
            (id, if first_run { 0 } else { ts })
        })
        .collect()
}

/// Human readable changes since the last fetch.
fn diff(prev: Option<&Tracked>, pr: &Pr, me: &str, cfg: &Config, first_run: bool) -> Vec<String> {
    if first_run {
        return vec![];
    }
    let is_me = |l: &str| l.eq_ignore_ascii_case(me);
    let requested_me = |p: &Pr| p.requested.iter().any(|r| is_me(r));
    let Some(prev) = prev else {
        let mut v = vec![format!("now tracking: {}", pr.title)];
        if requested_me(pr) {
            v.push("your review was requested".into());
        }
        return v;
    };
    let old = &prev.pr;
    let mut out = vec![];

    // new comments / reviews
    let mut commenters: Vec<String> = vec![];
    let mut n_comments = 0;
    for (id, author) in events(pr) {
        if prev.first_seen.contains_key(&id) || (is_me(author) && !cfg.notify_own_activity) {
            continue;
        }
        if let Some(r) = id
            .strip_prefix('r')
            .and_then(|n| pr.reviews.iter().find(|r| r.id.to_string() == n))
        {
            match r.state.as_str() {
                "APPROVED" => out.push(format!("approved by @{author}")),
                "CHANGES_REQUESTED" => out.push(format!("changes requested by @{author}")),
                "COMMENTED" if !r.body.is_empty() => {}
                _ => continue,
            }
            if r.state != "COMMENTED" {
                continue;
            }
        }
        n_comments += 1;
        if !commenters.iter().any(|c| c == author) {
            commenters.push(author.to_string());
        }
    }
    if n_comments > 0 {
        let who: Vec<String> = commenters.iter().map(|c| format!("@{c}")).collect();
        out.push(format!(
            "{n_comments} new comment{} by {}",
            if n_comments == 1 { "" } else { "s" },
            who.join(", ")
        ));
    }

    if old.state != pr.state {
        out.push(match (old.state.as_str(), pr.state.as_str()) {
            (_, "merged") => "merged".into(),
            (_, "closed") => "closed".into(),
            ("draft", "open") => "ready for review".into(),
            (_, "draft") => "converted to draft".into(),
            _ => "reopened".into(),
        });
    }
    if requested_me(pr) && !requested_me(old) {
        out.push("your review was requested".into());
    }
    if is_me(&pr.author) && pr.is_open() {
        let failing = |p: &Pr| matches!(p.checks.as_deref(), Some("FAILURE" | "ERROR"));
        if failing(pr) && !failing(old) {
            out.push("checks failing".into());
        } else if !failing(pr) && failing(old) && pr.checks.as_deref() == Some("SUCCESS") {
            out.push("checks passing again".into());
        }
        if pr.mergeable == "CONFLICTING" && old.mergeable != "CONFLICTING" {
            out.push("merge conflicts".into());
        }
    }
    if old.title != pr.title && !old.title.is_empty() {
        out.push(format!("renamed to \"{}\"", pr.title));
    }
    out
}

fn abs(cfg: &Config, rel: &str) -> PathBuf {
    Path::new(&cfg.vault).join(rel)
}

fn remove_note(cfg: &Config, rel: &str) {
    if !rel.is_empty() {
        let _ = std::fs::remove_file(abs(cfg, rel));
    }
}

/// Render and write all notes. Returns the number of open to-dos.
fn write_all(cfg: &Config, state: &mut State, me: &str, now: i64) -> Res<usize> {
    let dash = render::dashboard_file(cfg);

    // Pick up checkboxes the user (un)ticked in Obsidian since the last render.
    let mut files: Vec<String> = state
        .prs
        .values()
        .map(|t| t.file.clone())
        .filter(|f| !f.is_empty())
        .collect();
    files.push(dash.clone());
    for f in &files {
        let Ok(text) = std::fs::read_to_string(abs(cfg, f)) else {
            continue;
        };
        for (id, ticked) in render::read_ticks(&text) {
            let was = state.rendered_done.contains(&id);
            if ticked != was {
                if ticked {
                    state.local_done.insert(id);
                } else {
                    state.local_done.remove(&id);
                }
            }
        }
    }

    // New file names (title changes) - remove old notes.
    for t in state.prs.values_mut() {
        let file = render::note_file(cfg, &t.pr);
        if t.file != file {
            remove_note(cfg, &t.file);
            t.file = file;
        }
    }

    let mut rendered_done = BTreeSet::new();
    let mut all_ids = BTreeSet::new();
    let ctx = Ctx {
        cfg,
        me,
        now,
        local_done: &state.local_done,
    };
    let mut written = 0;

    for t in state.prs.values() {
        let (text, todos) = render::pr_note(&ctx, t, state.repos.get(&t.pr.repo));
        for x in &todos {
            all_ids.insert(x.id.clone());
            if x.checked(&state.local_done) {
                rendered_done.insert(x.id.clone());
            }
        }
        written += util::write_if_changed(&abs(cfg, &t.file), &text)? as usize;
    }
    for repo in state.repos.values() {
        let prs: Vec<&Tracked> = state
            .prs
            .values()
            .filter(|t| t.pr.repo == repo.name)
            .collect();
        let text = render::repo_note(&ctx, repo, &prs);
        written +=
            util::write_if_changed(&abs(cfg, &render::repo_file(cfg, &repo.name)), &text)? as usize;
    }
    let (text, _, todo_count) = render::dashboard(&ctx, state);
    util::write_if_changed(&abs(cfg, &dash), &text)?;
    util::write_if_changed(
        &abs(cfg, &render::activity_file(cfg)),
        &render::activity(&ctx, state),
    )?;
    if written > 0 {
        crate::info!("Wrote {written} note(s)");
    }

    state.local_done.retain(|id| all_ids.contains(id));
    state.rendered_done = rendered_done;
    Ok(todo_count)
}

fn send_notifications(
    cfg: &Config,
    state: &State,
    changes: &[(String, String)],
    first_run: bool,
    s: &Summary,
) {
    let dash = abs(cfg, &render::dashboard_file(cfg));
    if first_run {
        notify::show(
            "pr2md is ready",
            &format!(
                "Tracking {} pull request(s) · {} to-do(s)",
                s.open_prs, s.todos
            ),
            Some(&dash),
        );
        return;
    }
    // Group by PR, keep order.
    let mut by_pr: Vec<(&str, Vec<&str>)> = vec![];
    for (key, text) in changes {
        match by_pr.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => v.push(text),
            None => by_pr.push((key, vec![text])),
        }
    }
    if by_pr.len() > 3 {
        let what: Vec<&str> = by_pr.iter().map(|(k, _)| *k).take(4).collect();
        notify::show(
            &format!("{} pull requests updated", by_pr.len()),
            &format!("{}…", what.join(", ")),
            Some(&dash),
        );
        return;
    }
    for (key, texts) in by_pr {
        let Some(t) = state.prs.get(key) else {
            continue;
        };
        notify::show(
            &format!("{key} · {}", t.pr.title),
            &texts.join("\n"),
            Some(&abs(cfg, &t.file)),
        );
    }
}
