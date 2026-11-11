# pr2md 0.2.0 (2026-11-11)

pr2md is now a native desktop app. It mirrors the GitHub pull requests you're involved
in into your Obsidian vault and tells you when something needs your attention.
This release replaces the Python script with a single Rust executable for Windows,
Linux and macOS.

## Highlights

- **Dashboard**: `Dashboard.md` lists your repositories, then their pull requests,
  then the to-dos that are yours. It also has a "Needs your attention" summary, the
  last 24 hours of activity and a list of recently closed PRs.
- **Requested changes become checkboxes**:
  - each unresolved review thread
  - each "changes requested" review
  - review requests
  - failing checks and merge conflicts on your PRs
  - @-mentions you haven't answered

  A checkbox is ticked automatically when the thread is resolved on GitHub. When you
  tick one in Obsidian, it stays ticked across syncs and drops off the dashboard.
- **Detailed PR notes**:
  - Obsidian properties: state, review decision, checks, labels, reviewers, to-do count
  - links to the PR's files, commits and checks on GitHub
  - the description, the conversation, and the review threads with the code they refer to
  - a *(new)* marker on anything from the last 24 hours
- **Repository notes**: the description, language, stars, default branch, and the
  number of open PRs and issues, plus a table of tracked PRs. There's also an
  `Activity.md` log.
- **Readable bot reviews**: comments from CodeScene, Copilot and other bots are
  converted from HTML to Markdown, so they display properly in Obsidian.
- **Tray app**: syncs in the background. An orange dot on the icon means you have
  open to-dos, a red dot means the last sync failed. The menu has Sync now,
  Settings, Pause, Start at login, Open log and Quit. On Windows, left-click opens
  the dashboard.
- **Settings window**:
  - GitHub sign-in (code in the browser, GitHub CLI login, or a personal access token)
  - vault picker
  - searches, followed repositories and ignored repositories
  - sync interval, notifications and start at login
- **Notifications**: a desktop notification for new comments, reviews, state changes,
  review requests and failing checks. On Windows, clicking it opens the note in
  Obsidian.
- **Faster and lighter**:
  - uses the GitHub GraphQL API and fetches only the PRs that changed
  - repeat syncs take about 2 seconds
  - a single ~6 MB executable
  - notes are only rewritten when their content changes
- **Read-only**: pr2md never changes anything on GitHub.
- **Installer**: `install.cmd` (Windows) and `install.sh` (Linux and macOS) install
  pr2md for your user without admin rights and turn on start at login. Running the
  installer again updates pr2md if the binary changed.

## Install

1. Unzip the release.
2. Windows: double-click `install.cmd`. Linux and macOS: run `./install.sh`.
3. The settings window opens. Sign in, choose your vault, then click
   **Save & sync now**.

## Upgrading from the Python version

- The config is now `config.toml` in `%APPDATA%\pr2md\`. Your old `config.json` isn't
  read, so set the vault again in the settings window.
- Notes now go to the `GitHub` folder in your vault. To keep using your old folder,
  set **Notes folder** to `Pull Requests`. pr2md doesn't delete the old notes; remove
  them yourself once you're happy with the new ones.
- `state.json` starts fresh, so the first sync doesn't send a notification for every
  existing comment.

## Known limitations

- **Sign in with GitHub** only works if the build has an OAuth app client ID, or if you
  set one under Settings → Advanced. Without it, use your GitHub CLI login or a
  personal access token. The README explains how to set up the client ID.
- The Linux and macOS builds haven't been tested on real machines yet.
- On GNOME, the tray icon needs the *AppIndicator and KStatusNotifierItem Support*
  extension.
