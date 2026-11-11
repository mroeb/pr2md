# pr2md

Mirror the GitHub pull requests you're involved in into Markdown notes for
[Obsidian](https://obsidian.md). pr2md runs in the system tray, checks GitHub
every few minutes, and shows a desktop notification when something changes.

It's a single ~6 MB executable for Windows, Linux and macOS, with a settings window and
sign-in built in.

## What you get

```
<vault>/GitHub/
├── Dashboard.md                 ← repos → PRs → things you need to do
├── Activity.md                  ← everything that happened, newest first
└── owner/repo/
    ├── repo.md                  ← repo info + table of tracked PRs
    └── 123 - Fix the thing.md   ← one note per PR
```

**Dashboard** (`Dashboard.md`)
- **Needs your attention**: one line per PR that's waiting on you (review requested,
  changes requested, open threads, failing checks, conflicts, mentions)
- **Last 24 hours**: new comments, reviews, merges, and so on
- **One section per repository** with its metadata, then one heading per open PR
  (state, review, checks, branch, comment count) followed by **checkboxes for the
  things you need to do**
- **Recently closed** PRs

**PR note**
- **Properties** (front matter): repo, state, author, review decision, checks, labels,
  reviewers, open threads, to-do count, dates, aliases and tags. Works with
  Dataview and Bases.
- **Header callout**: links to the PR, files, commits, checks, the repo note and
  the dashboard
- **To-do**: every review thread and "changes requested" review as a checkbox.
  A box is ticked automatically when the thread is resolved on GitHub.
- **Description**, then the **conversation** as callouts (approved, changes
  requested, comments). Bot comments are folded, and new items are marked *(new)*.
- HTML in bot comments (CodeScene, Copilot, ...) is converted to plain Markdown, so it
  renders properly inside Obsidian callouts.
- **Review threads** with the diff hunk and every reply. Resolved threads are folded.

### What counts as "your" to-do

| To-do | When |
|---|---|
| Review requested | Your review is requested on the PR |
| Changes requested | On your PR, until that reviewer submits a newer review |
| Review thread | Unresolved, the last reply isn't yours, and it's your PR or you're in the thread or mentioned |
| Checks failing / Merge conflicts | On your open PRs |
| Mention | Someone @-mentioned you and you haven't replied since |

### Ticking checkboxes in Obsidian

Every checkbox ends with an Obsidian block id (`^t123456`). If you tick one in
Obsidian, pr2md keeps it ticked on the next sync and removes it from the dashboard,
so the dashboard works as an inbox. To bring an item back, untick it in the PR note.

pr2md overwrites the rest of each generated note on every sync, so keep your own
notes in separate files and link to them. Every PR note has the alias `owner/repo#123`,
so `[[owner/repo#123]]` works as a link.

## Install

The install scripts don't need admin or root rights. Each one builds pr2md if needed
(Rust 1.89+), copies it to a per-user folder, turns on start at login, and starts the
tray app:

| OS | Command | Installs to |
|---|---|---|
| Windows | double-click `install.cmd` (or `.\install.ps1`) | `%LOCALAPPDATA%\Programs\pr2md\pr2md.exe` |
| Linux / macOS | `./install.sh` | `~/.local/bin/pr2md` |

To update, run the script again. If the binary changed, the script stops the running
pr2md, replaces the file and starts it again. If nothing changed, it leaves the
installed file alone. Your config, state and login aren't touched either way.

| | Windows | Linux / macOS |
|---|---|---|
| Rebuild before installing | `-Build` | `--build` |
| Install a specific binary | `-Source path\to\pr2md.exe` | `--source path` |
| Don't start the tray app | `-NoStart` | `--no-start` |
| Uninstall | `-Uninstall` | `--uninstall` |

You can also build it yourself with `cargo build --release`. The result is a single
self-contained binary that runs from anywhere.

Start `pr2md`. The settings window opens the first time:

1. **Sign in with GitHub**: a code is copied to your clipboard and github.com
   opens in your browser. Paste the code there and confirm. If you prefer, you can use
   your GitHub CLI login (`gh auth login`) or paste a personal access token instead.
2. **Choose your vault** folder.
3. **What to follow**: the defaults are "PRs involving me" and "PRs where my review
   is requested". You can add more GitHub searches, or whole repositories.
4. Click **Save & sync now**.

pr2md then runs in the tray. Left-click the icon to open the dashboard (Windows), or
right-click it for the menu: *Sync now · Settings… · Pause · Start at login · Open log · Quit*.
An orange dot on the icon means you have open to-dos. A red dot means the last sync failed.

### Linux

The tray uses the StatusNotifierItem D-Bus protocol, so GTK isn't needed. KDE, XFCE
and most other desktops show it out of the box. GNOME needs the
*AppIndicator and KStatusNotifierItem Support* extension. Notifications use
`notify-send`, and the folder picker uses `zenity` if it's installed.

### Without a tray (servers, WSL, cron)

```sh
cargo build --release --no-default-features   # CLI only, no GUI dependencies
pr2md login            # device-code sign-in in the terminal
pr2md watch            # sync every interval_minutes
```

## Command line

| Command | What it does |
|---|---|
| `pr2md` / `pr2md tray` | Tray app (default) |
| `pr2md settings` | Open the settings window |
| `pr2md sync` | Sync once |
| `pr2md sync owner/repo#123` | Sync a single PR (also accepts a PR URL) |
| `pr2md watch` | Sync periodically, no tray |
| `pr2md login [--token T]` | Sign in (browser code) or store a token |
| `pr2md logout` | Forget the stored login |
| `pr2md status` | Account, paths, last sync, problems |
| `pr2md autostart on\|off` | Start the tray app at login |

## Configuration

`config.toml` is in `%APPDATA%\pr2md\` (Windows), `~/Library/Application Support/pr2md/`
(macOS) or `~/.config/pr2md/` (Linux). In **portable mode**, a `config.toml` next to the
executable is used instead. The settings window writes the same file, comments included.

```toml
vault = "C:/Users/me/Documents/Work"
folder = "GitHub"
interval_minutes = 5
queries = [
  "is:pr is:open involves:@me",
  "is:pr is:open review-requested:@me",
]
repos = ["my-org/important-repo"]     # every open PR in these
exclude_repos = ["my-org/noisy-*"]    # "owner/repo" or "owner/*"
notifications = true
notify_own_activity = false
keep_closed_days = 14
api_url = "https://api.github.com"    # GHE: https://github.example.com/api/v3
client_id = ""
```

`state.json` (cache), `pr2md.log` and the stored token are in the same folder.

### GitHub sign-in and the OAuth client id

*Sign in with GitHub* uses GitHub's
[device flow](https://docs.github.com/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow),
which needs the client id of an OAuth app. You only need to register one once:

1. GitHub → *Settings → Developer settings → OAuth Apps → New OAuth App*.
   Any homepage and callback URL will do, for example `https://github.com`.
2. Tick **Enable Device Flow** and save. Copy the **Client ID**. You don't need a client
   secret.
3. Do one of the following:
   - Bake the id in at build time: `PR2MD_CLIENT_ID=Iv1.abc… cargo build --release`
   - Put it in `client_id` in the config, or set it under *Settings → Advanced*.

If your organisation restricts OAuth apps, an org owner has to approve the app once.
Until then, the GitHub CLI login or a personal access token work without a client id.

pr2md looks for a token in this order: `PR2MD_TOKEN` env → stored sign-in → `gh auth token` →
`GITHUB_TOKEN`/`GH_TOKEN`. The stored token is encrypted with DPAPI on Windows. On
Linux and macOS it's a file only you can read (`0600`).

## How it works

- **GraphQL**: one search per query returns a small "fingerprint" of each PR (updated
  time, state, review decision, checks). Only PRs whose fingerprint changed are fetched
  again. Each fetch request covers 5 PRs, with up to 4 requests in parallel, and
  returns comments, reviews, threads (including whether they're resolved), checks
  and repo metadata. Open PRs are also re-fetched every 30 minutes, because resolving
  a thread doesn't change the PR's update time.
- Files are only written when their content changes, so Obsidian Sync and git don't
  see unnecessary changes.
- **Notifications**: one per changed PR, or a single summary when more than 3 PRs
  changed. On Windows, clicking the notification opens the note in Obsidian.
- **PR leaves the search** (merged or closed): pr2md fetches it one last time, then
  lists it under *Recently closed* for `keep_closed_days`. If it's still open but no
  longer matches (you removed the repo, for example), its note is deleted.
- Uses the OS certificate store, so it works behind TLS-inspecting corporate proxies.
  It also respects `HTTPS_PROXY`.

## Dataview example

````markdown
```dataview
TABLE state, review, checks, todo AS "To-do", updated
FROM "GitHub" WHERE type = "pull-request" AND todo > 0
SORT updated DESC
```
````

## Dependencies

`ureq` (HTTP), `serde`/`serde_json`/`toml`, `chrono`, `tray-icon` + `winit` (tray),
`eframe` (settings window), and `windows-sys` on Windows. Notifications, autostart,
the folder picker and opening links all use built-in OS tools, so they need no
extra crates. The icon is drawn in code.
