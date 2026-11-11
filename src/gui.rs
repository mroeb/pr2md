//! Settings window (egui): sign in to GitHub, pick the vault, manage what to follow.

use crate::auth::{self, DeviceCode, Source};
use crate::config::{Config, normalize_repo};
use crate::state::State;
use crate::sync::{self, Options};
use crate::util::{self, autostart};
use eframe::egui::{self, Color32, RichText};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
enum Account {
    Checking,
    SignedIn { login: String, source: Source },
    SignedOut,
    Invalid(String),
}

#[derive(Clone)]
enum SignIn {
    Idle,
    Starting,
    Waiting(DeviceCode),
    Failed(String),
}

#[derive(Clone, Default)]
enum Task {
    #[default]
    Idle,
    Running(String),
    Done(String),
    Failed(String),
}

struct Settings {
    cfg: Config,
    saved: Config,
    account: Arc<Mutex<Account>>,
    sign_in: Arc<Mutex<SignIn>>,
    cancel: Arc<AtomicBool>,
    task: Arc<Mutex<Task>>,
    token_input: String,
    new_query: String,
    new_repo: String,
    new_exclude: String,
    autostart: bool,
    gh_available: bool,
    last_sync: String,
}

pub fn run() -> util::Res<()> {
    let size = 64;
    let icon = egui::IconData {
        rgba: crate::icon::rgba(size, crate::icon::Badge::None, false),
        width: size,
        height: size,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("pr2md – Settings")
            .with_inner_size([620.0, 760.0])
            .with_min_inner_size([480.0, 420.0])
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "pr2md",
        options,
        Box::new(|cc| Ok(Box::new(Settings::new(&cc.egui_ctx)))),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

impl Settings {
    fn new(ctx: &egui::Context) -> Self {
        let cfg = Config::load().unwrap_or_default();
        let s = Settings {
            saved: cfg.clone(),
            cfg,
            account: Arc::new(Mutex::new(Account::Checking)),
            sign_in: Arc::new(Mutex::new(SignIn::Idle)),
            cancel: Arc::new(AtomicBool::new(false)),
            task: Arc::default(),
            token_input: String::new(),
            new_query: String::new(),
            new_repo: String::new(),
            new_exclude: String::new(),
            autostart: autostart::is_enabled(),
            gh_available: auth::gh_cli_token().is_some(),
            last_sync: Self::last_sync_text(),
        };
        s.check_account(ctx);
        s
    }

    fn last_sync_text() -> String {
        match State::load() {
            Some(st) => match (st.last_error, st.last_sync) {
                (Some(e), _) => format!("⚠ Last sync failed: {e}"),
                (None, Some(t)) => format!(
                    "Last sync {} · {} to-do(s)",
                    util::fmt_time(&t),
                    st.todo_count
                ),
                _ => String::new(),
            },
            None => String::new(),
        }
    }

    fn check_account(&self, ctx: &egui::Context) {
        let account = self.account.clone();
        let cfg = self.cfg.clone();
        let ctx = ctx.clone();
        *account.lock().unwrap() = Account::Checking;
        std::thread::spawn(move || {
            let a = match auth::token() {
                None => Account::SignedOut,
                Some((token, source)) => match auth::whoami(&cfg, &token) {
                    Ok(login) => Account::SignedIn { login, source },
                    Err(e) => Account::Invalid(e.to_string()),
                },
            };
            *account.lock().unwrap() = a;
            ctx.request_repaint();
        });
    }

    fn start_sign_in(&self, ctx: &egui::Context) {
        let (sign_in, cancel, account, cfg, ctx) = (
            self.sign_in.clone(),
            self.cancel.clone(),
            self.account.clone(),
            self.cfg.clone(),
            ctx.clone(),
        );
        cancel.store(false, Ordering::SeqCst);
        *sign_in.lock().unwrap() = SignIn::Starting;
        std::thread::spawn(move || {
            let result = auth::device_start(&cfg).and_then(|code| {
                *sign_in.lock().unwrap() = SignIn::Waiting(code.clone());
                ctx.copy_text(code.user_code.clone());
                util::open(&code.verification_uri);
                ctx.request_repaint();
                let token = auth::device_finish(&cfg, &code, &|| cancel.load(Ordering::SeqCst))?;
                auth::whoami(&cfg, &token)
            });
            match result {
                Ok(login) => {
                    *account.lock().unwrap() = Account::SignedIn {
                        login,
                        source: Source::Stored,
                    };
                    *sign_in.lock().unwrap() = SignIn::Idle;
                }
                Err(e) => *sign_in.lock().unwrap() = SignIn::Failed(e.to_string()),
            }
            ctx.request_repaint();
        });
    }

    fn save(&mut self) {
        self.cfg.repos = self.cfg.repos.iter().map(|r| normalize_repo(r)).collect();
        self.cfg.exclude_repos = self
            .cfg
            .exclude_repos
            .iter()
            .map(|r| normalize_repo(r))
            .collect();
        match self.cfg.save() {
            Ok(()) => self.saved = self.cfg.clone(),
            Err(e) => *self.task.lock().unwrap() = Task::Failed(format!("Could not save: {e}")),
        }
    }

    fn sync_now(&mut self, ctx: &egui::Context) {
        self.save();
        let (task, cfg, ctx) = (self.task.clone(), self.cfg.clone(), ctx.clone());
        *task.lock().unwrap() = Task::Running("Syncing…".into());
        std::thread::spawn(move || {
            let t = match sync::run(
                &cfg,
                &Options {
                    single: None,
                    notify: false,
                },
            ) {
                Ok(s) => Task::Done(format!(
                    "✔ Synced: {} open PR(s), {} to-do(s), {} change(s)",
                    s.open_prs, s.todos, s.changes
                )),
                Err(e) => Task::Failed(e.to_string()),
            };
            *task.lock().unwrap() = t;
            ctx.request_repaint();
        });
    }

    // ----------------------------------------------------------------------- sections

    fn account_ui(&mut self, ui: &mut egui::Ui) {
        let account = self.account.lock().unwrap().clone();
        let sign_in = self.sign_in.lock().unwrap().clone();
        match &account {
            Account::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking GitHub account…");
                });
            }
            Account::SignedIn { login, source } => {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("✔ Signed in as @{login}"))
                            .strong()
                            .color(Color32::from_rgb(46, 160, 67)),
                    );
                    ui.label(RichText::new(format!("via {}", source.describe())).weak());
                });
                ui.horizontal(|ui| {
                    if *source == Source::Stored && ui.button("Sign out").clicked() {
                        auth::forget();
                        self.check_account(ui.ctx());
                    }
                    if *source != Source::Stored
                        && ui.button("Sign in with a different account…").clicked()
                    {
                        self.start_sign_in(ui.ctx());
                    }
                });
            }
            Account::SignedOut => {
                ui.label("Not signed in yet.");
            }
            Account::Invalid(e) => {
                ui.colored_label(
                    Color32::from_rgb(218, 54, 51),
                    format!("⚠ The saved login does not work: {e}"),
                );
            }
        }

        match &sign_in {
            SignIn::Idle => {
                if !matches!(account, Account::SignedIn { .. } | Account::Checking) {
                    ui.add_space(4.0);
                    let btn = egui::Button::new(
                        RichText::new("  Sign in with GitHub  ").strong().size(16.0),
                    );
                    if ui.add(btn).clicked() {
                        self.start_sign_in(ui.ctx());
                    }
                }
            }
            SignIn::Starting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Contacting GitHub…");
                });
            }
            SignIn::Waiting(code) => {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label("Enter this code on the GitHub page that just opened (it's already in your clipboard):");
                    ui.label(RichText::new(&code.user_code).monospace().size(28.0).strong());
                    ui.horizontal(|ui| {
                        if ui.button("Copy code").clicked() {
                            ui.ctx().copy_text(code.user_code.clone());
                        }
                        if ui.button("Open GitHub again").clicked() {
                            util::open(&code.verification_uri);
                        }
                        if ui.button("Cancel").clicked() {
                            self.cancel.store(true, Ordering::SeqCst);
                        }
                        ui.spinner();
                    });
                });
            }
            SignIn::Failed(e) => {
                ui.colored_label(Color32::from_rgb(218, 54, 51), e);
                if ui.button("Try again").clicked() {
                    self.start_sign_in(ui.ctx());
                }
            }
        }

        egui::CollapsingHeader::new("Other ways to sign in").show(ui, |ui| {
            if self.gh_available {
                ui.horizontal(|ui| {
                    ui.label("GitHub CLI is logged in.");
                    if ui.button("Use the GitHub CLI login").clicked() {
                        auth::forget();
                        self.check_account(ui.ctx());
                    }
                });
            }
            ui.label("Personal access token (classic: scope `repo`; fine-grained: read access to pull requests):");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.token_input).password(true).hint_text("ghp_… / github_pat_…").desired_width(320.0));
                if ui.add_enabled(!self.token_input.trim().is_empty(), egui::Button::new("Save token")).clicked() {
                    match auth::whoami(&self.cfg, self.token_input.trim()).and_then(|login| auth::store(&self.token_input).map(|_| login)) {
                        Ok(login) => {
                            *self.account.lock().unwrap() = Account::SignedIn { login, source: Source::Stored };
                            self.token_input.clear();
                        }
                        Err(e) => *self.account.lock().unwrap() = Account::Invalid(e.to_string()),
                    }
                }
                if ui.button("Create token…").clicked() {
                    util::open(&format!("{}/settings/tokens/new?scopes=repo,read:org&description=pr2md", self.cfg.web_url()));
                }
            });
        });
    }

    fn vault_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Vault folder");
            let w = ui.available_width() - 80.0;
            ui.add(
                egui::TextEdit::singleline(&mut self.cfg.vault)
                    .hint_text("C:/Users/you/Documents/MyVault")
                    .desired_width(w),
            );
            if ui.button("Browse…").clicked() {
                if let Some(p) = util::pick_folder() {
                    self.cfg.vault = p;
                }
            }
        });
        let vault = std::path::Path::new(&self.cfg.vault);
        if self.cfg.vault.is_empty() {
            ui.label(RichText::new("Choose the folder of your Obsidian vault.").weak());
        } else if !vault.is_dir() {
            ui.colored_label(
                Color32::from_rgb(218, 54, 51),
                "This folder does not exist.",
            );
        } else if !vault.join(".obsidian").is_dir() {
            ui.colored_label(
                Color32::from_rgb(210, 150, 0),
                "No .obsidian folder here – is this really your vault? (Works anyway.)",
            );
        } else {
            ui.colored_label(Color32::from_rgb(46, 160, 67), "✔ Obsidian vault found");
        }
        ui.horizontal(|ui| {
            ui.label("Notes folder");
            ui.add(egui::TextEdit::singleline(&mut self.cfg.folder).desired_width(200.0));
            ui.label(RichText::new(format!("→ {}/Dashboard.md", self.cfg.folder)).weak());
        });
    }

    fn follow_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Searches").strong());
        ui.label(RichText::new("GitHub search queries. @me means you.").weak());
        list_editor(
            ui,
            "q",
            &mut self.cfg.queries,
            &mut self.new_query,
            "is:pr is:open author:@me",
        );
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Quick add:").weak());
            for (label, q) in [
                ("involves me", "is:pr is:open involves:@me"),
                ("review requested", "is:pr is:open review-requested:@me"),
                ("my PRs", "is:pr is:open author:@me"),
                ("assigned", "is:pr is:open assignee:@me"),
            ] {
                if ui.small_button(label).on_hover_text(q).clicked()
                    && !self.cfg.queries.iter().any(|x| x == q)
                {
                    self.cfg.queries.push(q.into());
                }
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Repositories").strong());
        ui.label(
            RichText::new(
                "Follow every open PR in these repositories (owner/repo or a GitHub URL).",
            )
            .weak(),
        );
        list_editor(
            ui,
            "r",
            &mut self.cfg.repos,
            &mut self.new_repo,
            "owner/repo",
        );
        ui.add_space(8.0);
        ui.label(RichText::new("Ignore").strong());
        ui.label(
            RichText::new("Never show PRs from these repositories (owner/repo or owner/*).").weak(),
        );
        list_editor(
            ui,
            "x",
            &mut self.cfg.exclude_repos,
            &mut self.new_exclude,
            "owner/repo",
        );
    }

    fn behaviour_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Check GitHub every");
            ui.add(
                egui::DragValue::new(&mut self.cfg.interval_minutes)
                    .range(1..=1440)
                    .suffix(" min"),
            );
        });
        ui.checkbox(
            &mut self.cfg.notifications,
            "Desktop notifications for new comments, reviews and status changes",
        );
        ui.add_enabled(
            self.cfg.notifications,
            egui::Checkbox::new(
                &mut self.cfg.notify_own_activity,
                "…also for my own activity",
            ),
        );
        ui.horizontal(|ui| {
            ui.label("Keep merged / closed PRs on the dashboard for");
            ui.add(
                egui::DragValue::new(&mut self.cfg.keep_closed_days)
                    .range(0..=365)
                    .suffix(" days"),
            );
        });
        if ui
            .checkbox(&mut self.autostart, "Start pr2md when I log in")
            .changed()
        {
            if let Err(e) = autostart::set(self.autostart) {
                *self.task.lock().unwrap() = Task::Failed(format!("Autostart: {e}"));
            }
            self.autostart = autostart::is_enabled();
        }
        egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("API URL");
                ui.add(egui::TextEdit::singleline(&mut self.cfg.api_url).desired_width(300.0));
            });
            ui.label(RichText::new("GitHub Enterprise: https://github.example.com/api/v3").weak());
            ui.horizontal(|ui| {
                ui.label("OAuth client id");
                ui.add(
                    egui::TextEdit::singleline(&mut self.cfg.client_id)
                        .hint_text("built-in")
                        .desired_width(300.0),
                );
            });
            ui.horizontal(|ui| {
                if ui.button("Open config file").clicked() {
                    self.save();
                    util::open(&Config::path().to_string_lossy());
                }
                if ui.button("Open log").clicked() {
                    util::open(&util::log_path().to_string_lossy());
                }
                ui.label(
                    RichText::new(util::app_dir().to_string_lossy())
                        .weak()
                        .small(),
                );
            });
        });
    }
}

fn list_editor(ui: &mut egui::Ui, id: &str, items: &mut Vec<String>, new: &mut String, hint: &str) {
    let mut remove = None;
    for (i, item) in items.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let w = ui.available_width() - 34.0;
            ui.add(
                egui::TextEdit::singleline(item)
                    .desired_width(w)
                    .id_salt((id, i)),
            );
            if ui.small_button("✖").on_hover_text("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        items.remove(i);
    }
    ui.horizontal(|ui| {
        let w = ui.available_width() - 60.0;
        let r = ui.add(
            egui::TextEdit::singleline(new)
                .hint_text(hint)
                .desired_width(w)
                .id_salt((id, "new")),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button("+ Add").clicked() || enter) && !new.trim().is_empty() {
            items.push(new.trim().to_string());
            new.clear();
        }
    });
}

fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    egui::Frame::group(ui.style())
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).heading());
            ui.add_space(4.0);
            add(ui);
        });
}

impl eframe::App for Settings {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Close = save. Nobody likes losing settings.
        if ctx.input(|i| i.viewport().close_requested()) && self.cfg != self.saved {
            self.save();
        }
        let busy = matches!(*self.task.lock().unwrap(), Task::Running(_))
            || matches!(
                *self.sign_in.lock().unwrap(),
                SignIn::Starting | SignIn::Waiting(_)
            )
            || matches!(*self.account.lock().unwrap(), Account::Checking);
        if busy {
            ctx.request_repaint_after(Duration::from_millis(300));
        }

        egui::Panel::bottom("actions").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let dirty = self.cfg != self.saved;
                if ui.add_enabled(dirty, egui::Button::new("Save")).clicked() {
                    self.save();
                }
                let can_sync = !matches!(*self.task.lock().unwrap(), Task::Running(_));
                if ui
                    .add_enabled(
                        can_sync,
                        egui::Button::new(RichText::new("Save & sync now").strong()),
                    )
                    .clicked()
                {
                    self.sync_now(&ctx);
                }
                if ui
                    .add_enabled(
                        !self.cfg.vault.is_empty(),
                        egui::Button::new("Open dashboard"),
                    )
                    .clicked()
                {
                    let p = std::path::Path::new(&self.cfg.vault)
                        .join(crate::render::dashboard_file(&self.cfg));
                    util::open(&util::obsidian_uri(&p));
                }
                if dirty {
                    ui.label(RichText::new("unsaved changes").italics().weak());
                }
            });
            let task = self.task.lock().unwrap().clone();
            match task {
                Task::Idle => {
                    if !self.last_sync.is_empty() {
                        ui.label(RichText::new(&self.last_sync).weak());
                    }
                }
                Task::Running(m) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(m);
                    });
                }
                Task::Done(m) => {
                    ui.colored_label(Color32::from_rgb(46, 160, 67), m);
                }
                Task::Failed(m) => {
                    ui.colored_label(Color32::from_rgb(218, 54, 51), format!("⚠ {m}"));
                }
            }
            for p in self.cfg.problems() {
                ui.colored_label(Color32::from_rgb(210, 150, 0), format!("• {p}"));
            }
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("pr2md").size(24.0).strong());
                        ui.label(RichText::new("GitHub pull requests → Obsidian").weak());
                    });
                    section(ui, "1 · GitHub account", |ui| self.account_ui(ui));
                    section(ui, "2 · Obsidian", |ui| self.vault_ui(ui));
                    section(ui, "3 · What to follow", |ui| self.follow_ui(ui));
                    section(ui, "4 · Behaviour", |ui| self.behaviour_ui(ui));
                    ui.add_space(8.0);
                });
        });
    }
}
