//! pr2md - mirror the GitHub pull requests you are involved in into Obsidian Markdown.

// Release builds with the tray are GUI apps on Windows (no console window popping up);
// CLI commands re-attach to the parent console.
#![cfg_attr(
    all(windows, feature = "tray", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod auth;
mod clean;
mod config;
mod github;
#[cfg(feature = "gui")]
mod gui;
#[cfg(any(feature = "tray", feature = "gui"))]
mod icon;
mod notify;
mod render;
mod state;
mod sync;
#[cfg(feature = "tray")]
mod tray;
mod util;

use config::Config;
use std::time::Duration;
use util::Res;

const HELP: &str = "\
pr2md - GitHub pull requests -> Obsidian Markdown

USAGE:
    pr2md [COMMAND]

COMMANDS:
    tray                   Run in the system tray and sync periodically (default)
    settings               Open the settings window
    sync [owner/repo#N]    Sync once (optionally just one PR) and exit
    watch                  Sync periodically without a tray (servers, terminals)
    login [--token TOKEN]  Sign in to GitHub (browser code flow, or save a token)
    logout                 Forget the saved GitHub login
    status                 Show account, paths and last sync
    autostart on|off       Start the tray app when you log in
    help                   Show this help

Config: {config}
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args
        .first()
        .map(String::as_str)
        .unwrap_or(if cfg!(feature = "tray") {
            "tray"
        } else {
            "sync"
        });
    let interactive = !matches!(cmd, "tray" | "settings");
    if interactive {
        attach_console();
    }
    util::init_log(interactive);
    if let Err(e) = run(cmd, &args[args.len().min(1)..]) {
        util::log("ERROR", &e.to_string());
        if !interactive {
            notify::show("pr2md", &e.to_string(), None);
            std::thread::sleep(Duration::from_secs(2));
        }
        std::process::exit(1);
    }
}

fn run(cmd: &str, rest: &[String]) -> Res<()> {
    match cmd {
        #[cfg(feature = "tray")]
        "tray" => tray::run(),
        #[cfg(feature = "gui")]
        "settings" | "gui" | "config" => gui::run(),
        "sync" | "once" => {
            let cfg = load_config()?;
            let single = match rest.first() {
                Some(s) => Some(parse_pr(s)?),
                None => None,
            };
            sync::run(
                &cfg,
                &sync::Options {
                    single,
                    notify: true,
                },
            )?;
            Ok(())
        }
        "watch" => {
            let _instance = util::Lock::try_acquire("tray.lock")
                .ok_or("pr2md is already running (tray or watch)")?;
            loop {
                let cfg = load_config()?;
                let _ = sync::run(
                    &cfg,
                    &sync::Options {
                        single: None,
                        notify: true,
                    },
                );
                std::thread::sleep(Duration::from_secs(cfg.interval_minutes * 60));
            }
        }
        "login" => login(rest),
        "logout" => {
            auth::forget();
            println!(
                "Signed out. (A GitHub CLI login or GITHUB_TOKEN would still be used if present.)"
            );
            Ok(())
        }
        "status" => status(),
        "autostart" => {
            let on = match rest.first().map(String::as_str) {
                Some("on") => true,
                Some("off") => false,
                _ => {
                    println!(
                        "Autostart is {}",
                        if util::autostart::is_enabled() {
                            "on"
                        } else {
                            "off"
                        }
                    );
                    return Ok(());
                }
            };
            util::autostart::set(on)?;
            println!("Autostart {}", if on { "enabled" } else { "disabled" });
            Ok(())
        }
        "help" | "-h" | "--help" => {
            print!(
                "{}",
                HELP.replace("{config}", &Config::path().to_string_lossy())
            );
            Ok(())
        }
        "-V" | "--version" | "version" => {
            println!("pr2md {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => {
            print!(
                "{}",
                HELP.replace("{config}", &Config::path().to_string_lossy())
            );
            bail!("Unknown command: {other}")
        }
    }
}

fn load_config() -> Res<Config> {
    if !Config::exists() {
        Config::default().save()?;
        println!(
            "Created {} - set `vault` to your Obsidian vault, then run again.",
            Config::path().display()
        );
    }
    Config::load()
}

fn parse_pr(s: &str) -> Res<(String, u64)> {
    // owner/repo#123 or https://github.com/owner/repo/pull/123
    if let Some((repo, n)) = s.rsplit_once('#') {
        return Ok((config::normalize_repo(repo), n.parse()?));
    }
    if let Some((left, n)) = s.trim_end_matches('/').rsplit_once("/pull/") {
        return Ok((
            config::normalize_repo(left),
            n.split('/').next().unwrap_or("").parse()?,
        ));
    }
    bail!("Expected owner/repo#123 or a pull request URL, got {s}")
}

fn login(rest: &[String]) -> Res<()> {
    let cfg = Config::load()?;
    if let Some(pos) = rest.iter().position(|a| a == "--token") {
        let token = rest.get(pos + 1).ok_or("--token needs a value")?;
        let login = auth::whoami(&cfg, token)?;
        auth::store(token)?;
        println!("Signed in as @{login}.");
        return Ok(());
    }
    let code = auth::device_start(&cfg)?;
    println!(
        "\n  Open {}\n  and enter the code:  {}\n",
        code.verification_uri, code.user_code
    );
    util::open(&code.verification_uri);
    println!("Waiting for you to approve in the browser…");
    let token = auth::device_finish(&cfg, &code, &|| false)?;
    println!("Signed in as @{}.", auth::whoami(&cfg, &token)?);
    Ok(())
}

fn status() -> Res<()> {
    let cfg = Config::load()?;
    println!("pr2md {}", env!("CARGO_PKG_VERSION"));
    println!("Config:    {}", Config::path().display());
    println!(
        "Vault:     {}",
        if cfg.vault.is_empty() {
            "(not set)".into()
        } else {
            cfg.out_dir().display().to_string()
        }
    );
    match auth::token() {
        None => println!("GitHub:    not signed in (run `pr2md login`)"),
        Some((t, src)) => match auth::whoami(&cfg, &t) {
            Ok(l) => println!("GitHub:    @{l} (via {})", src.describe()),
            Err(e) => println!(
                "GitHub:    token from {} does not work: {e}",
                src.describe()
            ),
        },
    }
    if let Some(st) = state::State::load() {
        let open = st.prs.values().filter(|t| t.pr.is_open()).count();
        println!(
            "Last sync: {}",
            st.last_sync
                .as_deref()
                .map(util::fmt_time)
                .unwrap_or_else(|| "never".into())
        );
        println!("Tracking:  {open} open PR(s), {} to-do(s)", st.todo_count);
        if let Some(e) = st.last_error {
            println!("Error:     {e}");
        }
    }
    for p in cfg.problems() {
        println!("Problem:   {p}");
    }
    println!(
        "Autostart: {}",
        if util::autostart::is_enabled() {
            "on"
        } else {
            "off"
        }
    );
    Ok(())
}

/// GUI-subsystem builds have no console; attach to the terminal we were started from.
fn attach_console() {
    #[cfg(all(windows, feature = "tray", not(debug_assertions)))]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}
